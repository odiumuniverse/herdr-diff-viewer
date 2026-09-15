use std::collections::BTreeMap;
use std::path::PathBuf;

use rusqlite::{Connection, OpenFlags};

use super::{read_jsonl_since, root, Adapter, Edits, SessionRef};

pub struct Antigravity;

fn cli_root() -> PathBuf {
    root().join(".gemini/antigravity-cli")
}

fn transcript_for(id: &str) -> Option<PathBuf> {
    let p = cli_root()
        .join("brain")
        .join(id)
        .join(".system_generated/logs/transcript.jsonl");
    p.is_file().then_some(p)
}

fn parse_workspace_uri(uris: &str) -> Option<String> {
    for part in uris.split(['[', '"', ',', ']']) {
        let p = part.trim();
        if let Some(rest) = p.strip_prefix("file://") {
            if rest.starts_with('/') {
                return Some(rest.to_string());
            }
        }
    }
    None
}

fn summary_row(id: &str, pane_cwd: &str) -> Option<(String, Option<String>)> {
    let summaries = cli_root().join("conversation_summaries.db");
    let conn = Connection::open_with_flags(&summaries, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    let row: Option<(String, String)> = if !id.is_empty() {
        conn.query_row(
            "select conversation_id, workspace_uris from conversation_summaries
             where conversation_id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok()
    } else {
        conn.query_row(
            "select conversation_id, workspace_uris from conversation_summaries
             where workspace_uris like ?1 order by last_modified_time desc limit 1",
            [format!("%{pane_cwd}%")],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok()
    };
    let (id, uris) = row?;
    Some((id, parse_workspace_uri(&uris)))
}

fn unquote(v: &str) -> String {
    let t = v.trim();
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        return serde_json::from_str::<String>(t)
            .unwrap_or_else(|_| t.trim_matches('"').to_string());
    }
    t.to_string()
}

fn tool_target(name: &str, args: &serde_json::Value) -> Option<PathBuf> {
    if !matches!(
        name,
        "write_to_file" | "replace_file_content" | "multi_replace_file_content" | "edit_file"
    ) {
        return None;
    }
    for key in ["TargetFile", "AbsolutePath", "file_path", "path"] {
        if let Some(v) = args.get(key) {
            let s = match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            let s = unquote(&s);
            if s.starts_with('/') {
                return Some(PathBuf::from(s));
            }
        }
    }
    None
}

fn is_path_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | '+' | '@' | '~')
}

fn scan_abs_paths(text: &str, out: &mut Vec<PathBuf>) {
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '/' || (i > 0 && is_path_char(chars[i - 1])) {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && is_path_char(chars[i]) {
            i += 1;
        }
        let token: String = chars[start..i].iter().collect();
        let mut t = token.as_str();
        while t.starts_with("//") {
            t = &t[1..];
        }
        let t = t.trim_end_matches('.');
        if t.len() >= 4 && t.matches('/').count() >= 2 {
            out.push(PathBuf::from(t));
        }
    }
}

fn blob_edits(sess: &SessionRef, cursor: Option<&str>) -> Result<Edits, String> {
    let min_idx = cursor
        .and_then(|c| serde_json::from_str::<serde_json::Value>(c).ok())
        .and_then(|v| v.get("idx").and_then(|i| i.as_i64()))
        .unwrap_or(-1);
    let mut paths = Vec::new();
    let mut max_idx = min_idx;
    for src in &sess.sources {
        let Ok(conn) = Connection::open_with_flags(src, OpenFlags::SQLITE_OPEN_READ_ONLY) else {
            continue;
        };
        let Ok(mut st) = conn.prepare(
            "select idx, step_payload, metadata from steps where idx > ?1 order by idx asc",
        ) else {
            continue;
        };
        let Ok(rows) = st.query_map([min_idx], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<Vec<u8>>>(1)?,
                r.get::<_, Option<Vec<u8>>>(2)?,
            ))
        }) else {
            continue;
        };
        for row in rows.flatten() {
            let (idx, payload, meta) = row;
            max_idx = max_idx.max(idx);
            for blob in [payload, meta].into_iter().flatten() {
                let text = String::from_utf8_lossy(&blob);
                scan_abs_paths(&text, &mut paths);
            }
        }
    }
    paths.sort();
    paths.dedup();
    Ok(Edits {
        paths,
        cursor: serde_json::json!({ "idx": max_idx }).to_string(),
    })
}

