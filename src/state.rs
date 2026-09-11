use std::fs;
use std::path::PathBuf;

use crate::ctx::find_str;

#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub struct ToggleState {
    pub viewer_pane: String,
    pub agent_pane: String,
    pub repo: String,
}

fn state_path(tab: &str) -> PathBuf {
    let safe: String = tab
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    std::env::temp_dir().join(format!("diff-viewer-{safe}.json"))
}

pub fn save(tab: &str, st: &ToggleState) -> Result<(), String> {
    let body = format!(
        "{{\"viewer_pane\":\"{}\",\"agent_pane\":\"{}\",\"repo\":\"{}\"}}",
        esc(&st.viewer_pane),
        esc(&st.agent_pane),
        esc(&st.repo)
    );
    fs::write(state_path(tab), body).map_err(|e| format!("write state: {e}"))
}

pub fn load(tab: &str) -> Option<ToggleState> {
    let body = fs::read_to_string(state_path(tab)).ok()?;
    Some(ToggleState {
        viewer_pane: find_str(&body, "viewer_pane")?,
        agent_pane: find_str(&body, "agent_pane")?,
        repo: find_str(&body, "repo")?,
    })
}

pub fn remove(tab: &str) {
    let _ = fs::remove_file(state_path(tab));
}

pub const MAX_TOUCHED: usize = 64;

fn touched_path(tab: &str) -> PathBuf {
    let safe: String = tab
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    if let Ok(dir) = std::env::var("HERDR_PLUGIN_STATE_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir).join(format!("touched-{safe}.json"));
        }
    }
    state_path(tab).with_extension("touched.json")
}

pub fn load_touched(tab: &str) -> Vec<String> {
    let Ok(body) = fs::read_to_string(touched_path(tab)) else {
        return Vec::new();
    };
    parse_str_array(&body)
}

pub fn save_touched(tab: &str, repos: &[String]) {
    let items: Vec<String> = repos
        .iter()
        .take(MAX_TOUCHED)
        .map(|r| format!("\"{}\"", esc(r)))
        .collect();
    let _ = fs::write(touched_path(tab), format!("[{}]", items.join(",")));
}

pub fn note_touched(tab: &str, path: &str) {
    if path.is_empty() {
        return;
    }
    let mut v = load_touched(tab);
    if v.last().is_some_and(|l| l == path) {
        return;
    }
    v.retain(|p| p != path);
    v.push(path.to_string());
    save_touched(tab, &v);
}

pub fn theme_file() -> PathBuf {
    if let Ok(dir) = std::env::var("DIFF_VIEWER_CONFIG_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir).join("theme");
        }
    }
    let herdr = std::env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".to_string());
    if let Ok(out) = std::process::Command::new(&herdr)
        .args(["plugin", "config-dir", crate::herdr_cli::PLUGIN_ID])
        .output()
    {
        if out.status.success() {
            let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !dir.is_empty() {
                return PathBuf::from(dir).join("theme");
            }
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home)
        .join(".config/herdr/plugins/config")
        .join(crate::herdr_cli::PLUGIN_ID)
        .join("theme")
}

pub fn save_theme(name: &str) -> Result<(), String> {
    let path = theme_file();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create config dir: {e}"))?;
    }
    fs::write(&path, name.trim()).map_err(|e| format!("write theme: {e}"))
}

pub fn load_theme() -> Option<String> {
    let name = fs::read_to_string(theme_file()).ok()?;
    let name = name.trim().to_string();
    (!name.is_empty()).then_some(name)
}

pub fn clear_theme() {
    let _ = fs::remove_file(theme_file());
}

fn parse_str_array(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_s = false;
    let mut esc = false;
    for c in body.chars() {
        if in_s {
            if esc {
                match c {
                    '"' => cur.push('"'),
                    '\\' => cur.push('\\'),
                    'n' => cur.push('\n'),
                    't' => cur.push('\t'),
                    'r' => cur.push('\r'),
                    _ => cur.push(c),
                }
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_s = false;
                out.push(std::mem::take(&mut cur));
            } else {
                cur.push(c);
            }
        } else if c == '"' {
            in_s = true;
        }
    }
    out
}

const LOCK_STALE: std::time::Duration = std::time::Duration::from_secs(5);

pub struct Lock {
    path: PathBuf,
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn lock_path(tab: &str) -> PathBuf {
    state_path(tab).with_extension("lock")
}

fn claim(path: PathBuf) -> Option<Lock> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .ok()
        .map(|_| Lock { path })
}

pub fn lock(tab: &str) -> Option<Lock> {
    let path = lock_path(tab);
    if let Some(l) = claim(path.clone()) {
        return Some(l);
    }
    let stale = fs::metadata(&path)
        .and_then(|m| m.modified())
        .map(|t| t.elapsed().is_ok_and(|age| age > LOCK_STALE))
        .unwrap_or(true);
    if !stale {
        return None;
    }
    let _ = fs::remove_file(&path);
    claim(path)
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let tab = "test_tab_roundtrip_xyz";
        let st = ToggleState {
            viewer_pane: "w1:p9".into(),
            agent_pane: "w1:p2".into(),
            repo: "/tmp/repo".into(),
        };
        save(tab, &st).unwrap();
        let back = load(tab).expect("state must load");
        assert_eq!(back.viewer_pane, "w1:p9");
        assert_eq!(back.agent_pane, "w1:p2");
        assert_eq!(back.repo, "/tmp/repo");
        remove(tab);
        assert!(load(tab).is_none());
    }

    #[test]
    fn lock_is_exclusive_until_dropped() {
        let tab = "test_tab_lock_xyz";
        let _ = fs::remove_file(lock_path(tab));
        let first = lock(tab).expect("first lock");
        assert!(lock(tab).is_none(), "second toggle must not open a viewer");
        drop(first);
        assert!(lock(tab).is_some(), "lock must release on drop");
        let _ = fs::remove_file(lock_path(tab));
    }

    #[test]
    fn touched_roundtrip_and_latest_wins() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tab = "test_tab_touched_xyz";
        let _ = fs::remove_file(touched_path(tab));
        assert!(load_touched(tab).is_empty());
        note_touched(tab, "/a/repo");
        note_touched(tab, "/b/repo");
        note_touched(tab, "/b/repo");
        note_touched(tab, "/a/repo");
        assert_eq!(load_touched(tab), vec!["/b/repo", "/a/repo"]);
        let _ = fs::remove_file(touched_path(tab));
    }

    #[test]
    fn str_array_parses_escapes() {
        assert_eq!(parse_str_array(r#"["a","b\"c"]"#), vec!["a", "b\"c"]);
        assert!(parse_str_array("garbage").is_empty());
    }
}
