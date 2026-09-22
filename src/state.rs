use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::git;

#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub const MAX_SESSION_REPOS: usize = 64;
pub const MAX_SEEN: usize = 256;
const GATE_INTERVAL: u64 = 5;

#[derive(Serialize, Deserialize, Default, Clone)]
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
    let body = serde_json::to_string(st).map_err(|e| format!("encode state: {e}"))?;
    fs::write(state_path(tab), body).map_err(|e| format!("write state: {e}"))
}

pub fn load(tab: &str) -> Option<ToggleState> {
    let body = fs::read_to_string(state_path(tab)).ok()?;
    serde_json::from_str(&body).ok()
}

pub fn remove(tab: &str) {
    let _ = fs::remove_file(state_path(tab));
}

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct PaneEntry {
    pub pane: String,
    #[serde(default)]
    pub tab: String,
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub session: String,
    #[serde(default)]
    pub repos: Vec<String>,
    #[serde(default)]
    pub candidates: Vec<String>,
    #[serde(default)]
    pub seen: Vec<String>,
    #[serde(default)]
    pub sig: BTreeMap<String, u64>,
    #[serde(default)]
    pub sig_at: BTreeMap<String, u64>,
    #[serde(default)]
    pub cursors: BTreeMap<String, String>,
}

pub struct LivePane {
    pub pane: String,
    pub tab: String,
    pub agent: String,
    pub session: String,
    pub cwd: Option<String>,
}