impl Adapter for Antigravity {
    fn resolve(&self, pane_cwd: &str, session_id: &str) -> Option<SessionRef> {
        let (id, cwd) = match summary_row(session_id, pane_cwd) {
            Some((id, cwd)) => (id, cwd),
            None if !session_id.is_empty() => (session_id.to_string(), None),
            None => return None,
        };
        if let Some(t) = transcript_for(&id) {
            return Some(SessionRef {
                id,
                cwd,
                sources: vec![t],
                gated: false,
            });
        }
        let db = cli_root().join("conversations").join(format!("{id}.db"));
        if db.is_file() {
            return Some(SessionRef {
                id,
                cwd,
                sources: vec![db],
                gated: true,
            });
        }
        None
    }

    fn edits(&self, sess: &SessionRef, cursor: Option<&str>) -> Result<Edits, String> {
        if sess.gated {
            return blob_edits(sess, cursor);
        }
        let mut offsets: BTreeMap<String, u64> = cursor
            .and_then(|c| serde_json::from_str(c).ok())
            .unwrap_or_default();
        let mut paths = Vec::new();
        for src in &sess.sources {
            let key = src.to_string_lossy().into_owned();
            let off = *offsets.get(&key).unwrap_or(&0);
            let Ok((vals, noff)) = read_jsonl_since(src, off) else {
                continue;
            };
            offsets.insert(key, noff);
            for v in &vals {
                let Some(calls) = v.get("tool_calls").and_then(|c| c.as_array()) else {
                    continue;
                };
                for call in calls {
                    let name = call.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    let Some(args) = call.get("args") else {
                        continue;
                    };
                    if let Some(p) = tool_target(name, args) {
                        paths.push(p);
                    }
                }
            }
        }
        Ok(Edits {
            paths,
            cursor: serde_json::to_string(&offsets).unwrap_or_default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_write_to_file_is_exact() {
        let dir = std::env::temp_dir().join(format!("dv-ag-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let logs = dir.join(".gemini/antigravity-cli/brain/conv-1/.system_generated/logs");
        std::fs::create_dir_all(&logs).unwrap();
        let lines = [
            r#"{"step_index":0,"type":"USER_INPUT","content":"hi"}"#.to_string(),
            serde_json::json!({"step_index":1,"type":"PLANNER_RESPONSE","tool_calls":[{"name":"write_to_file","args":{"TargetFile":"\"/tmp/proj/src/a.rs\"","CodeContent":"x"}},{"name":"view_file","args":{"AbsolutePath":"\"/tmp/proj/read.rs\""}}]}).to_string(),
        ];
        std::fs::write(logs.join("transcript.jsonl"), lines.join("\n") + "\n").unwrap();
        let _guard = crate::state::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("DIFF_JOURNAL_ROOT", &dir);
        let adapter = Antigravity;
        let sess = adapter.resolve("/tmp/proj", "conv-1").expect("resolve");
        assert!(!sess.gated, "transcript source is exact");
        let edits = adapter.edits(&sess, None).expect("edits");
        assert_eq!(edits.paths, vec![PathBuf::from("/tmp/proj/src/a.rs")]);
        let again = adapter
            .edits(&sess, Some(&edits.cursor))
            .expect("idempotent");
        assert!(again.paths.is_empty());
        std::env::remove_var("DIFF_JOURNAL_ROOT");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn blob_fallback_is_gated() {
        let dir = std::env::temp_dir().join(format!("dv-agb-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let conv = dir.join(".gemini/antigravity-cli/conversations");
        std::fs::create_dir_all(&conv).unwrap();
        let db = conv.join("conv-2.db");
        {
            let conn = Connection::open(&db).unwrap();
            conn.execute_batch(
                "create table steps (idx integer primary key, step_payload blob, metadata blob);",
            )
            .unwrap();
            conn.execute(
                "insert into steps values (1, ?1, null)",
                [&b"///tmp/proj/src/a.rs"[..]],
            )
            .unwrap();
        }
        let summaries = dir.join(".gemini/antigravity-cli/conversation_summaries.db");
        {
            let conn = Connection::open(&summaries).unwrap();
            conn.execute_batch(
                "create table conversation_summaries (conversation_id text, workspace_uris text, last_modified_time text);",
            )
            .unwrap();
            conn.execute(
                "insert into conversation_summaries values ('conv-2', '[\"file:///tmp/proj\"]', 'x')",
                [],
            )
            .unwrap();
        }
        let _guard = crate::state::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("DIFF_JOURNAL_ROOT", &dir);
        let adapter = Antigravity;
        let sess = adapter.resolve("/tmp/proj", "conv-2").expect("resolve");
        assert!(sess.gated, "blob source needs the signature gate");
        let edits = adapter.edits(&sess, None).expect("edits");
        assert_eq!(edits.paths, vec![PathBuf::from("/tmp/proj/src/a.rs")]);
        std::env::remove_var("DIFF_JOURNAL_ROOT");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
