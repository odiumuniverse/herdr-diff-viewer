use std::collections::HashMap;
use std::process::Command;

#[allow(dead_code)]
pub struct StatusEntry {
    pub staged: char,
    pub work: char,
    pub path: String,
    pub tracked: bool,
}

fn git(top: &str, args: &[&str]) -> Result<(bool, String), String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(top)
        .args(args)
        .output()
        .map_err(|e| format!("spawn git: {e}"))?;
    Ok((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    ))
}

pub fn canonical(p: &str) -> String {
    std::fs::canonicalize(p)
        .map(|c| c.to_string_lossy().into_owned())
        .unwrap_or_else(|_| p.trim_end_matches('/').to_string())
}

pub fn toplevel(cwd: &str) -> Result<String, String> {
    let (ok, out) = git(cwd, &["rev-parse", "--show-toplevel"])?;
    if !ok {
        return Err(format!(
            "{cwd} is not inside a git repo — open the viewer from an agent pane"
        ));
    }
    Ok(canonical(out.trim()))
}

pub fn status(top: &str) -> Result<Vec<StatusEntry>, String> {
    let (ok, out) = git(
        top,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    if !ok {
        return Err("git status failed".to_string());
    }
    Ok(parse_porcelain_z(&out))
}

pub fn status_raw(top: &str) -> String {
    git(
        top,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )
    .map(|(_, out)| out)
    .unwrap_or_default()
}

pub fn parse_porcelain_z(out: &str) -> Vec<StatusEntry> {
    let mut entries = Vec::new();
    let mut toks = out.split('\0').peekable();
    while let Some(tok) = toks.next() {
        if tok.len() < 4 {
            continue;
        }
        let mut xy = tok.chars();
        let staged = xy.next().unwrap_or(' ');
        let work = xy.next().unwrap_or(' ');
        let path = tok[3..].to_string();
        let mut tracked = true;
        if staged == '?' && work == '?' {
            tracked = false;
        }
        if staged == 'R' || staged == 'C' {
            let _ = toks.next();
        }
        entries.push(StatusEntry {
            staged,
            work,
            path,
            tracked,
        });
    }
    entries
}

pub fn numstat(top: &str) -> HashMap<String, (u32, u32)> {
    let Ok((_, out)) = git(top, &["diff", "--numstat", "HEAD", "--", "."]) else {
        return HashMap::new();
    };
    let mut map = HashMap::new();
    for line in out.lines() {
        let mut parts = line.split('\t');
        let (Some(a), Some(d), Some(p)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        map.insert(normalize_numstat_path(p), (num(a), num(d)));
    }
    map
}

fn normalize_numstat_path(p: &str) -> String {
    let Some((l, r)) = p.split_once(" => ") else {
        return p.to_string();
    };
    if let Some(brace) = l.rfind('{') {
        let (new, suffix) = r.split_once('}').unwrap_or((r, ""));
        format!("{}{}{}", &l[..brace], new, suffix)
    } else {
        r.trim_matches(|c| c == '{' || c == '}').trim().to_string()
    }
}

pub fn resolve(map: &HashMap<String, (u32, u32)>, path: &str) -> Option<(u32, u32)> {
    map.get(path).copied()
}

fn num(s: &str) -> u32 {
    s.parse().unwrap_or(0)
}

pub fn untracked_adds(top: &str, path: &str) -> u32 {
    let full = format!("{top}/{path}");
    let Ok(body) = std::fs::read(&full) else {
        return 0;
    };
    if body.is_empty() {
        return 0;
    }
    let mut n = body.iter().filter(|b| **b == b'\n').count() as u32;
    if !body.ends_with(b"\n") {
        n += 1;
    }
    n
}

pub fn child_repos(root: &str) -> Vec<String> {
    let mut repos = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return repos;
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let p = e.path();
        if p.is_dir() && p.join(".git").exists() {
            repos.push(name);
        }
    }
    repos.sort();
    repos
}

#[derive(Clone, Copy, PartialEq)]
pub enum DLKind {
    Ctx,
    Add,
    Del,
}

#[derive(Clone)]
pub struct DiffLine {
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub kind: DLKind,
    pub text: String,
}

pub fn unified_all(top: &str) -> HashMap<String, Vec<DiffLine>> {
    let Ok((_, out)) = git(
        top,
        &[
            "diff",
            "--no-color",
            "--no-ext-diff",
            "-U3",
            "HEAD",
            "--",
            ".",
        ],
    ) else {
        return HashMap::new();
    };
    parse_unified_multi(&out)
}

pub fn parse_unified_multi(out: &str) -> HashMap<String, Vec<DiffLine>> {
    let mut map = HashMap::new();
    let mut section = Vec::new();
    let flush = |section: &mut Vec<&str>, map: &mut HashMap<String, Vec<DiffLine>>| {
        if section.is_empty() {
            return;
        }
        if let Some(path) = section_path(section) {
            map.insert(path, parse_unified(&section.join("\n")));
        }
        section.clear();
    };
    for line in out.lines() {
        if line.starts_with("diff --git ") {
            flush(&mut section, &mut map);
        }
        section.push(line);
    }
    flush(&mut section, &mut map);
    map
}

fn section_path(section: &[&str]) -> Option<String> {
    let mut old = None;
    for line in section {
        if let Some(p) = line.strip_prefix("+++ b/") {
            return Some(p.to_string());
        }
        if let Some(p) = line.strip_prefix("--- a/") {
            old = Some(p.to_string());
        }
    }
    old
}

pub fn unified(top: &str, path: &str, tracked: bool) -> Result<Vec<DiffLine>, String> {
    let (ok, out) = if tracked {
        git(
            top,
            &["diff", "--no-color", "--no-ext-diff", "-U3", "--", path],
        )?
    } else {
        git(
            top,
            &[
                "diff",
                "--no-color",
                "--no-ext-diff",
                "--no-index",
                "--",
                "/dev/null",
                path,
            ],
        )?
    };
    if !ok && out.is_empty() {
        return Err(format!("git diff failed for {path}"));
    }
    Ok(parse_unified(&out))
}

pub fn parse_unified(out: &str) -> Vec<DiffLine> {
    let mut rows = Vec::new();
    let (mut old, mut new) = (0u32, 0u32);
    let mut in_hunk = false;
    for line in out.lines() {
        if let Some(h) = line.strip_prefix("@@") {
            if let Some((o, n)) = parse_hunk(h) {
                old = o;
                new = n;
                in_hunk = true;
            }
            continue;
        }
        if !in_hunk
            && (line.starts_with("---") || line.starts_with("+++") || line.starts_with('\\'))
        {
            continue;
        }
        let (kind, body) = match line.as_bytes().first() {
            Some(b' ') => (DLKind::Ctx, &line[1..]),
            Some(b'+') => (DLKind::Add, &line[1..]),
            Some(b'-') => (DLKind::Del, &line[1..]),
            _ => continue,
        };
        let (o, n) = match kind {
            DLKind::Ctx => (Some(old), Some(new)),
            DLKind::Add => (None, Some(new)),
            DLKind::Del => (Some(old), None),
        };
        if matches!(kind, DLKind::Ctx | DLKind::Del) {
            old += 1;
        }
        if matches!(kind, DLKind::Ctx | DLKind::Add) {
            new += 1;
        }
        rows.push(DiffLine {
            old: o,
            new: n,
            kind,
            text: body.to_string(),
        });
    }
    rows
}

fn parse_hunk(h: &str) -> Option<(u32, u32)> {
    let mut it = h.split_whitespace();
    let o = it.next()?.strip_prefix('-')?;
    let n = it.next()?.strip_prefix('+')?;
    let num = |s: &str| s.split(',').next()?.parse().ok();
    Some((num(o)?, num(n)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_repos_finds_marked_dirs_only() {
        let root = std::env::temp_dir().join(format!("dv-repos-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["apisrv", "plain", ".hidden"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        for d in ["apisrv", ".hidden"] {
            std::fs::create_dir_all(root.join(d).join(".git")).unwrap();
        }
        assert_eq!(
            child_repos(&root.to_string_lossy()),
            vec!["apisrv".to_string()]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn parses_modified_added_and_renamed_porcelain() {
        let out = " M src/a.rs\0?? new.rs\0R  new_name.rs\0old_name.rs\0";
        let e = parse_porcelain_z(out);
        assert_eq!(e.len(), 3);
        assert_eq!(e[0].path, "src/a.rs");
        assert!(e[0].tracked);
        assert_eq!(e[1].path, "new.rs");
        assert!(!e[1].tracked);
        assert_eq!(e[2].path, "new_name.rs");
    }

    #[test]
    fn hunk_counters_track_both_sides() {
        let diff = "@@ -10,4 +10,4 @@\n ctx\n-old\n+new\n+more\n tail\n";
        let rows = parse_unified(diff);
        assert_eq!(rows.len(), 5);
        assert_eq!((rows[0].old, rows[0].new), (Some(10), Some(10)));
        assert_eq!((rows[1].old, rows[1].new), (Some(11), None));
        assert_eq!((rows[2].old, rows[2].new), (None, Some(11)));
        assert_eq!((rows[3].old, rows[3].new), (None, Some(12)));
        assert_eq!((rows[4].old, rows[4].new), (Some(12), Some(13)));
    }

    #[test]
    fn new_file_hunk_starts_at_zero() {
        let rows = parse_unified("@@ -0,0 +1,2 @@\n+a\n+b\n");
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.old.is_none()));
        assert_eq!(rows[1].new, Some(2));
    }

    #[test]
    fn numstat_rename_forms_reduce_to_new_path() {
        assert_eq!(normalize_numstat_path("b.rs"), "b.rs");
        assert_eq!(normalize_numstat_path("a.rs => b.rs"), "b.rs");
        assert_eq!(normalize_numstat_path("{a.rs => b.rs}"), "b.rs");
        assert_eq!(normalize_numstat_path("src/{old => new}.rs"), "src/new.rs");
        let mut m = HashMap::new();
        m.insert("b.rs".into(), (3, 1));
        assert_eq!(resolve(&m, "b.rs"), Some((3, 1)));
        assert_eq!(resolve(&m, "zzz.rs"), None);
    }

    #[test]
    fn multi_file_stream_splits_by_path() {
        let out = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-x\n+y\n\
             diff --git a/old.rs b/old.rs\n--- a/old.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-gone\n";
        let m = parse_unified_multi(out);
        assert_eq!(m.len(), 2);
        assert_eq!(m["a.rs"].len(), 2);
        assert_eq!(m["old.rs"].len(), 1);
        assert!(matches!(m["old.rs"][0].kind, DLKind::Del));
    }

    #[test]
    fn dash_dash_dash_content_survives_inside_hunks() {
        let diff = "@@ -1,2 +1,3 @@\n ctx\n---- x\n++++ y\n";
        let rows = parse_unified(diff);
        assert_eq!(rows.len(), 3);
        assert!(matches!(rows[1].kind, DLKind::Del));
        assert_eq!(rows[1].text, "--- x");
        assert!(matches!(rows[2].kind, DLKind::Add));
        assert_eq!(rows[2].text, "+++ y");
    }
}
