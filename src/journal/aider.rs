use std::path::PathBuf;

use super::{absolutize, Adapter, Edits, SessionRef};

pub struct Aider;

fn history_for(pane_cwd: &str) -> Option<PathBuf> {
    let mut dir = PathBuf::from(pane_cwd);
    for _ in 0..4 {
        let f = dir.join(".aider.chat.history.md");
        if f.is_file() {
            return Some(f);
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

fn scan(text: &str, out: &mut Vec<PathBuf>) {
    for line in text.lines() {
        let t = line.trim_start().trim_start_matches('>').trim_start();
        if t.starts_with("Did not apply") || t.starts_with("Skipped edits") {
            continue;
        }
        for prefix in ["Applied edit to ", "Creating empty file "] {
            if let Some(p) = t.strip_prefix(prefix) {
                let p = p.trim();
                if !p.is_empty() && !p.contains(' ') {
                    out.push(PathBuf::from(p));
                }
            }
        }
    }
}

impl Adapter for Aider {
    fn resolve(&self, pane_cwd: &str, session_id: &str) -> Option<SessionRef> {
        let file = history_for(pane_cwd)?;
        let cwd = file
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| pane_cwd.to_string());
        Some(SessionRef {
            id: session_id.to_string(),
            cwd: Some(cwd),
            sources: vec![file],
            gated: true,
        })
    }

    fn edits(&self, sess: &SessionRef, cursor: Option<&str>) -> Result<Edits, String> {
        let src = sess
            .sources
            .first()
            .ok_or_else(|| "no source".to_string())?;
        let off = cursor
            .and_then(|c| serde_json::from_str::<serde_json::Value>(c).ok())
            .and_then(|v| v.get("off").and_then(|o| o.as_u64()))
            .unwrap_or(0);
        let text = std::fs::read_to_string(src).unwrap_or_default();
        let len = text.len() as u64;
        let start = if off <= len { off as usize } else { 0 };
        let start = if text.is_char_boundary(start) {
            start
        } else {
            0
        };
        let mut paths = Vec::new();
        scan(&text[start..], &mut paths);
        Ok(Edits {
            paths: absolutize(paths, sess.cwd.as_deref()),
            candidates: Vec::new(),
            cursor: serde_json::json!({ "off": len }).to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history_path(dir: &std::path::Path) -> PathBuf {
        let f = dir.join(".aider.chat.history.md");
        std::fs::write(
            &f,
            "# aider chat started at 2026-09-15 10:00:00\n\
             > Applied edit to src/a.rs\n\
             > Creating empty file src/new.rs\n\
             > Did not apply edit to src/skip.rs\n\
             > Skipped edits to 2 files\n",
        )
        .unwrap();
        f
    }

    #[test]
    fn finds_history_upwards_and_parses_lines() {
        let dir = std::env::temp_dir().join(format!("dv-aider-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let repo = dir.join("repo");
        let sub = repo.join("nested/deep");
        std::fs::create_dir_all(&sub).unwrap();
        history_path(&repo);
        let adapter = Aider;
        let sess = adapter
            .resolve(&sub.to_string_lossy(), "")
            .expect("resolve");
        assert!(sess.gated);
        let edits = adapter.edits(&sess, None).expect("edits");
        let expected: Vec<PathBuf> = ["src/a.rs", "src/new.rs"]
            .iter()
            .map(|p| repo.join(p))
            .collect();
        assert_eq!(edits.paths, expected);
        let again = adapter
            .edits(&sess, Some(&edits.cursor))
            .expect("idempotent");
        assert!(again.paths.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
