//! Rendering package READMEs (markdown) for the terminal.
//!
//! [`termimad`] does the block layout, but its markdown parser ([`minimad`])
//! has no notion of links, so `[text](url)` would come out verbatim — and the
//! badge block at the top of most R READMEs is a wall of
//! `[![CRAN status](https://…svg)](https://…)`. It also knows nothing about
//! GitHub emoji shortcodes.
//!
//! So we sandwich `termimad` between two passes:
//!
//! 1. [`rewrite`] parses the markdown with `pulldown-cmark` — only to locate
//!    spans, the source is otherwise left byte-identical. It substitutes emoji
//!    shortcodes in prose (never in code), replaces every link and image span
//!    with just its label, remembering the URLs in document order, reduces raw
//!    HTML to what a terminal can show, and joins the lines of a paragraph so
//!    that termimad can rewrap them.
//! 2. [`apply_hyperlinks`] turns the labels into OSC 8 terminal hyperlinks in
//!    the *rendered* output.
//!
//! The order matters: `termimad` computes its wrapping on the markdown it is
//! given, counting every byte of an escape sequence as a visible column, so
//! the escapes can only go in once the text has been laid out. The two passes
//! communicate through the zero-width [`LINK_START`] / [`LINK_END`] sentinels,
//! which `termimad` passes through and `unicode-width` scores as 0 columns.

use lazy_static::lazy_static;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use regex::Regex;

use super::manifest;

/// Wrapping width for rendered READMEs, in columns.
pub const README_WIDTH: usize = 78;

/// Marks the start of a hyperlink label. Its URL is the next unused entry of
/// the `urls` vector [`rewrite`] returns.
const LINK_START: char = '\u{1}';
/// Marks the end of a hyperlink label.
const LINK_END: char = '\u{2}';

/// The README of the package, ready to print: markdown ones (`readme_type` is
/// `"md"`) rendered with termimad, plain text ones (`"txt"`) as they are.
/// READMEs in any other format, and packages without one, give `None`.
///
/// `color` also gates the OSC 8 hyperlinks: when it is off — stdout is not a
/// terminal, or `NO_COLOR` is set — the output holds no escape sequences at
/// all, and link targets are spelled out as `label (url)` instead.
pub fn format_readme(info: &manifest::PackageInfo, color: bool) -> Option<String> {
    let readme = info.readme.as_deref()?.trim_matches('\n');
    if readme.is_empty() {
        return None;
    }

    match info.readme_type.as_deref() {
        Some("md") => {
            // The sentinels are ours; a README must not be able to smuggle
            // them in and pick up a hyperlink it did not ask for.
            let readme = if readme.contains([LINK_START, LINK_END]) {
                readme.replace([LINK_START, LINK_END], "")
            } else {
                readme.to_string()
            };
            let (md, urls) = rewrite(&readme, color);
            let skin = if color {
                termimad::MadSkin::default_dark()
            } else {
                termimad::MadSkin::no_style()
            };
            let out = termimad::FmtText::from(&skin, &md, Some(README_WIDTH)).to_string();
            Some(if color {
                apply_hyperlinks(&out, &urls)
            } else {
                out
            })
        }
        Some("txt") => Some(format!("{}\n", readme)),
        _ => None,
    }
}