pub struct JournalFeed {
    pub key: String,
    pub gated: bool,
    pub paths: Vec<PathBuf>,
    pub candidates: Vec<PathBuf>,
    pub cursor: String,
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

pub fn load_entry(pane: &str) -> Option<PaneEntry> {
    let body = fs::read_to_string(session_path(pane)).ok()?;
    serde_json::from_str(&body).ok()
}

pub fn save_entry(e: &PaneEntry) {
    let path = session_path(&e.pane);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let tmp = path.with_extension(format!("tmp.{}.{}", std::process::id(), nanos));
    let Ok(body) = serde_json::to_string(e) else {
        return;
    };
    if fs::write(&tmp, body).is_ok() && fs::rename(&tmp, &path).is_err() {
        let _ = fs::remove_file(&tmp);
    }
}

pub fn remove_entry(pane: &str) {
    let _ = fs::remove_file(session_path(pane));
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
            if let Ok(en) = serde_json::from_str::<PaneEntry>(&body) {
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

fn add_candidate(e: &mut PaneEntry, top: &str) -> bool {
    if e.repos.iter().any(|r| r == top) {
        return false;
    }
    if e.candidates.iter().any(|r| r == top) {
        return false;
    }
    e.candidates.push(top.to_string());
    if e.candidates.len() > MAX_SESSION_REPOS {
        e.candidates.remove(0);
    }
    true
}

fn add_repo(e: &mut PaneEntry, top: &str) -> bool {
    if e.repos.iter().any(|r| r == top) {
        return false;
    }
    e.candidates.retain(|r| r != top);
    e.repos.push(top.to_string());
    if e.repos.len() > MAX_SESSION_REPOS {
        e.repos.remove(0);
    }
    true
}

fn run_gate(e: &mut PaneEntry) -> bool {
    let now = now_secs();
    let mut changed = false;
    let mut candidates = e.candidates.clone();
    candidates.sort();
    for c in candidates {
        if e.repos.iter().any(|r| r == &c) {
            continue;
        }
        if !e.sig.contains_key(&c) {
            e.sig
                .insert(c.clone(), crate::model::signature(std::slice::from_ref(&c)));
            e.sig_at.insert(c.clone(), now);
            changed = true;
            continue;
        }
        let last = e.sig_at.get(&c).copied().unwrap_or(0);
        if now.saturating_sub(last) < GATE_INTERVAL {
            continue;
        }
        let s = crate::model::signature(std::slice::from_ref(&c));
        let prev = e.sig.get(&c).copied().unwrap_or(s);
        e.sig_at.insert(c.clone(), now);
        e.sig.insert(c.clone(), s);
        if s != prev {
            changed |= add_repo(e, &c);
        } else {
            changed = true;
        }
    }
    changed
}

fn repo_of(path: &std::path::Path) -> Option<String> {
    let dir = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()?.to_path_buf()
    };
    git::toplevel(&dir.to_string_lossy()).ok()
}

pub fn observe(live: &LivePane, procs: &[(u32, String)], feed: Option<JournalFeed>) -> PaneEntry {
    let _lock = lock_entry(&live.pane);
    let loaded = load_entry(&live.pane);
    let fresh = loaded.is_none();
    let mut e = loaded.unwrap_or_default();
    e.pane = live.pane.clone();
    let mut changed = false;

    let session_changed = !live.session.is_empty() && e.session != live.session;
    let agent_changed = !e.agent.is_empty() && e.agent != live.agent;
    if session_changed || agent_changed {
        e.repos.clear();
        e.candidates.clear();
        e.sig.clear();
        e.sig_at.clear();
        e.cursors.clear();
        changed = true;
    }
    if e.session != live.session {
        e.session = live.session.clone();
        changed = true;
    }
    if e.agent != live.agent {
        e.agent = live.agent.clone();
        changed = true;
    }
    if e.tab != live.tab {
        e.tab = live.tab.clone();
        changed = true;
    }

    if !procs.is_empty() {
        let alive: HashSet<u32> = procs.iter().map(|(p, _)| *p).collect();
        let before = e.seen.len();
        e.seen.retain(|k| {
            k.split_once('\u{1f}')
                .and_then(|(p, _)| p.parse::<u32>().ok())
                .is_some_and(|p| alive.contains(&p))
        });
        if e.seen.len() != before {
            changed = true;
        }
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
            if fresh {
                continue;
            }
            if let Ok(top) = git::toplevel(cwd) {
                changed |= add_candidate(&mut e, &top);
            }
        }
        while e.seen.len() > MAX_SEEN {
            e.seen.remove(0);
        }
    }

    if let Some(f) = feed {
        if e.cursors.get(&f.key) != Some(&f.cursor) {
            e.cursors.insert(f.key, f.cursor);
            changed = true;
        }
        for p in &f.paths {
            let Some(top) = repo_of(p) else {
                continue;
            };
            if f.gated {
                changed |= add_candidate(&mut e, &top);
            } else {
                changed |= add_repo(&mut e, &top);
            }
        }
        for p in &f.candidates {
            let Some(top) = repo_of(p) else {
                continue;
            };
            changed |= add_candidate(&mut e, &top);
        }
    }

    changed |= run_gate(&mut e);
    if changed {
        save_entry(&e);
    }
    e
}

pub fn prune(live: &[LivePane]) {
    if live.is_empty() {
        return;
    }
    legacy_cleanup();
    for e in list_entries() {
        if !live.iter().any(|l| l.pane == e.pane) {
            remove_entry(&e.pane);
        }
    }
}

pub fn repos_for(pane: &str, live: &[LivePane]) -> Vec<String> {
    let Some(l) = live.iter().find(|p| p.pane == pane && !p.agent.is_empty()) else {
        return Vec::new();
    };
    match load_entry(pane) {
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
    if let Ok(dir) = std::env::var("HERDR_PLUGIN_CONFIG_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir).join("theme");
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

const LOCK_STALE: Duration = Duration::from_secs(5);

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

fn claim_stale(path: PathBuf) -> Option<Lock> {
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

pub fn lock(tab: &str) -> Option<Lock> {
    claim_stale(lock_path(tab))
}

fn lock_entry(pane: &str) -> Option<Lock> {
    claim_stale(session_path(pane).with_extension("lock"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live(pane: &str, tab: &str, agent: &str, session: &str) -> LivePane {
        LivePane {
            pane: pane.into(),
            tab: tab.into(),
            agent: agent.into(),
            session: session.into(),
            cwd: None,
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
        git::canonical(&s)
    }

    #[test]
    fn roundtrip_toggle_state() {
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
    fn procs_baseline_then_candidates_gated_until_change() {
        with_state_dir(|| {
            let base = std::env::temp_dir().join(format!("dv-admit-{}", std::process::id()));
            let _ = fs::remove_dir_all(&base);
            fs::create_dir_all(&base).unwrap();
            let top = init_repo(&base);
            let l = live("w1:p1", "w1:t1", "opencode", "ses_a");
            let p = base.to_string_lossy().into_owned();
            let e = observe(&l, &[(100, p.clone())], None);
            assert!(e.repos.is_empty() && e.candidates.is_empty());
            let e = observe(&l, &[(101, p.clone())], None);
            assert_eq!(e.candidates, vec![top.clone()]);
            assert!(e.repos.is_empty(), "no signature change yet");
            let _ = fs::remove_dir_all(&base);
        });
    }

    #[test]
    fn gated_candidate_admits_on_signature_change() {
        with_state_dir(|| {
            let base = std::env::temp_dir().join(format!("dv-gate-{}", std::process::id()));
            let _ = fs::remove_dir_all(&base);
            fs::create_dir_all(&base).unwrap();
            let top = init_repo(&base);
            let l = live("w1:p2", "w1:t1", "opencode", "ses_g");
            let p = base.to_string_lossy().into_owned();
            observe(&l, &[(1, p.clone())], None);
            observe(&l, &[(2, p.clone())], None);
            fs::write(base.join("a.txt"), "one\ntwo\n").unwrap();
            let mut e = load_entry("w1:p2").unwrap();
            e.sig_at.insert(top.clone(), 0);
            save_entry(&e);
            let e = observe(&l, &[(2, p)], None);
            assert_eq!(e.repos, vec![top.clone()]);
            assert!(e.candidates.is_empty());
            let _ = fs::remove_dir_all(&base);
        });
    }

    #[test]
    fn exact_feed_admits_without_gate() {
        with_state_dir(|| {
            let base = std::env::temp_dir().join(format!("dv-feed-{}", std::process::id()));
            let _ = fs::remove_dir_all(&base);
            fs::create_dir_all(&base).unwrap();
            let top = init_repo(&base);
            let file = base.join("a.rs");
            fs::write(&file, "x\n").unwrap();
            let l = live("w1:p3", "w1:t1", "claude", "ses_f");
            let feed = JournalFeed {
                key: "claude".into(),
                gated: false,
                paths: vec![file],
                candidates: Vec::new(),
                cursor: "1".into(),
            };
            let e = observe(&l, &[], Some(feed));
            assert_eq!(e.repos, vec![top]);
            assert_eq!(e.cursors.get("claude").map(String::as_str), Some("1"));
            let _ = fs::remove_dir_all(&base);
        });
    }

    #[test]
    fn feed_candidates_wait_for_signature_change() {
        with_state_dir(|| {
            let base = std::env::temp_dir().join(format!("dv-feed-gate-{}", std::process::id()));
            let _ = fs::remove_dir_all(&base);
            fs::create_dir_all(&base).unwrap();
            let top = init_repo(&base);
            let l = live("w1:pB", "w1:t1", "opencode", "ses_wd");
            let feed = |cursor: &str| JournalFeed {
                key: "opencode".into(),
                gated: false,
                paths: Vec::new(),
                candidates: vec![base.clone()],
                cursor: cursor.into(),
            };
            let e = observe(&l, &[], Some(feed("1")));
            assert_eq!(e.candidates, vec![top.clone()]);
            assert!(e.repos.is_empty(), "a workdir alone must not adopt");
            fs::write(base.join("a.txt"), "one\n").unwrap();
            let mut e = load_entry("w1:pB").unwrap();
            e.sig_at.insert(top.clone(), 0);
            save_entry(&e);
            let e = observe(&l, &[], Some(feed("2")));
            assert_eq!(e.repos, vec![top.clone()]);
            let _ = fs::remove_dir_all(&base);
        });
    }

    #[test]
    fn session_switch_resets_repos_and_keeps_seen() {
        with_state_dir(|| {
            let base = std::env::temp_dir().join(format!("dv-switch-{}", std::process::id()));
            let _ = fs::remove_dir_all(&base);
            fs::create_dir_all(&base).unwrap();
            init_repo(&base);
            let file = base.join("a.rs");
            fs::write(&file, "x\n").unwrap();
            let a = live("w1:p4", "w1:t1", "claude", "ses_1");
            let feed = JournalFeed {
                key: "claude".into(),
                gated: false,
                paths: vec![file.clone()],
                candidates: Vec::new(),
                cursor: "1".into(),
            };
            observe(&a, &[(7, base.to_string_lossy().into_owned())], Some(feed));
            let b = live("w1:p4", "w1:t1", "claude", "ses_2");
            let e = observe(&b, &[(7, base.to_string_lossy().into_owned())], None);
            assert!(e.repos.is_empty(), "new session starts empty");
            assert!(e.seen.iter().any(|k| k.starts_with("7\u{1f}")));
            assert!(e.cursors.is_empty());
            let _ = fs::remove_dir_all(&base);
        });
    }

    #[test]
    fn agent_swap_resets_repos() {
        with_state_dir(|| {
            let base = std::env::temp_dir().join(format!("dv-swap-{}", std::process::id()));
            let _ = fs::remove_dir_all(&base);
            fs::create_dir_all(&base).unwrap();
            let top = init_repo(&base);
            let file = base.join("a.rs");
            fs::write(&file, "x\n").unwrap();
            let a = live("w1:p5", "w1:t1", "claude", "ses_1");
            let feed = JournalFeed {
                key: "claude".into(),
                gated: false,
                paths: vec![file],
                candidates: Vec::new(),
                cursor: "1".into(),
            };
            observe(&a, &[], Some(feed));
            assert_eq!(load_entry("w1:p5").unwrap().repos, vec![top]);
            let b = live("w1:p5", "w1:t1", "opencode", "ses_9");
            let e = observe(&b, &[], None);
            assert_eq!(e.agent, "opencode");
            assert!(e.repos.is_empty());
            let _ = fs::remove_dir_all(&base);
        });
    }

    #[test]
    fn prune_guard_and_gone_panes() {
        with_state_dir(|| {
            observe(&live("w1:p6", "w1:t1", "opencode", "s1"), &[], None);
            observe(&live("w1:p7", "w1:t1", "opencode", "s2"), &[], None);
            prune(&[]);
            assert!(
                load_entry("w1:p6").is_some() && load_entry("w1:p7").is_some(),
                "empty list must not wipe state"
            );
            prune(&[live("w1:p6", "w1:t1", "opencode", "s1")]);
            assert!(load_entry("w1:p6").is_some());
            assert!(load_entry("w1:p7").is_none());
        });
    }

    #[test]
    fn repos_for_only_matching_live_session() {
        with_state_dir(|| {
            save_entry(&PaneEntry {
                pane: "w1:p8".into(),
                tab: "w1:t1".into(),
                agent: "opencode".into(),
                session: "s1".into(),
                repos: vec!["/r/one".into()],
                ..Default::default()
            });
            let live_all = vec![live("w1:p8", "w1:t1", "opencode", "s1")];
            assert_eq!(repos_for("w1:p8", &live_all), vec!["/r/one"]);
            let dead = vec![live("w1:p8", "w1:t1", "", "s1")];
            assert!(repos_for("w1:p8", &dead).is_empty());
            let rotated = vec![live("w1:p8", "w1:t1", "opencode", "s-new")];
            assert!(repos_for("w1:p8", &rotated).is_empty());
            assert!(repos_for("w1:p9", &live_all).is_empty());
        });
    }

    #[test]
    fn seen_gc_drops_dead_pids_only() {
        with_state_dir(|| {
            let l = live("w1:pA", "w1:t1", "opencode", "s1");
            observe(&l, &[(11, "/tmp".into()), (12, "/tmp".into())], None);
            assert_eq!(load_entry("w1:pA").unwrap().seen.len(), 2);
            let e = observe(&l, &[(12, "/tmp".into())], None);
            assert!(e.seen.iter().any(|k| k.starts_with("12\u{1f}")));
            assert!(!e.seen.iter().any(|k| k.starts_with("11\u{1f}")));
        });
    }
}
