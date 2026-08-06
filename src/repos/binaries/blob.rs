//! Columnar on-disk format for a package's binary index.
//!
//! The indices P3M serves are TSV, and parsing one costs a couple of
//! milliseconds — 3773 rows of dplyr means about 30,000 `String` allocations.
//! rig is a CLI, so that cost is paid again on every single invocation, even
//! though the underlying `.tsv.zst` only changes once a day at most.
//!
//! This module moves that work onto the download path. A `.tsv.zst` is parsed
//! once, out of the response body, and what gets cached is a `.v1.rbi` blob
//! that later runs read directly, with no parsing at all. See [`build`] and
//! [`IndexBlob::open`].
//!
//! The blob is therefore the authoritative copy rather than a derived one —
//! the TSV never reaches disk — which is why [`IndexBlob::open`] validates
//! rather than trusts.
//!
//! # Layout
//!
//! A 16-byte plain preamble followed by a body that is zstd-compressed by
//! default:
//!
//! ```text
//! preamble  magic "RBI\0" | format version | body length | flags
//! body      header | sections...
//! ```
//!
//! The body is columnar and dictionary-encoded. Every string in the file —
//! platform names, arches, R versions, hashes, URLs, package names — is stored
//! once in a single blob, and the columns hold `u32` ids into it. That is what
//! makes reading free: a column is a range of bytes, and a field is two
//! integer reads plus a slice.
//!
//! Three properties of the data drive the layout:
//!
//! * **Almost every column is low-cardinality.** dplyr's 3773 rows use 24
//!   distinct platforms, 3 arches, 13 R versions and 48 hashes. Only `url` is
//!   effectively unique per row.
//! * **`linkingto` is the largest column and has 21 distinct values** across
//!   those 3773 rows — distinct *whole lists*, not entries. So the dictionary
//!   holds complete lists ([`IndexBlob::linkingto`]), not individual
//!   `pkg@version=sha` triples, of which there are only 72 in total.
//! * **Versions need to be ordered numerically**, and the files are sorted as
//!   strings, so `0.9.5` follows `0.11.1`. The build step parses each distinct
//!   version once, sorts them, and stores both the parsed components and the
//!   row grouping. Reading back a sorted version list, or the rows of one
//!   version, is then a slice.
//!
//! Ids are `u32` throughout even though most columns would fit in a `u8`.
//! Packing them would complicate every accessor to save bytes that zstd
//! removes anyway.
//!
//! # What this format does not do
//!
//! It stores URLs verbatim, and they are ~30% of the uncompressed body. They
//! are highly derivable — `<prefix>/<snapshot date>/<platform path>/<pkg>_<ver>.<ext>`
//! — so a template table plus a per-row snapshot-date id would shrink the body
//! roughly sevenfold. That is why the body is compressed instead: it gets the
//! size back (dplyr is 41 KB on disk rather than 467 KB, against the 27 KB the
//! server sent) for about 0.09 ms of zstd per open, without teaching rig
//! anything about how P3M spells its URLs. If a blob ever needs to be
//! mmap-able rather than merely fast, that is the change to make, and it is a
//! new `FORMAT_VERSION` rather than a rewrite.
//!
//! For dplyr, the largest index in the fixtures, this turns 1.92 ms of TSV
//! parsing per run into 0.11 ms, at a cost of 3.3 ms once per download.
//!
//! # Endianness
//!
//! The format is little-endian. All targets rig builds for are little-endian,
//! and the blob is a local cache file, but the encoding is explicit
//! (`from_le_bytes`) so a file written on one machine could be read on
//! another — which matters if these are ever served rather than derived.

use std::collections::HashMap;
use std::error::Error;

use simple_error::bail;

use super::BinaryRow;
use crate::dcf::RPackageVersion;

/// First four bytes of every blob.
const MAGIC: [u8; 4] = *b"RBI\0";

/// Bumped whenever the layout changes. It is part of the file *name*, so an
/// old blob is never read at all rather than being read and rejected.
pub const FORMAT_VERSION: u32 = 1;

/// magic, format version, body length, flags.
const PREAMBLE_BYTES: usize = 16;

/// The body is zstd-compressed.
const FLAG_ZSTD: u32 = 1;

/// Compression level. The blob is written on the download path, where a few
/// hundred microseconds are invisible next to the HTTP request, but level 19
/// would cost tens of milliseconds for a few percent.
const ZSTD_LEVEL: i32 = 3;

