use std::collections::BTreeMap;
use std::io::BufRead;
use std::path::{Path, PathBuf};

use super::{absolutize, read_jsonl_since, root, Adapter, Edits, SessionRef};

/// omp stores one append-only `<stamp>_<uuid>.jsonl` transcript per session,
/// plus a sibling directory `<stamp>_<uuid>/<Name>.jsonl` holding one
/// transcript per spawned subagent. herdr publishes the *path* of the main
/// transcript as the pane's session id, so resolution goes path-first and only
/// falls back to newest-in-cwd for panes that have no session at all.
pub struct Omp;

fn sessions_root() -> PathBuf {
    root().join(".omp/agent/sessions")
}

fn parse_line(line: &str) -> Option<serde_json::Value> {
    let line = line.trim_end();
    serde_json::from_str::<serde_json::Value>(line)
        .ok()
        .or_else(|| {
            line.find('{')
                .and_then(|i| serde_json::from_str(&line[i..]).ok())
        })
}

/// The `session` record is the second line of every transcript. Read the head
/// only — the main transcripts run to megabytes.
fn session_record(file: &Path) -> Option<serde_json::Value> {
    let f = std::fs::File::open(file).ok()?;
    std::io::BufReader::new(f)
        .lines()
        .take(8)
        .flatten()
        .filter_map(|l| parse_line(&l))
        .find(|v| v.get("type").and_then(|t| t.as_str()) == Some("session"))
}

fn session_field(file: &Path, key: &str) -> Option<String> {
    session_record(file).and_then(|v| v.get(key).and_then(|c| c.as_str()).map(str::to_string))
}

fn session_cwd(file: &Path) -> Option<String> {
    session_field(file, "cwd")
}

/// A transcript belongs to a subagent exactly when its `session` record
/// carries `parentSession`, which a main transcript never has. The sibling
/// `<session>/<Name>/` directory is *not* the marker: only 1 of 11 subagents
/// in a real session has one, while every session that spawned anything has a
/// same-named directory next to its own transcript.
fn is_subagent(file: &Path) -> bool {
    session_field(file, "parentSession").is_some()
}

fn collect(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth == 0 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, depth - 1, out);
        } else if p.extension().is_some_and(|x| x == "jsonl") && !is_subagent(&p) {
            out.push(p);
        }
    }
}

/// Main session transcripts, newest mtime first.
fn main_sessions(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect(root, 3, &mut files);
    files.sort_by_key(|f| std::fs::metadata(f).and_then(|m| m.modified()).ok());
    files.reverse();
    files
}

/// Every `*.jsonl` anywhere under `dir`, bounded and sorted by the caller.
fn descendants(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth == 0 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            descendants(&p, depth - 1, out);
        } else if p.extension().is_some_and(|x| x == "jsonl") {
            out.push(p);
        }
    }
}

/// herdr reports the transcript *path*; a bare uuid or the stamp stem also
/// resolve, and the record's own `id` is the last resort. A path is only
/// trusted as an absolute `.jsonl` inside this sessions root — anything else
/// is not one of our transcripts.
fn match_session_id(root: &Path, files: &[PathBuf], session_id: &str) -> Option<PathBuf> {
    let as_path = Path::new(session_id);
    if as_path.is_absolute()
        && as_path.extension().is_some_and(|x| x == "jsonl")
        && as_path.starts_with(root)
        && as_path.is_file()
    {
        return Some(as_path.to_path_buf());
    }
    if let Some(f) = files
        .iter()
        .find(|f| f.file_name().is_some_and(|n| n == session_id))
    {
        return Some(f.clone());
    }
    if let Some(f) = files.iter().find(|f| {
        f.file_name()
            .is_some_and(|n| n.to_string_lossy().contains(session_id))
    }) {
        return Some(f.clone());
    }
    files
        .iter()
        .find(|f| session_field(f, "id").as_deref() == Some(session_id))
        .cloned()
}

fn resolve_in(root: &Path, pane_cwd: &str, session_id: &str) -> Option<SessionRef> {
    let files = main_sessions(root);
    let main = if session_id.is_empty() {
        files
            .iter()
            .take(200)
            .find(|f| session_cwd(f).as_deref() == Some(pane_cwd))?
            .clone()
    } else {
        // A session id that names nothing resolves to nothing: adopting a
        // different session's transcript would leak a neighbour pane in.
        match_session_id(root, &files, session_id)?
    };
    let mut sources = vec![main.clone()];
    // Everything under the session's own directory descends from it: a
    // subagent's subagents nest one level further down.
    let mut subs = Vec::new();
    descendants(&main.with_extension(""), 4, &mut subs);
    subs.sort();
    sources.extend(subs);
    let cwd = session_cwd(&main).or_else(|| {
        if pane_cwd.is_empty() {
            None
        } else {
            Some(pane_cwd.to_string())
        }
    });
    Some(SessionRef {
        id: session_id.to_string(),
        cwd,
        sources,
        gated: false,
    })
}

