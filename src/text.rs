//! String helpers shared by the config writers and the UI.

/// Close to Python's `str.isprintable()` for a single character: control,
/// format, separator (other than the ASCII space) and private-use characters
/// are not printable.
pub fn is_printable(character: char) -> bool {
    if character == ' ' {
        return true;
    }
    if character.is_control() || character.is_whitespace() {
        return false;
    }
    let code = character as u32;
    let format = matches!(
        code,
        0x00AD
            | 0x0600..=0x0605
            | 0x061C
            | 0x06DD
            | 0x070F
            | 0x0890..=0x0891
            | 0x08E2
            | 0x180E
            | 0x200B..=0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x2064
            | 0x2066..=0x206F
            | 0xFEFF
            | 0xFFF9..=0xFFFB
            | 0x110BD
            | 0x110CD
            | 0x13430..=0x1343F
            | 0x1BCA0..=0x1BCA3
            | 0x1D173..=0x1D17A
            | 0xE0001
            | 0xE0020..=0xE007F
    );
    let private = matches!(code, 0xE000..=0xF8FF | 0xF0000..=0x10FFFF);
    !(format || private)
}

/// True if `value` cannot break out of a config line (no newlines etc.).
pub fn printable(value: &str) -> bool {
    value.chars().all(is_printable)
}

/// Python-style `repr()` of a string, used in messages.
pub fn repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut result = String::with_capacity(text.len() + 2);
    result.push(quote);
    for character in text.chars() {
        match character {
            '\\' => result.push_str("\\\\"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            c if c == quote => {
                result.push('\\');
                result.push(c);
            }
            c if !is_printable(c) => {
                let code = c as u32;
                if code <= 0xff {
                    result.push_str(&format!("\\x{code:02x}"));
                } else if code <= 0xffff {
                    result.push_str(&format!("\\u{code:04x}"));
                } else {
                    result.push_str(&format!("\\U{code:08x}"));
                }
            }
            c => result.push(c),
        }
    }
    result.push(quote);
    result
}

/// Python's `str.splitlines()`.
pub fn splitlines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((index, character)) = chars.next() {
        let breaks = matches!(
            character,
            '\n' | '\r'
                | '\u{0b}'
                | '\u{0c}'
                | '\u{1c}'
                | '\u{1d}'
                | '\u{1e}'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        );
        if !breaks {
            continue;
        }
        lines.push(&text[start..index]);
        let mut end = index + character.len_utf8();
        if character == '\r'
            && let Some((next, '\n')) = chars.peek().copied()
        {
            chars.next();
            end = next + 1;
        }
        start = end;
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

/// Python's `str.strip(chars)` for a set of characters.
pub fn strip_chars<'a>(text: &'a str, chars: &[char]) -> &'a str {
    text.trim_matches(|c| chars.contains(&c))
}

/// Quote a value for sway / niri configs and commands.
pub fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Standard base64 with padding (for OSC 52 clipboard writes).
pub fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let bytes = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let group = u32::from(bytes[0]) << 16 | u32::from(bytes[1]) << 8 | u32::from(bytes[2]);
        for position in 0..4 {
            if position <= chunk.len() {
                let index = (group >> (18 - 6 * position)) & 0x3f;
                result.push(char::from(ALPHABET[index as usize]));
            } else {
                result.push('=');
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printable_rejects_line_breaks_and_invisible_characters() {
        assert!(printable("HDMI-A-1 café 1080p"));
        assert!(!printable("a\nb"));
        assert!(!printable("a\u{2028}b"));
        assert!(!printable("a\u{200b}b"));
        assert!(!printable("a\u{a0}b"));
        assert!(!printable("\u{e000}"));
    }

    #[test]
    fn repr_matches_python() {
        assert_eq!(repr("work"), "'work'");
        assert_eq!(repr("it's"), "\"it's\"");
        assert_eq!(repr("a\nb"), "'a\\nb'");
        assert_eq!(repr("a\u{7}"), "'a\\x07'");
    }

    #[test]
    fn splitlines_matches_python() {
        assert_eq!(splitlines("a\nb\r\nc\rd\n"), ["a", "b", "c", "d"]);
        assert_eq!(splitlines("a\n\nb"), ["a", "", "b"]);
        assert!(splitlines("").is_empty());
        assert_eq!(splitlines("x\u{2028}y"), ["x", "y"]);
    }

    #[test]
    fn quote_escapes() {
        assert_eq!(quote(r#"a"b\c"#), r#""a\"b\\c""#);
    }

    #[test]
    fn base64_matches_the_standard_alphabet() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64("café/~".as_bytes()), "Y2Fmw6kvfg==");
    }
}
