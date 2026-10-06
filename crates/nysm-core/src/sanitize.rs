//! Make untrusted text (process names, paths, mount points) safe to print
//! on a terminal. Control characters could otherwise inject escape
//! sequences that move the cursor, retitle the window or worse.

use std::borrow::Cow;

fn is_dangerous(c: char) -> bool {
    c.is_control() // C0, DEL, C1
        // Bidirectional overrides/isolates can visually reorder text.
        || matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}')
}

/// Replace control and bidi-control characters with visible escapes.
pub fn for_terminal(s: &str) -> Cow<'_, str> {
    if !s.chars().any(is_dangerous) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        if is_dangerous(c) {
            match c {
                '\n' => out.push_str("\\n"),
                '\t' => out.push_str("\\t"),
                '\r' => out.push_str("\\r"),
                c if (c as u32) < 0x100 => out.push_str(&format!("\\x{:02x}", c as u32)),
                c => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            }
        } else {
            out.push(c);
        }
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_borrowed() {
        assert!(matches!(for_terminal("firefox"), Cow::Borrowed(_)));
    }

    #[test]
    fn escape_sequences_are_neutralised() {
        let evil = "x\u{1b}]0;pwned\u{7}\u{9b}31m";
        let s = for_terminal(evil);
        assert!(!s.contains('\u{1b}'));
        assert!(!s.contains('\u{7}'));
        assert!(!s.contains('\u{9b}'));
        assert_eq!(s, "x\\x1b]0;pwned\\x07\\x9b31m");
    }

    #[test]
    fn bidi_overrides_are_visible() {
        assert_eq!(for_terminal("a\u{202E}b"), "a\\u{202e}b");
    }

    #[test]
    fn unicode_names_survive() {
        assert_eq!(for_terminal("kworker/ü:1-é"), "kworker/ü:1-é");
    }
}
