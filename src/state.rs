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

pub const MAX_SESSION_REPOS: usize = 64;
pub const MAX_SEEN: usize = 256;

pub struct PaneEntry {
    pub pane: String,
    pub tab: String,
    pub agent: String,
    pub session: String,
    pub repos: Vec<String>,
    pub seen: Vec<String>,
    pub updated: u64,
}

pub struct LivePane {
    pub pane: String,
    pub tab: String,
    pub agent: String,
    pub session: String,
}

fn sessions_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("HERDR_PLUGIN_STATE_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    std::env::temp_dir()
}

fn session_path(pane: &str) -> PathBuf {
    let safe: String = pane
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    sessions_dir().join(format!("session-{safe}.json"))
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn entry_body(e: &PaneEntry) -> String {
    let repos: Vec<String> = e.repos.iter().map(|r| format!("\"{}\"", esc(r))).collect();
    let seen: Vec<String> = e.seen.iter().map(|s| format!("\"{}\"", esc(s))).collect();
    format!(
        "{{\"pane\":\"{}\",\"tab\":\"{}\",\"agent\":\"{}\",\"session\":\"{}\",\"repos\":[{}],\"seen\":[{}],\"updated\":\"{}\"}}",
        esc(&e.pane),
        esc(&e.tab),
        esc(&e.agent),
        esc(&e.session),
        repos.join(","),
        seen.join(","),
        e.updated
    )
}

fn entry_from_body(body: &str) -> Option<PaneEntry> {
    Some(PaneEntry {
        pane: find_str(body, "pane")?,
        tab: find_str(body, "tab").unwrap_or_default(),
        agent: find_str(body, "agent").unwrap_or_default(),
        session: find_str(body, "session").unwrap_or_default(),
        repos: str_array(body, "repos"),
        seen: str_array(body, "seen"),
        updated: find_str(body, "updated")
            .and_then(|u| u.parse().ok())
            .unwrap_or(0),
    })
}

fn str_array(body: &str, key: &str) -> Vec<String> {
    let pat = format!("\"{key}\"");
    let Some(i) = body.find(&pat) else {
        return Vec::new();
    };
    let rest = &body[i + pat.len()..];
    let Some(b) = rest.find('[') else {
        return Vec::new();
    };
    let Some(e) = rest[b..].find(']') else {
        return Vec::new();
    };
    parse_str_array(&rest[b..b + e + 1])
}

pub fn load_entry(pane: &str) -> Option<PaneEntry> {
    let body = fs::read_to_string(session_path(pane)).ok()?;
    entry_from_body(&body)
}

pub fn save_entry(e: &PaneEntry) {
    let path = session_path(&e.pane);
    let tmp = path.with_extension("tmp");
    if fs::write(&tmp, entry_body(e)).is_ok() {
        let _ = fs::rename(&tmp, &path);
    }
}

pub fn remove_entry(pane: &str) {
    let _ = fs::remove_file(session_path(pane));
}

pub fn remove_tab(tab: &str) {
    for e in list_entries() {
        if e.tab == tab {
            remove_entry(&e.pane);
        }
    }
}

pub fn list_entries() -> Vec<PaneEntry> {
    let Ok(rd) = fs::read_dir(sessions_dir()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.starts_with("session-") || !name.ends_with(".json") {
            continue;
        }
        if let Ok(body) = fs::read_to_string(e.path()) {
            if let Some(en) = entry_from_body(&body) {
                out.push(en);
            }
        }
    }
    out
}

fn legacy_cleanup() {
    let Ok(rd) = fs::read_dir(sessions_dir()) else {
        return;
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with("touched-") && name.ends_with(".json") {
            let _ = fs::remove_file(e.path());
        }
    }
}

pub fn observe(live: &LivePane, procs: &[(String, String)]) -> PaneEntry {
    let Some(e) = load_entry(&live.pane) else {
        let mut e = PaneEntry {
            pane: live.pane.clone(),
            tab: live.tab.clone(),
            agent: live.agent.clone(),
            session: live.session.clone(),
            repos: Vec::new(),
            seen: Vec::new(),
            updated: now_secs(),
        };
        baseline(&mut e, procs);
        save_entry(&e);
        return e;
    };
    let (mut e, mut changed) = reconcile(e, live);
    changed |= admit(&mut e, procs);
    if changed {
        e.updated = now_secs();
        save_entry(&e);
    }
    e
}

fn reconcile(mut e: PaneEntry, live: &LivePane) -> (PaneEntry, bool) {
    let mut changed = false;
    if e.session != live.session {
        e.session = live.session.clone();
        e.repos.clear();
        changed = true;
    }
    if e.agent.is_empty() {
        e.agent = live.agent.clone();
        changed = true;
    } else if e.agent != live.agent {
        e.session = live.session.clone();
        e.agent = live.agent.clone();
        e.repos.clear();
        changed = true;
    }
    if e.tab != live.tab {
        e.tab = live.tab.clone();
        changed = true;
    }
    (e, changed)
}

fn baseline(e: &mut PaneEntry, procs: &[(String, String)]) {
    for (pid, cwd) in procs {
        if cwd.is_empty() {
            continue;
        }
        let key = format!("{pid}\u{1f}{cwd}");
        if !e.seen.contains(&key) {
            e.seen.push(key);
        }
    }
    while e.seen.len() > MAX_SEEN {
        e.seen.remove(0);
    }
}

fn admit(e: &mut PaneEntry, procs: &[(String, String)]) -> bool {
    let mut changed = false;
    for (pid, cwd) in procs {
        if cwd.is_empty() {
            continue;
        }
        let key = format!("{pid}\u{1f}{cwd}");
        if e.seen.contains(&key) {
            continue;
        }
        e.seen.push(key);
        changed = true;
        if let Ok(top) = crate::git::toplevel(cwd) {
            if !e.repos.contains(&top) {
                e.repos.push(top);
            }
        }
    }
    while e.seen.len() > MAX_SEEN {
        e.seen.remove(0);
    }
    while e.repos.len() > MAX_SESSION_REPOS {
        e.repos.remove(0);
    }
    changed
}

pub fn prune(live: &[LivePane]) {
    legacy_cleanup();
    for e in list_entries() {
        match live.iter().find(|l| l.pane == e.pane) {
            None => remove_entry(&e.pane),
            Some(l) => {
                if l.agent.is_empty() {
                    remove_entry(&e.pane);
                } else {
                    let (fresh, changed) = reconcile(e, l);
                    if changed {
                        save_entry(&fresh);
                    }
                }
            }
        }
    }
}

pub fn session_repos(agent: &str, live: &[LivePane]) -> Vec<String> {
    let Some(l) = live.iter().find(|p| p.pane == agent && !p.agent.is_empty()) else {
        return Vec::new();
    };
    match load_entry(&l.pane) {
        Some(e) if e.session == l.session && (e.agent.is_empty() || e.agent == l.agent) => e.repos,
        _ => Vec::new(),
    }
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
    let chars: Vec<char> = body.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if in_s {
            if esc {
                match c {
                    '"' => cur.push('"'),
                    '\\' => cur.push('\\'),
                    'n' => cur.push('\n'),
                    't' => cur.push('\t'),
                    'r' => cur.push('\r'),
                    'u' => {
                        let hex: String = chars[i + 1..(i + 5).min(chars.len())].iter().collect();
                        if hex.len() == 4 {
                            if let Ok(code) = u32::from_str_radix(&hex, 16) {
                                if let Some(ch) = char::from_u32(code) {
                                    cur.push(ch);
                                    i += 4;
                                } else {
                                    cur.push('u');
                                }
                            } else {
                                cur.push('u');
                            }
                        } else {
                            cur.push('u');
                        }
                    }
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
        i += 1;
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
        .replace('\u{1f}', "\\u001f")
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

    fn live(pane: &str, tab: &str, agent: &str, session: &str) -> LivePane {
        LivePane {
            pane: pane.into(),
            tab: tab.into(),
            agent: agent.into(),
            session: session.into(),
        }
    }

    fn with_state_dir(f: impl FnOnce()) {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("dv-sess-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        std::env::set_var("HERDR_PLUGIN_STATE_DIR", &dir);
        f();
        std::env::remove_var("HERDR_PLUGIN_STATE_DIR");
        let _ = fs::remove_dir_all(&dir);
    }

    fn init_repo(path: &std::path::Path) -> String {
        let s = path.to_string_lossy().into_owned();
        let ok = std::process::Command::new("git")
            .args(["init", "-q", &s])
            .status()
            .is_ok_and(|st| st.success());
        assert!(ok, "git init failed for {s}");
        crate::git::canonical(&s)
    }

    #[test]
    fn session_observe_admits_only_new_processes() {
        with_state_dir(|| {
            let base = std::env::temp_dir().join(format!("dv-admit-{}", std::process::id()));
            let _ = fs::remove_dir_all(&base);
            fs::create_dir_all(&base).unwrap();
            let top = init_repo(&base);
            let l = live("w1:p1", "w1:t1", "opencode", "ses_a");
            let e = observe(&l, &[("100".into(), base.to_string_lossy().into_owned())]);
            assert!(e.repos.is_empty());
            let e = observe(&l, &[("101".into(), base.to_string_lossy().into_owned())]);
            assert_eq!(e.repos, vec![top.clone()]);
            let e = observe(&l, &[("101".into(), base.to_string_lossy().into_owned())]);
            assert_eq!(e.repos, vec![top]);
            let e = observe(&l, &[("102".into(), "/tmp".into())]);
            assert_eq!(e.repos.len(), 1);
            assert!(e.seen.iter().any(|s| s.starts_with("102\x1f")));
            let _ = fs::remove_dir_all(&base);
        });
    }

    #[test]
    fn session_switch_resets_repos_but_keeps_baseline() {
        with_state_dir(|| {
            let a = live("w1:p2", "w1:t1", "opencode", "ses_a");
            let e = observe(&a, &[("200".into(), "/tmp".into())]);
            assert!(e.seen.iter().any(|s| s.starts_with("200\x1f")));
            assert!(e.repos.is_empty());
            let b = live("w1:p2", "w1:t1", "opencode", "ses_b");
            let e = observe(&b, &[("200".into(), "/tmp".into())]);
            assert_eq!(e.session, "ses_b");
            assert!(e.repos.is_empty());
            assert!(e.seen.iter().any(|s| s.starts_with("200\x1f")));
        });
    }

    #[test]
    fn prune_drops_gone_panes_and_dead_agents() {
        with_state_dir(|| {
            observe(&live("w1:p1", "w1:t1", "opencode", "s1"), &[]);
            observe(&live("w1:p2", "w1:t1", "opencode", "s2"), &[]);
            prune(&[
                live("w1:p1", "w1:t1", "opencode", "s1"),
                live("w1:p2", "w1:t1", "", "s2"),
            ]);
            assert!(load_entry("w1:p1").is_some());
            assert!(
                load_entry("w1:p2").is_none(),
                "agent gone drops the session"
            );
            prune(&[live("w1:p1", "w1:t1", "opencode", "s1")]);
            assert!(load_entry("w1:p1").is_some());
            prune(&[]);
            assert!(load_entry("w1:p1").is_none(), "pane gone drops the session");
        });
    }

    #[test]
    fn agent_swap_resets_repos() {
        with_state_dir(|| {
            save_entry(&PaneEntry {
                pane: "w1:p1".into(),
                tab: "w1:t1".into(),
                agent: "opencode".into(),
                session: "s1".into(),
                repos: vec!["/r/one".into()],
                seen: vec!["1\x1f/x".into()],
                updated: 1,
            });
            let e = observe(&live("w1:p1", "w1:t1", "claude", "s9"), &[]);
            assert_eq!(e.agent, "claude");
            assert_eq!(e.session, "s9");
            assert!(e.repos.is_empty());
            assert_eq!(e.seen, vec!["1\x1f/x".to_string()]);
        });
    }

    #[test]
    fn empty_agent_is_adopted_without_reset() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("dv-adopt-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        std::env::set_var("HERDR_PLUGIN_STATE_DIR", &dir);
        std::fs::write(
            dir.join("session-w1_p1.json"),
            r#"{"pane":"w1:p1","tab":"w1:t1","session":"s1","repos":["/r/one"],"seen":[],"updated":"1"}"#,
        )
        .unwrap();
        let e = observe(&live("w1:p1", "w1:t1", "opencode", "s1"), &[]);
        assert_eq!(e.agent, "opencode");
        assert_eq!(e.repos, vec!["/r/one".to_string()]);
        std::env::remove_var("HERDR_PLUGIN_STATE_DIR");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_repos_serves_only_own_live_session() {
        with_state_dir(|| {
            save_entry(&PaneEntry {
                pane: "w1:p1".into(),
                tab: "w1:t1".into(),
                agent: "opencode".into(),
                session: "s1".into(),
                repos: vec!["/r/one".into()],
                seen: Vec::new(),
                updated: 1,
            });
            save_entry(&PaneEntry {
                pane: "w1:p2".into(),
                tab: "w1:t1".into(),
                agent: "opencode".into(),
                session: "s2".into(),
                repos: vec!["/r/two".into()],
                seen: Vec::new(),
                updated: 1,
            });
            let live_all = vec![
                live("w1:p1", "w1:t1", "opencode", "s1"),
                live("w1:p2", "w1:t1", "opencode", "s2"),
            ];
            assert_eq!(session_repos("w1:p1", &live_all), vec!["/r/one"]);
            assert_eq!(session_repos("w1:p2", &live_all), vec!["/r/two"]);
            assert!(session_repos("w1:p9", &live_all).is_empty());
            let rotated = vec![live("w1:p1", "w1:t1", "opencode", "s-new")];
            assert!(session_repos("w1:p1", &rotated).is_empty());
            let dead = vec![live("w1:p1", "w1:t1", "", "s1")];
            assert!(session_repos("w1:p1", &dead).is_empty());
        });
    }

    #[test]
    fn str_array_parses_escapes() {
        assert_eq!(parse_str_array(r#"["a","b\"c"]"#), vec!["a", "b\"c"]);
        assert!(parse_str_array("garbage").is_empty());
    }
}
