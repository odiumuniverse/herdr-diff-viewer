use std::path::{Path, PathBuf};

pub mod aider;
pub mod antigravity;
pub mod claude;
pub mod codex;
pub mod cursor;
pub mod gemini;
pub mod opencode;
pub mod pi;
pub mod qwen;

#[derive(Clone)]
pub struct SessionRef {
    pub id: String,
    pub cwd: Option<String>,
    pub sources: Vec<PathBuf>,
    pub gated: bool,
}

#[derive(Clone)]
pub struct Edits {
    pub paths: Vec<PathBuf>,
    /// Gated paths (shell workdirs): a repo is adopted only once its working
    /// tree changes, so a repo the session merely reads stays out.
    pub candidates: Vec<PathBuf>,
    pub cursor: String,
}

pub trait Adapter: Sync {
    fn resolve(&self, pane_cwd: &str, session_id: &str) -> Option<SessionRef>;
    fn edits(&self, sess: &SessionRef, cursor: Option<&str>) -> Result<Edits, String>;

    /// True when every session of the agent runs behind one shared daemon
    /// (opencode/kilo). The pane's process tree then spans unrelated sessions,
    /// so the process fallback must not walk it.
    fn shares_process_tree(&self) -> bool {
        false
    }
}

pub fn adapter_for(agent: &str) -> Option<&'static dyn Adapter> {
    match agent {
        "claude" => Some(&claude::Claude),
        "opencode" => Some(&opencode::Opencode),
        "kilo" => Some(&opencode::Kilo),
        "codex" => Some(&codex::Codex),
        "gemini" => Some(&gemini::Gemini),
        "qwen" => Some(&qwen::Qwen),
        "pi" => Some(&pi::Pi),
        "omp" => Some(&pi::Omp),
        "cursor" => Some(&cursor::Cursor),
        "aider" => Some(&aider::Aider),
        "antigravity" | "antigravity-cli" => Some(&antigravity::Antigravity),
        _ => None,
    }
}

pub(crate) fn root() -> PathBuf {
    if let Ok(x) = std::env::var("DIFF_JOURNAL_ROOT") {
        if !x.is_empty() {
            return PathBuf::from(x);
        }
    }
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".to_string()))
}

pub(crate) fn data_dir(app: &str) -> PathBuf {
    if let Ok(x) = std::env::var("XDG_DATA_HOME") {
        if !x.is_empty() {
            return PathBuf::from(x).join(app);
        }
    }
    root().join(".local/share").join(app)
}

pub(crate) fn newest_file(dir: &Path, suffix: &str) -> Option<PathBuf> {
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    let entries = std::fs::read_dir(dir).ok()?;
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if !p.is_file() || !name.ends_with(suffix) {
            continue;
        }
        let Some(mt) = e.metadata().ok().and_then(|m| m.modified().ok()) else {
            continue;
        };
        if best.as_ref().is_none_or(|(bm, _)| mt > *bm) {
            best = Some((mt, p));
        }
    }
    best.map(|(_, p)| p)
}

pub(crate) fn read_jsonl_since(
    path: &Path,
    offset: u64,
) -> Result<(Vec<serde_json::Value>, u64), String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let len = f.metadata().map_err(|e| e.to_string())?.len();
    let mut pos = if offset > len { 0 } else { offset };
    f.seek(SeekFrom::Start(pos)).map_err(|e| e.to_string())?;
    let mut buf = String::new();
    f.read_to_string(&mut buf).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    let mut consumed = 0usize;
    for line in buf.split_inclusive('\n') {
        if !line.ends_with('\n') {
            break;
        }
        let text = line.trim_end();
        let parsed = serde_json::from_str::<serde_json::Value>(text)
            .ok()
            .or_else(|| {
                let start = text.find('{')?;
                serde_json::from_str::<serde_json::Value>(&text[start..]).ok()
            });
        if let Some(v) = parsed {
            out.push(v);
        }
        consumed += line.len();
    }
    pos += consumed as u64;
    Ok((out, pos))
}

pub(crate) fn patch_paths(text: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        for prefix in [
            "*** Add File: ",
            "*** Update File: ",
            "*** Delete File: ",
            "*** Move to: ",
        ] {
            if let Some(p) = t.strip_prefix(prefix) {
                let p = p.trim();
                if !p.is_empty() {
                    out.push(PathBuf::from(p));
                }
            }
        }
    }
    out
}

pub(crate) fn absolutize(paths: Vec<PathBuf>, base: Option<&str>) -> Vec<PathBuf> {
    let base = base.map(PathBuf::from);
    paths
        .into_iter()
        .map(|p| {
            if p.is_absolute() {
                p
            } else if let Some(b) = &base {
                b.join(&p)
            } else {
                p
            }
        })
        .collect()
}