/// Rewrite the markdown of a README for termimad.
///
/// Substitutes GitHub emoji shortcodes, and replaces every link and image span
/// with its label alone, so that `[![CRAN status](badge.svg)](https://…)`
/// collapses to `CRAN status`. Returns the new markdown and the link targets
/// in document order.
///
/// With `links` on, labels are wrapped in the [`LINK_START`] / [`LINK_END`]
/// sentinels for [`apply_hyperlinks`] to pick up. With it off, the target is
/// appended as text instead — `label (url)` — so it is not lost.
///
/// Only spans that need changing are touched; the rest of the markdown, and
/// with it all the block structure termimad cares about, is left alone.
fn rewrite(md: &str, links: bool) -> (String, Vec<String>) {
    /// A link or image span we are inside of, collecting its label.
    struct Ctx {
        start: usize,
        end: usize,
        url: String,
        label: String,
    }

    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;

    let mut urls: Vec<String> = Vec::new();
    // (byte range in `md`, replacement), non-overlapping and in source order.
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let mut stack: Vec<Ctx> = Vec::new();
    // The body of a fenced or indented code block arrives as `Event::Text`,
    // unlike an inline code span, and must be left exactly as it is.
    let mut in_code_block = 0usize;
    // Inside a block quote the line prefix (`> `) belongs to no event, so the
    // lines there cannot be joined by replacing the line break alone.
    let mut in_quote = 0usize;
    // Raw HTML state, carried between events: a chunk can end in the middle of
    // a comment or between an `<a href>` and its `</a>`.
    let mut html = HtmlState::default();

    for (event, range) in Parser::new_ext(md, opts).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(_)) => in_code_block += 1,
            Event::End(TagEnd::CodeBlock) => in_code_block = in_code_block.saturating_sub(1),
            Event::Text(_) if in_code_block > 0 => {}
            Event::Start(Tag::BlockQuote(_)) => in_quote += 1,
            Event::End(TagEnd::BlockQuote(_)) => in_quote = in_quote.saturating_sub(1),
            Event::Start(Tag::Link { dest_url, .. })
            | Event::Start(Tag::Image { dest_url, .. }) => {
                stack.push(Ctx {
                    start: range.start,
                    end: range.end,
                    url: dest_url.to_string(),
                    label: String::new(),
                });
            }
            Event::End(TagEnd::Link) | Event::End(TagEnd::Image) => {
                let Some(ctx) = stack.pop() else { continue };
                match stack.last_mut() {
                    // An image inside a link (a badge): the outer link wins,
                    // the alt text becomes part of its label.
                    Some(parent) => parent.label.push_str(&ctx.label),
                    None => {
                        let text = link_text(&ctx.label, &ctx.url, links, &mut urls);
                        let (start, end) = if text.is_empty() {
                            drop_whole_line(md, ctx.start, ctx.end)
                        } else {
                            (ctx.start, ctx.end)
                        };
                        edits.push((start, end, text));
                    }
                }
            }
            Event::Text(text) => {
                let text = emojify(&text);
                match stack.last_mut() {
                    Some(ctx) => ctx.label.push_str(&text),
                    None => {
                        let text = if links {
                            linkify_bare_urls(&text, &mut urls)
                        } else {
                            text
                        };
                        if text != md[range.clone()] {
                            edits.push((range.start, range.end, text));
                        }
                    }
                }
            }
            // Inside a label these would otherwise be dropped, gluing words
            // together across a line break or losing the code span's text.
            Event::Code(code) => {
                if let Some(ctx) = stack.last_mut() {
                    ctx.label.push_str(&code);
                }
            }
            // Paragraphs in a README are usually hard-wrapped in the source,
            // and minimad renders one source line as one output line: it only
            // splits lines that are too long, it never joins short ones. So
            // the lines of a paragraph are joined here and left for termimad
            // to wrap at the full width -- otherwise replacing a long
            // `[text](url)` with a short label leaves a stubby line behind.
            Event::SoftBreak => {
                match stack.last_mut() {
                    Some(ctx) => ctx.label.push(' '),
                    None if in_quote == 0 && !at_a_badge_block(md, &range) => {
                        // Eat the indentation of the next line too, or the
                        // joined line gets a run of spaces in the middle.
                        let next = &md[range.end..];
                        let indent = next.len() - next.trim_start_matches([' ', '\t']).len();
                        edits.push((range.start, range.end + indent, " ".to_string()));
                    }
                    None => {}
                }
            }
            Event::HardBreak => {
                if let Some(ctx) = stack.last_mut() {
                    ctx.label.push(' ');
                }
            }
            // Raw HTML has no meaning in a terminal, so the tags go: an
            // `<img>` leaves its `alt` text behind, an `<a href>` becomes a
            // hyperlink, and comments -- which are addressed at whoever reads
            // the markdown source, and which R READMEs generated from an
            // `.Rmd` are full of (`<!-- badges: start -->`) -- vanish.
            Event::Html(raw) | Event::InlineHtml(raw) => {
                if !stack.is_empty() {
                    continue;
                }
                let (kept, has_break) = strip_html(
                    &md[range.clone()],
                    md,
                    range.start,
                    links,
                    &mut urls,
                    &mut html,
                );
                if kept == *raw {
                    continue;
                }
                // A `<br>` leaves a bare newline behind, which is the point of
                // it; anything else that renders to whitespace is dropped.
                if !kept.trim().is_empty() || has_break {
                    edits.push((range.start, range.end, kept));
                    continue;
                }
                // The whole chunk goes. If it was a block of its own, take
                // the blank line after it too, or it shows as a gap.
                let alone = range.start == 0 || md[..range.start].ends_with("\n\n");
                let end = if alone && md[range.end..].starts_with('\n') {
                    range.end + 1
                } else {
                    range.end
                };
                edits.push((range.start, end, String::new()));
            }
            _ => {}
        }
    }

    let mut out = String::with_capacity(md.len());
    let mut at = 0;
    for (start, end, text) in edits {
        // Defensive: `pulldown-cmark` gives us non-overlapping spans in source
        // order, but a stray one must not panic or scramble the README.
        if start < at || end > md.len() {
            continue;
        }
        out.push_str(&md[at..start]);
        out.push_str(&text);
        at = end;
    }
    out.push_str(&md[at..]);

    (out, urls)
}

