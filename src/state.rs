use std::fs;
use std::path::PathBuf;

use crate::ctx::find_str;

pub struct ToggleState {
    pub viewer_pane: String,
    pub agent_pane: String,
    pub repo: String,
}

fn state_path(tab: &str) -> PathBuf {
    let safe: String = tab
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    std::env::temp_dir().join(format!("diff-viewer-{safe}.json"))
}

pub fn save(tab: &str, st: &ToggleState) -> Result<(), String> {
    let body = format!(
        "{{\"viewer_pane\":\"{}\",\"agent_pane\":\"{}\",\"repo\":\"{}\"}}",
        esc(&st.viewer_pane),
        esc(&st.agent_pane),
        esc(&st.repo)
    );
    fs::write(state_path(tab), body).map_err(|e| format!("write state: {e}"))
}

pub fn load(tab: &str) -> Option<ToggleState> {
    let body = fs::read_to_string(state_path(tab)).ok()?;
    Some(ToggleState {
        viewer_pane: find_str(&body, "viewer_pane")?,
        agent_pane: find_str(&body, "agent_pane")?,
        repo: find_str(&body, "repo")?,
    })
}

pub fn remove(tab: &str) {
    let _ = fs::remove_file(state_path(tab));
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let tab = "test_tab_roundtrip_xyz";
        let st = ToggleState {
            viewer_pane: "w1:p9".into(),
            agent_pane: "w1:p2".into(),
            repo: "/tmp/repo".into(),
        };
        save(tab, &st).unwrap();
        let back = load(tab).expect("state must load");
        assert_eq!(back.viewer_pane, "w1:p9");
        assert_eq!(back.agent_pane, "w1:p2");
        assert_eq!(back.repo, "/tmp/repo");
        remove(tab);
        assert!(load(tab).is_none());
    }
}