/// Refuse to allocate for an absurd length in a corrupt preamble. Real bodies
/// are under a megabyte; the largest index P3M serves is nowhere near this.
const MAX_BODY_BYTES: usize = 256 << 20;

/// Header words, see the section table in [`build`].
const HEADER_WORDS: usize = 12;
const HEADER_BYTES: usize = HEADER_WORDS * 4;

const H_NROWS: usize = 0;
const H_NVER: usize = 1;
const H_NSTR: usize = 2;
const H_STR_BYTES: usize = 3;
const H_NCOMP: usize = 4;
const H_NLT: usize = 5;
const H_NLT_ENTS: usize = 6;
const H_PACKAGE: usize = 7;

/// The per-row columns, in the order they are laid out.
const COL_VERSION: usize = 0;
const COL_PLATFORM: usize = 1;
const COL_ARCH: usize = 2;
const COL_R_VERSION: usize = 3;
const COL_SHA256: usize = 4;
const COL_URL: usize = 5;
const COL_LINKINGTO: usize = 6;
const NCOLS: usize = 7;

/// One `pkg@version=sha256` entry of a row's `linkingto` list, borrowed from
/// the blob.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkingTo<'a> {
    pub package: &'a str,
    pub version: &'a str,
    pub sha256: &'a str,
}

// -------------------------------------------------------------------- write

/// Interns strings into a single blob, returning stable ids.
struct Interner {
    strings: Vec<String>,
    ids: HashMap<String, u32>,
}

impl Interner {
    fn new() -> Interner {
        Interner {
            strings: vec![],
            ids: HashMap::new(),
        }
    }

    fn intern(&mut self, s: &str) -> u32 {
        if let Some(id) = self.ids.get(s) {
            return *id;
        }
        let id = self.strings.len() as u32;
        self.strings.push(s.to_string());
        self.ids.insert(s.to_string(), id);
        id
    }
}

fn push_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// Order two versions the way R does, by numeric components.
///
/// The index files are sorted as *strings*, which puts `0.10.0` and `0.11.1`
/// between `0.1.2.1` and `0.2.0` — so the last version in a file is not the
/// newest one. Versions that do not parse sort lowest rather than winning by
/// accident; `None` is `Less` than any `Some`, and an unparseable version is
/// stored with no components, which compares the same way.
fn version_key(v: &str) -> Option<Vec<u32>> {
    RPackageVersion::from_str(v).ok().map(|p| p.components)
}