/// What a link or image span is replaced by: its label, hyperlinked to `url`.
///
/// Relative targets (`man/figures/plot.png`) are not something a terminal can
/// open, so those keep the label alone. A span with no label at all —
/// `[](url)`, or an image with no alt text, i.e. a purely decorative badge or
/// logo — is dropped: there is nothing to say about it.
fn link_text(label: &str, url: &str, links: bool, urls: &mut Vec<String>) -> String {
    let label = label.trim();
    let url = url.trim();
    if label.is_empty() {
        return String::new();
    }

    if !is_absolute_url(url) {
        return label.to_string();
    }
    if !links {
        // Autolinks and bare URLs have the URL as their label already.
        return if label == url {
            label.to_string()
        } else {
            format!("{} ({})", label, url)
        };
    }

    urls.push(url.to_string());
    format!("{}{}{}", LINK_START, label, LINK_END)
}

/// Wrap the bare `https://…` URLs in `text` in hyperlink sentinels.
///
/// `pulldown-cmark` only recognizes links in brackets and autolinks in angle
/// brackets, but READMEs are full of URLs written out plainly.
fn linkify_bare_urls(text: &str, urls: &mut Vec<String>) -> String {
    lazy_static! {
        // Stop at anything that is more likely to delimit the URL than to be
        // part of it. Trailing sentence punctuation is trimmed below.
        static ref BARE_URL: Regex = Regex::new(r#"https?://[^\s<>()\[\]{}`'"]+"#).unwrap();
    }
    if !text.contains("http") {
        return text.to_string();
    }

    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for m in BARE_URL.find_iter(text) {
        let url = m.as_str().trim_end_matches(['.', ',', ';', ':', '!', '?']);
        if url.is_empty() {
            continue;
        }
        out.push_str(&text[at..m.start()]);
        urls.push(url.to_string());
        out.push(LINK_START);
        out.push_str(url);
        out.push(LINK_END);
        at = m.start() + url.len();
    }
    out.push_str(&text[at..]);
    out
}

/// Grow a span that renders to nothing to cover its whole line, when there is
/// nothing else on that line — a badge with no alt text — so that no blank line
/// is left behind where it was.
fn drop_whole_line(md: &str, start: usize, end: usize) -> (usize, usize) {
    let before = md[..start].rsplit('\n').next().unwrap_or("");
    if !before.trim().is_empty() {
        return (start, end);
    }
    let after = &md[end..];
    let spaces = after.len() - after.trim_start_matches([' ', '\t']).len();
    if after[spaces..].starts_with('\n') {
        (start - before.len(), end + spaces + 1)
    } else {
        (start, end)
    }
}

/// Whether the line break at `at` borders a line holding an image.
///
/// The badge block at the top of an R README is one line per badge, so it is a
/// single paragraph as far as markdown is concerned; joining those lines would
/// run all the badge names together. A badge is an image, in a link or not, and
/// prose almost never has one, which makes an image on the line a good sign
/// that the break is worth keeping.
fn at_a_badge_block(md: &str, at: &std::ops::Range<usize>) -> bool {
    fn has_image(line: &str) -> bool {
        line.contains("![") || line.to_ascii_lowercase().contains("<img")
    }
    let before = md[..at.start].rsplit('\n').next().unwrap_or("");
    let after = md[at.end..].split('\n').next().unwrap_or("");
    has_image(before) || has_image(after)
}

/// Raw HTML state that has to survive from one event to the next: a comment or
/// an `<a href>` … `</a>` pair can be split across several `Event::Html`s.
#[derive(Default)]
struct HtmlState {
    /// An unterminated `<!--`.
    in_comment: bool,
    /// The target of an `<a href>` that has not been closed yet.
    link: Option<String>,
}

/// Reduce the raw HTML in a chunk of a README to what a terminal can show.
///
/// `<img>` leaves its `alt` text (nothing, if it has none — a decorative
/// badge), `<a href>` becomes a hyperlink around whatever it wraps, `<br>` a
/// line break, comments vanish, and every other tag is dropped, since a
/// terminal can do nothing with it. Text between the tags is kept as it is.
///
/// `chunk` is `md[at..]`, truncated to the chunk: the tags are looked up in the
/// full markdown, not just the chunk, because `pulldown-cmark` hands out a raw
/// HTML block one line at a time and an `<a href>` and its `</a>` routinely end
/// up in different ones.
///
/// Also reports whether a `<br>` contributed a line break, so that the caller
/// can tell it apart from a chunk that rendered to nothing at all.
fn strip_html(
    chunk: &str,
    md: &str,
    at: usize,
    links: bool,
    urls: &mut Vec<String>,
    state: &mut HtmlState,
) -> (String, bool) {
    let mut out = String::with_capacity(chunk.len());
    let mut has_break = false;
    let mut seen = 0;
    while seen < chunk.len() {
        if state.in_comment {
            match chunk[seen..].find("-->") {
                Some(found) => {
                    state.in_comment = false;
                    seen += found + 3;
                }
                None => return (out, has_break),
            }
            continue;
        }
        let Some(found) = chunk[seen..].find('<') else {
            out.push_str(&chunk[seen..]);
            return (out, has_break);
        };
        out.push_str(&chunk[seen..seen + found]);
        seen += found;
        if chunk[seen..].starts_with("<!--") {
            state.in_comment = true;
            seen += 4;
            continue;
        }
        let Some(len) = tag_len(&chunk[seen..]) else {
            // An unterminated `<`: not a tag, keep the rest verbatim.
            out.push_str(&chunk[seen..]);
            return (out, has_break);
        };
        let tag = &chunk[seen..seen + len];
        seen += len;

        match tag_name(tag).as_str() {
            "img" => {
                let alt = attr(tag, "alt").unwrap_or_default();
                if !alt.trim().is_empty() {
                    out.push_str(alt.trim());
                }
            }
            "br" => {
                out.push('\n');
                has_break = true;
            }
            "a" => {
                let url = attr(tag, "href").unwrap_or_default();
                // An `<a>` with nothing to label -- around an `<img>` with no
                // alt text, say -- is a decorative logo or badge, and is left
                // out along with the image itself.
                if is_absolute_url(&url) && html_link_has_label(&md[at + seen..]) {
                    if links {
                        urls.push(url.clone());
                        out.push(LINK_START);
                    }
                    state.link = Some(url);
                }
            }
            "/a" => {
                if let Some(url) = state.link.take() {
                    if links {
                        out.push(LINK_END);
                    } else {
                        out.push_str(&format!(" ({})", url));
                    }
                }
            }
            _ => {}
        }
    }
    (out, has_break)
}

/// Whether an `<a href>` has anything for a terminal to show as its label.
///
/// `after` is the markdown from just past the opening `<a …>` tag onwards; text
/// or an `<img>` with alt text before the `</a>` counts.
fn html_link_has_label(after: &str) -> bool {
    let mut rest = after;
    loop {
        let Some(found) = rest.find('<') else {
            return !rest.trim().is_empty();
        };
        if !rest[..found].trim().is_empty() {
            return true;
        }
        rest = &rest[found..];
        let Some(len) = tag_len(rest) else {
            return false;
        };
        let tag = &rest[..len];
        rest = &rest[len..];
        match tag_name(tag).as_str() {
            "/a" => return false,
            "img" if attr(tag, "alt").is_some_and(|alt| !alt.trim().is_empty()) => return true,
            _ => {}
        }
    }
}

/// The length of the HTML tag at the start of `s`, quotes accounted for, or
/// `None` if it is not closed.
fn tag_len(s: &str) -> Option<usize> {
    let mut quote = None;
    for (at, ch) in s.char_indices() {
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(ch),
            (None, '>') => return Some(at + 1),
            (None, _) => {}
        }
    }
    None
}

