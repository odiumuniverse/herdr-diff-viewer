use std::collections::{HashMap, HashSet};

pub struct SessionRef {
    pub agent: String,
    pub id: String,
    pub cwd: String,
}

pub fn resolve(agent_pane: &str) -> Option<SessionRef> {
    let out = crate::herdr_cli::run(&["agent", "get", agent_pane]).ok()?;
    let i = out.find("\"agent_session\"")?;
    let rest = &out[i..];
    let agent = scoped(rest, "agent")?;
    let id = scoped(rest, "value")?;
    let cwd = crate::ctx::find_str(&out, "cwd").unwrap_or_default();
    if id.is_empty() {
        return None;
    }
    Some(SessionRef { agent, id, cwd })
}

fn scoped(json: &str, key: &str) -> Option<String> {
    let i = json.find(&format!("\"{key}\""))?;
    crate::ctx::find_str(&json[i..], key)
}

fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

pub struct SessCache {
    db: Option<(u64, u64, Vec<String>)>,
    logs: HashMap<String, (u64, u64, Vec<String>)>,
}

impl SessCache {
    pub fn new() -> Self {
        SessCache {
            db: None,
            logs: HashMap::new(),
        }
    }
}

fn sig(path: &str) -> Option<(u64, u64)> {
    let m = std::fs::metadata(path).ok()?;
    let t = m
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some((m.len(), t))
}

pub fn edited_files_cached(s: &SessionRef, cache: &mut SessCache) -> Vec<String> {
    match s.agent.as_str() {
        "opencode" => {
            let home = std::env::var("HOME").unwrap_or_default();
            let db = format!("{home}/.local/share/opencode/opencode.db");
            if !safe_id(&s.id) || !std::path::Path::new(&db).exists() {
                return Vec::new();
            }
            if let Some(sg) = sig(&db) {
                if let Some((z, t, v)) = &cache.db {
                    if (*z, *t) == sg {
                        return v.clone();
                    }
                }
                let v = mine_opencode_db(&db, &s.id);
                cache.db = Some((sg.0, sg.1, v.clone()));
                return v;
            }
            mine_opencode_db(&db, &s.id)
        }
        "claude" => mine_logs_cached(s, Claude, cache),
        "pi" => mine_logs_cached(s, Pi, cache),
        _ => Vec::new(),
    }
}

fn mine_logs_cached(s: &SessionRef, dialect: Lines, cache: &mut SessCache) -> Vec<String> {
    let mut set = HashSet::new();
    for path in candidate_logs(dialect_agent(&dialect), &s.id, &s.cwd) {
        let entry = cache.logs.get(&path).and_then(|(z, t, v)| {
            sig(&path).and_then(|sg| {
                if (*z, *t) == sg {
                    Some(v.clone())
                } else {
                    None
                }
            })
        });
        let paths = match entry {
            Some(v) => v,
            None => {
                let v = mine_log_file(&path, dialect);
                if let Some(sg) = sig(&path) {
                    cache.logs.insert(path.clone(), (sg.0, sg.1, v.clone()));
                }
                v
            }
        };
        let had = !paths.is_empty();
        set.extend(paths);
        if had {
            break;
        }
    }
    let mut v: Vec<String> = set.into_iter().collect();
    v.sort();
    v
}

fn mine_log_file(path: &str, dialect: Lines) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut set = HashSet::new();
    for line in text.lines() {
        if let Some(p) = parse_jsonl_line(line, dialect) {
            set.insert(p);
        }
    }
    let mut v: Vec<String> = set.into_iter().collect();
    v.sort();
    v
}

fn mine_opencode_db(db: &str, id: &str) -> Vec<String> {
    let sql = format!(
        "SELECT data FROM part WHERE session_id='{id}' AND (data LIKE '%\"tool\":\"write\"%' OR data LIKE '%\"tool\":\"edit\"%')"
    );
    let Ok(out) = std::process::Command::new("sqlite3")
        .args([db, &sql])
        .output()
    else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut set = HashSet::new();
    for line in text.lines() {
        if let Some(p) = parse_opencode_row(line) {
            set.insert(p);
        }
    }
    let mut v: Vec<String> = set.into_iter().collect();
    v.sort();
    v
}

pub fn parse_opencode_row(doc: &str) -> Option<String> {
    let is_edit = doc.contains("\"tool\":\"write\"") || doc.contains("\"tool\":\"edit\"");
    if !is_edit {
        return None;
    }
    crate::ctx::find_str(doc, "filePath")
}

#[derive(Clone, Copy)]
pub(crate) enum Lines {
    Claude,
    Pi,
}

