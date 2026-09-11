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
}
