use crate::git::{self, DiffLine};
use crate::group::{classify, Group};
use crate::hl::{self, Span};

/// One file card: stats for the list row, hunks for the section below.
/// `path` is repo-relative (git ops); `display` is shown and sent to the
/// agent (`svc/`-prefixed in multi-repo mode). `hl[i]` colors `hunks[i]`.
pub struct FileView {
    pub path: String,
    pub display: String,
    /// Absolute path — the session filter matches mined absolute paths here.
    pub abs: String,
    pub adds: u32,
    pub dels: u32,
    pub group: Group,
    pub hunks: Vec<DiffLine>,
    pub hl: Vec<Vec<Span>>,
}

pub struct Snapshot {
    pub main: Vec<FileView>,
    pub session: Vec<FileView>,
    pub files: usize,
    pub adds: u64,
    pub dels: u64,
}

/// Diff scope: one repo, or a gitless root (like ~/topscan) whose direct
/// children are repos — their diffs merge with `svc/`-prefixed paths.
pub enum Scope {
    Single(String),
    Multi(String, Vec<String>),
}

/// Resolve scope from any cwd: inside a repo -> it; else child repos.
pub fn detect(root: &str) -> Result<Scope, String> {
    if git::toplevel(root).is_ok() {
        return Ok(Scope::Single(root.to_string()));
    }
    let repos = git::child_repos(root);
    if repos.is_empty() {
        return Err(format!(
            "{root} is not a git repo and holds no repos — open the viewer from an agent pane"
        ));
    }
    Ok(Scope::Multi(root.to_string(), repos))
}

/// Working tree vs HEAD, split into main + collapsed tests/generated.
/// Deterministic alphabetical order like the screenshot's list.
pub fn build(scope: &Scope) -> Result<Snapshot, String> {
    match scope {
        Scope::Single(top) => {
            let (main, session) = build_one(top)?;
            Ok(finish(main, session))
        }
        Scope::Multi(root, repos) => {
            let mut main = Vec::new();
            let mut session = Vec::new();
            for r in repos {
                let (mut m, mut s) = build_one(&format!("{root}/{r}"))?;
                for f in m.iter_mut().chain(s.iter_mut()) {
                    f.display = format!("{r}/{}", f.path);
                }
                main.extend(m);
                session.extend(s);
            }
            Ok(finish(main, session))
        }
    }
}

fn finish(mut main: Vec<FileView>, mut session: Vec<FileView>) -> Snapshot {
    main.sort_by(|a, b| a.display.cmp(&b.display));
    session.sort_by(|a, b| a.display.cmp(&b.display));
    let (files, adds, dels) = totals(&main, &session);
    Snapshot {
        main,
        session,
        files,
        adds,
        dels,
    }
}

fn totals(main: &[FileView], session: &[FileView]) -> (usize, u64, u64) {
    let files = main.len() + session.len();
    let adds = main.iter().chain(session).map(|f| f.adds as u64).sum();
    let dels = main.iter().chain(session).map(|f| f.dels as u64).sum();
    (files, adds, dels)
}

/// Keep only files this session touched (absolute match or `/display`
/// suffix — logs carry absolute paths, the snapshot repo-relative ones).
/// Multi-session repos then show just one agent's footprint.
pub fn retain_session(snap: &mut Snapshot, touched: &[String]) {
    let keep = |f: &FileView| {
        let disp = format!("/{}", f.display);
        touched
            .iter()
            .any(|t| t == &f.abs || t == &f.display || t.ends_with(&disp))
    };
    snap.main.retain(&keep);
    snap.session.retain(&keep);
    let (files, adds, dels) = totals(&snap.main, &snap.session);
    snap.files = files;
    snap.adds = adds;
    snap.dels = dels;
}

fn build_one(top: &str) -> Result<(Vec<FileView>, Vec<FileView>), String> {
    let entries = git::status(top)?;
    let stats = git::numstat(top);
    let all = git::unified_all(top);
    let mut main = Vec::new();
    let mut session = Vec::new();
    for e in &entries {
        let (adds, dels) = match git::resolve(&stats, &e.path) {
            Some(v) => v,
            None if !e.tracked => (git::untracked_adds(top, &e.path), 0),
            None => (0, 0),
        };
        let hunks = if e.tracked {
            all.get(&e.path).cloned().unwrap_or_default()
        } else {
            git::unified(top, &e.path, false).unwrap_or_default()
        };
        let texts: Vec<String> = hunks.iter().map(|h| h.text.clone()).collect();
        let file = FileView {
            display: e.path.clone(),
            path: e.path.clone(),
            abs: format!("{top}/{}", e.path),
            adds,
            dels,
            group: classify(&e.path),
            hunks,
            hl: hl::highlight(&e.path, &texts),
        };
        if file.group == Group::Session {
            session.push(file);
        } else {
            main.push(file);
        }
    }
    Ok((main, session))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retain_keeps_session_files_only() {
        // Given a snapshot with two files When retaining one session path
        // Then only the touched file (abs match) survives with fresh totals.
        let mk = |display: &str, abs: &str| FileView {
            path: display.into(),
            display: display.into(),
            abs: abs.into(),
            adds: 1,
            dels: 0,
            group: Group::Main,
            hunks: vec![],
            hl: vec![],
        };
        let mut snap = finish(
            vec![
                mk("apisrv/a.go", "/top/apisrv/a.go"),
                mk("tpsrv/b.go", "/top/tpsrv/b.go"),
            ],
            vec![],
        );
        retain_session(&mut snap, &["/top/tpsrv/b.go".to_string()]);
        assert_eq!(snap.files, 1);
        assert_eq!(snap.main[0].display, "tpsrv/b.go");
        assert_eq!(snap.adds, 1);
    }
}
