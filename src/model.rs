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
    pub repos: usize,
    pub theme: hl::ThemeId,
}

pub const MAX_REPOS: usize = 32;

pub struct Scope {
    pub root: String,
    pub anchor: Option<String>,
    pub repos: Vec<String>,
}

pub fn detect(root: &str, touched: &[String]) -> Result<Scope, String> {
    let root_c = git::canonical(root);
    let mut anchor = None;
    let mut repos: Vec<String> = Vec::new();
    if let Ok(top) = git::toplevel(&root_c) {
        anchor = Some(top.clone());
        repos.push(top);
    }
    for t in touched {
        if let Ok(top) = git::toplevel(t) {
            if !repos.contains(&top) {
                repos.push(top);
            }
        }
    }
    if anchor.is_none() && !is_huge(&root_c) {
        let mut kids = git::child_repos(&root_c);
        kids.sort_by_key(|k| {
            std::fs::metadata(format!("{root_c}/{k}"))
                .and_then(|m| m.modified())
                .ok()
        });
        for k in kids.iter().rev() {
            let c = git::canonical(&format!("{root_c}/{k}"));
            if !repos.contains(&c) {
                repos.push(c);
            }
        }
    }
    if repos.is_empty() {
        return Err(format!(
            "{root} is not a git repo and holds no repos — open the viewer from an agent pane"
        ));
    }
    repos.truncate(MAX_REPOS);
    if let Some(a) = anchor.clone() {
        if !repos.contains(&a) {
            repos.pop();
            repos.insert(0, a);
        }
    }
    Ok(Scope {
        root: root_c,
        anchor,
        repos,
    })
}

fn is_huge(root_c: &str) -> bool {
    if root_c == "/" {
        return true;
    }
    std::env::var("HOME")
        .map(|h| git::canonical(&h) == root_c)
        .unwrap_or(false)
}

fn short(root: &str, repo: &str) -> String {
    let prefix = format!("{}/", root.trim_end_matches('/'));
    if let Some(rest) = repo.strip_prefix(&prefix).filter(|r| !r.is_empty()) {
        return rest.to_string();
    }
    repo.rsplit('/').next().unwrap_or(repo).to_string()
}

pub fn build(scope: &Scope, theme: &hl::Theme) -> Result<Snapshot, String> {
    let multi = scope.anchor.is_none() || scope.repos.len() > 1;
    let mut main = Vec::new();
    let mut session = Vec::new();
    for r in &scope.repos {
        let Ok((mut m, mut s)) = build_one(r, theme) else {
            continue;
        };
        if multi {
            let prefix = short(&scope.root, r);
            for f in m.iter_mut().chain(s.iter_mut()) {
                f.display = format!("{prefix}/{}", f.path);
            }
        }
        main.extend(m);
        session.extend(s);
    }
    Ok(finish(main, session, scope.repos.len(), theme.id))
}

pub fn signature(repos: &[String]) -> u64 {
    let mut h = DefaultHasher::new();
    for top in repos {
        top.hash(&mut h);
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

fn finish(
    mut main: Vec<FileView>,
    mut session: Vec<FileView>,
    repos: usize,
    theme: hl::ThemeId,
) -> Snapshot {
    main.sort_by(|a, b| a.display.cmp(&b.display));
    session.sort_by(|a, b| a.display.cmp(&b.display));
    let (files, adds, dels) = totals(&main, &session);
    Snapshot {
        main,
        session,
        files,
        adds,
        dels,
        repos,
        theme,
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
    if entries.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
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
        let repos = vec![git::canonical(&root_s)];
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        let first = signature(&repos);
        assert_eq!(first, signature(&repos));
        std::fs::write(root.join("a.txt"), "one\ntwo\n").unwrap();
        assert_ne!(first, signature(&repos));
        let _ = std::fs::remove_dir_all(&root);
    }

    fn init_repo(path: &std::path::Path) {
        let s = path.to_string_lossy().into_owned();
        let ok = std::process::Command::new("git")
            .args(["init", "-q", &s])
            .status()
            .is_ok_and(|st| st.success());
        assert!(ok, "git init failed for {s}");
    }

    #[test]
    fn detect_unions_children_and_touched() {
        let base = std::env::temp_dir().join(format!("dv-union-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let child = base.join("child");
        let outside = std::env::temp_dir().join(format!("dv-out-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(&child).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        init_repo(&child);
        init_repo(&outside);
        let base_s = base.to_string_lossy().into_owned();
        let outside_s = outside.to_string_lossy().into_owned();

        let s = detect(&base_s, &[]).unwrap();
        assert!(s.anchor.is_none());
        assert_eq!(s.repos.len(), 1);

        let s = detect(&base_s, std::slice::from_ref(&outside_s)).unwrap();
        assert_eq!(s.repos.len(), 2);
        assert!(s.repos.contains(&git::canonical(&outside_s)));

        let sub = child.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        let s = detect(&base_s, &[sub.to_string_lossy().into_owned()]).unwrap();
        assert_eq!(s.repos.len(), 1, "subdir resolves to the child toplevel");

        let s = detect(&child.to_string_lossy(), &[outside_s]).unwrap();
        assert_eq!(
            s.anchor.as_deref(),
            Some(git::canonical(&child.to_string_lossy()).as_str())
        );
        assert_eq!(s.repos.len(), 2);

        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn short_labels_nested_relative_and_outside_basename() {
        assert_eq!(short("/a/my", "/a/my/foo"), "foo");
        assert_eq!(short("/a/my", "/a/my"), "my");
        assert_eq!(short("/a/my", "/other/topscan"), "topscan");
        assert_eq!(short("/", "/a/b"), "a/b");
    }

    #[test]
    fn build_union_prefixes_display_with_repo_names() {
        let base = std::env::temp_dir().join(format!("dv-build-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let child = base.join("child");
        std::fs::create_dir_all(&child).unwrap();
        init_repo(&child);
        std::fs::write(child.join("a.txt"), "one\ntwo\n").unwrap();
        let base_s = base.to_string_lossy().into_owned();

        let scope = detect(&base_s, &[]).unwrap();
        let snap = build(&scope, &hl::theme(hl::ThemeId::ClaudeDark)).unwrap();
        assert_eq!(snap.files, 1);
        assert_eq!(snap.main[0].display, "child/a.txt");
        assert_eq!(snap.main[0].adds, 2);
        assert_eq!(snap.repos, 1);

        let _ = std::fs::remove_dir_all(&base);
    }
}
