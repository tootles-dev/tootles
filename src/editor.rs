//! Grapheme-aware editing shared by all text inputs. Cursor offsets are UTF-8 bytes.
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_segmentation::UnicodeSegmentation;

pub fn clean_paste(text: &str, multiline: bool) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .filter(|c| !c.is_control() || (multiline && *c == '\n'))
        .collect()
}
pub fn previous(text: &str, cursor: usize) -> usize {
    text.grapheme_indices(true)
        .map(|(i, _)| i)
        .take_while(|i| *i < cursor)
        .last()
        .unwrap_or(0)
}
pub fn next(text: &str, cursor: usize) -> usize {
    text.grapheme_indices(true)
        .map(|(i, _)| i)
        .find(|i| *i > cursor)
        .unwrap_or(text.len())
}
pub fn insert(text: &mut String, cursor: &mut usize, value: &str, multiline: bool) {
    *cursor = (*cursor).min(text.len());
    while !text.is_char_boundary(*cursor) {
        *cursor -= 1;
    }
    let value = clean_paste(value, multiline);
    text.insert_str(*cursor, &value);
    *cursor += value.len();
}
pub fn edit(text: &mut String, cursor: &mut usize, key: KeyEvent, multiline: bool) {
    *cursor = (*cursor).min(text.len());
    while !text.is_char_boundary(*cursor) {
        *cursor -= 1;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('a') if ctrl => *cursor = 0,
        KeyCode::Char('e') if ctrl => *cursor = text.len(),
        KeyCode::Char('u') if ctrl => {
            text.drain(..*cursor);
            *cursor = 0;
        }
        KeyCode::Char('w') if ctrl => {
            let mut p = *cursor;
            while p > 0
                && text[..p]
                    .chars()
                    .next_back()
                    .is_some_and(char::is_whitespace)
            {
                p = previous(text, p);
            }
            while p > 0
                && text[..p]
                    .chars()
                    .next_back()
                    .is_some_and(|c| !c.is_whitespace())
            {
                p = previous(text, p);
            }
            text.drain(p..*cursor);
            *cursor = p;
        }
        KeyCode::Left => *cursor = previous(text, *cursor),
        KeyCode::Right => *cursor = next(text, *cursor),
        KeyCode::Home => *cursor = text[..*cursor].rfind('\n').map_or(0, |i| i + 1),
        KeyCode::End => {
            *cursor = text[*cursor..]
                .find('\n')
                .map_or(text.len(), |i| *cursor + i)
        }
        KeyCode::Up | KeyCode::Down if multiline => {
            let start = text[..*cursor].rfind('\n').map_or(0, |i| i + 1);
            let col = text[start..*cursor].graphemes(true).count();
            let range = if key.code == KeyCode::Up && start > 0 {
                let end = start - 1;
                Some((text[..end].rfind('\n').map_or(0, |i| i + 1), end))
            } else if key.code == KeyCode::Down {
                text[*cursor..].find('\n').map(|i| {
                    let a = *cursor + i + 1;
                    let b = text[a..].find('\n').map_or(text.len(), |j| a + j);
                    (a, b)
                })
            } else {
                None
            };
            if let Some((a, b)) = range {
                *cursor = text[a..b]
                    .grapheme_indices(true)
                    .nth(col)
                    .map_or(b, |(i, _)| a + i);
            }
        }
        KeyCode::Backspace if *cursor > 0 => {
            let p = previous(text, *cursor);
            text.drain(p..*cursor);
            *cursor = p;
        }
        KeyCode::Delete if *cursor < text.len() => {
            let n = next(text, *cursor);
            text.drain(*cursor..n);
        }
        KeyCode::Enter if multiline => insert(text, cursor, "\n", true),
        KeyCode::Char(c)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            insert(text, cursor, &c.to_string(), multiline)
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn delete_graphemes_without_corrupting_utf8() {
        let mut s = "a👩‍💻e\u{301}".to_string();
        let mut c = s.len();
        edit(
            &mut s,
            &mut c,
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
            true,
        );
        assert_eq!(s, "a👩‍💻");
        edit(
            &mut s,
            &mut c,
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
            true,
        );
        assert_eq!(s, "a");
    }
    #[test]
    fn paste_cannot_inject_terminal_escape_sequences() {
        assert_eq!(
            clean_paste("hi\x1b[31m\r\nthere\x07", true),
            "hi[31m\nthere"
        );
    }
    #[test]
    fn vertical_cursor_keeps_grapheme_column() {
        let mut s = "aé\nx👩‍💻z".to_string();
        let mut c = s.len();
        edit(
            &mut s,
            &mut c,
            KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
            true,
        );
        assert_eq!(c, 3);
    }
}