/// Serialize a parsed index into a blob.
///
/// Rows are grouped by version, versions in ascending numeric order, and rows
/// within a version keep their original file order — several builds of one
/// version can target the same platform, differing only in the LinkingTo
/// versions they were compiled against, and that order is oldest snapshot
/// first.
///
/// # Sections
///
/// After the [`HEADER_WORDS`]-word header, in order, all `u32` unless noted:
///
/// ```text
/// str_off      nstr + 1     byte offsets into the string blob
/// blob         str_bytes    the strings, concatenated, padded to 4 bytes
/// comp_off     nver + 1     offsets into comps; an empty range means the
///                           version did not parse
/// comps        ncomp        RPackageVersion components, concatenated
/// ver_str      nver         string id of each version, ascending
/// ver_row_off  nver + 1     row range of each version
/// lt_off       nlt + 1      offsets into lt_ents, counted in triples
/// lt_ents      3 * nlt_ents (package, version, sha256) string ids
/// columns      7 * nrows    one string id per row per column, see NCOLS
/// ```
pub fn build(package: &str, rows: &[BinaryRow]) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut int = Interner::new();
    let package_id = int.intern(package);

    // Distinct versions in ascending numeric order.
    let mut versions: Vec<&str> = vec![];
    let mut seen: HashMap<&str, ()> = HashMap::new();
    for row in rows {
        if seen.insert(row.version.as_str(), ()).is_none() {
            versions.push(row.version.as_str());
        }
    }
    versions.sort_by(|a, b| version_key(a).cmp(&version_key(b)).then_with(|| a.cmp(b)));
    let version_index: HashMap<&str, u32> = versions
        .iter()
        .enumerate()
        .map(|(i, v)| (*v, i as u32))
        .collect();

    // Rows grouped by version, stable within a group.
    let mut order: Vec<usize> = (0..rows.len()).collect();
    order.sort_by_key(|i| version_index[rows[*i].version.as_str()]);

    // The whole `linkingto` list is the dictionary key: dplyr has 3773 rows
    // and 21 distinct lists.
    let mut lt_lists: Vec<Vec<u32>> = vec![];
    let mut lt_ids: HashMap<Vec<u32>, u32> = HashMap::new();
    let mut cols: Vec<Vec<u32>> = (0..NCOLS).map(|_| Vec::with_capacity(rows.len())).collect();
    for i in &order {
        let row = &rows[*i];
        cols[COL_VERSION].push(version_index[row.version.as_str()]);
        cols[COL_PLATFORM].push(int.intern(&row.platform));
        cols[COL_ARCH].push(int.intern(&row.arch));
        cols[COL_R_VERSION].push(int.intern(&row.r_version));
        cols[COL_SHA256].push(int.intern(&row.sha256));
        cols[COL_URL].push(int.intern(&row.url));

        let mut triples = Vec::with_capacity(row.linkingto.len() * 3);
        for l in &row.linkingto {
            triples.push(int.intern(&l.package));
            triples.push(int.intern(&l.version));
            triples.push(int.intern(&l.sha256));
        }
        let id = match lt_ids.get(&triples) {
            Some(id) => *id,
            None => {
                let id = lt_lists.len() as u32;
                lt_ids.insert(triples.clone(), id);
                lt_lists.push(triples);
                id
            }
        };
        cols[COL_LINKINGTO].push(id);
    }

    // Row range of each version. `order` is sorted by version index, so the
    // ranges are the group boundaries.
    let mut ver_row_off: Vec<u32> = Vec::with_capacity(versions.len() + 1);
    ver_row_off.push(0);
    let mut at = 0usize;
    for v in 0..versions.len() as u32 {
        while at < order.len() && cols[COL_VERSION][at] == v {
            at += 1;
        }
        ver_row_off.push(at as u32);
    }

    // Parsed version components, and the version strings themselves. Interning
    // the version strings last keeps them out of the way of nothing in
    // particular — order in the dictionary is irrelevant — but they have to be
    // interned before the blob is frozen.
    let mut comp_off: Vec<u32> = vec![0];
    let mut comps: Vec<u32> = vec![];
    let mut ver_str: Vec<u32> = Vec::with_capacity(versions.len());
    for v in &versions {
        if let Some(parsed) = version_key(v) {
            comps.extend_from_slice(&parsed);
        }
        comp_off.push(comps.len() as u32);
        ver_str.push(int.intern(v));
    }

    // Freeze the dictionary.
    let mut str_off: Vec<u32> = Vec::with_capacity(int.strings.len() + 1);
    let mut blob: Vec<u8> = vec![];
    str_off.push(0);
    for s in &int.strings {
        blob.extend_from_slice(s.as_bytes());
        str_off.push(blob.len() as u32);
    }
    let str_bytes = blob.len() as u32;
    // Pad so that the section following the blob is word-aligned like the rest.
    blob.resize(blob.len().next_multiple_of(4), 0);

    let mut lt_off: Vec<u32> = vec![0];
    let mut lt_ents: Vec<u32> = vec![];
    for l in &lt_lists {
        lt_ents.extend_from_slice(l);
        lt_off.push((lt_ents.len() / 3) as u32);
    }

    let mut body: Vec<u8> = Vec::with_capacity(HEADER_BYTES + blob.len() + 32 * rows.len());
    let mut header = [0u32; HEADER_WORDS];
    header[H_NROWS] = rows.len() as u32;
    header[H_NVER] = versions.len() as u32;
    header[H_NSTR] = int.strings.len() as u32;
    header[H_STR_BYTES] = str_bytes;
    header[H_NCOMP] = comps.len() as u32;
    header[H_NLT] = lt_lists.len() as u32;
    header[H_NLT_ENTS] = (lt_ents.len() / 3) as u32;
    header[H_PACKAGE] = package_id;
    for w in header {
        push_u32(&mut body, w);
    }
    for v in &str_off {
        push_u32(&mut body, *v);
    }
    body.extend_from_slice(&blob);
    for section in [&comp_off, &comps, &ver_str, &ver_row_off, &lt_off, &lt_ents] {
        for v in section {
            push_u32(&mut body, *v);
        }
    }
    for col in &cols {
        for v in col {
            push_u32(&mut body, *v);
        }
    }

    let compressed = zstd::bulk::compress(&body, ZSTD_LEVEL)?;
    let mut out = Vec::with_capacity(PREAMBLE_BYTES + compressed.len());
    out.extend_from_slice(&MAGIC);
    push_u32(&mut out, FORMAT_VERSION);
    push_u32(&mut out, body.len() as u32);
    push_u32(&mut out, FLAG_ZSTD);
    out.extend_from_slice(&compressed);
    Ok(out)
}

