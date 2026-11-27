//! Rewrap the prose of the help Markdown files in `src/help/*.md`.
//!
//! The `--help` renderer (`render.rs`) keeps the line breaks of the Markdown
//! source, so the source line lengths are the line lengths users see in the
//! terminal. `fmt-help` refills every paragraph and every list item's text to
//! at most `WIDTH` visible columns. Markup that does not show up in the
//! terminal does not count: the backticks of code spans, the brackets and
//! targets of links, and emphasis markers. The indentation of list items
//! does count.
//!
//! Headings, code blocks, HTML and pipe tables are left alone. After
//! rewrapping, the file must parse to the same Markdown structure and text as
//! before, up to whitespace; if it does not, `fmt-help` refuses to write it.

use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

/// The maximum number of visible columns of a line.
pub(crate) const WIDTH: usize = 75;

pub(crate) fn fmt_help(help_dir: &Path, check: bool) -> ExitCode {
    let mut entries: Vec<PathBuf> = fs::read_dir(help_dir)
        .expect("read src/help")
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.extension().map(|e| e == "md").unwrap_or(false))
        .collect();
    entries.sort();

    let mut changed = Vec::new();
    for path in entries {
        let md = fs::read_to_string(&path).expect("read help md");
        let new = match format_md(&md) {
            Ok(new) => new,
            Err(e) => {
                eprintln!("error: {}: {}", path.display(), e);
                return ExitCode::FAILURE;
            }
        };
        if new != md {
            if !check {
                fs::write(&path, &new).expect("write help md");
            }
            changed.push(path);
        }
    }

    if check {
        if !changed.is_empty() {
            for p in &changed {
                eprintln!("error: {} is not wrapped", p.display());
            }
            eprintln!("run `make help` to rewrap them");
            return ExitCode::FAILURE;
        }
        eprintln!("src/help/*.md are wrapped");
    } else {
        for p in &changed {
            eprintln!("rewrapped {}", p.display());
        }
    }
    ExitCode::SUCCESS
}

/// Rewrap all prose in a Markdown document.
pub(crate) fn format_md(md: &str) -> Result<String, String> {
    let (regions, visible) = prose_regions(md);
    let mut out = md.to_string();
    // Back to front, so the earlier offsets stay valid.
    for region in regions.into_iter().rev() {
        if let Some(new) = rewrap_region(md, &region, &visible) {
            out.replace_range(region, &new);
        }
    }
    if signature(&out) != signature(md) {
        return Err("rewrapping would change the Markdown structure, \
                    rewrap this file by hand"
            .to_string());
    }
    Ok(out)
}

// Find the runs of inline content that can be rewrapped: the text of
// paragraphs and of tight list items, but not of headings, code blocks or
// HTML. Also return a mask of the source bytes that are visible in the
// rendered output.
fn prose_regions(md: &str) -> (Vec<Range<usize>>, Vec<bool>) {
    let mut visible = vec![false; md.len()];
    let mut regions = Vec::new();
    let mut run: Option<Range<usize>> = None;
    // Inside a heading, code block or HTML block nothing is rewrapped.
    let mut skip = 0usize;

    let flush = |run: &mut Option<Range<usize>>, regions: &mut Vec<Range<usize>>| {
        if let Some(r) = run.take() {
            regions.push(r);
        }
    };

    for (ev, range) in Parser::new_ext(md, Options::empty()).into_offset_iter() {
        let inline = match &ev {
            Event::Text(_) => {
                visible[range.clone()].iter_mut().for_each(|v| *v = true);
                true
            }
            Event::Code(_) => {
                let raw = &md[range.clone()];
                let ticks = raw.bytes().take_while(|&b| b == b'`').count();
                let inner = range.start + ticks..range.end - ticks;
                visible[inner].iter_mut().for_each(|v| *v = true);
                true
            }
            Event::SoftBreak
            | Event::InlineHtml(_)
            | Event::Start(Tag::Emphasis | Tag::Strong | Tag::Link { .. })
            | Event::End(TagEnd::Emphasis | TagEnd::Strong | TagEnd::Link) => true,
            // A hard break is significant, never move it.
            Event::HardBreak => false,
            Event::Start(Tag::Heading { .. } | Tag::CodeBlock(_) | Tag::HtmlBlock) => {
                flush(&mut run, &mut regions);
                skip += 1;
                false
            }
            Event::End(TagEnd::Heading(_) | TagEnd::CodeBlock | TagEnd::HtmlBlock) => {
                flush(&mut run, &mut regions);
                skip -= 1;
                false
            }
            _ => false,
        };
        if inline && skip == 0 {
            run = Some(match run {
                Some(r) => r.start.min(range.start)..r.end.max(range.end),
                None => range,
            });
        } else if !inline {
            flush(&mut run, &mut regions);
        }
    }
    flush(&mut run, &mut regions);
    (regions, visible)
}

