//! Maps byte offsets ↔ LSP `line:character` positions.
//!
//! LSP positions are **UTF-16** code-unit offsets within a line (the protocol
//! default). Mangrove source is UTF-8 `str`, so a column is the number of UTF-16
//! code units in the line's prefix up to the byte offset — not bytes, not chars.

/// A precomputed index of line-start byte offsets for one document.
pub struct LineIndex {
    /// Byte offset of the start of each line. `line_starts[0] == 0`.
    line_starts: Vec<usize>,
    /// The full source, kept to measure UTF-16 column widths.
    text: String,
}

/// A zero-based `line` / UTF-16 `character` position (LSP coordinates).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pos {
    pub line: u32,
    pub character: u32,
}

impl LineIndex {
    pub fn new(text: &str) -> LineIndex {
        let mut line_starts = vec![0usize];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        LineIndex {
            line_starts,
            text: text.to_string(),
        }
    }

    /// Byte offset → LSP position (UTF-16 column).
    pub fn position(&self, offset: usize) -> Pos {
        let offset = offset.min(self.text.len());
        // The line is the last line-start <= offset.
        let line = match self.line_starts.binary_search(&offset) {
            Ok(l) => l,
            Err(next) => next - 1,
        };
        let line_start = self.line_starts[line];
        // UTF-16 width of the slice [line_start, offset), excluding a trailing
        // `\r` so that an offset at the `\n` of a CRLF terminator reports the
        // end-of-line column (line content) rather than one past it. The `\r\n`
        // is a line terminator and is not counted as line content.
        let slice = &self.text[line_start..offset];
        let slice = slice.strip_suffix('\r').unwrap_or(slice);
        let character = slice.chars().map(char::len_utf16).sum::<usize>() as u32;
        Pos {
            line: line as u32,
            character,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_line_offsets() {
        let idx = LineIndex::new("abc\ndef\n");
        assert_eq!(
            idx.position(0),
            Pos {
                line: 0,
                character: 0
            }
        );
        assert_eq!(
            idx.position(2),
            Pos {
                line: 0,
                character: 2
            }
        );
        // start of second line
        assert_eq!(
            idx.position(4),
            Pos {
                line: 1,
                character: 0
            }
        );
        assert_eq!(
            idx.position(6),
            Pos {
                line: 1,
                character: 2
            }
        );
    }

    #[test]
    fn utf16_columns_count_code_units_not_bytes() {
        // "é" is 2 bytes in UTF-8 but 1 UTF-16 code unit;
        // "𝄞" (U+1D11E) is 4 bytes UTF-8 but 2 UTF-16 code units (surrogate pair).
        let idx = LineIndex::new("é𝄞x");
        // byte 0: before é
        assert_eq!(idx.position(0).character, 0);
        // after é (2 bytes) → 1 UTF-16 unit
        assert_eq!(idx.position(2).character, 1);
        // after 𝄞 (4 more bytes) → 1 + 2 = 3 UTF-16 units
        assert_eq!(idx.position(6).character, 3);
    }

    #[test]
    fn offset_past_end_clamps() {
        let idx = LineIndex::new("ab");
        assert_eq!(
            idx.position(999),
            Pos {
                line: 0,
                character: 2
            }
        );
    }

    #[test]
    fn crlf_positions_are_correct() {
        // Line 0: "a: 1\r\n" (6 bytes, line 1 starts at byte 6)
        // Line 1: "b: 2\r\n"
        let idx = LineIndex::new("a: 1\r\nb: 2\r\n");
        assert_eq!(
            idx.position(6),
            Pos {
                line: 1,
                character: 0
            }
        );
        // 'b: 2' — character 3 is '2'
        assert_eq!(
            idx.position(9),
            Pos {
                line: 1,
                character: 3
            }
        );
    }

    #[test]
    fn empty_document() {
        let idx = LineIndex::new("");
        assert_eq!(
            idx.position(0),
            Pos {
                line: 0,
                character: 0
            }
        );
    }

    #[test]
    fn position_at_eol_before_crlf() {
        // "abc\r\ndef": byte layout a(0) b(1) c(2) \r(3) \n(4) d(5) e(6) f(7)
        let idx = LineIndex::new("abc\r\ndef");
        // Offset at the `\r` (byte 3): prefix is "abc" → character 3.
        assert_eq!(
            idx.position(3),
            Pos {
                line: 0,
                character: 3
            }
        );
        // Offset at the `\n` (byte 4): prefix is "abc\r"; the trailing `\r` must
        // NOT inflate the column — end-of-line is still character 3, not 4.
        assert_eq!(
            idx.position(4),
            Pos {
                line: 0,
                character: 3
            }
        );
        // byte 5 is 'd' → start of line 1.
        assert_eq!(
            idx.position(5),
            Pos {
                line: 1,
                character: 0
            }
        );
    }

    #[test]
    fn position_at_eol_after_astral_char_crlf() {
        // An astral char before a CRLF: "𝄞\r\nx".
        // bytes: 𝄞 = [0..4), \r = 4, \n = 5, x = 6.
        // 𝄞 is 2 UTF-16 code units (surrogate pair).
        let idx = LineIndex::new("𝄞\r\nx");
        // After 𝄞 (byte 4 = the `\r`): prefix is "𝄞" → 2 UTF-16 units.
        assert_eq!(
            idx.position(4),
            Pos {
                line: 0,
                character: 2
            }
        );
        // At the `\n` (byte 5): prefix "𝄞\r"; trailing `\r` dropped → still 2.
        assert_eq!(
            idx.position(5),
            Pos {
                line: 0,
                character: 2
            }
        );
        // byte 6 is 'x' → start of line 1.
        assert_eq!(
            idx.position(6),
            Pos {
                line: 1,
                character: 0
            }
        );
    }
}