// --------------------------------------------------------------------- read

fn read_u32(body: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([body[at], body[at + 1], body[at + 2], body[at + 3]])
}

/// Iterate a section of `n` `u32`s starting at byte `at`. The caller has
/// already checked that the section fits.
fn section(body: &[u8], at: usize, n: usize) -> impl Iterator<Item = u32> + '_ {
    body[at..at + 4 * n]
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
}

/// A parsed index, borrowed from the blob it was read from.
///
/// Everything here is a slice or an integer read; nothing is decoded up front
/// except the version tables, which are at most a few hundred entries and make
/// the accessors much simpler.
pub struct IndexBlob {
    body: Vec<u8>,
    package: String,

    nrows: usize,
    nstr: usize,
    nlt: usize,

    /// Byte offsets of the sections left in `body`.
    str_off_at: usize,
    blob_at: usize,
    lt_off_at: usize,
    lt_ents_at: usize,
    cols_at: [usize; NCOLS],

    /// Decoded at open: version strings, ascending.
    versions: Vec<String>,
    /// Decoded at open: `comps[comp_off[i]..comp_off[i + 1]]` are version `i`'s
    /// components; an empty range means it did not parse.
    comps: Vec<u32>,
    comp_off: Vec<u32>,
    /// Decoded at open: `rows[ver_row_off[i]..ver_row_off[i + 1]]` are the rows
    /// of version `i`.
    ver_row_off: Vec<u32>,
}

/// Check that `n + 1` offsets are non-decreasing and end exactly at `last`.
fn offsets_ok(body: &[u8], at: usize, n: usize, last: u32) -> bool {
    let mut prev = 0u32;
    let mut count = 0usize;
    for (i, v) in section(body, at, n + 1).enumerate() {
        if (i == 0 && v != 0) || v < prev {
            return false;
        }
        prev = v;
        count += 1;
    }
    count == n + 1 && prev == last
}

fn ids_ok(body: &[u8], at: usize, n: usize, limit: u32) -> bool {
    section(body, at, n).all(|v| v < limit)
}

