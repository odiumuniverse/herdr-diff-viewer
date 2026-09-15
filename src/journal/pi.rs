use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::{absolutize, read_jsonl_since, root, Adapter, Edits, SessionRef};

pub struct Pi;
pub struct Omp;

fn sessions_root(kind: &str) -> PathBuf {
    if kind == "omp" {
        root().join(".omp/agent/sessions")
    } else {
        root().join(".pi/agent/sessions")
    }
}

fn collect_sessions(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth == 0 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_sessions(&p, depth - 1, out);
        } else if p.extension().is_some_and(|x| x == "jsonl") {
            out.push(p);
        }
    }
}

fn header_cwd(file: &Path) -> Option<String> {
    let text = std::fs::read_to_string(file).ok()?;
    for line in text.lines().take(2) {
        let parsed = serde_json::from_str::<serde_json::Value>(line)
            .ok()
            .or_else(|| {
                line.find('{')
                    .and_then(|i| serde_json::from_str(&line[i..]).ok())
            });
        if let Some(v) = parsed {
            if let Some(c) = v.get("cwd").and_then(|c| c.as_str()) {
                return Some(c.to_string());
            }
        }
    }
    None
}

fn resolve_root(kind: &str, pane_cwd: &str, session_id: &str) -> Option<SessionRef> {
    let mut files = Vec::new();
    collect_sessions(&sessions_root(kind), 3, &mut files);
    files.sort_by_key(|f| std::fs::metadata(f).and_then(|m| m.modified()).ok());
    files.reverse();
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
            if header_cwd(f).as_deref() == Some(pane_cwd) {
                picked = Some(f.clone());
                break;
            }
        }
    }
    let file = picked?;
    let cwd = header_cwd(&file).or_else(|| Some(pane_cwd.to_string()));
    Some(SessionRef {
        id: session_id.to_string(),
        cwd,
        sources: vec![file],
        gated: false,
    })
}

fn is_edit_tool(name: &str) -> bool {
    matches!(
        name,
        "edit" | "write" | "apply_patch" | "patch" | "multiedit" | "write_file"
    )
}

fn edits_root(sess: &SessionRef, cursor: Option<&str>) -> Result<Edits, String> {
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
            let Some(content) = v.pointer("/message/content").and_then(|c| c.as_array()) else {
                continue;
            };
            for block in content {
                if block.get("type").and_then(|t| t.as_str()) != Some("toolCall") {
                    continue;
                }
                let name = block.get("name").and_then(|n| n.as_str()).unwrap_or("");
                if !is_edit_tool(name) {
                    continue;
                }
                let args = block.get("arguments");
                for ptr in ["/path", "/file_path", "/absolute_path"] {
                    if let Some(p) = args.and_then(|a| a.pointer(ptr)).and_then(|p| p.as_str()) {
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

impl Adapter for Pi {
    fn resolve(&self, pane_cwd: &str, session_id: &str) -> Option<SessionRef> {
        resolve_root("pi", pane_cwd, session_id)
    }

    fn edits(&self, sess: &SessionRef, cursor: Option<&str>) -> Result<Edits, String> {
        edits_root(sess, cursor)
    }
}

impl Adapter for Omp {
    fn resolve(&self, pane_cwd: &str, session_id: &str) -> Option<SessionRef> {
        resolve_root("omp", pane_cwd, session_id)
    }

    fn edits(&self, sess: &SessionRef, cursor: Option<&str>) -> Result<Edits, String> {
        edits_root(sess, cursor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pi_reads_tool_call_paths() {
        let dir = std::env::temp_dir().join(format!("dv-pi-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let bucket = dir.join(".pi/agent/sessions/--tmp-proj--");
        std::fs::create_dir_all(&bucket).unwrap();
        let lines = [
            r#"{"type":"session","version":3,"id":"uuid-1","cwd":"/tmp/proj"}"#.to_string(),
            serde_json::json!({"type":"message","message":{"role":"assistant","content":[{"type":"toolCall","name":"edit","arguments":{"path":"src/a.rs"}},{"type":"toolCall","name":"read","arguments":{"path":"/tmp/proj/read.rs"}}]}}).to_string(),
        ];
        std::fs::write(
            bucket.join("1700000000_uuid-1.jsonl"),
            lines.join("\n") + "\n",
        )
        .unwrap();
        let _guard = crate::state::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("DIFF_JOURNAL_ROOT", &dir);
        let adapter = Pi;
        let sess = adapter.resolve("/tmp/proj", "uuid-1").expect("resolve");
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
