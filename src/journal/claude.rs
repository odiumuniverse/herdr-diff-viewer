use std::collections::BTreeMap;
use std::path::PathBuf;

use super::{absolutize, newest_file, read_jsonl_since, root, Adapter, Edits, SessionRef};

pub struct Claude;

fn slug(cwd: &str) -> String {
    cwd.replace('/', "-")
}

impl Adapter for Claude {
    fn resolve(&self, pane_cwd: &str, session_id: &str) -> Option<SessionRef> {
        let projects = root().join(".claude/projects");
        let mut sources = Vec::new();
        if !session_id.is_empty() {
            if let Ok(rd) = std::fs::read_dir(&projects) {
                for e in rd.flatten() {
                    let dir = e.path();
                    let main = dir.join(format!("{session_id}.jsonl"));
                    if main.is_file() {
                        sources.push(main);
                    }
                    let sub = dir.join(session_id).join("subagents");
                    if let Ok(rd2) = std::fs::read_dir(&sub) {
                        for e2 in rd2.flatten() {
                            let p = e2.path();
                            if p.extension().is_some_and(|x| x == "jsonl") {
                                sources.push(p);
                            }
                        }
                    }
                }
            }
        }
        if sources.is_empty() && !pane_cwd.is_empty() {
            let dir = projects.join(slug(pane_cwd));
            if let Some(p) = newest_file(&dir, ".jsonl") {
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
        for src in &sess.sources {
            let key = src.to_string_lossy().into_owned();
            let off = *offsets.get(&key).unwrap_or(&0);
            let Ok((vals, noff)) = read_jsonl_since(src, off) else {
                continue;
            };
            offsets.insert(key, noff);
            for v in &vals {
                let Some(parts) = v.pointer("/message/content").and_then(|c| c.as_array()) else {
                    continue;
                };
                for part in parts {
                    if part.get("type").and_then(|t| t.as_str()) != Some("tool_use") {
                        continue;
                    }
                    let name = part.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    let ptr = match name {
                        "Edit" | "Write" | "MultiEdit" => "/input/file_path",
                        "NotebookEdit" => "/input/notebook_path",
                        _ => continue,
                    };
                    if let Some(p) = part.pointer(ptr).and_then(|p| p.as_str()) {
                        paths.push(PathBuf::from(p));
                    }
                }
            }
        }
        Ok(Edits {
            paths: absolutize(paths, sess.cwd.as_deref()),
            cursor: serde_json::to_string(&offsets).unwrap_or_default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_edit_and_notebook_paths() {
        let dir = std::env::temp_dir().join(format!("dv-claude-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let proj = dir.join(".claude/projects/-tmp-proj");
        std::fs::create_dir_all(&proj).unwrap();
        let lines = [
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Edit","input":{"file_path":"/tmp/proj/a.rs"}}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"/tmp/proj/read.rs"}}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"NotebookEdit","input":{"notebook_path":"/tmp/proj/n.ipynb"}}]}}"#,
        ];
        std::fs::write(proj.join("sid1.jsonl"), lines.join("\n") + "\n").unwrap();
        let _guard = crate::state::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("DIFF_JOURNAL_ROOT", &dir);
        let adapter = Claude;
        let sess = adapter.resolve("/tmp/proj", "sid1").expect("resolve");
        let edits = adapter.edits(&sess, None).expect("edits");
        assert_eq!(
            edits.paths,
            vec![
                PathBuf::from("/tmp/proj/a.rs"),
                PathBuf::from("/tmp/proj/n.ipynb")
            ]
        );
        let again = adapter.edits(&sess, Some(&edits.cursor)).expect("edits");
        assert!(again.paths.is_empty(), "cursor must be idempotent");
        std::env::remove_var("DIFF_JOURNAL_ROOT");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
