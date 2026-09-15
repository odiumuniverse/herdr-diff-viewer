use std::path::{Path, PathBuf};

use super::{absolutize, root, Adapter, Edits, SessionRef};

pub struct Gemini;

fn projects_dir() -> PathBuf {
    root().join(".gemini/tmp")
}

fn project_cwd(dir: &Path) -> Option<String> {
    let s = std::fs::read_to_string(dir.join(".project_root")).ok()?;
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn is_session_file(name: &str) -> bool {
    name.starts_with("session-") && (name.ends_with(".json") || name.ends_with(".jsonl"))
}

fn newest_session(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !is_session_file(&name) {
            continue;
        }
        let Some(mt) = e.metadata().ok().and_then(|m| m.modified().ok()) else {
            continue;
        };
        if best.as_ref().is_none_or(|(bm, _)| mt > *bm) {
            best = Some((mt, e.path()));
        }
    }
    best.map(|(_, p)| p)
}

fn read_messages(path: &Path) -> Option<(Vec<serde_json::Value>, String)> {
    let text = std::fs::read_to_string(path).ok()?;
    if path.extension().is_some_and(|x| x == "jsonl") {
        let mut session_id = String::new();
        let mut messages: Vec<serde_json::Value> = Vec::new();
        for line in text.lines() {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if let Some(s) = v.get("sessionId").and_then(|s| s.as_str()) {
                session_id = s.to_string();
            }
            if let Some(arr) = v.pointer("/$set/messages").and_then(|m| m.as_array()) {
                messages = arr.clone();
            }
        }
        return Some((messages, session_id));
    }
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let session_id = v
        .get("sessionId")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    let messages = v.get("messages")?.as_array()?.clone();
    Some((messages, session_id))
}

impl Adapter for Gemini {
    fn resolve(&self, pane_cwd: &str, session_id: &str) -> Option<SessionRef> {
        let base = projects_dir();
        let mut found: Option<(PathBuf, String)> = None;
        for e in std::fs::read_dir(&base).ok()?.flatten() {
            let dir = e.path();
            if !dir.is_dir() {
                continue;
            }
            let pcwd = project_cwd(&dir);
            let chats = dir.join("chats");
            if !session_id.is_empty() {
                let prefix: String = session_id.chars().take(8).collect();
                if let Ok(rd) = std::fs::read_dir(&chats) {
                    for f in rd.flatten() {
                        let name = f.file_name().to_string_lossy().into_owned();
                        if is_session_file(&name) && name.contains(&prefix) {
                            found = Some((f.path(), pcwd.clone().unwrap_or_default()));
                            break;
                        }
                    }
                }
            }
            if found.is_none() && !pane_cwd.is_empty() && pcwd.as_deref() == Some(pane_cwd) {
                if let Some(f) = newest_session(&chats) {
                    found = Some((f, pane_cwd.to_string()));
                }
            }
            if found.is_some() {
                break;
            }
        }
        let (file, cwd) = found?;
        Some(SessionRef {
            id: session_id.to_string(),
            cwd: Some(cwd),
            sources: vec![file],
            gated: false,
        })
    }

    fn edits(&self, sess: &SessionRef, cursor: Option<&str>) -> Result<Edits, String> {
        let src = sess
            .sources
            .first()
            .ok_or_else(|| "no source".to_string())?;
        let len = std::fs::metadata(src).map_err(|e| e.to_string())?.len();
        let mut cur = cursor
            .and_then(|c| serde_json::from_str::<serde_json::Value>(c).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        let prev_len = cur.get("len").and_then(|v| v.as_u64());
        let ts0 = cur.get("ts").and_then(|v| v.as_i64()).unwrap_or(0);
        if prev_len == Some(len) {
            return Ok(Edits {
                paths: Vec::new(),
                cursor: cur.to_string(),
            });
        }
        let Some((messages, _)) = read_messages(src) else {
            return Err("unreadable session".to_string());
        };
        let mut paths = Vec::new();
        let mut ts = ts0;
        for m in &messages {
            let mts = m.get("timestamp").and_then(|t| t.as_i64()).unwrap_or(0);
            if mts <= ts0 {
                continue;
            }
            ts = ts.max(mts);
            let Some(calls) = m.get("toolCalls").and_then(|t| t.as_array()) else {
                continue;
            };
            for tc in calls {
                let name = tc.get("name").and_then(|n| n.as_str()).unwrap_or("");
                if !matches!(name, "write_file" | "replace") {
                    continue;
                }
                if let Some(p) = tc.pointer("/args/file_path").and_then(|p| p.as_str()) {
                    paths.push(PathBuf::from(p));
                }
            }
        }
        cur = serde_json::json!({ "len": len, "ts": ts });
        Ok(Edits {
            paths: absolutize(paths, sess.cwd.as_deref()),
            cursor: cur.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(dir: &Path) -> PathBuf {
        let proj = dir.join(".gemini/tmp/proj");
        std::fs::create_dir_all(proj.join("chats")).unwrap();
        std::fs::write(proj.join(".project_root"), "/tmp/proj\n").unwrap();
        let json = serde_json::json!({
            "sessionId": "abcdef12-0000-0000-0000-000000000000",
            "messages": [
                {"timestamp": 10, "toolCalls": [{"name": "write_file", "args": {"file_path": "src/a.rs"}}]},
                {"timestamp": 11, "toolCalls": [{"name": "read_file", "args": {"file_path": "/tmp/proj/read.rs"}}]}
            ]
        });
        std::fs::write(proj.join("chats/session-x-abcdef12.json"), json.to_string()).unwrap();
        let jsonl = [
            r#"{"sessionId":"11111111-0000-0000-0000-000000000000","kind":"main"}"#.to_string(),
            serde_json::json!({"$set":{"messages":[{"timestamp":20,"toolCalls":[{"name":"replace","args":{"file_path":"/tmp/proj/src/b.rs"}}]}]}}).to_string(),
        ];
        std::fs::write(
            proj.join("chats/session-y-11111111.jsonl"),
            jsonl.join("\n") + "\n",
        )
        .unwrap();
        proj
    }

    #[test]
    fn resolves_json_and_jsonl_sessions() {
        let dir = std::env::temp_dir().join(format!("dv-gem-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let proj = setup(&dir);
        let _guard = crate::state::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("DIFF_JOURNAL_ROOT", &dir);
        let adapter = Gemini;

        let s1 = adapter
            .resolve("/tmp/proj", "abcdef12-0000-0000-0000-000000000000")
            .expect("json session");
        let e1 = adapter.edits(&s1, None).expect("json edits");
        assert_eq!(e1.paths, vec![PathBuf::from("/tmp/proj/src/a.rs")]);

        let s2 = adapter
            .resolve("/tmp/proj", "11111111-0000-0000-0000-000000000000")
            .expect("jsonl session");
        let e2 = adapter.edits(&s2, None).expect("jsonl edits");
        assert_eq!(e2.paths, vec![PathBuf::from("/tmp/proj/src/b.rs")]);
        let e2b = adapter.edits(&s2, Some(&e2.cursor)).expect("idempotent");
        assert!(e2b.paths.is_empty());

        let by_cwd = adapter.resolve("/tmp/proj", "").expect("by cwd");
        assert!(by_cwd.sources[0].starts_with(&proj));

        std::env::remove_var("DIFF_JOURNAL_ROOT");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
