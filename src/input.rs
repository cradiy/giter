use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Default)]
pub struct Input {
    pub text: String,
    pub cursor: usize,
}

impl Input {
    pub fn insert(&mut self, text: &str, multiline: bool) {
        let text: String = text
            .replace("\r\n", "\n")
            .chars()
            .filter(|c| !c.is_control() || (multiline && *c == '\n'))
            .collect();
        self.text.insert_str(self.cursor, &text);
        self.cursor += text.len();
        // A combining character may have joined the following grapheme.
        self.cursor = self
            .text
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .find(|i| *i >= self.cursor)
            .unwrap_or(self.text.len());
    }

    fn previous(&self) -> usize {
        self.text[..self.cursor]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(i, _)| i)
    }

    fn next(&self) -> usize {
        self.text[self.cursor..]
            .graphemes(true)
            .next()
            .map_or(self.cursor, |g| self.cursor + g.len())
    }

    fn vertical(&mut self, down: bool) {
        let start = self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1);
        let column = self.text[start..self.cursor].width();
        let target = if down {
            let Some(end) = self.text[self.cursor..].find('\n') else {
                return;
            };
            self.cursor + end + 1
        } else {
            if start == 0 {
                return;
            }
            self.text[..start - 1].rfind('\n').map_or(0, |i| i + 1)
        };
        let end = target
            + self.text[target..]
                .find('\n')
                .unwrap_or(self.text.len() - target);
        let mut width = 0;
        self.cursor = target;
        for grapheme in self.text[target..end].graphemes(true) {
            if width + grapheme.width() > column {
                break;
            }
            width += grapheme.width();
            self.cursor += grapheme.len();
        }
    }

    pub fn key(&mut self, key: KeyEvent, multiline: bool) {
        match key.code {
            KeyCode::Left => self.cursor = self.previous(),
            KeyCode::Right => self.cursor = self.next(),
            KeyCode::Up if multiline => self.vertical(false),
            KeyCode::Down if multiline => self.vertical(true),
            KeyCode::Home => {
                self.cursor = self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1)
            }
            KeyCode::End => {
                self.cursor += self.text[self.cursor..]
                    .find('\n')
                    .unwrap_or(self.text.len() - self.cursor)
            }
            KeyCode::Backspace if self.cursor > 0 => {
                let previous = self.previous();
                self.text.replace_range(previous..self.cursor, "");
                self.cursor = previous;
            }
            KeyCode::Delete if self.cursor < self.text.len() => {
                self.text.replace_range(self.cursor..self.next(), "");
            }
            KeyCode::Enter if multiline => self.insert("\n", true),
            KeyCode::Char(c)
                if !key.modifiers.intersects(
                    KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                ) =>
            {
                self.insert(&c.to_string(), multiline)
            }
            _ => {}
        }
    }

    pub fn cursor_cell(&self) -> (usize, usize) {
        let before = &self.text[..self.cursor];
        (
            before.rsplit('\n').next().unwrap_or("").width(),
            before.bytes().filter(|b| *b == b'\n').count(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_cjk_combining_and_emoji_as_graphemes() {
        let mut input = Input::default();
        input.insert("修复e\u{301}👩‍💻", false);
        input.key(KeyCode::Left.into(), false);
        input.key(KeyCode::Backspace.into(), false);
        assert_eq!(input.text, "修复👩‍💻");
        assert_eq!(input.cursor_cell(), (4, 0));
        input.key(KeyCode::Delete.into(), false);
        assert_eq!(input.text, "修复");
        input.insert("\r\n第二行\u{1b}", true);
        assert_eq!(input.text, "修复\n第二行");
        assert_eq!(input.cursor_cell(), (6, 1));
        input.key(KeyCode::Up.into(), true);
        assert_eq!(input.cursor_cell(), (4, 0));
        input.key(KeyCode::Down.into(), true);
        assert_eq!(input.cursor_cell(), (4, 1));
    }
}
