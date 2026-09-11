use std::env;

/// Invocation context of the `toggle` action: which tab called, which pane
/// was focused (the agent pane — our send-text target), and its cwd.
pub struct ActionCtx {
    pub tab: String,
    pub agent_pane: String,
    pub cwd: String,
}

pub fn read_action_ctx() -> Result<ActionCtx, String> {
    // Given: action invoked from a pane, env populated by herdr.
    // When: reading the invocation context.
    // Should: resolve tab + focused pane + cwd or explain what is missing.
    let json = env::var("HERDR_PLUGIN_CONTEXT_JSON").map_err(|_| {
        "HERDR_PLUGIN_CONTEXT_JSON is not set (run via the herdr toggle action)".to_string()
    })?;
    let agent_pane = env::var("HERDR_PANE_ID")
        .map_err(|_| "HERDR_PANE_ID is not set (run via the herdr toggle action)".to_string())?;
    let tab = find_str(&json, "tab_id").ok_or("plugin context is missing tab_id")?;
    let cwd = find_str(&json, "workspace_cwd")
        .or_else(|| find_str(&json, "focused_pane_cwd"))
        .ok_or("plugin context is missing workspace_cwd")?;
    Ok(ActionCtx {
        tab,
        agent_pane,
        cwd,
    })
}

/// First `"key": "value"` match, string-aware: keys embedded inside string
/// literals (nested blobs, notes) are skipped — only real object keys
/// outside strings count. Not a general JSON parser by design.
pub fn find_str(json: &str, key: &str) -> Option<String> {
    let bytes = json.as_bytes();
    let pat = format!("\"{key}\"");
    let pb = pat.as_bytes();
    let n = bytes.len();
    let mut i = 0;
    while i < n {
        if bytes[i] != b'"' {
            i += 1;
            continue;
        }
        if i + pb.len() <= n && &bytes[i..i + pb.len()] == pb {
            let mut j = skip_ws(bytes, i + pb.len());
            if bytes.get(j) == Some(&b':') {
                j = skip_ws(bytes, j + 1);
                if bytes.get(j) == Some(&b'"') && j < n {
                    return Some(unquote(&json[j + 1..]));
                }
            }
        }
        i = skip_string(bytes, i);
    }
    None
}

fn skip_ws(bytes: &[u8], mut j: usize) -> usize {
    while j < bytes.len() && matches!(bytes[j], b' ' | b'\t' | b'\n' | b'\r') {
        j += 1;
    }
    j
}

/// Index just past the closing quote of the literal opening at `i`.
fn skip_string(bytes: &[u8], i: usize) -> usize {
    let mut j = i + 1;
    while j < bytes.len() {
        match bytes[j] {
            b'\\' => j += 2,
            b'"' => return j + 1,
            _ => j += 1,
        }
    }
    bytes.len()
}

fn unquote(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            if c == '"' {
                break;
            }
            out.push(c);
            continue;
        }
        match it.next() {
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some('/') => out.push('/'),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('u') => {
                let hex: String = it.by_ref().take(4).collect();
                let code = u32::from_str_radix(&hex, 16).unwrap_or(0xFFFD);
                // Combine UTF-16 surrogate pairs (emoji in paths).
                if (0xD800..0xDC00).contains(&code) {
                    let mut look = it.clone();
                    if look.next() == Some('\\') && look.next() == Some('u') {
                        let low_hex: String = look.take(4).collect();
                        if let Ok(low) = u32::from_str_radix(&low_hex, 16) {
                            if (0xDC00..0xE000).contains(&low) {
                                let full = 0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00);
                                if let Some(ch) = char::from_u32(full) {
                                    for _ in 0..6 {
                                        it.next();
                                    }
                                    out.push(ch);
                                    continue;
                                }
                            }
                        }
                    }
                }
                out.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
            }
            Some(other) => out.push(other),
            None => break,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_top_level_fields() {
        // Given a herdr-style context blob.
        let json = r#"{"tab_id":"w4:t2","workspace_cwd":"/Users/u/my/myx","pane_id":"w4:p2"}"#;
        // When extracting fields Then values match.
        assert_eq!(find_str(json, "tab_id").as_deref(), Some("w4:t2"));
        assert_eq!(
            find_str(json, "workspace_cwd").as_deref(),
            Some("/Users/u/my/myx")
        );
    }

    #[test]
    fn skips_non_string_values() {
        // Given a key bound to a number first, string later.
        let json = r#"{"n":42,"s":"hi"}"#;
        // When extracting Then the string key resolves, the number key misses.
        assert_eq!(find_str(json, "n"), None);
        assert_eq!(find_str(json, "s").as_deref(), Some("hi"));
    }

    #[test]
    fn unescapes_quotes_and_unicode() {
        // Given escaped content.
        let json = r#"{"p":"a\"b\\c ☃"}"#;
        // When extracting Then escapes resolve.
        assert_eq!(find_str(json, "p").as_deref(), Some("a\"b\\c ☃"));
    }

    #[test]
    fn missing_key_is_none() {
        assert_eq!(find_str(r#"{"a":"1"}"#, "zzz"), None);
    }

    #[test]
    fn key_inside_string_literal_is_skipped() {
        // Given a key-like blob inside a string value before the real key.
        let json = r#"{"note":"see \"tab_id\": \"fake\" here","tab_id":"real"}"#;
        // When extracting Then the real object key wins, not the fake.
        assert_eq!(find_str(json, "tab_id").as_deref(), Some("real"));
    }
}