// Rewrap one run of inline content. Returns `None` if it must be left alone.
fn rewrap_region(md: &str, region: &Range<usize>, visible: &[bool]) -> Option<String> {
    let text = &md[region.clone()];
    // Pipe tables are not parsed as tables, they look like paragraphs.
    if text.lines().any(|l| l.trim_start().starts_with('|')) {
        return None;
    }
    // What precedes the region on its first line: nothing for a paragraph,
    // the indentation and marker for a list item. The following lines are
    // indented to the item's content column.
    let line_start = md[..region.start].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let prefix = &md[line_start..region.start];
    if !prefix.chars().all(|c| {
        c == ' ' || c == '-' || c == '*' || c == '+' || c == '.' || c == ')' || c.is_ascii_digit()
    }) {
        return None;
    }
    let indent = prefix.chars().count();

    // Words, with their visible widths.
    let mut words: Vec<(&str, usize)> = Vec::new();
    let mut start = None;
    for (i, c) in text
        .char_indices()
        .chain(std::iter::once((text.len(), ' ')))
    {
        if c.is_whitespace() {
            if let Some(s) = start.take() {
                let abs = region.start + s..region.start + i;
                let width = md[abs.clone()]
                    .char_indices()
                    .filter(|(j, _)| visible[abs.start + j])
                    .count();
                words.push((&text[s..i], width));
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }

    // Greedy fill. A word that would start a new Markdown block if it started
    // a line, e.g. `-` or `1.`, is kept on the line of the word before it.
    let avail = WIDTH.saturating_sub(indent);
    let mut lines: Vec<Vec<(&str, usize)>> = vec![Vec::new()];
    let mut cur_width = 0usize;
    for w in words {
        let line = lines.last_mut().unwrap();
        if line.is_empty() || cur_width + 1 + w.1 <= avail {
            cur_width += if line.is_empty() { w.1 } else { 1 + w.1 };
            line.push(w);
        } else if !starts_block(w.0) {
            lines.push(vec![w]);
            cur_width = w.1;
        } else if line.len() >= 2 && !starts_block(line.last().unwrap().0) {
            let prev = line.pop().unwrap();
            cur_width = prev.1 + 1 + w.1;
            lines.push(vec![prev, w]);
        } else {
            cur_width += 1 + w.1;
            line.push(w);
        }
    }

    let cont = " ".repeat(indent);
    let new = lines
        .iter()
        .map(|l| l.iter().map(|w| w.0).collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join(&format!("\n{}", cont));
    Some(new)
}

// Whether a word at the start of a line could start a new Markdown block
// (a list item, a heading, a block quote, a code fence, a table row, raw
// HTML), in CommonMark or in Pandoc's Markdown, which renders the website.
fn starts_block(word: &str) -> bool {
    if matches!(word, "-" | "+" | "*" | ">" | "|" | "=" | "(@)")
        || word.starts_with('#')
        || word.starts_with("```")
        || word.starts_with("~~~")
        || word.starts_with('<')
        || word.starts_with('|')
        || word.starts_with('>')
        || word.chars().all(|c| c == '-' || c == '=')
    {
        return true;
    }
    // Ordered list markers: `1.`, `2)`, `a.`, `IV)`, `(b)`.
    let core = word
        .strip_prefix('(')
        .and_then(|w| w.strip_suffix(')'))
        .or_else(|| word.strip_suffix('.'))
        .or_else(|| word.strip_suffix(')'));
    match core {
        Some(c) if !c.is_empty() => {
            c.chars().all(|c| c.is_ascii_digit())
                || (c.chars().count() == 1 && c.chars().all(|c| c.is_ascii_alphabetic()))
                || c.chars().all(|c| "ivxlcdmIVXLCDM".contains(c))
        }
        _ => false,
    }
}

// The Markdown structure and text of a document, with all whitespace runs in
// the text collapsed to a single space. Rewrapping must not change it.
fn signature(md: &str) -> Vec<String> {
    let mut sig = Vec::new();
    let mut text = String::new();
    let flush = |text: &mut String, sig: &mut Vec<String>| {
        let t = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if !t.is_empty() {
            sig.push(t);
        }
        text.clear();
    };
    for ev in Parser::new_ext(md, Options::empty()) {
        match ev {
            Event::Text(t) => text.push_str(&t),
            Event::Code(t) => {
                text.push('`');
                text.push_str(&t);
                text.push('`');
            }
            Event::SoftBreak => text.push(' '),
            other => {
                flush(&mut text, &mut sig);
                sig.push(format!("{:?}", other));
            }
        }
    }
    flush(&mut text, &mut sig);
    sig
}

#[cfg(test)]
mod tests {
    use super::format_md;

    fn words(n: usize) -> String {
        (0..n)
            .map(|i| format!("w{:03}", i))
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn fills_paragraph() {
        // 4-char words: 15 of them take 74 columns, 16 would take 79.
        let md = format!("{}\n\n## Description\n\n{}\n", "Short.", words(20));
        let out = format_md(&md).unwrap();
        let body: Vec<&str> = out.lines().skip(4).collect();
        assert_eq!(body.len(), 2);
        assert_eq!(body[0].len(), 74);
        assert!(body[1].starts_with("w015"));
    }

    #[test]
    fn joins_short_lines() {
        let md = "one\ntwo\nthree\n";
        assert_eq!(format_md(md).unwrap(), "one two three\n");
    }

    #[test]
    fn invisible_markup_does_not_count() {
        // 74 visible columns, but much wider in the source.
        let md = format!(
            "{} `abc` [link text](../some/long/target/page.qmd) **x**\n",
            "a".repeat(50)
        );
        assert_eq!(format_md(&md).unwrap(), md);
    }

    #[test]
    fn list_items_keep_indentation() {
        let md = format!("- {}\n  - {}\n", words(20), words(20));
        let out = format_md(&md).unwrap();
        for l in out.lines() {
            assert!(l.len() <= 75, "{}", l);
        }
        assert!(out.contains("\n  w"));
        assert!(out.contains("\n  - w000"));
        assert!(out.contains("\n    w"));
    }

    #[test]
    fn leaves_code_blocks_headings_and_tables() {
        let long = words(30);
        let md = format!("## {long}\n\n```\n{long}\n```\n\n| a | b |\n|---|---|\n| {long} | x |\n");
        assert_eq!(format_md(&md).unwrap(), md);
    }

    #[test]
    fn does_not_start_a_line_with_a_list_marker() {
        // `-` would land at the start of the second line, so `bb` goes with it.
        let md = format!("{} bb - and more\n", "a".repeat(71));
        let out = format_md(&md).unwrap();
        assert_eq!(out, format!("{}\nbb - and more\n", "a".repeat(71)));
    }
}