fn is_edit_tool(name: &str) -> bool {
    matches!(
        name,
        "edit" | "write" | "apply_patch" | "patch" | "multiedit" | "write_file"
    )
}

/// Hashline patch headers: `[<path>#<snapshot-hash>]` standing at the start of
/// a line. Anything else is body — a `+[foo#1234]` inside added content can
/// never match. A single patch may name any number of files.
fn patch_header_paths(input: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for line in input.lines() {
        let Some(inner) = line
            .trim()
            .strip_prefix('[')
            .and_then(|t| t.strip_suffix(']'))
        else {
            continue;
        };
        let Some((path, hash)) = inner.rsplit_once('#') else {
            continue;
        };
        if path.is_empty()
            || path.contains('#')
            || hash.is_empty()
            || !hash.bytes().all(|b| b.is_ascii_hexdigit())
            || path.contains("://")
        {
            continue;
        }
        out.push(PathBuf::from(path));
    }
    out
}

fn edits_in(_root: &Path, sess: &SessionRef, cursor: Option<&str>) -> Result<Edits, String> {
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
            // Only the `session` record carries a cwd; re-basing on any other
            // record would move every relative path harvested after it.
            if v.get("type").and_then(|t| t.as_str()) == Some("session") {
                if let Some(c) = v.get("cwd").and_then(|c| c.as_str()) {
                    base = Some(c.to_string());
                }
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
                let mut from_key = false;
                for ptr in ["/path", "/file_path", "/absolute_path"] {
                    let Some(p) = args.and_then(|a| a.pointer(ptr)).and_then(|p| p.as_str()) else {
                        continue;
                    };
                    // Spawning a subagent is an edit-tool call with an
                    // `agent://<name>` argument — a handle, not a path.
                    if p.contains("://") {
                        continue;
                    }
                    paths.push(PathBuf::from(p));
                    from_key = true;
                    break;
                }
                // Patch form: the touched files are `[<path>#<hash>]` headers.
                if !from_key {
                    if let Some(input) = args.and_then(|a| a.get("input")).and_then(|i| i.as_str())
                    {
                        paths.extend(patch_header_paths(input));
                    }
                }
            }
        }
    }
    Ok(Edits {
        paths: absolutize(paths, base.as_deref()),
        candidates: Vec::new(),
        cursor: serde_json::to_string(&offsets).unwrap_or_default(),
    })
}

impl Adapter for Omp {
    fn resolve(&self, pane_cwd: &str, session_id: &str) -> Option<SessionRef> {
        resolve_in(&sessions_root(), pane_cwd, session_id)
    }

