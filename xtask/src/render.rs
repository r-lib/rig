//! Minimal Markdown -> ANSI renderer for rig's `--help` text.
//!
//! The help prose for each command lives in a Markdown file under `src/help/`.
//! This renders that Markdown to the colored, indented text clap shows for
//! `--help`. The supported subset is small on purpose -- headings, paragraphs,
//! bullet/ordered lists, inline `code`, **bold**, *italic* and fenced code
//! blocks -- which is all the help text uses. The same Markdown files are
//! included verbatim by the documentation website, where Quarto renders them to
//! HTML.
//!
//! Rendering happens here, in the dev-only `xtask` crate, at generation time;
//! the output is committed to `src/help-generated.in`. That keeps
//! `pulldown-cmark` off the `rig` binary's dependency graph and means `--help`
//! pays no rendering cost at run time. clap strips the ANSI on non-terminal
//! output, exactly as it did for the previous hand-written strings.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

// Indentation (in spaces) of block content under a section heading.
const HELP_INDENT: usize = 2;

// The kind of the most recently emitted block, used to decide how much
// vertical space to put before the next one.
#[derive(PartialEq, Clone, Copy)]
enum HelpBlock {
    Heading,
    Item,
    Other,
}

// Render Markdown to ANSI. `color` is always true for the generated help
// strings; clap removes the escapes when the output is not a terminal.
pub(crate) fn md_to_ansi_impl(md: &str, color: bool) -> String {
    let parser = Parser::new_ext(md, Options::empty());

    let mut out = String::new();
    // Inline content of the current block, with embedded '\n' for line breaks.
    let mut inline = String::new();
    // Stack of list counters: None for a bullet list, Some(n) for the next
    // number in an ordered list.
    let mut lists: Vec<Option<u64>> = Vec::new();
    let mut in_code_block = false;
    let mut last: Option<HelpBlock> = None;

    // Separator before the next block: nothing for the first block, a single
    // newline after a heading or between list items, otherwise a blank line.
    let sep = |out: &mut String, last: Option<HelpBlock>, cur: HelpBlock| match last {
        None => {}
        Some(HelpBlock::Heading) => out.push('\n'),
        Some(HelpBlock::Item) if cur == HelpBlock::Item => out.push('\n'),
        _ => out.push_str("\n\n"),
    };

    // Emit `text` (which may contain '\n') with `prefix` on the first line and
    // `cont` on every following line. Blank lines are left empty rather than
    // padded with trailing whitespace.
    let emit = |out: &mut String, text: &str, prefix: &str, cont: &str| {
        for (i, line) in text.split('\n').enumerate() {
            let pad = if i == 0 { prefix } else { cont };
            if i > 0 {
                out.push('\n');
            }
            if !line.is_empty() {
                out.push_str(pad);
                out.push_str(line);
            }
        }
    };

    for ev in parser {
        match ev {
            // -- block starts ---------------------------------------------
            Event::Start(Tag::Heading { .. }) | Event::Start(Tag::Paragraph) => {
                inline.clear();
            }
            Event::Start(Tag::List(start)) => {
                lists.push(start);
            }
            Event::Start(Tag::Item) => {
                inline.clear();
            }
            Event::Start(Tag::CodeBlock(_)) => {
                in_code_block = true;
                inline.clear();
            }

            // -- block ends -----------------------------------------------
            Event::End(TagEnd::Heading(_)) => {
                sep(&mut out, last, HelpBlock::Heading);
                let text = format!("{}:", inline.trim_end());
                if color {
                    out.push_str("\x1b[1m\x1b[34m");
                    out.push_str(&text);
                    out.push_str("\x1b[39m\x1b[22m");
                } else {
                    out.push_str(&text);
                }
                last = Some(HelpBlock::Heading);
            }
            Event::End(TagEnd::Paragraph) => {
                sep(&mut out, last, HelpBlock::Other);
                let indent = " ".repeat(HELP_INDENT);
                emit(&mut out, inline.trim_end(), &indent, &indent);
                last = Some(HelpBlock::Other);
            }
            // A tight list item carries its text directly; a nested list
            // flushes the parent's text early, leaving nothing here.
            Event::End(TagEnd::Item) if !inline.trim_end().is_empty() => {
                sep(&mut out, last, HelpBlock::Item);
                let depth = lists.len().max(1);
                let marker = match lists.last_mut() {
                    Some(Some(n)) => {
                        let m = format!("{}. ", *n);
                        *n += 1;
                        m
                    }
                    _ => "- ".to_string(),
                };
                let pad = " ".repeat(HELP_INDENT * depth);
                let prefix = format!("{}{}", pad, marker);
                let cont = " ".repeat(prefix.chars().count());
                emit(&mut out, inline.trim_end(), &prefix, &cont);
                inline.clear();
                last = Some(HelpBlock::Item);
            }
            Event::End(TagEnd::List(_)) => {
                lists.pop();
            }
            Event::End(TagEnd::CodeBlock) => {
                sep(&mut out, last, HelpBlock::Other);
                let indent = " ".repeat(HELP_INDENT);
                emit(&mut out, inline.trim_end_matches('\n'), &indent, &indent);
                in_code_block = false;
                inline.clear();
                last = Some(HelpBlock::Other);
            }

            // -- inline content -------------------------------------------
            Event::Text(t) => inline.push_str(&t),
            Event::Code(t) => {
                if in_code_block {
                    inline.push_str(&t);
                } else if color {
                    inline.push_str("\x1b[32m");
                    inline.push_str(&t);
                    inline.push_str("\x1b[39m");
                } else {
                    inline.push('`');
                    inline.push_str(&t);
                    inline.push('`');
                }
            }
            Event::Start(Tag::Strong) | Event::End(TagEnd::Strong) if color => {
                inline.push_str(if matches!(ev, Event::Start(_)) {
                    "\x1b[1m"
                } else {
                    "\x1b[22m"
                });
            }
            Event::Start(Tag::Emphasis) | Event::End(TagEnd::Emphasis) if color => {
                inline.push_str(if matches!(ev, Event::Start(_)) {
                    "\x1b[3m"
                } else {
                    "\x1b[23m"
                });
            }
            Event::SoftBreak | Event::HardBreak => inline.push('\n'),

            _ => {}
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::md_to_ansi_impl;

    #[test]
    fn plain_heading_and_paragraph() {
        let md = "## Description\n\nHello `world` and more.";
        assert_eq!(
            md_to_ansi_impl(md, false),
            "Description:\n  Hello `world` and more."
        );
    }

    #[test]
    fn color_heading_and_code() {
        let md = "## Description\n\nUse `R`.";
        assert_eq!(
            md_to_ansi_impl(md, true),
            "\x1b[1m\x1b[34mDescription:\x1b[39m\x1b[22m\n  Use \x1b[32mR\x1b[39m."
        );
    }

    #[test]
    fn bullet_list_indentation() {
        let md = "## D\n\nWays:\n\n- first item that is\n  wrapped onto two lines\n- second";
        assert_eq!(
            md_to_ansi_impl(md, false),
            "D:\n  Ways:\n\n  - first item that is\n    wrapped onto two lines\n  - second"
        );
    }

    #[test]
    fn ordered_list_numbers() {
        let md = "1. one\n2. two";
        assert_eq!(md_to_ansi_impl(md, false), "  1. one\n  2. two");
    }

    #[test]
    fn code_block_is_indented() {
        let md = "Run:\n\n```sh\nrig add devel\n```";
        assert_eq!(md_to_ansi_impl(md, false), "  Run:\n\n  rig add devel");
    }
}
