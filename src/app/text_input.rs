use std::ops::Range;

#[derive(Default)]
pub(super) struct TextInputState {
    text: String,
    cursor: usize,
    selection_anchor: Option<usize>,
}

impl TextInputState {
    pub(super) fn text(&self) -> &str {
        &self.text
    }

    pub(super) fn cursor(&self) -> usize {
        self.cursor
    }

    pub(super) fn set_text(&mut self, text: String) {
        self.text = text;
        self.cursor = self.text.len();
        self.selection_anchor = None;
    }

    pub(super) fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.selection_anchor = None;
    }

    pub(super) fn move_to_end(&mut self) {
        self.cursor = self.text.len();
        self.selection_anchor = None;
    }

    pub(super) fn selection_range(&self) -> Option<Range<usize>> {
        let anchor = self.selection_anchor?;
        (anchor != self.cursor).then(|| anchor.min(self.cursor)..anchor.max(self.cursor))
    }

    pub(super) fn selected_text(&self) -> Option<String> {
        self.selection_range()
            .map(|range| self.text[range].to_string())
    }

    pub(super) fn select_all(&mut self) {
        self.cursor = self.text.len();
        self.selection_anchor = Some(0);
    }

    pub(super) fn insert(&mut self, text: &str) {
        let range = self.selection_range().unwrap_or(self.cursor..self.cursor);
        self.text.replace_range(range.clone(), text);
        self.cursor = range.start + text.len();
        self.selection_anchor = None;
    }

    pub(super) fn backspace(&mut self, by_word: bool) {
        if self.selection_range().is_some() {
            self.insert("");
            return;
        }

        if self.cursor == 0 {
            return;
        }

        let start = if by_word {
            self.previous_word_boundary(self.cursor)
        } else {
            self.previous_boundary(self.cursor)
        };
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
        self.selection_anchor = None;
    }

    pub(super) fn delete(&mut self, by_word: bool) {
        if self.selection_range().is_some() {
            self.insert("");
            return;
        }

        if self.cursor >= self.text.len() {
            return;
        }

        let end = if by_word {
            self.next_word_boundary(self.cursor)
        } else {
            self.next_boundary(self.cursor)
        };
        self.text.replace_range(self.cursor..end, "");
        self.selection_anchor = None;
    }

    pub(super) fn move_left(&mut self, by_word: bool, selecting: bool) {
        let offset = if !selecting {
            self.selection_range().map_or_else(
                || {
                    if by_word {
                        self.previous_word_boundary(self.cursor)
                    } else {
                        self.previous_boundary(self.cursor)
                    }
                },
                |range| range.start,
            )
        } else if by_word {
            self.previous_word_boundary(self.cursor)
        } else {
            self.previous_boundary(self.cursor)
        };

        self.move_cursor(offset, selecting);
    }

    pub(super) fn move_right(&mut self, by_word: bool, selecting: bool) {
        let offset = if !selecting {
            self.selection_range().map_or_else(
                || {
                    if by_word {
                        self.next_word_boundary(self.cursor)
                    } else {
                        self.next_boundary(self.cursor)
                    }
                },
                |range| range.end,
            )
        } else if by_word {
            self.next_word_boundary(self.cursor)
        } else {
            self.next_boundary(self.cursor)
        };

        self.move_cursor(offset, selecting);
    }

    pub(super) fn move_home(&mut self, selecting: bool) {
        self.move_cursor(0, selecting);
    }

    pub(super) fn move_end(&mut self, selecting: bool) {
        self.move_cursor(self.text.len(), selecting);
    }

    fn move_cursor(&mut self, offset: usize, selecting: bool) {
        let offset = self.clamp_boundary(offset);
        if selecting {
            self.selection_anchor.get_or_insert(self.cursor);
        } else {
            self.selection_anchor = None;
        }
        self.cursor = offset;
    }

    fn clamp_boundary(&self, offset: usize) -> usize {
        if offset >= self.text.len() {
            return self.text.len();
        }

        self.text
            .char_indices()
            .map(|(ix, _)| ix)
            .take_while(|ix| *ix <= offset)
            .last()
            .unwrap_or(0)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.text
            .char_indices()
            .map(|(ix, _)| ix)
            .filter(|ix| *ix < offset)
            .next_back()
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.text
            .char_indices()
            .map(|(ix, _)| ix)
            .find(|ix| *ix > offset)
            .unwrap_or(self.text.len())
    }

    fn previous_word_boundary(&self, offset: usize) -> usize {
        let mut previous = 0;
        let mut in_word = false;

        for (ix, ch) in self.text[..offset].char_indices().rev() {
            if ch.is_whitespace() {
                if in_word {
                    return previous;
                }
            } else {
                in_word = true;
                previous = ix;
            }
        }

        0
    }

    fn next_word_boundary(&self, offset: usize) -> usize {
        let mut saw_word = false;

        for (relative_ix, ch) in self.text[offset..].char_indices() {
            let ix = offset + relative_ix;
            if ch.is_whitespace() {
                if saw_word {
                    return ix;
                }
            } else {
                saw_word = true;
            }
        }

        self.text.len()
    }
}
