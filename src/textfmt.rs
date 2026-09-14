//! Small text formatting helpers shared by the commands that print metadata
//! tables and `label   value` blocks (`rig repos available`, `rig pkg info`).

/// Print a single `label   value` line, wrapping long values under the label.
pub fn print_field(label: &str, value: &str, width: usize, color: bool) {
    let mut out = String::new();
    write_field(&mut out, label, value, width, color);
    print!("{}", out);
}

/// Write a single `label   value` line, wrapping long values under the label.
pub fn write_field(out: &mut String, label: &str, value: &str, width: usize, color: bool) {
    use owo_colors::OwoColorize;
    use std::fmt::Write;
    let padded = format!("{:width$}", label);
    let shown_label = if color {
        padded.dimmed().to_string()
    } else {
        padded
    };
    let indent = " ".repeat(width);
    let lines = wrap(value, 78usize.saturating_sub(width));
    for (i, line) in lines.iter().enumerate() {
        if i == 0 {
            let _ = writeln!(out, "{}{}", shown_label, line);
        } else {
            let _ = writeln!(out, "{}{}", indent, line);
        }
    }
}

/// Collapse runs of whitespace (including the newlines DCF fields carry) into
/// single spaces.
pub fn reflow(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Turn a raw, already-unfolded DCF field value (as parsed by `deb822_fast`,
/// continuation-line indent stripped but internal newlines kept) into plain
/// text, undoing the DCF convention of writing a blank line inside a folded
/// field as a continuation line containing a single `.`.
pub fn dcf_field_to_text(raw: &str) -> String {
    raw.split('\n')
        .map(|line| if line == "." { "" } else { line })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Inverse of [`dcf_field_to_text`]: render `text` as a DCF field named
/// `key`, folding every line break into a continuation line (4-space
/// indent) and re-encoding blank lines as a lone `.`, so the field survives
/// unchanged through a DESCRIPTION -> TOML -> DESCRIPTION round trip.
pub fn text_to_dcf_field(key: &str, text: &str) -> String {
    const INDENT: &str = "    ";
    let mut out = format!("{}: ", key);
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
            out.push_str(INDENT);
        }
        out.push_str(if line.is_empty() { "." } else { line });
    }
    out
}

/// Word-wrap `text` to at most `width` columns, keeping words intact.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines: Vec<String> = vec![];
    let mut line = String::new();
    for word in text.split_whitespace() {
        if line.is_empty() {
            line.push_str(word);
        } else if line.len() + 1 + word.len() <= width {
            line.push(' ');
            line.push_str(word);
        } else {
            lines.push(std::mem::take(&mut line));
            line.push_str(word);
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reflow_collapses_newlines_and_spaces() {
        assert_eq!(reflow("a\nb  c\n  d"), "a b c d");
        assert_eq!(reflow("  spaced  out  "), "spaced out");
        assert_eq!(reflow(""), "");
    }

    #[test]
    fn wrap_keeps_words_intact_within_width() {
        let lines = wrap("the quick brown fox", 10);
        assert_eq!(lines, vec!["the quick", "brown fox"]);
        for line in &lines {
            assert!(line.len() <= 10);
        }
    }

    #[test]
    fn wrap_does_not_split_overlong_words() {
        let lines = wrap("supercalifragilistic word", 8);
        assert_eq!(lines, vec!["supercalifragilistic", "word"]);
    }

    #[test]
    fn wrap_empty_yields_single_empty_line() {
        assert_eq!(wrap("", 10), vec![String::new()]);
    }

    #[test]
    fn dcf_field_to_text_turns_lone_dot_into_blank_line() {
        assert_eq!(
            dcf_field_to_text("First paragraph.\n.\nSecond paragraph."),
            "First paragraph.\n\nSecond paragraph."
        );
        assert_eq!(dcf_field_to_text("One line."), "One line.");
    }

    #[test]
    fn text_to_dcf_field_turns_blank_line_into_lone_dot() {
        assert_eq!(
            text_to_dcf_field("Description", "First paragraph.\n\nSecond paragraph."),
            "Description: First paragraph.\n    .\n    Second paragraph."
        );
        assert_eq!(
            text_to_dcf_field("Description", "One line."),
            "Description: One line."
        );
    }

    #[test]
    fn dcf_field_round_trips_through_text() {
        // `deb822_fast` strips the continuation indent before rig ever sees
        // the value, so a folded field with indented continuation lines...
        let folded = "Para one,\n    still para one.\n    .\n    Para two.";
        let unfolded = folded.replace("\n    ", "\n");
        let text = dcf_field_to_text(&unfolded);
        let field = text_to_dcf_field("Description", &text);
        // ...re-folds back to the original, indent included.
        assert_eq!(field, format!("Description: {}", folded));
    }
}
