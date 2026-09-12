use std::process::Command;

use crate::ctx::{find_all_str, find_str};

pub const PLUGIN_ID: &str = "odiumuniverse.diff-viewer";

pub fn run(args: &[&str]) -> Result<String, String> {
    let out = Command::new("herdr")
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
    let repo_env = format!("DIFF_REPO={cwd}");
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
        repo_env.as_str(),
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

pub fn pane_cwd(pane: &str) -> Option<String> {
    run(&["pane", "get", pane])
        .ok()
        .and_then(|out| pick_cwd(&out))
}

pub fn pick_cwd(json: &str) -> Option<String> {
    find_str(json, "foreground_cwd")
        .filter(|s| !s.is_empty())
        .or_else(|| find_str(json, "cwd").filter(|s| !s.is_empty()))
}

pub struct TabPane {
    pub pane_id: String,
    pub cwd: Option<String>,
}

/// Agent panes of one tab. Best-effort: empty vec when the shape is unknown.
pub fn tab_agent_panes(tab: &str) -> Option<Vec<TabPane>> {
    let out = run(&["pane", "list"]).ok()?;
    Some(parse_pane_list(&out, tab))
}

fn parse_pane_list(out: &str, tab: &str) -> Vec<TabPane> {
    let mut panes = Vec::new();
    let Some(rel) = out
        .find("\"panes\"")
        .and_then(|i| out[i..].find('[').map(|r| i + r))
    else {
        return panes;
    };
    let body = &out[rel..];
    let bytes = body.as_bytes();
    let mut depth = 0usize;
    let mut start = None;
    let mut in_str = false;
    let mut esc = false;
    for (k, &b) in bytes.iter().enumerate() {
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
            // `]` at brace-depth 0 closes the panes array — stop, the
            // trailing envelope must not leak in as phantom panes.
            b']' if depth == 0 && k > 0 => break,
            b'{' => {
                if depth == 0 {
                    start = Some(k);
                }
                depth += 1;
            }
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    if let Some(s) = start.take() {
                        let obj = &body[s..=k];
                        if find_str(obj, "tab_id").as_deref() == Some(tab)
                            && find_str(obj, "agent").is_some_and(|a| !a.is_empty())
                        {
                            if let Some(id) = find_str(obj, "pane_id") {
                                panes.push(TabPane {
                                    pane_id: id,
                                    cwd: pick_cwd(obj),
                                });
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    panes
}

/// Every cwd reported for a pane's process tree. Agent panes often sit at `~`
/// while their children (language servers, the agent session itself) run
/// inside the real repo — those cwds are how such repos get discovered.
pub fn pane_cwds(pane: &str) -> Vec<String> {
    run(&["pane", "process-info", "--pane", pane])
        .map(|out| extract_cwds(&out))
        .unwrap_or_default()
}

fn extract_cwds(out: &str) -> Vec<String> {
    let mut v: Vec<String> = find_all_str(out, "cwd")
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect();
    v.sort();
    v.dedup();
    v
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
    fn foreground_cwd_wins_over_pane_cwd() {
        let out =
            r#"{"result":{"pane":{"cwd":"/Users/u/my","foreground_cwd":"/Users/u/topscan"}}}"#;
        assert_eq!(pick_cwd(out).as_deref(), Some("/Users/u/topscan"));
    }

    #[test]
    fn pane_cwd_is_fallback_when_foreground_missing() {
        let out = r#"{"result":{"pane":{"cwd":"/Users/u/my"}}}"#;
        assert_eq!(pick_cwd(out).as_deref(), Some("/Users/u/my"));
    }

    #[test]
    fn empty_cwds_are_missing() {
        assert_eq!(pick_cwd(r#"{"cwd":"","foreground_cwd":""}"#), None);
        assert_eq!(pick_cwd(r#"{"n":1}"#), None);
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
    fn pane_list_keeps_only_tab_agents() {
        let out = r#"{"result":{"panes":[
            {"agent":"opencode","cwd":"/Users/u","foreground_cwd":"/Users/u","pane_id":"w4:p1","tab_id":"w4:t1"},
            {"cwd":"/Users/u/my","pane_id":"w4:pK","tab_id":"w4:t1"},
            {"agent":"","pane_id":"w4:pX","tab_id":"w4:t1"},
            {"agent":"opencode","cwd":"/Users/u/my/dv","pane_id":"w4:p3D","tab_id":"w4:t9"}
        ]},"type":"pane_list"}"#;
        let t1 = parse_pane_list(out, "w4:t1");
        assert_eq!(t1.len(), 1);
        assert_eq!(t1[0].pane_id, "w4:p1");
        assert_eq!(t1[0].cwd.as_deref(), Some("/Users/u"));
        let t9 = parse_pane_list(out, "w4:t9");
        assert_eq!(t9.len(), 1);
        assert_eq!(t9[0].pane_id, "w4:p3D");
        assert!(parse_pane_list(out, "w4:nope").is_empty());
        assert!(parse_pane_list("garbage", "w4:t1").is_empty());
        assert!(parse_pane_list(r#"{"result":{"panes":[]}}"#, "w4:t1").is_empty());
    }

    #[test]
    fn process_cwds_dedup_and_skip_empty() {
        let out = r#"{"result":{"process_info":{
            "foreground_processes":[
                {"argv":["opencode"],"cwd":"/Users/u","name":"opencode"},
                {"argv":["rust-analyzer"],"cwd":"/Users/u/my/reword-tui/cli","name":"rust-analyzer"},
                {"argv":["rust-analyzer"],"cwd":"/Users/u/my/reword-tui/cli","name":"rust-analyzer"},
                {"argv":["sh"],"cwd":"","name":"sh"}
            ]}}}"#;
        assert_eq!(
            extract_cwds(out),
            vec!["/Users/u", "/Users/u/my/reword-tui/cli"]
        );
        assert!(extract_cwds("garbage").is_empty());
    }
}