    fn edits(&self, sess: &SessionRef, cursor: Option<&str>) -> Result<Edits, String> {
        edits_in(&sessions_root(), sess, cursor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(PathBuf);

    impl Dir {
        fn new(label: &str) -> Self {
            let p = std::env::temp_dir().join(format!("dv-omp-{label}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Dir(p)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// mtime is the only thing the newest-first sort reads, so pin it instead
    /// of sleeping.
    fn touch(path: &Path, secs: u64) {
        let f = std::fs::File::options().write(true).open(path).unwrap();
        let at = std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
        f.set_times(std::fs::FileTimes::new().set_modified(at))
            .unwrap();
    }

    fn message(blocks: serde_json::Value) -> String {
        serde_json::json!({"type":"message","message":{"role":"assistant","content":blocks}})
            .to_string()
    }

    fn edit_call(path: &str) -> serde_json::Value {
        serde_json::json!({"type":"toolCall","name":"edit","arguments":{"path":path}})
    }

    /// A toolCall with arbitrary arguments — the patch form has no `path`.
    fn call(name: &str, args: serde_json::Value) -> serde_json::Value {
        serde_json::json!({"type":"toolCall","name":name,"arguments":args})
    }

    /// A transcript with its `session` record and the given message lines.
    fn write_transcript(dir: &Path, stem: &str, id: &str, cwd: &str, body: &[String]) -> PathBuf {
        let mut lines = vec![
            serde_json::json!({"type":"title","v":1,"title":""}).to_string(),
            serde_json::json!({"type":"session","version":3,"id":id,"cwd":cwd}).to_string(),
        ];
        lines.extend_from_slice(body);
        let path = dir.join(format!("{stem}.jsonl"));
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        path
    }

    /// A subagent transcript: `<session>/<Name>.jsonl`, its `session` record
    /// pointing back at the main transcript.
    fn write_subagent(main: &Path, name: &str, cwd: &str, body: &[String]) -> PathBuf {
        let dir = main.with_extension("");
        std::fs::create_dir_all(&dir).unwrap();
        let mut lines = vec![
            serde_json::json!({"type":"title","v":1,"title":""}).to_string(),
            serde_json::json!({
                "type":"session","version":3,"id":format!("{name}-id"),"cwd":cwd,
                "parentSession":main.to_str().unwrap()
            })
            .to_string(),
        ];
        lines.extend_from_slice(body);
        let path = dir.join(format!("{name}.jsonl"));
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        path
    }

    /// A subagent's own subagent: `<session>/<Name>/<Name>.<Sub>.jsonl`, whose
    /// `parentSession` points at the parent subagent, not the main session.
    fn write_nested_subagent(main: &Path, name: &str, cwd: &str, body: &[String]) -> PathBuf {
        let dir = main.with_extension("").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let mut lines = vec![
            serde_json::json!({"type":"title","v":1,"title":""}).to_string(),
            serde_json::json!({
                "type":"session","version":3,"id":format!("{name}.sub"),"cwd":cwd,
                "parentSession":main.with_extension("").join(format!("{name}.jsonl"))
                    .to_str().unwrap()
            })
            .to_string(),
        ];
        lines.extend_from_slice(body);
        let path = dir.join(format!("{name}.SkillLinkScout.jsonl"));
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        path
    }

    #[test]
    fn path_session_id_wins_over_newer_file() {
        let tmp = Dir::new("path-id");
        let root = tmp.0.join("-tmp-proj");
        std::fs::create_dir_all(&root).unwrap();
        let older = write_transcript(
            &root,
            "2026-09-28T10-05-43-142Z_aaa",
            "aaa",
            "/tmp/proj",
            &[message(serde_json::json!([edit_call("src/older.rs")]))],
        );
        let newer = write_transcript(
            &root,
            "2026-09-28T10-06-43-142Z_bbb",
            "bbb",
            "/tmp/proj",
            &[message(serde_json::json!([edit_call("src/newer.rs")]))],
        );
        touch(&older, 1_700_000_000);
        touch(&newer, 1_800_000_000);

        let sess = resolve_in(&root, "/tmp/proj", older.to_str().unwrap()).expect("resolve");
        assert_eq!(sess.sources[0], older, "path id must pick its own file");
        let edits = edits_in(&root, &sess, None).unwrap();
        assert_eq!(edits.paths, vec![PathBuf::from("/tmp/proj/src/older.rs")]);
    }

    #[test]
    fn stale_session_id_resolves_to_nothing() {
        let tmp = Dir::new("stale");
        let root = tmp.0.join("-tmp-proj");
        std::fs::create_dir_all(&root).unwrap();
        write_transcript(
            &root,
            "2026-09-28T10-05-43-142Z_aaa",
            "aaa",
            "/tmp/proj",
            &[message(serde_json::json!([edit_call("src/a.rs")]))],
        );
        let stale = root.join("2026-09-28T09-00-00-000Z_2e90-7372-a74d-cef449d90a25.jsonl");
        assert!(!stale.exists());
        assert!(
            resolve_in(&root, "/tmp/proj", stale.to_str().unwrap()).is_none(),
            "a stale path id must not adopt another session's transcript"
        );
        assert!(
            resolve_in(&root, "/tmp/proj", "2e90-7372-a74d-cef449d90a25").is_none(),
            "a stale bare id must not adopt another session's transcript"
        );
    }

    #[test]
    fn bare_uuid_session_id_resolves() {
        let tmp = Dir::new("bare-id");
        let root = tmp.0.join("-tmp-proj");
        std::fs::create_dir_all(&root).unwrap();
        let file = write_transcript(
            &root,
            "2026-09-28T10-05-43-142Z_01a0e77a-3d66-7372-a74d-cef449d90a25",
            "01a0e77a-3d66-7372-a74d-cef449d90a25",
            "/tmp/proj",
            &[],
        );
        let sess = resolve_in(&root, "/tmp/proj", "01a0e77a-3d66-7372-a74d-cef449d90a25")
            .expect("resolve");
        assert_eq!(sess.sources[0], file);
    }

    #[test]
    fn subagent_transcripts_join_the_session() {
        let tmp = Dir::new("subagents");
        let root = tmp.0.join("-tmp-proj");
        std::fs::create_dir_all(&root).unwrap();
        let main = write_transcript(
            &root,
            "2026-09-28T10-05-43-142Z_aaa",
            "aaa",
            "/tmp/proj",
            &[message(serde_json::json!([edit_call("src/main.rs")]))],
        );
        let sub = write_subagent(
            &main,
            "Alpha",
            "/tmp/proj",
            &[message(serde_json::json!([edit_call("src/alpha.rs")]))],
        );

        let sess = resolve_in(&root, "/tmp/proj", main.to_str().unwrap()).expect("resolve");
        assert_eq!(sess.sources, vec![main.clone(), sub.clone()]);
        let edits = edits_in(&root, &sess, None).unwrap();
        assert_eq!(
            edits.paths,
            vec![
                PathBuf::from("/tmp/proj/src/main.rs"),
                PathBuf::from("/tmp/proj/src/alpha.rs")
            ]
        );
    }

    #[test]
    fn subagent_transcript_is_never_the_session() {
        let tmp = Dir::new("subagent-pick");
        let root = tmp.0.join("-tmp-proj");
        std::fs::create_dir_all(&root).unwrap();
        let main = write_transcript(
            &root,
            "2026-09-28T10-05-43-142Z_aaa",
            "aaa",
            "/tmp/proj",
            &[],
        );
        let sub = write_subagent(&main, "FarmDedup", "/tmp/proj", &[]);
        touch(&main, 1_700_000_000);
        touch(&sub, 1_800_000_000);

        let sess = resolve_in(&root, "/tmp/proj", "").expect("resolve");
        assert_eq!(sess.sources[0], main);
    }

    #[test]
    fn agent_url_arguments_are_not_paths() {
        let tmp = Dir::new("agent-url");
        let root = tmp.0.join("-tmp-proj");
        std::fs::create_dir_all(&root).unwrap();
        let main = write_transcript(
            &root,
            "2026-09-28T10-05-43-142Z_aaa",
            "aaa",
            "/tmp/proj",
            &[message(serde_json::json!([
                edit_call("agent://Alpha"),
                edit_call("src/a.rs")
            ]))],
        );

        let sess = resolve_in(&root, "/tmp/proj", "").expect("resolve");
        assert_eq!(sess.sources[0], main);
        let edits = edits_in(&root, &sess, None).unwrap();
        assert_eq!(edits.paths, vec![PathBuf::from("/tmp/proj/src/a.rs")]);
    }

    #[test]
    fn patch_form_edit_yields_its_files() {
        let tmp = Dir::new("patch-form");
        let root = tmp.0.join("-tmp-proj");
        std::fs::create_dir_all(&root).unwrap();
        let main = write_transcript(
            &root,
            "2026-09-28T10-05-43-142Z_aaa",
            "aaa",
            "/tmp/proj",
            &[message(serde_json::json!([call(
                "edit",
                serde_json::json!({"i":"editing","input":
                    "[src/a.rs#1A2B]\nPUT 1.=2:\n+fn a() {}\n[src/b.rs#3C4D]\nCUT 5.=6\n"})
            )]))],
        );

        let sess = resolve_in(&root, "/tmp/proj", "").expect("resolve");
        assert_eq!(sess.sources[0], main);
        let edits = edits_in(&root, &sess, None).unwrap();
        assert_eq!(
            edits.paths,
            vec![
                PathBuf::from("/tmp/proj/src/a.rs"),
                PathBuf::from("/tmp/proj/src/b.rs")
            ]
        );
    }

    #[test]
    fn patch_form_header_must_start_the_line() {
        let tmp = Dir::new("patch-anchor");
        let root = tmp.0.join("-tmp-proj");
        std::fs::create_dir_all(&root).unwrap();
        write_transcript(
            &root,
            "2026-09-28T10-05-43-142Z_aaa",
            "aaa",
            "/tmp/proj",
            &[message(serde_json::json!([call(
                "edit",
                serde_json::json!({"input":
                    "[src/real.rs#1111]\nPUT 1*:\n+let s = \"+[src/decoy.rs#9999]\";\n"})
            )]))],
        );

        let sess = resolve_in(&root, "/tmp/proj", "").expect("resolve");
        let edits = edits_in(&root, &sess, None).unwrap();
        assert_eq!(edits.paths, vec![PathBuf::from("/tmp/proj/src/real.rs")]);
    }

    #[test]
    fn patch_form_agent_url_header_is_rejected() {
        let tmp = Dir::new("patch-agent-url");
        let root = tmp.0.join("-tmp-proj");
        std::fs::create_dir_all(&root).unwrap();
        write_transcript(
            &root,
            "2026-09-28T10-05-43-142Z_aaa",
            "aaa",
            "/tmp/proj",
            &[message(serde_json::json!([call(
                "edit",
                serde_json::json!({"input":"[agent://Alpha#1111]\nPUT 1.=2:\n+x\n"})
            )]))],
        );

        let sess = resolve_in(&root, "/tmp/proj", "").expect("resolve");
        let edits = edits_in(&root, &sess, None).unwrap();
        assert!(edits.paths.is_empty(), "agent handle is not a path");
    }

    #[test]
    fn nested_subagent_transcripts_join_the_session() {
        let tmp = Dir::new("nested");
        let root = tmp.0.join("-tmp-proj");
        std::fs::create_dir_all(&root).unwrap();
        let main = write_transcript(
            &root,
            "2026-09-28T10-05-43-142Z_aaa",
            "aaa",
            "/tmp/proj",
            &[message(serde_json::json!([edit_call("src/main.rs")]))],
        );
        let sub = write_subagent(
            &main,
            "BeadleAudit",
            "/tmp/proj",
            &[message(serde_json::json!([edit_call("src/parent.rs")]))],
        );
        let grandchild = write_nested_subagent(
            &main,
            "BeadleAudit",
            "/tmp/proj",
            &[message(serde_json::json!([edit_call("src/grandchild.rs")]))],
        );

        let sess = resolve_in(&root, "/tmp/proj", main.to_str().unwrap()).expect("resolve");
        // Path ordering is by component, so `BeadleAudit/` sorts ahead of
        // `BeadleAudit.jsonl`.
        assert_eq!(sess.sources, vec![main, grandchild, sub]);
        let edits = edits_in(&root, &sess, None).unwrap();
        assert_eq!(
            edits.paths,
            vec![
                PathBuf::from("/tmp/proj/src/main.rs"),
                PathBuf::from("/tmp/proj/src/grandchild.rs"),
                PathBuf::from("/tmp/proj/src/parent.rs"),
            ],
            "a subagent of a subagent still belongs to this session"
        );
    }

    #[test]
    fn non_jsonl_or_outside_root_id_is_refused() {
        let tmp = Dir::new("refuse");
        let root = tmp.0.join("-tmp-proj");
        std::fs::create_dir_all(&root).unwrap();
        // A real session in the pane's cwd that must NOT be adopted instead.
        write_transcript(
            &root,
            "2026-09-28T10-05-43-142Z_aaa",
            "aaa",
            "/tmp/proj",
            &[],
        );
        let notes = root.join("notes.txt");
        std::fs::write(&notes, "not a transcript\n").unwrap();
        let outside_dir = tmp.0.join("elsewhere");
        std::fs::create_dir_all(&outside_dir).unwrap();
        let outside = outside_dir.join("other.jsonl");
        std::fs::write(&outside, "{}\n").unwrap();

        assert!(
            resolve_in(&root, "/tmp/proj", notes.to_str().unwrap()).is_none(),
            "a non-transcript file is not a session"
        );
        assert!(
            resolve_in(&root, "/tmp/proj", outside.to_str().unwrap()).is_none(),
            "a transcript outside this root is not a session"
        );
    }

    #[test]
    fn non_session_record_cannot_rebase() {
        let tmp = Dir::new("rebase");
        let root = tmp.0.join("-tmp-proj");
        std::fs::create_dir_all(&root).unwrap();
        let main = write_transcript(
            &root,
            "2026-09-28T10-05-43-142Z_aaa",
            "aaa",
            "/tmp/proj",
            &[
                message(serde_json::json!([edit_call("src/before.rs")])),
                serde_json::json!({"type":"custom","cwd":"/other"}).to_string(),
                message(serde_json::json!([edit_call("src/after.rs")])),
            ],
        );

        let sess = resolve_in(&root, "/tmp/proj", "").expect("resolve");
        assert_eq!(sess.sources[0], main);
        let edits = edits_in(&root, &sess, None).unwrap();
        assert_eq!(
            edits.paths,
            vec![
                PathBuf::from("/tmp/proj/src/before.rs"),
                PathBuf::from("/tmp/proj/src/after.rs"),
            ]
        );
    }
}
