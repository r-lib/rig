//! Scan the source files of a project for the R packages they use, for
//! `rig proj scan` and `rig proj init --scan`.
//!
//! The scan is static and heuristic, like `renv::dependencies()` and
//! `pkgdepends::scan_deps()`, which it follows: R code is parsed with
//! tree-sitter-r, and calls like `library(pkg)`, `pkg::fn()`,
//! `requireNamespace("pkg")` are picked up. R Markdown, Quarto and Sweave
//! files are scanned chunk by chunk, using knitr's chunk patterns. A
//! package that is loaded dynamically, e.g. `library(x, character.only =
//! TRUE)`, is reported as a problem instead of being guessed.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::io::Read;
use std::path::Path;

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use lazy_static::lazy_static;
use regex::Regex;
use serde::Serialize;
use tree_sitter::{Node, Parser, Point, Range};

use crate::proj::BASE_PKGS;
use crate::rproj::{Rproj, RPROJ_MANIFEST_FILE};

/// Directories that are never scanned, wherever they are in the project.
const SKIP_DIRS: &[&str] = &[
    "renv",
    "packrat",
    "revdep",
    "vendor",
    "node_modules",
    "rsconnect",
];

/// A comment that excludes the hits on its line from the scan.
const IGNORE_COMMENT: &str = "# rig: ignore";

/// Whether a package is needed to run the project's code, or only to
/// develop it: tests, vignettes, linters, etc.
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum DepType {
    Prod,
    Dev,
}

impl std::fmt::Display for DepType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DepType::Prod => write!(f, "prod"),
            DepType::Dev => write!(f, "dev"),
        }
    }
}

/// How a package use was found.
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum HitKind {
    /// A call that loads or attaches the package, e.g. `library(pkg)`.
    Call,
    /// `pkg::fn` or `pkg:::fn`.
    Namespace,
    /// Implied by the file type or by a function, e.g. an `.Rmd` file needs
    /// rmarkdown.
    Implicit,
    /// A field in the YAML header of an R Markdown or Quarto document.
    Yaml,
}

/// One use of a package in one file.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct ScanHit {
    /// Path relative to the project root, with `/` separators.
    pub path: String,
    pub package: String,
    #[serde(rename = "type")]
    pub dep_type: DepType,
    pub kind: HitKind,
    /// The source line of the hit, trimmed.
    pub code: String,
    /// 1-based line and column.
    pub line: usize,
    pub column: usize,
}

/// Something the scan could not handle, e.g. a dynamic `library()` call.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct ScanProblem {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    pub message: String,
}

#[derive(Default, Debug)]
pub struct ScanResult {
    pub hits: Vec<ScanHit>,
    pub problems: Vec<ScanProblem>,
}

#[derive(Default, Debug, Clone)]
pub struct ScanOptions {
    /// Extra glob patterns of files and directories to skip, relative to
    /// the project root.
    pub exclude: Vec<String>,
    /// Packages that are dropped from the result.
    pub ignore_packages: Vec<String>,
    /// The project's own package, dropped from the result.
    pub own_package: Option<String>,
}

/// The `[tool.rig.scan]` table of `rproj.toml`.
#[derive(Default, Debug, Clone, PartialEq)]
pub struct ScanConfig {
    pub exclude: Vec<String>,
    pub ignore_packages: Vec<String>,
}

impl ScanConfig {
    /// Read `[tool.rig.scan]` from a manifest. Every key is optional.
    pub fn from_manifest(manifest: &Rproj) -> Result<ScanConfig, Box<dyn Error>> {
        let Some(scan) = manifest.tool.get("rig").and_then(|t| t.get("scan")) else {
            return Ok(ScanConfig::default());
        };
        let Some(scan) = scan.as_table() else {
            bail!(
                "`scan` in `[tool.rig]` of {} must be a table",
                RPROJ_MANIFEST_FILE
            );
        };
        let strings = |key: &str| -> Result<Vec<String>, Box<dyn Error>> {
            let Some(value) = scan.get(key) else {
                return Ok(vec![]);
            };
            let err = || {
                format!(
                    "`{}` in `[tool.rig.scan]` of {} must be an array of strings",
                    key, RPROJ_MANIFEST_FILE
                )
            };
            let Some(array) = value.as_array() else {
                bail!(err());
            };
            let mut out = Vec::new();
            for item in array {
                match item.as_str() {
                    Some(s) => out.push(s.to_string()),
                    None => bail!(err()),
                }
            }
            Ok(out)
        };
        Ok(ScanConfig {
            exclude: strings("exclude")?,
            ignore_packages: strings("ignore-packages")?,
        })
    }

    /// The scan options for a project with this config.
    pub fn scan_options(&self, manifest: Option<&Rproj>) -> ScanOptions {
        let own_package = manifest
            .filter(|m| m.project.is_package() && !m.project.name.is_empty())
            .map(|m| m.project.name.clone());
        ScanOptions {
            exclude: self.exclude.clone(),
            ignore_packages: self.ignore_packages.clone(),
            own_package,
        }
    }
}

// ---------------------------------------------------------------------------
// Walking the project
// ---------------------------------------------------------------------------

fn build_globset(patterns: &[String]) -> Result<GlobSet, Box<dyn Error>> {
    let mut builder = GlobSetBuilder::new();
    for pat in patterns {
        let pat = pat.trim_start_matches("./").trim_end_matches('/');
        let glob = GlobBuilder::new(pat)
            .literal_separator(true)
            .build()
            .map_err(|e| format!("Invalid exclude pattern '{}': {}", pat, e))?;
        builder.add(glob);
    }
    Ok(builder.build()?)
}

