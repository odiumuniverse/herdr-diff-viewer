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
    run(&[
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
        &format!("DIFF_AGENT={agent}"),
        "--env",
        &format!("DIFF_REPO={cwd}"),
        "--env",
        &format!("DIFF_TAB={tab}"),
    ])
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
