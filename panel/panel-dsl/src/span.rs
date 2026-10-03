//! Byte ranges in a source file and their GNU-style line and column form.

use std::ops::Range;

/// A half-open byte range in one source file.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    /// The smallest span covering both.
    pub fn join(self, other: Self) -> Self {
        Self::new(self.start.min(other.start), self.end.max(other.end))
    }

    pub fn range(self) -> Range<usize> {
        self.start..self.end
    }

    pub const fn is_empty(self) -> bool {
        self.start >= self.end
    }
}

/// Line starts of a file, for turning byte offsets into positions.
///
/// Lines and columns start at 1 and columns count characters, so a position
/// maps directly onto an editor's cursor.
#[derive(Clone, Debug)]
pub struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    pub fn new(text: &str) -> Self {
        let starts = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(offset, _)| offset + 1))
            .collect();
        Self { starts }
    }

    /// The 1-based line and character column of `offset`.
    pub fn position(&self, text: &str, offset: usize) -> (usize, usize) {
        let offset = offset.min(text.len());
        let line = self.starts.partition_point(|start| *start <= offset) - 1;
        let start = self.starts[line];
        let column = text
            .get(start..offset)
            .map_or(offset - start, |prefix| prefix.chars().count());
        (line + 1, column + 1)
    }

    /// The offset of a 1-based line and character column; a column past the
    /// end of its line is its end.
    pub fn offset(&self, text: &str, line: usize, column: usize) -> Option<usize> {
        let start = *self.starts.get(line.checked_sub(1)?)?;
        let end = self
            .starts
            .get(line)
            .map_or(text.len(), |next| next.saturating_sub(1));
        let line_text = text.get(start..end)?;
        Some(
            start
                + line_text
                    .char_indices()
                    .nth(column.checked_sub(1)?)
                    .map_or(line_text.len(), |(offset, _)| offset),
        )
    }

    /// The span as the GNU Coding Standards format it: `file:line.column`,
    /// `file:line.column-column` within a line, or
    /// `file:line.column-line.column`, with the end column inclusive.
    pub fn describe(&self, file: &str, text: &str, span: Span) -> String {
        let (line, column) = self.position(text, span.start);
        if span.is_empty() {
            return format!("{file}:{line}.{column}");
        }
        let last = text
            .get(..span.end)
            .and_then(|prefix| prefix.char_indices().next_back())
            .map_or(span.end.saturating_sub(1), |(offset, _)| offset)
            .max(span.start);
        let (end_line, end_column) = self.position(text, last);
        if end_line == line {
            if end_column == column {
                format!("{file}:{line}.{column}")
            } else {
                format!("{file}:{line}.{column}-{end_column}")
            }
        } else {
            format!("{file}:{line}.{column}-{end_line}.{end_column}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_count_characters_from_one() {
        let text = "ab\ncdé f\n";
        let index = LineIndex::new(text);
        assert_eq!(index.position(text, 0), (1, 1));
        assert_eq!(index.position(text, 3), (2, 1));
        let f = text.find('f').unwrap();
        assert_eq!(index.position(text, f), (2, 5));
        assert_eq!(index.position(text, text.len()), (3, 1));
    }

    #[test]
    fn offsets_invert_positions() {
        let text = "ab\ncdé f\n";
        let index = LineIndex::new(text);
        for offset in text.char_indices().map(|(offset, _)| offset) {
            let (line, column) = index.position(text, offset);
            assert_eq!(index.offset(text, line, column), Some(offset));
        }
        assert_eq!(index.offset(text, 1, 40), Some(2));
        assert_eq!(index.offset(text, 3, 1), Some(text.len()));
        assert_eq!(index.offset(text, 4, 1), None);
        assert_eq!(index.offset(text, 0, 1), None);
        assert_eq!(index.offset(text, 1, 0), None);
    }

    #[test]
    fn spans_use_the_gnu_forms() {
        let text = "listen 80;\nserver {\n}\n";
        let index = LineIndex::new(text);
        assert_eq!(
            index.describe("main.conf", text, Span::new(0, 6)),
            "main.conf:1.1-6"
        );
        assert_eq!(
            index.describe("main.conf", text, Span::new(9, 10)),
            "main.conf:1.10"
        );
        assert_eq!(
            index.describe("main.conf", text, Span::new(11, 21)),
            "main.conf:2.1-3.1"
        );
        assert_eq!(
            index.describe("main.conf", text, Span::new(4, 4)),
            "main.conf:1.5"
        );
    }
}
