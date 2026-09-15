use std::collections::BTreeMap;
use std::path::PathBuf;

use rusqlite::{Connection, OpenFlags};

use super::{read_jsonl_since, root, Adapter, Edits, SessionRef};

pub struct Cursor;

fn projects_dir() -> PathBuf {
    root().join(".cursor/projects")
}

fn ai_db() -> PathBuf {
    root().join(".cursor/ai-tracking/ai-code-tracking.db")
}

fn find_transcript(session_id: &str) -> Option<PathBuf> {
    if session_id.is_empty() {
        return None;
    }
    let rd = std::fs::read_dir(projects_dir()).ok()?;
    for e in rd.flatten() {
        let f = e
            .path()
            .join("agent-transcripts")
            .join(format!("{session_id}.jsonl"));
        if f.is_file() {
            return Some(f);
        }
    }
    None
}

fn collect_paths(v: &serde_json::Value, out: &mut Vec<PathBuf>) {
    match v {
        serde_json::Value::Object(map) => {
            for (k, val) in map {
                if matches!(
                    k.as_str(),
                    "filePath" | "file_path" | "path" | "absolutePath"
                ) {
                    if let Some(s) = val.as_str() {
                        if s.starts_with('/') {
                            out.push(PathBuf::from(s));
                        }
                    }
                } else {
                    collect_paths(val, out);
                }
            }
        }
        serde_json::Value::Array(a) => {
            for x in a {
                collect_paths(x, out);
            }
        }
        _ => {}
    }
}

impl Adapter for Cursor {
    fn resolve(&self, _pane_cwd: &str, session_id: &str) -> Option<SessionRef> {
        let file = find_transcript(session_id)?;
        Some(SessionRef {
            id: session_id.to_string(),
            cwd: None,
            sources: vec![file],
            gated: true,
        })
    }

    fn edits(&self, sess: &SessionRef, cursor: Option<&str>) -> Result<Edits, String> {
        let mut cur = cursor
            .and_then(|c| serde_json::from_str::<serde_json::Value>(c).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        let mut paths: Vec<PathBuf> = Vec::new();

        if !sess.id.is_empty() {
            let db_ts = cur.get("db_ts").and_then(|v| v.as_i64()).unwrap_or(0);
            let mut next_ts = db_ts;
            if let Ok(conn) = Connection::open_with_flags(ai_db(), OpenFlags::SQLITE_OPEN_READ_ONLY)
            {
                if let Ok(mut st) = conn.prepare(
                    "select fileName, timestamp from ai_code_hashes
                     where conversationId = ?1 and timestamp > ?2 order by timestamp",
                ) {
                    if let Ok(rows) = st.query_map(rusqlite::params![sess.id, db_ts], |r| {
                        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
                    }) {
                        for row in rows.flatten() {
                            paths.push(PathBuf::from(row.0));
                            next_ts = next_ts.max(row.1);
                        }
                    }
                }
            }
            cur["db_ts"] = serde_json::json!(next_ts);
        }

        let mut offsets: BTreeMap<String, u64> = cur
            .get("off")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        for src in &sess.sources {
            let key = src.to_string_lossy().into_owned();
            let off = *offsets.get(&key).unwrap_or(&0);
            let Ok((vals, noff)) = read_jsonl_since(src, off) else {
                continue;
            };
            offsets.insert(key, noff);
            for v in &vals {
                collect_paths(v, &mut paths);
            }
        }
        cur["off"] = serde_json::to_value(&offsets).unwrap_or_else(|_| serde_json::json!({}));
        Ok(Edits {
            paths,
            cursor: cur.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_paths_from_transcript_and_is_idempotent() {
        let dir = std::env::temp_dir().join(format!("dv-cursor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let tdir = dir.join(".cursor/projects/Users-u/agent-transcripts");
        std::fs::create_dir_all(&tdir).unwrap();
        let lines = [
            r#"{"role":"user","message":{"content":[{"type":"text","text":"hi"}]}}"#,
            r#"{"role":"assistant","message":{"content":[{"type":"tool_use","input":{"filePath":"/tmp/proj/src/a.rs"}},{"type":"tool_use","input":{"path":"relative/b.rs"}}]}}"#,
        ];
        std::fs::write(tdir.join("conv-1.jsonl"), lines.join("\n") + "\n").unwrap();
        let _guard = crate::state::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("DIFF_JOURNAL_ROOT", &dir);
        let adapter = Cursor;
        let sess = adapter.resolve("/tmp/proj", "conv-1").expect("resolve");
        assert!(sess.gated);
        let edits = adapter.edits(&sess, None).expect("edits");
        assert_eq!(edits.paths, vec![PathBuf::from("/tmp/proj/src/a.rs")]);
        let again = adapter
            .edits(&sess, Some(&edits.cursor))
            .expect("idempotent");
        assert!(again.paths.is_empty());
        std::env::remove_var("DIFF_JOURNAL_ROOT");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
