use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::{absolutize, patch_paths, read_jsonl_since, root, Adapter, Edits, SessionRef};

pub struct Codex;

fn sessions_root() -> PathBuf {
    root().join(".codex/sessions")
}

fn collect_rollouts(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth == 0 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_rollouts(&p, depth - 1, out);
        } else if p.extension().is_some_and(|x| x == "jsonl") {
            out.push(p);
        }
    }
}

fn meta_cwd(file: &Path) -> Option<String> {
    let text = std::fs::read_to_string(file).ok()?;
    for line in text.lines().take(3) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) == Some("session_meta") {
            if let Some(c) = v.pointer("/payload/cwd").and_then(|c| c.as_str()) {
                return Some(c.to_string());
            }
        }
    }
    None
}

fn newest_first(files: &mut [PathBuf]) {
    files.sort_by_key(|f| std::fs::metadata(f).and_then(|m| m.modified()).ok());
    files.reverse();
}

fn patch_text(v: &serde_json::Value) -> Option<String> {
    if let Some(s) = v.pointer("/payload/input").and_then(|x| x.as_str()) {
        return Some(s.to_string());
    }
    let args = v.pointer("/payload/arguments").and_then(|x| x.as_str())?;
    if let Ok(o) = serde_json::from_str::<serde_json::Value>(args) {
        for key in ["input", "patch", "patchText"] {
            if let Some(t) = o.get(key).and_then(|x| x.as_str()) {
                return Some(t.to_string());
            }
        }
    }
    Some(args.to_string())
}

impl Adapter for Codex {
    fn resolve(&self, pane_cwd: &str, session_id: &str) -> Option<SessionRef> {
        let mut files = Vec::new();
        collect_rollouts(&sessions_root(), 4, &mut files);
        newest_first(&mut files);
        let mut picked = None;
        if !session_id.is_empty() {
            for f in &files {
                let name = f.file_name().map(|n| n.to_string_lossy().into_owned());
                if name.is_some_and(|n| n.contains(session_id)) {
                    picked = Some(f.clone());
                    break;
                }
            }
        }
        if picked.is_none() && !pane_cwd.is_empty() {
            for f in files.iter().take(200) {
                if meta_cwd(f).as_deref() == Some(pane_cwd) {
                    picked = Some(f.clone());
                    break;
                }
            }
        }
        let file = picked?;
        let cwd = meta_cwd(&file).or_else(|| Some(pane_cwd.to_string()));
        Some(SessionRef {
            id: session_id.to_string(),
            cwd,
            sources: vec![file],
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
                match v.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                    "session_meta" => {
                        if let Some(c) = v.pointer("/payload/cwd").and_then(|c| c.as_str()) {
                            base = Some(c.to_string());
                        }
                    }
                    "event_msg" => {
                        let ptype = v
                            .pointer("/payload/type")
                            .and_then(|t| t.as_str())
                            .unwrap_or("");
                        if ptype == "patch_apply_end" {
                            if let Some(obj) =
                                v.pointer("/payload/changes").and_then(|c| c.as_object())
                            {
                                for k in obj.keys() {
                                    paths.push(PathBuf::from(k));
                                }
                            }
                        }
                    }
                    "response_item" => {
                        let ptype = v
                            .pointer("/payload/type")
                            .and_then(|t| t.as_str())
                            .unwrap_or("");
                        let name = v
                            .pointer("/payload/name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("");
                        if matches!(ptype, "custom_tool_call" | "function_call")
                            && name == "apply_patch"
                        {
                            if let Some(text) = patch_text(v) {
                                paths.extend(patch_paths(&text));
                            }
                        }
                    }
                    _ => {}
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
    fn reads_patch_apply_and_custom_tool_call() {
        let dir = std::env::temp_dir().join(format!("dv-codex-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let day = dir.join(".codex/sessions/2026/09/15");
        std::fs::create_dir_all(&day).unwrap();
        let lines = [
            r#"{"timestamp":"t","type":"session_meta","payload":{"session_id":"sid-1","cwd":"/tmp/proj"}}"#.to_string(),
            serde_json::json!({"type":"event_msg","payload":{"type":"patch_apply_end","changes":{"/tmp/proj/src/a.rs":{"type":"update"}}}}).to_string(),
            serde_json::json!({"type":"response_item","payload":{"type":"custom_tool_call","name":"apply_patch","input":"*** Update File: src/b.rs\n*** Add File: /tmp/proj/src/c.rs\n"}}).to_string(),
        ];
        std::fs::write(
            day.join("rollout-2026-09-15T00-00-00-sid-1.jsonl"),
            lines.join("\n") + "\n",
        )
        .unwrap();
        let _guard = crate::state::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("DIFF_JOURNAL_ROOT", &dir);
        let adapter = Codex;
        let sess = adapter.resolve("/tmp/proj", "sid-1").expect("resolve");
        assert_eq!(sess.cwd.as_deref(), Some("/tmp/proj"));
        let edits = adapter.edits(&sess, None).expect("edits");
        assert_eq!(
            edits.paths,
            vec![
                PathBuf::from("/tmp/proj/src/a.rs"),
                PathBuf::from("/tmp/proj/src/b.rs"),
                PathBuf::from("/tmp/proj/src/c.rs"),
            ]
        );
        let again = adapter
            .edits(&sess, Some(&edits.cursor))
            .expect("idempotent");
        assert!(again.paths.is_empty());
        let by_cwd = adapter.resolve("/tmp/proj", "").expect("by cwd");
        assert_eq!(by_cwd.id, "");
        std::env::remove_var("DIFF_JOURNAL_ROOT");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
