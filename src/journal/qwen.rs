use std::collections::BTreeMap;
use std::path::PathBuf;

use super::{absolutize, newest_file, read_jsonl_since, root, Adapter, Edits, SessionRef};

pub struct Qwen;

fn slug(cwd: &str) -> String {
    cwd.replace('/', "-")
}

impl Adapter for Qwen {
    fn resolve(&self, pane_cwd: &str, session_id: &str) -> Option<SessionRef> {
        let projects = root().join(".qwen/projects");
        let mut sources = Vec::new();
        if !session_id.is_empty() {
            if let Ok(rd) = std::fs::read_dir(&projects) {
                for e in rd.flatten() {
                    let f = e.path().join("chats").join(format!("{session_id}.jsonl"));
                    if f.is_file() {
                        sources.push(f);
                    }
                }
            }
        }
        if sources.is_empty() && !pane_cwd.is_empty() {
            let chats = projects.join(slug(pane_cwd)).join("chats");
            if let Some(p) = newest_file(&chats, ".jsonl") {
                sources.push(p);
            }
        }
        if sources.is_empty() {
            return None;
        }
        Some(SessionRef {
            id: session_id.to_string(),
            cwd: Some(pane_cwd.to_string()),
            sources,
            gated: false,
        })
    }

    fn edits(&self, sess: &SessionRef, cursor: Option<&str>) -> Result<Edits, String> {
        let mut offsets: BTreeMap<String, u64> = cursor
            .and_then(|c| serde_json::from_str(c).ok())
            .unwrap_or_default();
        let mut paths = Vec::new();
        let mut base = sess.cwd.clone();
        for src in &sess.sources {
            let key = src.to_string_lossy().into_owned();
            let off = *offsets.get(&key).unwrap_or(&0);
            let Ok((vals, noff)) = read_jsonl_since(src, off) else {
                continue;
            };
            offsets.insert(key, noff);
            for v in &vals {
                if let Some(c) = v.get("cwd").and_then(|c| c.as_str()) {
                    base = Some(c.to_string());
                }
                let Some(parts) = v.pointer("/message/parts").and_then(|p| p.as_array()) else {
                    continue;
                };
                for part in parts {
                    let Some(fc) = part.get("functionCall") else {
                        continue;
                    };
                    let name = fc.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    if !matches!(
                        name,
                        "edit" | "write_file" | "write" | "replace" | "apply_patch" | "multiedit"
                    ) {
                        continue;
                    }
                    let args = fc.get("args");
                    for ptr in ["/file_path", "/path", "/absolute_path"] {
                        if let Some(p) = args.and_then(|a| a.pointer(ptr)).and_then(|p| p.as_str())
                        {
                            paths.push(PathBuf::from(p));
                            break;
                        }
                    }
                }
            }
        }
        Ok(Edits {
            paths: absolutize(paths, base.as_deref()),
            cursor: serde_json::to_string(&offsets).unwrap_or_default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_function_call_paths_relative_to_line_cwd() {
        let dir = std::env::temp_dir().join(format!("dv-qwen-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let chats = dir.join(".qwen/projects/-tmp-proj/chats");
        std::fs::create_dir_all(&chats).unwrap();
        let lines = [
            r#"{"type":"assistant","cwd":"/tmp/proj","message":{"role":"model","parts":[{"functionCall":{"name":"write_file","args":{"file_path":"src/a.rs"}}}]}}"#,
            r#"{"type":"assistant","cwd":"/tmp/proj","message":{"role":"model","parts":[{"functionCall":{"name":"grep_search","args":{"path":"/tmp/proj"}}}]}}"#,
        ];
        std::fs::write(chats.join("sid9.jsonl"), lines.join("\n") + "\n").unwrap();
        let _guard = crate::state::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("DIFF_JOURNAL_ROOT", &dir);
        let adapter = Qwen;
        let sess = adapter.resolve("/tmp/proj", "sid9").expect("resolve");
        let edits = adapter.edits(&sess, None).expect("edits");
        assert_eq!(edits.paths, vec![PathBuf::from("/tmp/proj/src/a.rs")]);
        std::env::remove_var("DIFF_JOURNAL_ROOT");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
