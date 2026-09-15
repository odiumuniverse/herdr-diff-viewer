use std::process::Command;

use crate::ctx::find_str;

pub const PLUGIN_ID: &str = "odiumuniverse.diff-viewer";

pub fn herdr_bin() -> String {
    std::env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".to_string())
}

pub fn run(args: &[&str]) -> Result<String, String> {
    let out = Command::new(herdr_bin())
        .args(args)
        .output()
        .map_err(|e| format!("spawn herdr: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    if out.status.success() {
        Ok(text)
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(format!("herdr {} failed: {}", args.join(" "), err.trim()))
    }
}

pub fn pane_id_after(out: &str, marker: &str) -> Option<String> {
    let i = out.find(marker)?;
    let rest = &out[i + marker.len()..];
    let j = rest.find("\"pane_id\"")?;
    find_str(&rest[j..], "pane_id")
}

pub fn open_viewer(agent: &str, cwd: &str, tab: &str) -> Result<String, String> {
    let agent_env = format!("DIFF_AGENT={agent}");
    let tab_env = format!("DIFF_TAB={tab}");
    let theme_env = std::env::var("DIFF_THEME")
        .ok()
        .filter(|t| !t.is_empty())
        .map(|t| format!("DIFF_THEME={t}"));
    let mut args = vec![
        "plugin",
        "pane",
        "open",
        "--plugin",
        PLUGIN_ID,
        "--entrypoint",
        "viewer",
        "--placement",
        "split",
        "--target-pane",
        agent,
        "--direction",
        "right",
        "--cwd",
        cwd,
        "--no-focus",
        "--env",
        agent_env.as_str(),
        "--env",
        tab_env.as_str(),
    ];
    if let Some(t) = &theme_env {
        args.push("--env");
        args.push(t.as_str());
    }
    run(&args)
}

pub fn close_pane(pane: &str) -> Result<(), String> {
    run(&["pane", "close", pane]).map(|_| ())
}

pub fn sync_layout(pane: &str) {
    let _ = run(&[
        "pane",
        "resize",
        "--direction",
        "left",
        "--pane",
        pane,
        "--amount",
        "0",
    ]);
}

pub fn pane_alive(pane: &str) -> bool {
    run(&["pane", "get", pane]).is_ok()
}

pub struct PaneInfo {
    pub pane_id: String,
    pub tab_id: String,
    pub agent: String,
    pub session: String,
    pub cwd: Option<String>,
}

pub fn pick_cwd(json: &str) -> Option<String> {
    find_str(json, "foreground_cwd")
        .filter(|s| !s.is_empty())
        .or_else(|| find_str(json, "cwd").filter(|s| !s.is_empty()))
}

fn split_array_objects(out: &str, key: &str) -> Vec<String> {
    let mut objs = Vec::new();
    let pat = format!("\"{key}\"");
    let Some(k) = out.find(&pat) else {
        return objs;
    };
    let rest = &out[k + pat.len()..];
    let Some(rel) = rest.find('[') else {
        return objs;
    };
    let body = &rest[rel..];
    let bytes = body.as_bytes();
    let mut arr = 0usize;
    let mut depth = 0usize;
    let mut start: Option<usize> = None;
    let mut in_str = false;
    let mut esc = false;
    for (i, &b) in bytes.iter().enumerate() {
        if in_str {
            if esc {
                esc = false;
            } else if b == b'\\' {
                esc = true;
            } else if b == b'"' {
                in_str = false;
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'[' => arr += 1,
            b']' => {
                arr = arr.saturating_sub(1);
                if arr == 0 {
                    break;
                }
            }
            b'{' => {
                if depth == 0 {
                    start = Some(i);
                }
                depth += 1;
            }
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    if let Some(s) = start.take() {
                        objs.push(body[s..=i].to_string());
                    }
                }
            }
            _ => {}
        }
    }
    objs
}

