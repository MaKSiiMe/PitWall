//! Trackmania text formatting (`$f00red$z`, `$o`, `$l[url]link$l`, ...).

/// Removes formatting codes, keeping the visible text.
pub fn strip_formatting(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        let Some(&code) = chars.peek() else { break };
        chars.next();
        match code {
            '$' => out.push('$'),
            c if c.is_ascii_hexdigit() => {
                // Colors are 3 hex digits; tolerate shorter ones at the end of the string.
                for _ in 0..2 {
                    if chars.peek().is_some_and(char::is_ascii_hexdigit) {
                        chars.next();
                    }
                }
            }
            // Optional `[target]` after link codes.
            'l' | 'L' | 'h' | 'H' | 'p' | 'P' if chars.peek() == Some(&'[') => {
                for c in chars.by_ref() {
                    if c == ']' {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Escapes text so that it is displayed verbatim (no formatting).
pub fn escape_formatting(text: &str) -> String {
    text.replace('$', "$$")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_codes() {
        assert_eq!(strip_formatting("$f00Red$z $oBold$$"), "Red Bold$");
        assert_eq!(strip_formatting("$l[https://x.y]link$l end"), "link end");
        assert_eq!(strip_formatting("$0afPit$fffWall"), "PitWall");
        assert_eq!(strip_formatting("trailing $"), "trailing ");
        assert_eq!(escape_formatting("a$b"), "a$$b");
    }
}