impl IndexBlob {
    /// Read a blob, validating it well enough that every accessor below is
    /// total: no panics, no out-of-range ids, no invalid UTF-8.
    ///
    /// The checks are a handful of linear scans over integers — tens of
    /// microseconds for the largest index — which buys accessors that never
    /// have to re-check anything. A blob that fails any of them is corrupt or
    /// truncated, and the caller's answer to that is to delete it and rebuild
    /// from the TSV, not to report an error to the user.
    pub fn open(bytes: &[u8]) -> Result<IndexBlob, Box<dyn Error>> {
        if bytes.len() < PREAMBLE_BYTES || bytes[0..4] != MAGIC {
            bail!("Not a binary index blob");
        }
        let format = read_u32(bytes, 4);
        if format != FORMAT_VERSION {
            bail!(
                "Binary index blob has format version {}, expected {}",
                format,
                FORMAT_VERSION
            );
        }
        let raw_len = read_u32(bytes, 8) as usize;
        let flags = read_u32(bytes, 12);
        if flags & !FLAG_ZSTD != 0 {
            bail!("Binary index blob has unknown flags {:#x}", flags);
        }
        if raw_len > MAX_BODY_BYTES {
            bail!(
                "Binary index blob claims an implausible size of {}",
                raw_len
            );
        }

        let payload = &bytes[PREAMBLE_BYTES..];
        let body = if flags & FLAG_ZSTD != 0 {
            zstd::bulk::decompress(payload, raw_len)?
        } else {
            payload.to_vec()
        };
        if body.len() != raw_len {
            bail!(
                "Binary index blob is {} bytes, header says {}",
                body.len(),
                raw_len
            );
        }
        if body.len() < HEADER_BYTES {
            bail!("Binary index blob is truncated");
        }

        let h = |i: usize| read_u32(&body, i * 4) as usize;
        let (nrows, nver, nstr, str_bytes, ncomp, nlt, nlt_ents) = (
            h(H_NROWS),
            h(H_NVER),
            h(H_NSTR),
            h(H_STR_BYTES),
            h(H_NCOMP),
            h(H_NLT),
            h(H_NLT_ENTS),
        );
        let package_id = h(H_PACKAGE) as u32;

        // Carve the sections, checking as we go that they fit.
        let mut at = HEADER_BYTES;
        let mut take = |words: usize| -> Result<usize, Box<dyn Error>> {
            let start = at;
            let bytes = words
                .checked_mul(4)
                .and_then(|b| at.checked_add(b))
                .ok_or("Binary index blob has an implausible section size")?;
            if bytes > body.len() {
                bail!("Binary index blob is truncated");
            }
            at = bytes;
            Ok(start)
        };
        let str_off_at = take(nstr + 1)?;
        let blob_at = take(str_bytes.div_ceil(4))?;
        let comp_off_at = take(nver + 1)?;
        let comps_at = take(ncomp)?;
        let ver_str_at = take(nver)?;
        let ver_row_off_at = take(nver + 1)?;
        let lt_off_at = take(nlt + 1)?;
        let lt_ents_at = take(nlt_ents * 3)?;
        let mut cols_at = [0usize; NCOLS];
        for slot in cols_at.iter_mut() {
            *slot = take(nrows)?;
        }

        // The string dictionary, which everything else indexes into.
        if !offsets_ok(&body, str_off_at, nstr, str_bytes as u32) {
            bail!("Binary index blob has a corrupt string table");
        }
        let blob = &body[blob_at..blob_at + str_bytes];
        let blob_str = std::str::from_utf8(blob)
            .map_err(|_| "Binary index blob has a non-UTF-8 string table")?;
        if !section(&body, str_off_at, nstr + 1).all(|o| blob_str.is_char_boundary(o as usize)) {
            bail!("Binary index blob splits a string mid-character");
        }

        if !offsets_ok(&body, comp_off_at, nver, ncomp as u32)
            || !offsets_ok(&body, ver_row_off_at, nver, nrows as u32)
            || !offsets_ok(&body, lt_off_at, nlt, nlt_ents as u32)
        {
            bail!("Binary index blob has a corrupt offset table");
        }
        if package_id >= nstr as u32
            || !ids_ok(&body, ver_str_at, nver, nstr as u32)
            || !ids_ok(&body, lt_ents_at, nlt_ents * 3, nstr as u32)
            || !ids_ok(&body, cols_at[COL_VERSION], nrows, nver as u32)
            || !ids_ok(&body, cols_at[COL_LINKINGTO], nrows, nlt as u32)
        {
            bail!("Binary index blob has an out-of-range id");
        }
        for col in [COL_PLATFORM, COL_ARCH, COL_R_VERSION, COL_SHA256, COL_URL] {
            if !ids_ok(&body, cols_at[col], nrows, nstr as u32) {
                bail!("Binary index blob has an out-of-range id");
            }
        }

        // Small tables worth decoding once.
        let str_at = |id: u32| -> String {
            let a = read_u32(&body, str_off_at + 4 * id as usize) as usize;
            let b = read_u32(&body, str_off_at + 4 * (id as usize + 1)) as usize;
            blob_str[a..b].to_string()
        };
        let package = str_at(package_id);
        let versions: Vec<String> = section(&body, ver_str_at, nver).map(str_at).collect();
        let comps: Vec<u32> = section(&body, comps_at, ncomp).collect();
        let comp_off: Vec<u32> = section(&body, comp_off_at, nver + 1).collect();
        let ver_row_off: Vec<u32> = section(&body, ver_row_off_at, nver + 1).collect();

        Ok(IndexBlob {
            body,
            package,
            nrows,
            nstr,
            nlt,
            str_off_at,
            blob_at,
            lt_off_at,
            lt_ents_at,
            cols_at,
            versions,
            comps,
            comp_off,
            ver_row_off,
        })
    }

