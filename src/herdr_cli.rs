use std::process::Command;

use crate::ctx::find_str;

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

pub fn pane_alive(pane: &str) -> bool {
    run(&["pane", "get", pane]).is_ok()
}

pub fn pane_cwd(pane: &str) -> Option<String> {
    run(&["pane", "get", pane]).ok().and_then(|out| pick_cwd(&out))
}

pub fn pick_cwd(json: &str) -> Option<String> {
    find_str(json, "foreground_cwd")
        .filter(|s| !s.is_empty())
        .or_else(|| find_str(json, "cwd").filter(|s| !s.is_empty()))
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
        let out = r#"{"result":{"pane":{"cwd":"/Users/u/my","foreground_cwd":"/Users/u/topscan"}}}"#;
        assert_eq!(
            pick_cwd(out).as_deref(),
            Some("/Users/u/topscan")
        );
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
}