fn balanced_obj(s: &str) -> Option<String> {
    let b = s.find('{')?;
    let bytes = s.as_bytes();
    let mut depth = 0usize;
    let mut in_str = false;
    let mut esc = false;
    let mut i = b;
    while i < bytes.len() {
        let c = bytes[i];
        if in_str {
            if esc {
                esc = false;
            } else if c == b'\\' {
                esc = true;
            } else if c == b'"' {
                in_str = false;
            }
        } else {
            match c {
                b'"' => in_str = true,
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(s[b..=i].to_string());
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

fn agent_session_value(obj: &str) -> String {
    let Some(i) = obj.find("\"agent_session\"") else {
        return String::new();
    };
    let rest =
        obj[i + "\"agent_session\"".len()..].trim_start_matches([' ', '\t', '\n', '\r', ':']);
    if rest.starts_with('n') {
        return String::new();
    }
    let Some(sub) = balanced_obj(rest) else {
        return String::new();
    };
    find_str(&sub, "value").unwrap_or_default()
}

pub fn parse_panes(out: &str) -> Vec<PaneInfo> {
    split_array_objects(out, "panes")
        .into_iter()
        .filter_map(|o| {
            Some(PaneInfo {
                pane_id: find_str(&o, "pane_id")?,
                tab_id: find_str(&o, "tab_id").unwrap_or_default(),
                agent: find_str(&o, "agent").unwrap_or_default(),
                session: agent_session_value(&o),
                cwd: pick_cwd(&o),
            })
        })
        .collect()
}

pub fn parse_procs(out: &str) -> Vec<(u32, String)> {
    let mut v = Vec::new();
    for o in split_array_objects(out, "foreground_processes") {
        let (Some(pid), Some(cwd)) = (num_field(&o, "\"pid\""), find_str(&o, "cwd")) else {
            continue;
        };
        if cwd.is_empty() {
            continue;
        }
        v.push((pid as u32, cwd));
    }
    v.sort();
    v.dedup();
    v
}

pub fn parse_proc_group(out: &str) -> Option<u32> {
    num_field(out, "\"foreground_process_group_id\"").map(|v| v as u32)
}

pub fn pane_procs(pane: &str) -> (Option<u32>, Vec<(u32, String)>) {
    match run(&["pane", "process-info", "--pane", pane]) {
        Ok(out) => (parse_proc_group(&out), parse_procs(&out)),
        Err(_) => (None, Vec::new()),
    }
}

pub fn pane_rect(out: &str, pane: &str) -> Option<(usize, usize)> {
    let marker = format!("\"pane_id\":\"{pane}\"");
    let rest = out.get(out.find(&marker)?..)?;
    let rr = rest.get(rest.find("\"rect\"")?..)?;
    Some((num_field(rr, "\"height\"")?, num_field(rr, "\"width\"")?))
}

fn num_field(s: &str, key: &str) -> Option<usize> {
    let after = s.get(s.find(key)? + key.len()..)?;
    let digits: String = after
        .trim_start_matches([' ', ':'])
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

pub fn send_text(pane: &str, text: &str) -> Result<(), String> {
    run(&["pane", "send-text", pane, text]).map(|_| ())
}

pub fn focus_agent(agent: &str, viewer: Option<&str>) {
    if run(&["agent", "focus", agent]).is_ok() {
        return;
    }
    if let Some(v) = viewer {
        let _ = run(&["pane", "focus", "--direction", "left", "--pane", v]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_new_pane_id_after_marker() {
        let out = r#"{"result":{"anchor":{"pane_id":"w4:p2"},"plugin_pane":{"pane":{"pane_id":"w4:p9"}}}}"#;
        assert_eq!(
            pane_id_after(out, "\"plugin_pane\"").as_deref(),
            Some("w4:p9")
        );
    }

    #[test]
    fn unknown_marker_is_none() {
        assert_eq!(pane_id_after(r#"{"a":1}"#, "\"plugin_pane\""), None);
    }

    #[test]
    fn pane_rect_reads_own_rect_after_pane_id() {
        let out = r#"{"result":{"layout":{"area":{"height":58,"width":188},"panes":[{"focused":true,"pane_id":"w4:p1","rect":{"height":58,"width":94,"x":0,"y":0}},{"focused":false,"pane_id":"w4:p2E","rect":{"height":58,"width":94,"x":94,"y":0}}]}}}"#;
        assert_eq!(pane_rect(out, "w4:p2E"), Some((58, 94)));
        assert_eq!(pane_rect(out, "w4:p1"), Some((58, 94)));
        assert_eq!(pane_rect(out, "w4:nope"), None);
        assert_eq!(pane_rect(r#"{"pane_id":"w4:p2E"}"#, "w4:p2E"), None);
    }

    #[test]
    fn pane_list_parses_agent_session_and_cwd() {
        let out = r#"{"result":{"panes":[
            {"agent":"opencode","agent_session":{"agent":"opencode","kind":"id","source":"herdr:opencode","value":"ses_1"},"cwd":"/Users/u","foreground_cwd":"/Users/u/my/dv","pane_id":"w4:p1","tab_id":"w4:t1"},
            {"agent":"claude","agent_session":null,"cwd":"/Users/u/topscan","pane_id":"w9:p2","tab_id":"w9:t2"},
            {"cwd":"/Users/u/my","pane_id":"w4:pK","tab_id":"w4:t1"},
            {"agent":"","pane_id":"w4:pX","tab_id":"w4:t1"}
        ]},"type":"pane_list"}"#;
        let panes = parse_panes(out);
        assert_eq!(panes.len(), 4);
        assert_eq!(panes[0].pane_id, "w4:p1");
        assert_eq!(panes[0].tab_id, "w4:t1");
        assert_eq!(panes[0].agent, "opencode");
        assert_eq!(panes[0].session, "ses_1");
        assert_eq!(panes[1].session, "");
        assert_eq!(panes[2].agent, "");
        assert!(parse_panes("garbage").is_empty());
    }

    #[test]
    fn process_pairs_carry_pid_and_skip_empty() {
        let out = r#"{"result":{"process_info":{
            "foreground_process_group_id":33795,
            "foreground_processes":[
                {"argv":["opencode"],"cwd":"/Users/u/my/reword-tui","name":"opencode.exe","pid":33795},
                {"argv":["go","test"],"cwd":"/Users/u/my/agents-sync","name":"go","pid":44101},
                {"argv":["go","test"],"cwd":"/Users/u/my/agents-sync","name":"go","pid":44101},
                {"argv":["sh"],"cwd":"","name":"sh","pid":44102}
            ]}}}"#;
        assert_eq!(
            parse_procs(out),
            vec![
                (33795, "/Users/u/my/reword-tui".to_string()),
                (44101, "/Users/u/my/agents-sync".to_string()),
            ]
        );
        assert_eq!(parse_proc_group(out), Some(33795));
        assert!(parse_procs("garbage").is_empty());
    }
}
