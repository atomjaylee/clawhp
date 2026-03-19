/// Strip ANSI escape sequences (colors, cursor moves, etc.) from a string.
pub(crate) fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // ESC [ ... final_byte  or  ESC ] ... BEL/ST
            if let Some(&next) = chars.peek() {
                if next == '[' {
                    chars.next();
                    // consume until 0x40-0x7E
                    while let Some(&ch) = chars.peek() {
                        chars.next();
                        if ('\x40'..='\x7e').contains(&ch) {
                            break;
                        }
                    }
                    continue;
                } else if next == ']' {
                    chars.next();
                    // OSC: consume until BEL (\x07) or ST (ESC \)
                    while let Some(ch) = chars.next() {
                        if ch == '\x07' {
                            break;
                        }
                        if ch == '\x1b' {
                            let _ = chars.next(); // consume '\'
                            break;
                        }
                    }
                    continue;
                } else if next == '(' || next == ')' {
                    chars.next();
                    let _ = chars.next();
                    continue;
                }
            }
            continue;
        }
        // Also filter carriage returns and other control chars (except newline/tab)
        if c == '\r' {
            continue;
        }
        if c.is_control() && c != '\n' && c != '\t' {
            continue;
        }
        out.push(c);
    }
    out
}

/// Clean a log line: strip ANSI + trim
pub(crate) fn clean_line(s: &str) -> String {
    let stripped = strip_ansi(s);
    stripped.trim().to_string()
}

pub(crate) fn first_meaningful_line(value: &str) -> String {
    value
        .lines()
        .map(clean_line)
        .find(|line| !line.is_empty())
        .unwrap_or_else(|| clean_line(value))
}
