//! Line-oriented text helpers.
//!
//! Lines always keep their terminator so that a merge result can be written
//! back byte-for-byte (CRLF files stay CRLF, a missing final newline stays
//! missing).

/// Split `text` into lines, each retaining its terminator.
pub fn split_lines(text: &str) -> Vec<String> {
    text.split_inclusive('\n').map(str::to_owned).collect()
}

/// The line without its `\n` / `\r\n` terminator.
pub fn strip_eol(line: &str) -> &str {
    match line.strip_suffix('\n') {
        Some(rest) => rest.strip_suffix('\r').unwrap_or(rest),
        None => line,
    }
}

/// Whether the line ends with a line terminator.
pub fn has_eol(line: &str) -> bool {
    line.ends_with('\n')
}

/// The line ending used by the majority of `lines` (`\n` when undecided).
pub fn dominant_eol<'a>(lines: impl IntoIterator<Item = &'a str>) -> &'static str {
    let (mut crlf, mut lf) = (0usize, 0usize);
    for line in lines {
        if line.ends_with("\r\n") {
            crlf += 1;
        } else if line.ends_with('\n') {
            lf += 1;
        }
    }
    if crlf > lf {
        "\r\n"
    } else {
        "\n"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_keeps_terminators_and_roundtrips() {
        let text = "a\nb\r\n\nlast";
        let lines = split_lines(text);
        assert_eq!(lines, ["a\n", "b\r\n", "\n", "last"]);
        assert_eq!(lines.concat(), text);
        assert!(split_lines("").is_empty());
    }

    #[test]
    fn strip_eol_only_removes_the_terminator() {
        assert_eq!(strip_eol("a\n"), "a");
        assert_eq!(strip_eol("a\r\n"), "a");
        assert_eq!(strip_eol("a\r"), "a\r");
        assert_eq!(strip_eol("a"), "a");
    }

    #[test]
    fn dominant_eol_picks_the_majority() {
        assert_eq!(dominant_eol(["a\r\n", "b\r\n", "c\n"]), "\r\n");
        assert_eq!(dominant_eol(["a\n", "b"]), "\n");
        assert_eq!(dominant_eol([]), "\n");
    }
}