use Lines::{Claude, Pi};

pub fn parse_jsonl_line(line: &str, dialect: Lines) -> Option<String> {
    let find = crate::ctx::find_str;
    match dialect {
        Claude => {
            if !(line.contains("\"type\":\"tool_use\"") || line.contains("\"type\": \"tool_use\""))
            {
                return None;
            }
            match find(line, "name").as_deref() {
                Some("Write") | Some("Edit") | Some("MultiEdit") | Some("NotebookEdit") => {
                    find(line, "file_path").or_else(|| find(line, "path"))
                }
                _ => None,
            }
        }
        Pi => {
            if !(line.contains("\"type\":\"toolCall\"") || line.contains("\"type\": \"toolCall\""))
            {
                return None;
            }
            match find(line, "name").as_deref() {
                Some("write") | Some("edit") => {
                    find(line, "path").or_else(|| find(line, "file_path"))
                }
                _ => None,
            }
        }
    }
}

fn dialect_agent(d: &Lines) -> &'static str {
    match d {
        Claude => "claude",
        Pi => "pi",
    }
}

fn candidate_logs(agent: &str, id: &str, cwd: &str) -> Vec<String> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut out = Vec::new();
    if agent == "claude" {
        let slug = format!("-{}", cwd.replace('/', "-"));
        out.push(format!("{home}/.claude/projects/{slug}/{id}.jsonl"));
        if let Ok(dirs) = std::fs::read_dir(format!("{home}/.claude/projects")) {
            for e in dirs.flatten() {
                out.push(format!("{}/{id}.jsonl", e.path().display()));
            }
        }
    } else if agent == "pi" {
        if let Ok(dirs) = std::fs::read_dir(format!("{home}/.pi/agent/sessions")) {
            for e in dirs.flatten() {
                if !e.path().is_dir() {
                    continue;
                }
                if let Ok(files) = std::fs::read_dir(e.path()) {
                    for f in files.flatten() {
                        let n = f.file_name().to_string_lossy().into_owned();
                        if n.ends_with(".jsonl") && n.contains(id) {
                            out.push(f.path().display().to_string());
                        }
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opencode_row_routes_by_tool() {
        let w = r#"{"type":"tool","tool":"write","state":{"input":{"filePath":"/r/a.rs"}}}"#;
        let e = r#"{"type":"tool","tool":"edit","state":{"input":{"filePath":"/r/b.rs"}}}"#;
        let r = r#"{"type":"tool","tool":"read","state":{"input":{"filePath":"/r/c.rs"}}}"#;
        let s = r#"{"type":"step-finish","tokens":{}}"#;
        assert_eq!(parse_opencode_row(w).as_deref(), Some("/r/a.rs"));
        assert_eq!(parse_opencode_row(e).as_deref(), Some("/r/b.rs"));
        assert_eq!(parse_opencode_row(r), None);
        assert_eq!(parse_opencode_row(s), None);
    }

    #[test]
    fn claude_line_routes_by_tool_use() {
        let w = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Write","input":{"file_path":"/r/a.rs"}}]}}"#;
        let m = r#"{"message":{"content":[{"type":"tool_use","name":"MultiEdit","input":{"file_path":"/r/b.rs"}}]}}"#;
        let r = r#"{"message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"/r/c.rs"}}]}}"#;
        let t = r#"{"message":{"content":[{"type":"text","text":"hi"}]}}"#;
        assert_eq!(parse_jsonl_line(w, Claude).as_deref(), Some("/r/a.rs"));
        assert_eq!(parse_jsonl_line(m, Claude).as_deref(), Some("/r/b.rs"));
        assert_eq!(parse_jsonl_line(r, Claude), None);
        assert_eq!(parse_jsonl_line(t, Claude), None);
    }

    #[test]
    fn pi_line_routes_by_tool_call() {
        let w = r#"{"type":"message","message":{"content":[{"type":"toolCall","name":"write","arguments":{"path":"/r/a.rs"}}]}}"#;
        let b = r#"{"type":"message","message":{"content":[{"type":"toolCall","name":"bash","arguments":{"cmd":"ls"}}]}}"#;
        assert_eq!(parse_jsonl_line(w, Pi).as_deref(), Some("/r/a.rs"));
        assert_eq!(parse_jsonl_line(b, Pi), None);
    }

    #[test]
    fn unsafe_ids_never_reach_sqlite() {
        assert!(!safe_id("'; DROP TABLE part; --"));
        assert!(safe_id("ses_f6f18499bffeN5rz39a07j9POO"));
    }
}
