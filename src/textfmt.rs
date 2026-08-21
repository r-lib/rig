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
}
