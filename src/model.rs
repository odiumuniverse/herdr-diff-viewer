use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::git::{self, DiffLine};
use crate::group::{classify, Group};
use crate::hl::{self, Span};

pub struct FileView {
    pub path: String,
    pub display: String,
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

pub enum Scope {
    Single(String),
    Multi(String, Vec<String>),
}

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

pub fn build(scope: &Scope, theme: &hl::Theme) -> Result<Snapshot, String> {
    match scope {
        Scope::Single(top) => {
            let (main, session) = build_one(top, theme)?;
            Ok(finish(main, session))
        }
        Scope::Multi(root, repos) => {
            let mut main = Vec::new();
            let mut session = Vec::new();
            for r in repos {
                let (mut m, mut s) = build_one(&format!("{root}/{r}"), theme)?;
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

pub fn signature(root: &str) -> u64 {
    let mut h = DefaultHasher::new();
    let tops = match detect(root) {
        Ok(Scope::Single(top)) => vec![top],
        Ok(Scope::Multi(root, repos)) => repos.iter().map(|r| format!("{root}/{r}")).collect(),
        Err(e) => {
            e.hash(&mut h);
            return h.finish();
        }
    };
    for top in &tops {
        let raw = git::status_raw(top);
        for e in git::parse_porcelain_z(&raw) {
            if let Ok(m) = std::fs::metadata(format!("{top}/{}", e.path)) {
                m.len().hash(&mut h);
                m.modified().ok().hash(&mut h);
            }
        }
        raw.hash(&mut h);
    }
    h.finish()
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

fn build_one(top: &str, theme: &hl::Theme) -> Result<(Vec<FileView>, Vec<FileView>), String> {
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
            adds,
            dels,
            group: classify(&e.path),
            hunks,
            hl: hl::highlight(&e.path, &texts, theme),
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
    fn signature_moves_when_a_dirty_file_changes_again() {
        let root = std::env::temp_dir().join(format!("dv-sig-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let root_s = root.to_string_lossy().into_owned();
        let ok = std::process::Command::new("git")
            .args(["init", "-q", &root_s])
            .status()
            .is_ok_and(|s| s.success());
        assert!(ok, "git init failed");
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        let first = signature(&root_s);
        assert_eq!(first, signature(&root_s));
        std::fs::write(root.join("a.txt"), "one\ntwo\n").unwrap();
        assert_ne!(first, signature(&root_s));
        let _ = std::fs::remove_dir_all(&root);
    }
}