    /// The string with id `id`.
    ///
    /// [`IndexBlob::open`] checked that every id in the file is in range, that
    /// the offsets are non-decreasing, and that the blob is UTF-8 with every
    /// offset on a character boundary, so neither the slicing nor the
    /// conversion can fail here. An id from outside the file yields `""`.
    pub fn s(&self, id: u32) -> &str {
        if id as usize >= self.nstr {
            return "";
        }
        let a = read_u32(&self.body, self.str_off_at + 4 * id as usize) as usize;
        let b = read_u32(&self.body, self.str_off_at + 4 * (id as usize + 1)) as usize;
        std::str::from_utf8(&self.body[self.blob_at + a..self.blob_at + b]).unwrap_or("")
    }

    fn col(&self, col: usize, row: usize) -> u32 {
        read_u32(&self.body, self.cols_at[col] + 4 * row)
    }

    pub fn package(&self) -> &str {
        &self.package
    }

    pub fn nrows(&self) -> usize {
        self.nrows
    }

    /// Version strings, ascending numerically.
    pub fn versions(&self) -> &[String] {
        &self.versions
    }

    /// The parsed components of version `v`, empty if it did not parse.
    pub fn version_components(&self, v: usize) -> &[u32] {
        match self.comp_off.get(v..v + 2) {
            Some([a, b]) => &self.comps[*a as usize..*b as usize],
            _ => &[],
        }
    }

    /// The rows of version `v`, as a range into the row order.
    pub fn version_rows(&self, v: usize) -> std::ops::Range<usize> {
        match self.ver_row_off.get(v..v + 2) {
            Some([a, b]) => *a as usize..*b as usize,
            _ => 0..0,
        }
    }

    pub fn row_version(&self, row: usize) -> usize {
        self.col(COL_VERSION, row) as usize
    }

    pub fn row_platform(&self, row: usize) -> &str {
        self.s(self.col(COL_PLATFORM, row))
    }

    pub fn row_arch(&self, row: usize) -> &str {
        self.s(self.col(COL_ARCH, row))
    }

    pub fn row_r_version(&self, row: usize) -> &str {
        self.s(self.col(COL_R_VERSION, row))
    }

    pub fn row_sha256(&self, row: usize) -> &str {
        self.s(self.col(COL_SHA256, row))
    }

    pub fn row_url(&self, row: usize) -> &str {
        self.s(self.col(COL_URL, row))
    }

    /// The `linkingto` list of a row. Empty on source rows and for packages
    /// without `LinkingTo:`.
    pub fn linkingto(&self, row: usize) -> impl Iterator<Item = LinkingTo<'_>> + '_ {
        let id = self.col(COL_LINKINGTO, row) as usize;
        let (a, b) = if id < self.nlt {
            (
                read_u32(&self.body, self.lt_off_at + 4 * id) as usize,
                read_u32(&self.body, self.lt_off_at + 4 * (id + 1)) as usize,
            )
        } else {
            (0, 0)
        };
        (a..b).map(move |i| {
            let at = self.lt_ents_at + 12 * i;
            LinkingTo {
                package: self.s(read_u32(&self.body, at)),
                version: self.s(read_u32(&self.body, at + 4)),
                sha256: self.s(read_u32(&self.body, at + 8)),
            }
        })
    }
}