/// The lowercased name of an HTML tag, `/a` for a closing `</a>`.
fn tag_name(tag: &str) -> String {
    tag.trim_start_matches('<')
        .trim_end_matches('>')
        .split(|c: char| c.is_whitespace())
        .next()
        .unwrap_or("")
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

/// The value of the `name` attribute of an HTML tag, e.g. `x` for `alt` in
/// `<img alt="x">`.
fn attr(tag: &str, name: &str) -> Option<String> {
    // `to_ascii_lowercase` keeps the byte offsets of `tag`, unlike
    // `to_lowercase`, so it is safe to index the original with them.
    let lower = tag.to_ascii_lowercase();
    let mut at = 0;
    while let Some(found) = lower[at..].find(name) {
        let start = at + found;
        at = start + name.len();
        // Must be an attribute of its own, not the tail of another one.
        if !lower[..start]
            .chars()
            .next_back()
            .is_some_and(char::is_whitespace)
        {
            continue;
        }
        let Some(value) = tag[at..].trim_start().strip_prefix('=') else {
            continue;
        };
        let value = value.trim_start();
        return Some(match value.chars().next() {
            Some(quote @ ('"' | '\'')) => value[1..].split(quote).next().unwrap_or("").to_string(),
            _ => value
                .split(|c: char| c.is_whitespace() || c == '>' || c == '/')
                .next()
                .unwrap_or("")
                .to_string(),
        });
    }
    None
}

/// Whether a terminal could open `url`, i.e. whether it has a scheme.
fn is_absolute_url(url: &str) -> bool {
    url.contains("://") || url.starts_with("mailto:")
}

/// Replace the GitHub emoji shortcodes in `text`, e.g. `:rocket:` with 🚀.
///
/// Shortcodes that are not GitHub's are left as they are; `:` is far too
/// common in prose (and in R code) to rewrite anything we are unsure about.
fn emojify(text: &str) -> String {
    lazy_static! {
        static ref SHORTCODE: Regex = Regex::new(r"(?i):([a-z0-9_+-]+):").unwrap();
    }
    if !text.contains(':') {
        return text.to_string();
    }

    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for caps in SHORTCODE.captures_iter(text) {
        let all = caps.get(0).unwrap();
        // Overlapping shortcodes (`:a:b:`) are matched left to right, so a
        // match may start before where the last one ended; skip those.
        if all.start() < at {
            continue;
        }
        let Some(emoji) = emojis::get_by_shortcode(&caps[1].to_lowercase()) else {
            continue;
        };
        out.push_str(&text[at..all.start()]);
        out.push_str(emoji.as_str());
        at = all.end();
    }
    out.push_str(&text[at..]);
    out
}

/// Turn the hyperlink sentinels in rendered output into OSC 8 escapes.
///
/// The n-th [`LINK_START`] opens a hyperlink to the n-th entry of `urls`, and
/// the matching [`LINK_END`] closes it. A hyperlink is closed and reopened
/// around a line break — termimad may have wrapped a multi-word label, and
/// terminals cope badly with an OSC 8 sequence spanning a newline.
fn apply_hyperlinks(text: &str, urls: &[String]) -> String {
    const CLOSE: &str = "\x1b]8;;\x1b\\";

    if urls.is_empty() {
        return text.replace([LINK_START, LINK_END], "");
    }

    let mut out =
        String::with_capacity(text.len() + urls.iter().map(|u| u.len() + 20).sum::<usize>());
    let mut next = 0;
    let mut open: Option<&str> = None;
    for ch in text.chars() {
        match ch {
            LINK_START => {
                if let Some(url) = urls.get(next) {
                    next += 1;
                    out.push_str(&format!("\x1b]8;;{}\x1b\\", url));
                    open = Some(url);
                }
            }
            LINK_END => {
                if open.take().is_some() {
                    out.push_str(CLOSE);
                }
            }
            '\n' => {
                if open.is_some() {
                    out.push_str(CLOSE);
                }
                out.push('\n');
                if let Some(url) = open {
                    out.push_str(&format!("\x1b]8;;{}\x1b\\", url));
                }
            }
            _ => out.push(ch),
        }
    }
    if open.is_some() {
        out.push_str(CLOSE);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info_with_readme(readme: Option<&str>, readme_type: Option<&str>) -> manifest::PackageInfo {
        manifest::PackageInfo {
            description: serde_json::json!({ "Package": "pkg", "Version": "1.0.0" }),
            readme: readme.map(|s| s.to_string()),
            readme_type: readme_type.map(|s| s.to_string()),
            archived: None,
        }
    }

    /// Renders a markdown README the way `format_readme` does, without the
    /// `PackageInfo` wrapper.
    fn render(md: &str, color: bool) -> String {
        format_readme(&info_with_readme(Some(md), Some("md")), color).unwrap()
    }

    // -- the assumption the two-pass design rests on ------------------------

    #[test]
    fn termimad_passes_sentinels_through_at_width_zero() {
        let skin = termimad::MadSkin::no_style();
        let plain = termimad::FmtText::from(&skin, "one two three four five", Some(12)).to_string();
        let marked = termimad::FmtText::from(
            &skin,
            "\u{1}one two\u{2} three \u{1}four\u{2} five",
            Some(12),
        )
        .to_string();
        assert_eq!(marked.replace([LINK_START, LINK_END], ""), plain);
    }

    // -- emoji -------------------------------------------------------------

    #[test]
    fn emoji_shortcodes_are_substituted() {
        assert_eq!(emojify("ship it :rocket:"), "ship it 🚀");
        assert_eq!(emojify(":+1: and :-1:"), "👍 and 👎");
        assert_eq!(emojify(":ROCKET:"), "🚀");
    }

    #[test]
    fn unknown_emoji_shortcodes_are_left_alone() {
        assert_eq!(emojify("a :notanemoji: b"), "a :notanemoji: b");
        assert_eq!(emojify("Note: no colons here"), "Note: no colons here");
        assert_eq!(emojify("x[1:10:2]"), "x[1:10:2]");
    }

    #[test]
    fn emoji_shortcodes_are_rendered_in_readmes() {
        assert!(render("Ship it :rocket:\n", false).contains('🚀'));
    }

    #[test]
    fn emoji_shortcodes_in_code_are_left_alone() {
        let inline = render("Call `f(:rocket:)` now.\n", false);
        assert!(inline.contains(":rocket:"), "{}", inline);
        assert!(!inline.contains('🚀'), "{}", inline);

        let block = render("```\nx <- :rocket:\n```\n", false);
        assert!(block.contains(":rocket:"), "{}", block);
        assert!(!block.contains('🚀'), "{}", block);
    }

    // -- links -------------------------------------------------------------

    #[test]
    fn links_become_label_plus_url_without_color() {
        let out = render("See the [docs](https://cli.r-lib.org) for more.\n", false);
        assert!(out.contains("docs (https://cli.r-lib.org)"), "{}", out);
        assert!(!out.contains('\u{1}') && !out.contains('\u{1b}'), "{}", out);
    }

    #[test]
    fn links_become_osc8_hyperlinks_with_color() {
        let out = render("See the [docs](https://cli.r-lib.org) for more.\n", true);
        assert!(
            out.contains("\x1b]8;;https://cli.r-lib.org\x1b\\"),
            "{}",
            out
        );
        assert!(out.contains("docs"), "{}", out);
        // The URL is the hyperlink target, not something to read.
        assert!(!out.contains("(https://cli.r-lib.org)"), "{}", out);
        assert!(
            !out.contains(LINK_START) && !out.contains(LINK_END),
            "{}",
            out
        );
    }

    #[test]
    fn badges_collapse_to_one_link_on_the_alt_text() {
        let md = "[![CRAN status](https://www.r-pkg.org/badges/version/cli)](https://CRAN.R-project.org/package=cli)\n";
        let out = render(md, true);
        assert!(out.contains("CRAN status"), "{}", out);
        assert!(
            out.contains("\x1b]8;;https://CRAN.R-project.org/package=cli\x1b\\"),
            "{}",
            out
        );
        // The badge image URL is gone entirely.
        assert!(!out.contains("r-pkg.org"), "{}", out);
    }

    #[test]
    fn badge_lines_are_not_joined_into_one() {
        let md = "[![one](a.svg)](https://a.example)\n[![two](b.svg)](https://b.example)\n";
        let out = render(md, false);
        assert_eq!(out.lines().count(), 2, "{:?}", out);
    }

    #[test]
    fn relative_targets_keep_the_label_only() {
        let out = render("![plot](man/figures/plot.png)\n", true);
        assert!(out.contains("plot"), "{}", out);
        assert!(!out.contains("man/figures"), "{}", out);
        assert!(!out.contains('\u{1b}'), "{}", out);
    }

    #[test]
    fn autolinks_and_bare_urls_do_not_repeat_the_url() {
        for md in ["<https://cli.r-lib.org>\n", "See https://cli.r-lib.org\n"] {
            let out = render(md, true);
            assert_eq!(
                out.matches("https://cli.r-lib.org").count(),
                2, // once in the OSC 8 target, once as the label
                "{}",
                out
            );
            assert!(
                out.contains("\x1b]8;;https://cli.r-lib.org\x1b\\"),
                "{}",
                out
            );
        }
    }

    #[test]
    fn reference_links_resolve() {
        let out = render("See the [docs][d].\n\n[d]: https://cli.r-lib.org\n", true);
        assert!(
            out.contains("\x1b]8;;https://cli.r-lib.org\x1b\\"),
            "{}",
            out
        );
        assert!(out.contains("docs"), "{}", out);
    }

    #[test]
    fn several_links_keep_their_own_targets() {
        let md = "[a](https://a.example) and [b](https://b.example)\n";
        let out = render(md, true);
        let a = out.find("\x1b]8;;https://a.example\x1b\\").expect("a");
        let b = out.find("\x1b]8;;https://b.example\x1b\\").expect("b");
        assert!(a < b, "{}", out);
    }

    #[test]
    fn link_labels_spanning_a_line_break_are_reopened() {
        // Long enough that termimad has to wrap the label.
        let label = "word ".repeat(30);
        let out = render(&format!("[{}](https://x.example)\n", label.trim()), true);
        let opens = out.matches("\x1b]8;;https://x.example\x1b\\").count();
        let closes = out.matches("\x1b]8;;\x1b\\").count();
        assert!(
            opens > 1,
            "expected a reopen per line, got {}: {:?}",
            opens,
            out
        );
        assert_eq!(opens, closes, "{:?}", out);
    }

    #[test]
    fn sentinels_in_the_readme_source_are_stripped() {
        let out = render("a \u{1}b\u{2} c\n", true);
        assert!(
            !out.contains(LINK_START) && !out.contains(LINK_END),
            "{:?}",
            out
        );
        assert!(!out.contains('\u{1b}'), "{:?}", out);
    }

    #[test]
    fn label_words_are_not_glued_across_a_soft_break() {
        let out = render("[some long\nlink text](https://x.example)\n", false);
        assert!(out.contains("some long link text"), "{}", out);
    }

    // -- HTML comments -----------------------------------------------------

    #[test]
    fn html_comments_are_hidden() {
        let md = "<!-- README.md is generated from README.Rmd -->\n\nHello.\n\n<!-- badges: start -->\n\n[a](https://a.example)\n\n<!-- badges: end -->\n";
        let out = render(md, false);
        assert!(!out.contains("<!--"), "{:?}", out);
        assert!(!out.contains("-->"), "{:?}", out);
        assert!(!out.contains("badges"), "{:?}", out);
        assert!(!out.contains("README.Rmd"), "{:?}", out);
        // The content between them survives.
        assert!(out.contains("Hello."), "{:?}", out);
        assert!(out.contains("a (https://a.example)"), "{:?}", out);
    }

    #[test]
    fn multi_line_html_comments_are_hidden() {
        let out = render("<!--\nhidden\nlines\n-->\n\nkept\n", false);
        assert!(
            !out.contains("hidden") && !out.contains("lines"),
            "{:?}",
            out
        );
        assert!(out.contains("kept"), "{:?}", out);
    }

    #[test]
    fn html_tags_go_but_their_text_stays() {
        let out = render("<div><!-- c -->text</div>\n", false);
        assert!(!out.contains("<!--") && !out.contains(" c "), "{:?}", out);
        assert!(
            !out.contains("<div>") && !out.contains("</div>"),
            "{:?}",
            out
        );
        assert!(out.contains("text"), "{:?}", out);
    }

    #[test]
    fn comments_in_code_blocks_are_kept() {
        let out = render("```\n<!-- keep me -->\n```\n", false);
        assert!(out.contains("<!-- keep me -->"), "{:?}", out);
    }

    // -- raw HTML links and images -----------------------------------------

    #[test]
    fn html_image_is_replaced_by_its_alt_text() {
        let out = render("<img src=\"logo.png\" alt=\"Hex logo\" />\n", false);
        assert!(out.contains("Hex logo"), "{:?}", out);
        assert!(
            !out.contains("logo.png") && !out.contains("<img"),
            "{:?}",
            out
        );
    }

    #[test]
    fn html_image_without_alt_text_leaves_nothing() {
        let out = render("<img src=\"logo.png\" width=\"100%\" />\n", false);
        assert!(out.trim().is_empty(), "{:?}", out);
    }

    #[test]
    fn html_link_around_an_image_shows_the_alt_text() {
        // dplyr's badge block is written like this.
        let md = "<a href=\"https://dplyr.tidyverse.org\"><img src=\"b.svg\" alt=\"CRAN status\" /></a>\n";
        let out = render(md, true);
        assert!(out.contains("CRAN status"), "{:?}", out);
        assert!(
            out.contains("\x1b]8;;https://dplyr.tidyverse.org\x1b\\"),
            "{:?}",
            out
        );
        assert!(!out.contains("b.svg") && !out.contains("<a "), "{:?}", out);

        let plain = render(md, false);
        assert!(
            plain.contains("CRAN status (https://dplyr.tidyverse.org)"),
            "{:?}",
            plain
        );
    }

    #[test]
    fn a_decorative_html_badge_is_dropped_whole() {
        // No alt text, so there is nothing to hang a hyperlink on.
        let md = "<a href=\"https://x.example\"><img src=\"b.svg\" /></a>\n";
        assert!(
            render(md, false).trim().is_empty(),
            "{:?}",
            render(md, false)
        );
        let out = render(md, true);
        assert!(!out.contains("x.example"), "{:?}", out);
        assert!(!out.contains('\u{1b}'), "{:?}", out);
    }

    #[test]
    fn a_decorative_markdown_badge_is_dropped_whole() {
        let md = "[![](https://cranlogs.r-pkg.org/badges/cli)](https://www.r-pkg.org/pkg/cli)\n";
        let out = render(md, true);
        assert!(!out.contains("r-pkg.org"), "{:?}", out);
        assert!(!out.contains('\u{1b}'), "{:?}", out);
    }

    #[test]
    fn html_link_around_text_keeps_the_text_once() {
        let out = render("<a href=\"https://x.example\">click</a>\n", false);
        assert!(out.contains("click (https://x.example)"), "{:?}", out);
    }

    #[test]
    fn html_line_break_breaks_the_line() {
        let out = render("one<br />two\n", false);
        assert!(out.contains("one\ntwo"), "{:?}", out);
    }

    #[test]
    fn attributes_are_read_case_insensitively_and_either_quoting() {
        let tag = "<IMG SRC=x ALT='q' >";
        assert_eq!(attr(tag, "alt").as_deref(), Some("q"));
        assert_eq!(attr(tag, "src").as_deref(), Some("x"));
        // `srcset` must not answer for `src`.
        assert_eq!(attr("<img srcset=\"a.svg\">", "src"), None);
        assert_eq!(attr("<img>", "alt"), None);
    }

    #[test]
    fn tag_len_ignores_angle_brackets_inside_quotes() {
        let tag = "<img alt=\"a > b\" />";
        assert_eq!(tag_len(tag), Some(tag.len()));
        assert_eq!(tag_len("<img"), None);
    }

    // -- reflow ------------------------------------------------------------

    #[test]
    fn paragraph_lines_are_joined_and_rewrapped() {
        // A source-wrapped paragraph, shorter than the render width once
        // joined: it must come out as a single line.
        let out = render(
            "See at\n[the docs](https://x.example)\nand also here.\n",
            false,
        );
        assert!(
            out.lines().next().unwrap().starts_with("See at the docs"),
            "{:?}",
            out
        );
    }

    #[test]
    fn block_quote_lines_are_not_joined() {
        // The `> ` prefix belongs to no span, so joining would show it inline.
        let out = render("> quote a\n> quote b\n", false);
        assert!(!out.contains('>'), "{:?}", out);
    }

    #[test]
    fn code_block_lines_are_not_joined() {
        let out = render("```\nfirst\nsecond\n```\n", false);
        assert!(out.contains("first") && out.contains("second"), "{:?}", out);
        assert!(!out.contains("first second"), "{:?}", out);
    }

    // -- unchanged behavior ------------------------------------------------

    #[test]
    fn markdown_readme_is_rendered() {
        let out = render("# Title\n\nSome *text*.\n", false);
        // Markup characters are gone, the words are not.
        assert!(out.contains("Title"));
        assert!(out.contains("Some text."));
        assert!(!out.contains('#'));
        assert!(!out.contains('*'));
    }

    #[test]
    fn text_readme_is_kept_verbatim() {
        let info = info_with_readme(Some("  keep   this\n\tand this\n"), Some("txt"));
        assert_eq!(
            format_readme(&info, false).as_deref(),
            Some("  keep   this\n\tand this\n")
        );
    }

    #[test]
    fn missing_or_unknown_readme_is_skipped() {
        assert!(format_readme(&info_with_readme(None, None), false).is_none());
        assert!(format_readme(&info_with_readme(Some("x"), None), false).is_none());
        assert!(format_readme(&info_with_readme(Some("<p>x</p>"), Some("html")), false).is_none());
        assert!(format_readme(&info_with_readme(Some("\n\n"), Some("md")), false).is_none());
    }
}
