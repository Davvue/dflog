//! Escaping of untrusted text for terminals and plain-text files.
//!
//! Text from the journal is untrusted: file names come from evidence (e.g.
//! an unpacked archive) and a tampered journal can contain anything. Written
//! raw, a newline in a file name could forge report lines and an escape
//! sequence would be interpreted by the examiner's terminal. Everything
//! taken from the journal and printed as plain text goes through [`esc`].
//!
//! The HTML and Markdown reports have their own escaping for their syntax.

use std::fmt::{self, Display, Formatter};

/// Wraps a value so that its `Display` output is safe to print on one line
/// of a terminal or text file.
///
/// - `\` becomes `\\`, so the escapes below are unambiguous: a file named
///   `a\nb` (backslash, n) shows as `a\\nb`, one with a real newline as `a\nb`.
/// - Newline, carriage return and tab become `\n`, `\r` and `\t`.
/// - Every other control character (C0, DEL, C1, including ESC, which
///   starts terminal escape sequences) becomes `\u{..}`.
/// - Invisible bidirectional formatting characters (e.g. U+202E, which
///   makes `gpj.exe` display as `exe.jpg`) also become `\u{..}`, so a name
///   always reads in the order its characters are stored.
///
/// Everything else, including non-ASCII letters, is written unchanged.
pub fn esc<T: Display>(value: T) -> Escaped<T> {
    Escaped(value)
}

/// A value whose `Display` output is escaped, see [`esc`].
pub struct Escaped<T>(T);

impl<T: Display> Display for Escaped<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        use fmt::Write as _;

        /// Escapes each chunk the inner `Display` writes, so nothing is
        /// buffered.
        struct EscapeWriter<'a, 'b>(&'a mut Formatter<'b>);

        impl fmt::Write for EscapeWriter<'_, '_> {
            fn write_str(&mut self, s: &str) -> fmt::Result {
                for c in s.chars() {
                    match c {
                        '\\' => self.0.write_str("\\\\")?,
                        '\n' => self.0.write_str("\\n")?,
                        '\r' => self.0.write_str("\\r")?,
                        '\t' => self.0.write_str("\\t")?,
                        c if c.is_control() || is_bidi_control(c) => {
                            write!(self.0, "\\u{{{:x}}}", c as u32)?;
                        }
                        c => self.0.write_char(c)?,
                    }
                }
                Ok(())
            }
        }

        write!(EscapeWriter(f), "{}", self.0)
    }
}

/// Characters that change the display order of the text around them
/// without being visible themselves.
fn is_bidi_control(c: char) -> bool {
    matches!(
        c,
        '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn esc_escapes_control_and_bidi_characters() {
        let cases = [
            ("plain name.txt", "plain name.txt"),
            ("Ärger-数据.txt", "Ärger-数据.txt"),
            ("a\nb", "a\\nb"),
            ("a\\nb", "a\\\\nb"),
            ("a\r\tb", "a\\r\\tb"),
            ("\u{1b}[31mred", "\\u{1b}[31mred"),
            ("nul\0", "nul\\u{0}"),
            ("del\u{7f}", "del\\u{7f}"),
            ("c1\u{9b}31m", "c1\\u{9b}31m"),
            ("photo\u{202e}gpj.exe", "photo\\u{202e}gpj.exe"),
            ("x\u{2066}y\u{2069}", "x\\u{2066}y\\u{2069}"),
        ];
        for (input, expected) in cases {
            assert_eq!(esc(input).to_string(), expected, "input {input:?}");
        }
    }
}