/// Path of `path` relative to `root`, with `/` separators.
fn rel_path(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Scan the project at `root`.
pub fn scan_project(root: &Path, opts: &ScanOptions) -> Result<ScanResult, Box<dyn Error>> {
    let globs = build_globset(&opts.exclude)?;
    let mut result = ScanResult::default();
    let mut parser = new_parser();

    let walk_root = root.to_path_buf();
    let walk_globs = globs.clone();
    let mut builder = ignore::WalkBuilder::new(root);
    builder
        .hidden(true)
        .ignore(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(true)
        .require_git(false)
        .follow_links(false)
        .add_custom_ignore_filename(".renvignore")
        .sort_by_file_name(|a, b| a.cmp(b))
        .filter_entry(move |entry| {
            if entry.depth() == 0 {
                return true;
            }
            let rel = rel_path(&walk_root, entry.path());
            if walk_globs.is_match(&rel) {
                return false;
            }
            if entry.file_type().is_some_and(|t| t.is_dir()) {
                let name = entry.file_name().to_string_lossy();
                if SKIP_DIRS.contains(&name.as_ref()) {
                    return false;
                }
                // A nested project has its own manifest and its own scan.
                if entry.path().join(RPROJ_MANIFEST_FILE).exists() {
                    return false;
                }
            }
            true
        });

    for entry in builder.build() {
        let entry = match entry {
            Ok(e) => e,
            Err(err) => {
                result.problems.push(ScanProblem {
                    path: String::new(),
                    line: None,
                    message: err.to_string(),
                });
                continue;
            }
        };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let rel = rel_path(root, entry.path());
        if let Err(err) = scan_file(&mut parser, entry.path(), &rel, &mut result) {
            result.problems.push(ScanProblem {
                path: rel,
                line: None,
                message: err.to_string(),
            });
        }
    }

    // Hidden files and skipped directories that still imply a package.
    if root.join(".lintr").is_file() && !globs.is_match(".lintr") {
        result
            .hits
            .push(implicit_hit(".lintr", "lintr", DepType::Dev, 1));
    }
    if root.join("rsconnect").is_dir() && !globs.is_match("rsconnect") {
        result
            .hits
            .push(implicit_hit("rsconnect", "rsconnect", DepType::Dev, 1));
    }

    filter_hits(&mut result.hits, opts);
    result
        .hits
        .sort_by(|a, b| (&a.path, a.line, a.column).cmp(&(&b.path, b.line, b.column)));
    Ok(result)
}

/// Drop base packages, `R`, the project's own package and the ignored
/// packages.
fn filter_hits(hits: &mut Vec<ScanHit>, opts: &ScanOptions) {
    hits.retain(|hit| {
        hit.package != "R"
            && !BASE_PKGS.contains(&hit.package.as_str())
            && opts.own_package.as_deref() != Some(hit.package.as_str())
            && !opts.ignore_packages.contains(&hit.package)
    });
}

/// Whether a file at this relative path is development-only code.
fn path_dep_type(rel: &str) -> DepType {
    let first = rel.split('/').next().unwrap_or("");
    let in_dir = rel.contains('/');
    if (in_dir && matches!(first, "tests" | "test" | "vignettes"))
        || rel.starts_with("inst/doc/")
        || rel.starts_with("man/roxygen/")
    {
        DepType::Dev
    } else {
        DepType::Prod
    }
}

fn implicit_hit(path: &str, package: &str, dep_type: DepType, line: usize) -> ScanHit {
    ScanHit {
        path: path.to_string(),
        package: package.to_string(),
        dep_type,
        kind: HitKind::Implicit,
        code: String::new(),
        line,
        column: 1,
    }
}

fn read_text(path: &Path) -> Result<String, Box<dyn Error>> {
    let bytes = std::fs::read(path)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Whether an extensionless file is an R script, from its shebang line.
fn has_r_shebang(path: &Path) -> bool {
    lazy_static! {
        static ref RE_SHEBANG: Regex = Regex::new(r"^#!.*\b(R|r|Rscript)\b").unwrap();
    }
    let mut buf = [0u8; 256];
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let Ok(n) = file.read(&mut buf) else {
        return false;
    };
    let head = String::from_utf8_lossy(&buf[..n]);
    let first = head.lines().next().unwrap_or("");
    RE_SHEBANG.is_match(first)
}

fn scan_file(
    parser: &mut Parser,
    path: &Path,
    rel: &str,
    result: &mut ScanResult,
) -> Result<(), Box<dyn Error>> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let dep_type = path_dep_type(rel);

    match name.as_str() {
        "_bookdown.yml" => {
            result
                .hits
                .push(implicit_hit(rel, "bookdown", DepType::Prod, 1));
            return Ok(());
        }
        "_pkgdown.yml" => {
            result
                .hits
                .push(implicit_hit(rel, "pkgdown", DepType::Dev, 1));
            return Ok(());
        }
        _ => {}
    }

    match ext.as_str() {
        "r" => {
            let src = read_text(path)?;
            scan_r_code(parser, &src, None, rel, dep_type, result)?;
        }
        "rmd" | "rmarkdown" => {
            let src = read_text(path)?;
            scan_markdown(parser, &src, rel, dep_type, false, result)?;
        }
        "qmd" => {
            let src = read_text(path)?;
            scan_markdown(parser, &src, rel, dep_type, true, result)?;
        }
        "rnw" => {
            let src = read_text(path)?;
            scan_rnw(parser, &src, rel, dep_type, result)?;
        }
        "rproj" => {
            let src = read_text(path)?;
            if src.lines().any(|l| l.trim() == "PackageUseDevtools: Yes") {
                for pkg in ["devtools", "roxygen2"] {
                    result.hits.push(implicit_hit(rel, pkg, DepType::Dev, 1));
                }
            }
        }
        "" if has_r_shebang(path) => {
            let src = read_text(path)?;
            scan_r_code(parser, &src, None, rel, dep_type, result)?;
        }
        _ => {}
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// R code
// ---------------------------------------------------------------------------

fn new_parser() -> Parser {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_r::LANGUAGE.into())
        .expect("the R grammar is compatible with tree-sitter");
    parser
}

/// Parse R code and collect the package uses. `range`, if given, restricts
/// parsing to that byte range of `src` (a code chunk), so positions are
/// still positions in the whole file.
fn scan_r_code(
    parser: &mut Parser,
    src: &str,
    range: Option<Range>,
    rel: &str,
    dep_type: DepType,
    result: &mut ScanResult,
) -> Result<(), Box<dyn Error>> {
    match range {
        Some(r) => parser.set_included_ranges(&[r])?,
        None => parser.set_included_ranges(&[])?,
    }
    let Some(tree) = parser.parse(src, None) else {
        bail!("Could not parse R code");
    };
    let mut scanner = RScanner {
        src,
        rel,
        dep_type,
        result,
    };
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        match node.kind() {
            "namespace_operator" => scanner.namespace(node),
            "call" => scanner.call(node),
            _ => {}
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }
    Ok(())
}

/// One function argument: its name, if named, and its value.
struct CallArg<'t> {
    name: Option<String>,
    value: Option<Node<'t>>,
}

struct RScanner<'a> {
    src: &'a str,
    rel: &'a str,
    dep_type: DepType,
    result: &'a mut ScanResult,
}

fn is_valid_package_name(name: &str) -> bool {
    lazy_static! {
        static ref RE_PKG: Regex = Regex::new(r"^[A-Za-z][A-Za-z0-9.]*[A-Za-z0-9]$").unwrap();
    }
    RE_PKG.is_match(name)
}

/// Strip a version requirement, e.g. `"dplyr (>= 1.0.0)"` -> `"dplyr"`.
fn strip_version(name: &str) -> &str {
    name.split(|c: char| c == '(' || c.is_whitespace())
        .next()
        .unwrap_or("")
}

impl<'a> RScanner<'a> {
    fn text(&self, node: Node) -> &'a str {
        &self.src[node.byte_range()]
    }

    /// The trimmed source line that `row` (0-based) is on.
    fn line_text(&self, row: usize) -> &'a str {
        self.src.lines().nth(row).unwrap_or("").trim()
    }

    fn push_hit(&mut self, at: Node, package: &str, kind: HitKind) {
        if !is_valid_package_name(package) {
            return;
        }
        let pos = at.start_position();
        let line = self.line_text(pos.row);
        if line.contains(IGNORE_COMMENT) {
            return;
        }
        self.result.hits.push(ScanHit {
            path: self.rel.to_string(),
            package: package.to_string(),
            dep_type: self.dep_type,
            kind,
            code: line.to_string(),
            line: pos.row + 1,
            column: pos.column + 1,
        });
    }

    fn push_problem(&mut self, at: Node, message: String) {
        let row = at.start_position().row;
        if self.line_text(row).contains(IGNORE_COMMENT) {
            return;
        }
        self.result.problems.push(ScanProblem {
            path: self.rel.to_string(),
            line: Some(row + 1),
            message,
        });
    }

    /// A symbol or a string literal, as a string.
    fn symbol_or_string(&self, node: Node) -> Option<String> {
        match node.kind() {
            "identifier" => Some(self.text(node).trim_matches('`').to_string()),
            "string" => self.string_value(node),
            _ => None,
        }
    }

    fn string_value(&self, node: Node) -> Option<String> {
        if node.kind() != "string" {
            return None;
        }
        match node.child_by_field_name("content") {
            Some(content) => Some(self.text(content).to_string()),
            None => Some(String::new()),
        }
    }

    /// The string literals of a string or of a `c(...)` call of strings.
    fn strings_in(&self, node: Node) -> Vec<String> {
        match node.kind() {
            "string" => self.string_value(node).into_iter().collect(),
            "call" => {
                let is_c = node
                    .child_by_field_name("function")
                    .is_some_and(|f| f.kind() == "identifier" && self.text(f) == "c");
                if !is_c {
                    return vec![];
                }
                self.args(node)
                    .iter()
                    .filter_map(|a| a.value)
                    .flat_map(|v| self.strings_in(v))
                    .collect()
            }
            _ => vec![],
        }
    }

    fn args<'t>(&self, call: Node<'t>) -> Vec<CallArg<'t>> {
        let Some(arguments) = call.child_by_field_name("arguments") else {
            return vec![];
        };
        let mut out = Vec::new();
        let mut cursor = arguments.walk();
        for arg in arguments.named_children(&mut cursor) {
            if arg.kind() != "argument" {
                continue;
            }
            let name = arg
                .child_by_field_name("name")
                .and_then(|n| self.symbol_or_string(n));
            let value = arg.child_by_field_name("value");
            out.push(CallArg { name, value });
        }
        out
    }

    /// The value of an argument, matched by exact name, or else by position
    /// among the unnamed arguments.
    fn match_arg<'t>(&self, args: &[CallArg<'t>], name: &str, position: usize) -> Option<Node<'t>> {
        if let Some(arg) = args.iter().find(|a| a.name.as_deref() == Some(name)) {
            return arg.value;
        }
        args.iter()
            .filter(|a| a.name.is_none())
            .nth(position)
            .and_then(|a| a.value)
    }

    fn is_true(&self, node: Option<Node>) -> bool {
        match node {
            Some(n) => {
                matches!(n.kind(), "true") || (n.kind() == "identifier" && self.text(n) == "T")
            }
            None => false,
        }
    }

    /// `pkg::fn` and `pkg:::fn`.
    fn namespace(&mut self, node: Node) {
        let Some(lhs) = node.child_by_field_name("lhs") else {
            return;
        };
        let Some(pkg) = self.symbol_or_string(lhs) else {
            return;
        };
        if pkg == "base" {
            return;
        }
        self.push_hit(node, &pkg, HitKind::Namespace);
    }

    fn call(&mut self, node: Node) {
        let Some(func) = node.child_by_field_name("function") else {
            return;
        };
        let (ns, name) = match func.kind() {
            "identifier" => (None, self.text(func).to_string()),
            "namespace_operator" => {
                let ns = func
                    .child_by_field_name("lhs")
                    .and_then(|n| self.symbol_or_string(n));
                let name = func
                    .child_by_field_name("rhs")
                    .and_then(|n| self.symbol_or_string(n));
                match (ns, name) {
                    (Some(ns), Some(name)) => (Some(ns), name),
                    _ => return,
                }
            }
            _ => return,
        };
        let ns = ns.as_deref();
        let ns_is = |allowed: &[&str]| ns.is_none() || ns.is_some_and(|n| allowed.contains(&n));

        match name.as_str() {
            "library" | "require" if ns_is(&["base"]) => self.library(node),
            "requireNamespace" | "loadNamespace" if ns_is(&["base"]) => {
                let args = self.args(node);
                if let Some(value) = self.match_arg(&args, "package", 0) {
                    if let Some(pkg) = self.string_value(value) {
                        self.push_hit(value, &pkg, HitKind::Call);
                    }
                }
            }
            "p_load" if ns_is(&["pacman"]) => self.p_load(node),
            "use" if ns == Some("box") => self.box_use(node),
            "use" if ns_is(&["base"]) => {
                let args = self.args(node);
                if let Some(value) = self.match_arg(&args, "package", 0) {
                    if let Some(pkg) = self.symbol_or_string(value) {
                        self.push_hit(value, &pkg, HitKind::Call);
                    }
                }
            }
            "from" | "here" | "into" if ns == Some("import") => {
                let args = self.args(node);
                let position = if name == "into" { 1 } else { 0 };
                if let Some(value) = self.match_arg(&args, ".from", position) {
                    if let Some(pkg) = self.symbol_or_string(value) {
                        if !pkg.ends_with(".R") && !pkg.ends_with(".r") {
                            self.push_hit(value, &pkg, HitKind::Call);
                        }
                    }
                }
            }
            "import" if ns == Some("modules") => {
                let args = self.args(node);
                if let Some(value) = self.match_arg(&args, "from", 0) {
                    if let Some(pkg) = self.symbol_or_string(value) {
                        self.push_hit(value, &pkg, HitKind::Call);
                    }
                }
            }
            "check_installed" | "is_installed" if ns_is(&["rlang"]) => {
                let args = self.args(node);
                if let Some(value) = self.match_arg(&args, "pkg", 0) {
                    for pkg in self.strings_in(value) {
                        self.push_hit(value, strip_version(&pkg), HitKind::Call);
                    }
                }
            }
            "pkg_attach" | "pkg_attach2" if ns_is(&["xfun"]) => {
                let args = self.args(node);
                for arg in args.iter().filter(|a| a.name.is_none()) {
                    if let Some(value) = arg.value {
                        for pkg in self.strings_in(value) {
                            self.push_hit(value, &pkg, HitKind::Call);
                        }
                    }
                }
            }
            "tar_option_set" if ns_is(&["targets"]) => {
                self.push_hit(node, "targets", HitKind::Implicit);
                let args = self.args(node);
                if let Some(value) = args
                    .iter()
                    .find(|a| a.name.as_deref() == Some("packages"))
                    .and_then(|a| a.value)
                {
                    for pkg in self.strings_in(value) {
                        self.push_hit(value, &pkg, HitKind::Call);
                    }
                }
            }
            "geom_hex" if ns_is(&["ggplot2"]) => {
                self.push_hit(node, "hexbin", HitKind::Implicit);
            }
            _ => {}
        }
    }

    /// `library()` and `require()`.
    fn library(&mut self, node: Node) {
        let args = self.args(node);
        let Some(value) = self.match_arg(&args, "package", 0) else {
            return;
        };
        let character_only = self.is_true(
            args.iter()
                .find(|a| a.name.as_deref() == Some("character.only"))
                .and_then(|a| a.value),
        );
        match value.kind() {
            "string" => {
                if let Some(pkg) = self.string_value(value) {
                    self.push_hit(value, &pkg, HitKind::Call);
                }
            }
            "identifier" if !character_only => {
                let pkg = self.text(value).trim_matches('`').to_string();
                self.push_hit(value, &pkg, HitKind::Call);
            }
            _ => {
                let code = self.text(node).lines().next().unwrap_or("").to_string();
                self.push_problem(
                    node,
                    format!("Could not tell which package `{}` loads", code),
                );
            }
        }
    }

    /// `pacman::p_load()`.
    fn p_load(&mut self, node: Node) {
        let args = self.args(node);
        let character_only = self.is_true(
            args.iter()
                .find(|a| a.name.as_deref() == Some("character.only"))
                .and_then(|a| a.value),
        );
        for arg in args.iter() {
            let Some(value) = arg.value else { continue };
            match arg.name.as_deref() {
                Some("char") => {
                    for pkg in self.strings_in(value) {
                        self.push_hit(value, &pkg, HitKind::Call);
                    }
                }
                Some(_) => {}
                None => match value.kind() {
                    "string" => {
                        if let Some(pkg) = self.string_value(value) {
                            self.push_hit(value, &pkg, HitKind::Call);
                        }
                    }
                    "identifier" if !character_only => {
                        let pkg = self.text(value).trim_matches('`').to_string();
                        self.push_hit(value, &pkg, HitKind::Call);
                    }
                    _ => {}
                },
            }
        }
    }

    /// `box::use(pkg, pkg[fn], alias = pkg, ./local/module)`.
    fn box_use(&mut self, node: Node) {
        let args = self.args(node);
        for arg in args.iter() {
            let Some(value) = arg.value else { continue };
            let target = match value.kind() {
                "identifier" => Some(value),
                "subset" => value
                    .child_by_field_name("function")
                    .filter(|f| f.kind() == "identifier"),
                _ => None,
            };
            if let Some(target) = target {
                let pkg = self.text(target).trim_matches('`').to_string();
                self.push_hit(target, &pkg, HitKind::Call);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// R Markdown, Quarto, Sweave
// ---------------------------------------------------------------------------

/// Byte offset and 0-based row of the start of each line.
fn line_starts(src: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (i, b) in src.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    starts
}

/// The tree-sitter point of a byte offset.
fn point_at(starts: &[usize], byte: usize) -> Point {
    let row = match starts.binary_search(&byte) {
        Ok(i) => i,
        Err(i) => i - 1,
    };
    Point {
        row,
        column: byte - starts[row],
    }
}

fn byte_range(starts: &[usize], start: usize, end: usize) -> Range {
    Range {
        start_byte: start,
        end_byte: end,
        start_point: point_at(starts, start),
        end_point: point_at(starts, end),
    }
}

/// The lines of `src`, each with its byte offset, without line endings.
fn lines_with_offsets(src: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut offset = 0;
    for line in src.split_inclusive('\n') {
        let text = line.trim_end_matches('\n').trim_end_matches('\r');
        out.push((offset, text));
        offset += line.len();
    }
    out
}

/// Whether a chunk is evaluated, from its header options (`eval = FALSE`,
/// etc.) and its `#|` option lines.
fn chunk_is_evaluated(options: &str, body: &str) -> bool {
    lazy_static! {
        static ref RE_EVAL_FALSE: Regex = Regex::new(r"\beval\s*=\s*(FALSE|F)\b").unwrap();
        static ref RE_EXERCISE: Regex = Regex::new(r"\bexercise\s*=\s*(TRUE|T)\b").unwrap();
        static ref RE_IGNORE_TRUE: Regex = Regex::new(r"\brenv\.ignore\s*=\s*(TRUE|T)\b").unwrap();
        static ref RE_IGNORE_FALSE: Regex =
            Regex::new(r"\brenv\.ignore\s*=\s*(FALSE|F)\b").unwrap();
        static ref RE_YAML_EVAL_FALSE: Regex =
            Regex::new(r"^\s*#\|\s*eval\s*[:=]\s*(false|FALSE|F)\s*$").unwrap();
        static ref RE_YAML_EXERCISE: Regex =
            Regex::new(r"^\s*#\|\s*exercise\s*[:=]\s*(true|TRUE|T)\s*$").unwrap();
        static ref RE_YAML_IGNORE_TRUE: Regex =
            Regex::new(r"^\s*#\|\s*renv\.ignore\s*[:=]\s*(true|TRUE|T)\s*$").unwrap();
        static ref RE_YAML_IGNORE_FALSE: Regex =
            Regex::new(r"^\s*#\|\s*renv\.ignore\s*[:=]\s*(false|FALSE|F)\s*$").unwrap();
    }
    let opt_lines: Vec<&str> = body
        .lines()
        .take_while(|l| l.trim_start().starts_with("#|"))
        .collect();
    let any_line = |re: &Regex| opt_lines.iter().any(|l| re.is_match(l));

    if RE_IGNORE_FALSE.is_match(options) || any_line(&RE_YAML_IGNORE_FALSE) {
        return true;
    }
    if RE_IGNORE_TRUE.is_match(options) || any_line(&RE_YAML_IGNORE_TRUE) {
        return false;
    }
    if RE_EVAL_FALSE.is_match(options) || any_line(&RE_YAML_EVAL_FALSE) {
        return false;
    }
    if RE_EXERCISE.is_match(options) || any_line(&RE_YAML_EXERCISE) {
        return false;
    }
    // The chunk label is the first option without a `=`.
    let label = options
        .trim_start_matches([' ', ','])
        .split(',')
        .next()
        .unwrap_or("")
        .trim();
    if !label.contains('=') && label.ends_with("-display") {
        return false;
    }
    true
}

/// The YAML header of a document: its text and the byte range of the
/// whole header, including the `---` lines.
fn yaml_header(lines: &[(usize, &str)]) -> Option<(String, usize)> {
    let first = lines.first()?;
    if first.1.trim_end() != "---" {
        return None;
    }
    let mut text = String::new();
    for (offset, line) in lines.iter().skip(1) {
        let trimmed = line.trim_end();
        if trimmed == "---" || trimmed == "..." {
            return Some((text, offset + line.len()));
        }
        text.push_str(line);
        text.push('\n');
    }
    None
}

/// Packages named in a YAML header: `pkg::fn` values and keys, and
/// shiny for Shiny documents.
fn yaml_packages(value: &serde_yaml::Value, top: bool, out: &mut Vec<String>) {
    lazy_static! {
        static ref RE_YAML_NS: Regex =
            Regex::new(r"^\s*([A-Za-z][A-Za-z0-9.]*[A-Za-z0-9])\s*:::?").unwrap();
    }
    use serde_yaml::Value;
    match value {
        Value::String(s) => {
            if let Some(m) = RE_YAML_NS.captures(s) {
                out.push(m[1].to_string());
            }
        }
        Value::Sequence(seq) => {
            for v in seq {
                yaml_packages(v, false, out);
            }
        }
        Value::Mapping(map) => {
            for (k, v) in map {
                let key = k.as_str().unwrap_or("");
                if top && key == "runtime" && v.as_str().is_some_and(|s| s.starts_with("shiny")) {
                    out.push("shiny".to_string());
                }
                if top && key == "server" {
                    let is_shiny = v.as_str() == Some("shiny")
                        || v.get("type").and_then(|t| t.as_str()) == Some("shiny");
                    if is_shiny {
                        out.push("shiny".to_string());
                    }
                }
                yaml_packages(k, false, out);
                yaml_packages(v, false, out);
            }
        }
        Value::Tagged(tagged) => yaml_packages(&tagged.value, top, out),
        _ => {}
    }
}

/// Scan an R Markdown (`quarto = false`) or Quarto (`quarto = true`)
/// document.
fn scan_markdown(
    parser: &mut Parser,
    src: &str,
    rel: &str,
    dep_type: DepType,
    quarto: bool,
    result: &mut ScanResult,
) -> Result<(), Box<dyn Error>> {
    lazy_static! {
        static ref RE_START: Regex =
            Regex::new(r"^[\t >]*(`{3,})\s*\{([a-zA-Z0-9_]+)((?:[ ,].*)?)\}\s*$").unwrap();
        static ref RE_END: Regex = Regex::new(r"^[\t >]*(`{3,})\s*$").unwrap();
        static ref RE_INLINE: Regex = Regex::new(r"`r[ #]([^`]+)`").unwrap();
    }

    let starts = line_starts(src);
    let lines = lines_with_offsets(src);

    // YAML header.
    let mut body_start = 0;
    let mut jupyter = false;
    let mut knitr_engine = false;
    if let Some((yaml, end)) = yaml_header(&lines) {
        body_start = end;
        match serde_yaml::from_str::<serde_yaml::Value>(&yaml) {
            Ok(value) => {
                jupyter = value.get("jupyter").is_some();
                knitr_engine = value.get("knitr").is_some()
                    || value.get("engine").and_then(|e| e.as_str()) == Some("knitr");
                let mut pkgs = Vec::new();
                yaml_packages(&value, true, &mut pkgs);
                for pkg in pkgs {
                    if is_valid_package_name(&pkg) {
                        result.hits.push(ScanHit {
                            path: rel.to_string(),
                            package: pkg,
                            dep_type,
                            kind: HitKind::Yaml,
                            code: "YAML header".to_string(),
                            line: 1,
                            column: 1,
                        });
                    }
                }
            }
            Err(err) => result.problems.push(ScanProblem {
                path: rel.to_string(),
                line: Some(1),
                message: format!("Invalid YAML header: {}", err),
            }),
        }
    }

    // Code chunks and inline code.
    struct Chunk {
        fence: usize,
        engine: String,
        options: String,
        body_start: usize,
        line: usize,
    }
    let mut chunk: Option<Chunk> = None;
    let mut r_chunks: Vec<(usize, usize, usize)> = Vec::new();
    let mut python_chunk_line: Option<usize> = None;
    let mut inline: Vec<(usize, usize)> = Vec::new();

    for (i, (offset, line)) in lines.iter().enumerate() {
        if *offset < body_start {
            continue;
        }
        match &chunk {
            None => {
                if let Some(m) = RE_START.captures(line) {
                    chunk = Some(Chunk {
                        fence: m[1].len(),
                        engine: m[2].to_lowercase(),
                        options: m[3].to_string(),
                        body_start: offset + line.len() + 1,
                        line: i,
                    });
                } else {
                    for m in RE_INLINE.captures_iter(line) {
                        let g = m.get(1).unwrap();
                        inline.push((offset + g.start(), offset + g.end()));
                    }
                }
            }
            Some(c) => {
                if let Some(m) = RE_END.captures(line) {
                    if m[1].len() >= c.fence {
                        let end = *offset;
                        let start = c.body_start.min(end);
                        let body = &src[start..end];
                        if matches!(c.engine.as_str(), "r" | "rscript") {
                            if chunk_is_evaluated(&c.options, body) {
                                r_chunks.push((start, end, c.line));
                            }
                        } else if c.engine == "python" && python_chunk_line.is_none() {
                            python_chunk_line = Some(c.line);
                        }
                        chunk = None;
                    }
                }
            }
        }
    }

    // Implied packages. Every R Markdown document needs rmarkdown; a Quarto
    // document only if knitr runs it.
    let first_r_line = r_chunks.first().map(|c| c.2);
    let knitr = !quarto || knitr_engine || (!jupyter && first_r_line.is_some());
    if !quarto {
        result
            .hits
            .push(implicit_hit(rel, "rmarkdown", dep_type, 1));
    } else if knitr {
        if let Some(line) = first_r_line.or(python_chunk_line) {
            result
                .hits
                .push(implicit_hit(rel, "rmarkdown", dep_type, line + 1));
        }
    }
    if knitr {
        if let Some(line) = python_chunk_line {
            result
                .hits
                .push(implicit_hit(rel, "reticulate", dep_type, line + 1));
        }
    }

    for (start, end, _) in r_chunks {
        if start < end {
            let range = byte_range(&starts, start, end);
            scan_r_code(parser, src, Some(range), rel, dep_type, result)?;
        }
    }
    for (start, end) in inline {
        let range = byte_range(&starts, start, end);
        scan_r_code(parser, src, Some(range), rel, dep_type, result)?;
    }

    Ok(())
}

/// Scan a Sweave / knitr `.Rnw` document.
fn scan_rnw(
    parser: &mut Parser,
    src: &str,
    rel: &str,
    dep_type: DepType,
    result: &mut ScanResult,
) -> Result<(), Box<dyn Error>> {
    lazy_static! {
        static ref RE_START: Regex = Regex::new(r"^\s*<<(.*)>>=.*$").unwrap();
        static ref RE_END: Regex = Regex::new(r"^\s*@\s*(%+.*|)$").unwrap();
        static ref RE_ENGINE: Regex = Regex::new(r#"\bengine\s*=\s*["']?([A-Za-z0-9_]+)"#).unwrap();
    }
    let starts = line_starts(src);
    let lines = lines_with_offsets(src);
    let mut chunk: Option<(String, usize)> = None;
    let mut ranges = Vec::new();

    for (offset, line) in lines.iter() {
        if let Some(m) = RE_START.captures(line) {
            // A new chunk also ends the previous one.
            if let Some((options, start)) = chunk.take() {
                ranges.push((options, start, *offset));
            }
            chunk = Some((m[1].to_string(), offset + line.len() + 1));
        } else if RE_END.is_match(line) {
            if let Some((options, start)) = chunk.take() {
                ranges.push((options, start, *offset));
            }
        }
    }
    if let Some((options, start)) = chunk.take() {
        ranges.push((options, start, src.len()));
    }

    for (options, start, end) in ranges {
        let start = start.min(end);
        let engine = RE_ENGINE
            .captures(&options)
            .map(|m| m[1].to_lowercase())
            .unwrap_or_else(|| "r".to_string());
        if !matches!(engine.as_str(), "r" | "rscript") {
            continue;
        }
        if !chunk_is_evaluated(&options, &src[start..end]) || start >= end {
            continue;
        }
        let range = byte_range(&starts, start, end);
        scan_r_code(parser, src, Some(range), rel, dep_type, result)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Summary, check against the manifest
// ---------------------------------------------------------------------------

/// One package found by the scan, with all its uses merged.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct ScannedPackage {
    pub package: String,
    /// `prod` if any non-dev file uses it.
    #[serde(rename = "type")]
    pub dep_type: DepType,
    /// The files that use the package.
    pub files: Vec<String>,
}

/// Merge the hits of a scan into one entry per package. Prod packages come
/// first, then dev packages, each sorted by name (case-insensitive).
pub fn summarize(hits: &[ScanHit]) -> Vec<ScannedPackage> {
    let mut by_pkg: BTreeMap<&str, Vec<&ScanHit>> = BTreeMap::new();
    for hit in hits {
        by_pkg.entry(hit.package.as_str()).or_default().push(hit);
    }
    let mut out: Vec<ScannedPackage> = by_pkg
        .into_iter()
        .map(|(pkg, hits)| {
            let dep_type = hits
                .iter()
                .map(|h| h.dep_type)
                .min()
                .unwrap_or(DepType::Prod);
            let files: BTreeSet<&str> = hits.iter().map(|h| h.path.as_str()).collect();
            ScannedPackage {
                package: pkg.to_string(),
                dep_type,
                files: files.into_iter().map(String::from).collect(),
            }
        })
        .collect();
    out.sort_by(|a, b| {
        a.dep_type
            .cmp(&b.dep_type)
            .then_with(|| a.package.to_lowercase().cmp(&b.package.to_lowercase()))
            .then_with(|| a.package.cmp(&b.package))
    });
    out
}

/// The result of comparing the scan with the declared dependencies.
#[derive(Serialize, Debug, Default, PartialEq)]
pub struct CheckReport {
    /// Used, but not declared anywhere.
    pub missing: Vec<ScannedPackage>,
    /// Used by non-dev code, but only declared as a dev dependency.
    pub misplaced: Vec<ScannedPackage>,
    /// Declared in `[dependencies]`, but not used anywhere.
    pub unused: Vec<String>,
}

impl CheckReport {
    pub fn is_clean(&self, strict: bool) -> bool {
        self.missing.is_empty() && self.misplaced.is_empty() && (!strict || self.unused.is_empty())
    }
}

/// Compare the scanned packages with the dependencies declared in the
/// manifest.
pub fn check_manifest(pkgs: &[ScannedPackage], manifest: &Rproj) -> CheckReport {
    let in_extras = |name: &str| {
        manifest
            .optional_dependencies
            .values()
            .any(|deps| deps.contains_key(name))
    };
    let in_groups = |name: &str| {
        manifest
            .dependency_groups
            .values()
            .any(|g| g.dependencies.contains_key(name))
    };
    let in_prod = |name: &str| {
        manifest.dependencies.contains_key(name) || manifest.linking_dependencies.contains_key(name)
    };

    let mut report = CheckReport::default();
    for pkg in pkgs {
        let name = pkg.package.as_str();
        match pkg.dep_type {
            DepType::Prod => {
                if in_prod(name) || in_extras(name) {
                    continue;
                }
                if in_groups(name) {
                    report.misplaced.push(pkg.clone());
                } else {
                    report.missing.push(pkg.clone());
                }
            }
            DepType::Dev => {
                if !(in_prod(name) || in_extras(name) || in_groups(name)) {
                    report.missing.push(pkg.clone());
                }
            }
        }
    }

    let used: BTreeSet<&str> = pkgs.iter().map(|p| p.package.as_str()).collect();
    for name in manifest.dependencies.keys() {
        if name == "R" || used.contains(name.as_str()) {
            continue;
        }
        report.unused.push(name.clone());
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan_str(code: &str) -> ScanResult {
        let mut parser = new_parser();
        let mut result = ScanResult::default();
        scan_r_code(&mut parser, code, None, "x.R", DepType::Prod, &mut result).unwrap();
        filter_hits(&mut result.hits, &ScanOptions::default());
        result.hits.sort_by_key(|h| (h.line, h.column));
        result
    }

    fn pkgs(code: &str) -> Vec<String> {
        scan_str(code).hits.into_iter().map(|h| h.package).collect()
    }

    #[test]
    fn library_and_require() {
        assert_eq!(
            pkgs("library(dplyr)\nrequire('tidyr')\nlibrary(package = \"ggplot2\")"),
            vec!["dplyr", "tidyr", "ggplot2"]
        );
        assert_eq!(pkgs("base::library(cli)"), vec!["cli"]);
        assert_eq!(
            pkgs("suppressPackageStartupMessages(library(cli))"),
            vec!["cli"]
        );
        assert_eq!(pkgs("library(`data.table`)"), vec!["data.table"]);
        assert!(pkgs("library()").is_empty());
    }

    #[test]
    fn not_in_comments_or_strings() {
        assert!(pkgs("# library(dplyr)\nx <- 'library(tidyr)'\n").is_empty());
        assert!(pkgs("foo::library(dplyr)").iter().all(|p| p == "foo"));
    }

    #[test]
    fn character_only() {
        let res = scan_str("library(pkg, character.only = TRUE)");
        assert!(res.hits.is_empty());
        assert_eq!(res.problems.len(), 1);
        assert_eq!(res.problems[0].line, Some(1));
        assert_eq!(pkgs("library('cli', character.only = TRUE)"), vec!["cli"]);
    }

    #[test]
    fn namespace_ops() {
        assert_eq!(
            pkgs("x <- dplyr::filter(df)\ny <- cli:::internal"),
            vec!["dplyr", "cli"]
        );
        assert!(pkgs("base::paste('a')\nutils::head(x)").is_empty());
    }

    #[test]
    fn require_namespace() {
        let res = scan_str("if (requireNamespace('cli', quietly = TRUE)) x()\nloadNamespace('ps')");
        let names: Vec<_> = res.hits.iter().map(|h| h.package.as_str()).collect();
        assert_eq!(names, vec!["cli", "ps"]);
        assert!(scan_str("requireNamespace(pkg)").hits.is_empty());
    }

    #[test]
    fn pacman_box_import() {
        assert_eq!(
            pkgs("pacman::p_load(dplyr, 'tidyr', char = c('cli'))"),
            vec!["pacman", "dplyr", "tidyr", "cli"]
        );
        assert_eq!(
            pkgs("box::use(dplyr[filter], purrr, ./local/mod, p = processx)"),
            vec!["box", "dplyr", "purrr", "processx"]
        );
        assert_eq!(
            pkgs("import::from(magrittr, '%>%')\nimport::from('helpers.R', f)"),
            vec!["import", "magrittr", "import"]
        );
        assert_eq!(pkgs("use('cli', 'cli_text')"), vec!["cli"]);
    }

    #[test]
    fn rlang_targets_misc() {
        assert_eq!(
            pkgs("rlang::check_installed(c('cli (>= 3.0.0)', 'ps'))"),
            vec!["rlang", "cli", "ps"]
        );
        assert_eq!(
            pkgs("tar_option_set(packages = c('dplyr', 'readr'))"),
            vec!["targets", "dplyr", "readr"]
        );
        assert_eq!(pkgs("ggplot(df) + geom_hex()"), vec!["hexbin"]);
        assert_eq!(
            pkgs("xfun::pkg_attach('cli', 'ps')"),
            vec!["xfun", "cli", "ps"]
        );
    }

    #[test]
    fn ignore_comment() {
        assert_eq!(
            pkgs("library(dplyr) # rig: ignore\nlibrary(cli)"),
            vec!["cli"]
        );
        assert!(scan_str("library(p, character.only = TRUE) # rig: ignore")
            .problems
            .is_empty());
    }

    #[test]
    fn syntax_errors_keep_valid_calls() {
        assert_eq!(
            pkgs("library(cli)\nx <- (((\nlibrary(ps)"),
            vec!["cli", "ps"]
        );
    }

    #[test]
    fn positions() {
        let res = scan_str("\n  library(cli)");
        assert_eq!(res.hits[0].line, 2);
        assert_eq!(res.hits[0].column, 11);
        assert_eq!(res.hits[0].code, "library(cli)");
    }

    fn scan_md(src: &str, quarto: bool) -> ScanResult {
        let mut parser = new_parser();
        let mut result = ScanResult::default();
        scan_markdown(&mut parser, src, "doc", DepType::Prod, quarto, &mut result).unwrap();
        result.hits.sort_by_key(|h| (h.line, h.column));
        result
    }

    #[test]
    fn rmarkdown_chunks() {
        let src = "---\ntitle: x\noutput: bookdown::html_document2\nruntime: shiny\n---\n\
                   \n```{r setup}\nlibrary(dplyr)\n```\n\n```{r, eval = FALSE}\nlibrary(nope)\n```\n\
                   \n```{r}\n#| eval: false\nlibrary(nope2)\n```\n\
                   \n```{python}\nimport numpy\n```\n\
                   Inline `r cli::format_inline('x')` code.\n";
        let res = scan_md(src, false);
        let names: Vec<_> = res.hits.iter().map(|h| h.package.as_str()).collect();
        assert!(names.contains(&"bookdown"));
        assert!(names.contains(&"shiny"));
        assert!(names.contains(&"rmarkdown"));
        assert!(names.contains(&"dplyr"));
        assert!(names.contains(&"reticulate"));
        assert!(names.contains(&"cli"));
        assert!(!names.contains(&"nope"));
        assert!(!names.contains(&"nope2"));
        let dplyr = res.hits.iter().find(|h| h.package == "dplyr").unwrap();
        assert_eq!(dplyr.line, 8);
        assert_eq!(dplyr.column, 9);
        let cli = res.hits.iter().find(|h| h.package == "cli").unwrap();
        assert_eq!(cli.line, 23);
    }

    #[test]
    fn quarto_documents() {
        let res = scan_md("---\ntitle: x\n---\n\nNo code.\n", true);
        assert!(res.hits.is_empty());
        let res = scan_md(
            "---\njupyter: python3\n---\n```{python}\nimport x\n```\n",
            true,
        );
        assert!(res.hits.is_empty());
        let res = scan_md("```{r}\nlibrary(gt)\n```\n", true);
        let names: Vec<_> = res.hits.iter().map(|h| h.package.as_str()).collect();
        assert_eq!(names, vec!["rmarkdown", "gt"]);
    }

    #[test]
    fn chunk_options() {
        assert!(chunk_is_evaluated(" setup, include=FALSE", ""));
        assert!(!chunk_is_evaluated(", eval=F", ""));
        assert!(!chunk_is_evaluated(" ex, exercise=TRUE", ""));
        assert!(!chunk_is_evaluated(" foo-display", ""));
        assert!(chunk_is_evaluated(" eval=FALSE, renv.ignore=FALSE", ""));
        assert!(!chunk_is_evaluated("", "#| renv.ignore: true\nlibrary(x)"));
    }

    #[test]
    fn rnw_chunks() {
        let src = "\\documentclass{article}\n<<setup>>=\nlibrary(knitr)\n@\n\
                   <<eval=FALSE>>=\nlibrary(nope)\n@\n<<engine='python'>>=\nimport x\n@\n";
        let mut parser = new_parser();
        let mut result = ScanResult::default();
        scan_rnw(&mut parser, src, "a.Rnw", DepType::Prod, &mut result).unwrap();
        let names: Vec<_> = result.hits.iter().map(|h| h.package.as_str()).collect();
        assert_eq!(names, vec!["knitr"]);
        assert_eq!(result.hits[0].line, 3);
    }

    #[test]
    fn dep_types_by_path() {
        assert_eq!(path_dep_type("R/a.R"), DepType::Prod);
        assert_eq!(path_dep_type("tests/testthat/test-a.R"), DepType::Dev);
        assert_eq!(path_dep_type("vignettes/a.Rmd"), DepType::Dev);
        assert_eq!(path_dep_type("inst/doc/a.R"), DepType::Dev);
        assert_eq!(path_dep_type("tests.R"), DepType::Prod);
    }

    fn write(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn walk_project() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(root, "analysis.R", "library(dplyr)\n");
        write(root, "R/utils.R", "cli::cli_text('x')\n");
        write(
            root,
            "tests/testthat/test-a.R",
            "library(testthat)\nlibrary(dplyr)\n",
        );
        write(root, "renv/library/x.R", "library(nope1)\n");
        write(root, ".hidden/x.R", "library(nope2)\n");
        write(root, "ignored/x.R", "library(nope3)\n");
        write(root, ".gitignore", "ignored/\n");
        write(root, "skipme/x.R", "library(nope4)\n");
        write(root, ".renvignore", "skipme\n");
        write(root, "data-raw/x.R", "library(nope5)\n");
        write(root, "sub/rproj.toml", "");
        write(root, "sub/x.R", "library(nope6)\n");
        write(root, "run", "#!/usr/bin/env Rscript\nlibrary(ps)\n");
        write(root, "notes", "library(nope7)\n");
        write(root, ".lintr", "linters: x\n");
        write(root, "_pkgdown.yml", "url: x\n");

        let opts = ScanOptions {
            exclude: vec!["data-raw/**".to_string()],
            ..Default::default()
        };
        let res = scan_project(root, &opts).unwrap();
        let summary = summarize(&res.hits);
        let got: Vec<(String, DepType)> = summary
            .iter()
            .map(|p| (p.package.clone(), p.dep_type))
            .collect();
        assert_eq!(
            got,
            vec![
                ("cli".to_string(), DepType::Prod),
                ("dplyr".to_string(), DepType::Prod),
                ("ps".to_string(), DepType::Prod),
                ("lintr".to_string(), DepType::Dev),
                ("pkgdown".to_string(), DepType::Dev),
                ("testthat".to_string(), DepType::Dev),
            ]
        );
        let dplyr = summary.iter().find(|p| p.package == "dplyr").unwrap();
        assert_eq!(dplyr.files, vec!["analysis.R", "tests/testthat/test-a.R"]);
    }

    #[test]
    fn own_package_and_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "a.R",
            "library(mypkg)\nlibrary(cli)\nlibrary(ps)\n",
        );
        let opts = ScanOptions {
            own_package: Some("mypkg".to_string()),
            ignore_packages: vec!["ps".to_string()],
            ..Default::default()
        };
        let res = scan_project(tmp.path(), &opts).unwrap();
        let names: Vec<_> = res.hits.iter().map(|h| h.package.as_str()).collect();
        assert_eq!(names, vec!["cli"]);
    }

    fn manifest(text: &str) -> Rproj {
        toml::from_str(text).unwrap()
    }

    fn scanned(name: &str, dep_type: DepType) -> ScannedPackage {
        ScannedPackage {
            package: name.to_string(),
            dep_type,
            files: vec!["a.R".to_string()],
        }
    }

    #[test]
    fn check_against_manifest() {
        let m = manifest(
            r#"
[project]
name = "p"
version = "1.0.0"
[dependencies]
R = ">= 4.1"
dplyr = "*"
unused = "*"
[dependency-groups.dev]
testthat = "*"
cli = "*"
"#,
        );
        let pkgs = vec![
            scanned("dplyr", DepType::Prod),
            scanned("cli", DepType::Prod),
            scanned("ps", DepType::Prod),
            scanned("covr", DepType::Dev),
            scanned("testthat", DepType::Dev),
        ];
        let report = check_manifest(&pkgs, &m);
        let names = |v: &[ScannedPackage]| v.iter().map(|p| p.package.clone()).collect::<Vec<_>>();
        assert_eq!(names(&report.missing), vec!["ps", "covr"]);
        assert_eq!(names(&report.misplaced), vec!["cli"]);
        assert_eq!(report.unused, vec!["unused"]);
        assert!(!report.is_clean(false));
    }

    #[test]
    fn scan_config() {
        let m = manifest(
            r#"
[project]
name = "p"
version = "1.0.0"
[tool.rig.scan]
exclude = ["data-raw/**"]
ignore-packages = ["a"]
"#,
        );
        let cfg = ScanConfig::from_manifest(&m).unwrap();
        assert_eq!(cfg.exclude, vec!["data-raw/**"]);
        assert_eq!(cfg.ignore_packages, vec!["a"]);
        let bad =
            manifest("[project]\nname = \"p\"\nversion = \"1\"\n[tool.rig.scan]\nexclude = 1\n");
        assert!(ScanConfig::from_manifest(&bad).is_err());
    }
}