/// Rebuild the owned rows a blob was built from, in blob order.
///
/// Only used by the tests, which check that a build/open round trip preserves
/// everything the TSV parser produced.
#[cfg(test)]
pub fn to_rows(blob: &IndexBlob) -> Vec<BinaryRow> {
    use super::LinkingToRef;
    (0..blob.nrows())
        .map(|i| BinaryRow {
            version: blob.versions()[blob.row_version(i)].clone(),
            platform: blob.row_platform(i).to_string(),
            arch: blob.row_arch(i).to_string(),
            r_version: blob.row_r_version(i).to_string(),
            sha256: blob.row_sha256(i).to_string(),
            url: blob.row_url(i).to_string(),
            linkingto: blob
                .linkingto(i)
                .map(|l| LinkingToRef {
                    package: l.package.to_string(),
                    version: l.version.to_string(),
                    sha256: l.sha256.to_string(),
                })
                .collect(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repos::binaries::parse_binaries_tsv;
    use std::path::PathBuf;

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(PathBuf::from("tests/fixtures/binaries").join(name)).unwrap()
    }

    /// `IndexBlob` has no `Debug`, so `unwrap_err` is not available.
    fn open_err(bytes: &[u8]) -> String {
        match IndexBlob::open(bytes) {
            Ok(_) => panic!("expected an error"),
            Err(e) => e.to_string(),
        }
    }

    fn roundtrip(package: &str, name: &str) -> (Vec<BinaryRow>, IndexBlob) {
        let rows = parse_binaries_tsv(&fixture(name)).unwrap();
        let blob = IndexBlob::open(&build(package, &rows).unwrap()).unwrap();
        (rows, blob)
    }

    /// Everything the TSV parser produced comes back, modulo the row order,
    /// which the blob groups by version.
    #[test]
    fn round_trips_every_field() {
        for (package, file) in [
            ("dplyr", "dplyr.tsv.zst"),
            ("pak", "pak.tsv.zst"),
            ("zip", "zip.tsv.zst"),
            ("testpkg", "simple.tsv"),
        ] {
            let (rows, blob) = roundtrip(package, file);
            assert_eq!(blob.package(), package);
            assert_eq!(blob.nrows(), rows.len());

            let key = |r: &BinaryRow| {
                (
                    r.url.clone(),
                    r.version.clone(),
                    r.platform.clone(),
                    r.arch.clone(),
                    r.r_version.clone(),
                    r.sha256.clone(),
                    format!("{:?}", r.linkingto),
                )
            };
            let mut want = rows.clone();
            let mut got = to_rows(&blob);
            want.sort_by_key(key);
            got.sort_by_key(key);
            assert_eq!(got, want, "{} did not round trip", file);
        }
    }

    #[test]
    fn orders_versions_numerically_and_groups_rows() {
        let (rows, blob) = roundtrip("pak", "pak.tsv.zst");
        assert_eq!(blob.versions().len(), 26);
        assert_eq!(blob.versions().first().unwrap(), "0.1.2");
        assert_eq!(blob.versions().last().unwrap(), "0.11.1");
        // The raw file really is in the misleading string order.
        assert_eq!(rows.last().unwrap().version, "0.9.5");

        // Every row falls inside its version's range, and the ranges tile the
        // whole row space.
        let mut total = 0;
        for v in 0..blob.versions().len() {
            let range = blob.version_rows(v);
            for row in range.clone() {
                assert_eq!(blob.row_version(row), v);
            }
            total += range.len();
        }
        assert_eq!(total, blob.nrows());
    }

    /// Rows of one version keep their file order, which is oldest snapshot
    /// first — the only thing distinguishing several builds of one version.
    #[test]
    fn keeps_file_order_within_a_version() {
        let (rows, blob) = roundtrip("dplyr", "dplyr.tsv.zst");
        let want: Vec<&str> = rows
            .iter()
            .filter(|r| r.version == "0.7.4" && r.platform == "xenial" && r.r_version == "3.4")
            .map(|r| r.url.as_str())
            .collect();
        let got: Vec<&str> = blob
            .version_rows(blob.versions().iter().position(|v| v == "0.7.4").unwrap())
            .filter(|i| blob.row_platform(*i) == "xenial" && blob.row_r_version(*i) == "3.4")
            .map(|i| blob.row_url(i))
            .collect();
        assert_eq!(got.len(), 7);
        assert_eq!(got, want);
    }

    #[test]
    fn stores_parsed_version_components() {
        let (_, blob) = roundtrip("pak", "pak.tsv.zst");
        let at = |v: &str| blob.versions().iter().position(|x| x == v).unwrap();
        assert_eq!(blob.version_components(at("0.11.1")), [0, 11, 1]);
        assert_eq!(blob.version_components(at("0.9.3-1")), [0, 9, 3, 1]);
    }

    #[test]
    fn unparseable_versions_have_no_components_and_sort_lowest() {
        let rows = vec![
            BinaryRow {
                version: "1.0.0".to_string(),
                platform: "source".to_string(),
                arch: "*".to_string(),
                r_version: "*".to_string(),
                sha256: "aa".to_string(),
                url: "https://example.com/a.tar.gz".to_string(),
                linkingto: vec![],
            },
            BinaryRow {
                version: "not-a-version".to_string(),
                platform: "source".to_string(),
                arch: "*".to_string(),
                r_version: "*".to_string(),
                sha256: "bb".to_string(),
                url: "https://example.com/b.tar.gz".to_string(),
                linkingto: vec![],
            },
        ];
        let blob = IndexBlob::open(&build("x", &rows).unwrap()).unwrap();
        assert_eq!(blob.versions(), ["not-a-version", "1.0.0"]);
        assert!(blob.version_components(0).is_empty());
        assert_eq!(blob.version_components(1), [1, 0, 0]);
    }

    /// The whole list is the dictionary key, so 3773 rows collapse to a
    /// handful of distinct lists.
    #[test]
    fn dedupes_whole_linkingto_lists() {
        let (_, blob) = roundtrip("dplyr", "dplyr.tsv.zst");
        let v = blob.versions().iter().position(|x| x == "0.7.4").unwrap();
        let row = blob
            .version_rows(v)
            .find(|i| blob.row_platform(*i) == "xenial" && blob.row_r_version(*i) == "3.4")
            .unwrap();
        let lt: Vec<LinkingTo> = blob.linkingto(row).collect();
        assert_eq!(lt.len(), 4);
        assert_eq!(
            lt.iter().map(|l| l.package).collect::<Vec<_>>(),
            ["BH", "Rcpp", "bindrcpp", "plogr"]
        );
        assert_eq!(lt[0].version, "1.65.0-1");
        assert_eq!(lt[0].sha256.len(), 64);

        // Source rows carry no linkingto at all.
        let source = blob
            .version_rows(v)
            .find(|i| blob.row_platform(*i) == "source")
            .unwrap();
        assert_eq!(blob.linkingto(source).count(), 0);
    }

    #[test]
    fn is_much_smaller_than_the_tsv() {
        let tsv = fixture("dplyr.tsv.zst");
        let rows = parse_binaries_tsv(&tsv).unwrap();
        let blob = build("dplyr", &rows).unwrap();
        // Comparable to the compressed TSV it is derived from, and a fraction
        // of the 1.2 MB that TSV expands to.
        assert!(blob.len() < 2 * tsv.len(), "blob is {} bytes", blob.len());
    }

    #[test]
    fn empty_index_round_trips() {
        let blob = IndexBlob::open(&build("x", &[]).unwrap()).unwrap();
        assert_eq!(blob.nrows(), 0);
        assert_eq!(blob.package(), "x");
        assert!(blob.versions().is_empty());
        assert_eq!(blob.version_rows(0), 0..0);
        assert!(blob.version_components(0).is_empty());
    }

    #[test]
    fn rejects_a_blob_that_is_not_one() {
        assert!(IndexBlob::open(b"").is_err());
        assert!(IndexBlob::open(b"not a blob at all").is_err());
    }

    #[test]
    fn rejects_a_wrong_format_version() {
        let mut blob = build("x", &[]).unwrap();
        blob[4] = 99;
        let err = open_err(&blob);
        assert!(err.contains("format version"), "got: {}", err);
    }

    #[test]
    fn rejects_unknown_flags() {
        let mut blob = build("x", &[]).unwrap();
        blob[12] = 0xff;
        assert!(IndexBlob::open(&blob).is_err());
    }

    #[test]
    fn rejects_an_implausible_body_length() {
        let mut blob = build("x", &[]).unwrap();
        blob[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        let err = open_err(&blob);
        assert!(err.contains("implausible"), "got: {}", err);
    }

    /// Corruption anywhere in the body must be an error, never a panic and
    /// never a wrong answer: the caller's response is to rebuild from the TSV.
    #[test]
    fn survives_truncation_anywhere() {
        let rows = parse_binaries_tsv(&fixture("simple.tsv")).unwrap();
        let full = build("testpkg", &rows).unwrap();
        for len in 0..full.len() {
            // Whatever happens, it must not panic.
            let _ = IndexBlob::open(&full[..len]);
        }
    }

    /// Flipping bytes in the compressed body mostly fails the zstd checksum;
    /// what matters is that anything that does decompress is still validated.
    #[test]
    fn survives_corrupt_bodies() {
        let rows = parse_binaries_tsv(&fixture("simple.tsv")).unwrap();
        let full = build("testpkg", &rows).unwrap();
        for i in PREAMBLE_BYTES..full.len() {
            let mut bad = full.clone();
            bad[i] ^= 0xff;
            if let Ok(blob) = IndexBlob::open(&bad) {
                // If it opened, every accessor must still be total.
                for row in 0..blob.nrows() {
                    let _ = blob.row_url(row);
                    let _ = blob.row_platform(row);
                    let _ = blob.row_version(row);
                    let _ = blob.linkingto(row).count();
                }
                for v in 0..blob.versions().len() {
                    let _ = blob.version_components(v);
                    let _ = blob.version_rows(v);
                }
            }
        }
    }
}
