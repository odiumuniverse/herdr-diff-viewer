mod ctx;
mod git;
mod group;
mod herdr_cli;
mod hl;
mod model;
mod palette;
mod render;
mod screen;
mod state;
mod tty;

use std::env;

fn main() {
    let code = match env::args().nth(1).as_deref() {
        Some("toggle") => toggle(),
        Some("viewer") => viewer(),
        Some("track-event") => track_event(),
        Some("theme") => theme_cmd(),
        Some("themes") => themes(),
        _ => {
            eprintln!("usage: diff-viewer <toggle|viewer|track-event|theme [name]|themes>");
            2
        }
    };
    std::process::exit(code);
}

fn toggle() -> i32 {
    let ctx = match ctx::read_action_ctx() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("diff-viewer: {e}");
            return 1;
        }
    };
    let Some(_lock) = state::lock(&ctx.tab) else {
        return 0;
    };
    if let Some(st) = state::load(&ctx.tab) {
        if herdr_cli::pane_alive(&st.viewer_pane) {
            let _ = herdr_cli::close_pane(&st.viewer_pane);
            herdr_cli::sync_layout(&ctx.agent_pane);
            state::remove(&ctx.tab);
            return 0;
        }
        state::remove(&ctx.tab);
    }
    let out = match herdr_cli::open_viewer(&ctx.agent_pane, &ctx.cwd, &ctx.tab) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("diff-viewer: {e}");
            return 1;
        }
    };
    let Some(id) = herdr_cli::pane_id_after(&out, "\"plugin_pane\"") else {
        eprintln!("diff-viewer: could not find new pane id in: {}", out.trim());
        return 1;
    };
    herdr_cli::sync_layout(&id);
    let _ = state::save(
        &ctx.tab,
        &state::ToggleState {
            viewer_pane: id,
            agent_pane: ctx.agent_pane,
            repo: ctx.cwd,
        },
    );
    0
}

fn track_event() -> i32 {
    if std::env::var("DIFF_TRACK").is_ok_and(|v| v == "0") {
        return 0;
    }
    let ctx = std::env::var("HERDR_PLUGIN_CONTEXT_JSON").unwrap_or_default();
    if !ctx.is_empty() {
        let agent = ctx::find_str(&ctx, "focused_pane_agent").unwrap_or_default();
        if !agent.is_empty() {
            if let (Some(tab), Some(cwd)) = (
                ctx::find_str(&ctx, "tab_id"),
                ctx::find_str(&ctx, "focused_pane_cwd"),
            ) {
                if !cwd.is_empty() {
                    state::note_touched(&tab, &cwd);
                    return 0;
                }
            }
        }
    }
    let evt = std::env::var("HERDR_PLUGIN_EVENT_JSON").unwrap_or_default();
    let Some(pane) = ctx::find_str(&evt, "pane_id") else {
        return 0;
    };
    let Ok(out) = herdr_cli::run(&["pane", "get", &pane]) else {
        return 0;
    };
    if ctx::find_str(&out, "agent").is_none() {
        return 0;
    }
    if let (Some(tab), Some(cwd)) = (ctx::find_str(&out, "tab_id"), herdr_cli::pick_cwd(&out)) {
        state::note_touched(&tab, &cwd);
    }
    0
}

fn theme_cmd() -> i32 {
    match env::args().nth(2) {
        None => {
            match state::load_theme() {
                Some(n) => println!("{n}"),
                None => println!("(auto)"),
            }
            0
        }
        Some(name) => {
            if name == "auto" {
                state::clear_theme();
                return 0;
            }
            let ok = matches!(name.as_str(), "dark" | "light")
                || hl::ThemeId::from_name(&name).is_some();
            if !ok {
                eprintln!("unknown theme: {name}");
                return 1;
            }
            match state::save_theme(&name) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("diff-viewer: {e}");
                    1
                }
            }
        }
    }
}

fn themes() -> i32 {
    for id in hl::ThemeId::all() {
        println!(
            "{}\t{}",
            id.name(),
            if id.is_dark() { "dark" } else { "light" }
        );
    }
    0
}

fn viewer() -> i32 {
    let tab = env::var("DIFF_TAB")
        .or_else(|_| env::var("HERDR_TAB_ID"))
        .unwrap_or_default();
    let agent = match env::var("DIFF_AGENT") {
        Ok(a) if !a.is_empty() => a,
        _ => {
            eprintln!("diff-viewer: DIFF_AGENT is not set — open via the toggle action");
            return 1;
        }
    };
    let repo_arg = env::var("DIFF_REPO").unwrap_or_default();
    let repo_src = if repo_arg.is_empty() {
        state::load(&tab).map(|s| s.repo).unwrap_or_default()
    } else {
        repo_arg
    };
    if repo_src.is_empty() {
        eprintln!("diff-viewer: no repo — open via the toggle action from an agent pane");
        return 1;
    }
    if !std::path::Path::new(&repo_src).is_dir() {
        eprintln!("diff-viewer: {repo_src} is gone — reopen via the toggle action");
        return 1;
    }
    let me = env::var("HERDR_PANE_ID")
        .ok()
        .filter(|p| !p.is_empty() && *p != agent);
    let code = tty::run_viewer(&agent, &repo_src, me.clone(), &tab);
    if code == 0 {
        if let Some(me) = me {
            state::remove(&tab);
            let _ = herdr_cli::close_pane(&me);
            herdr_cli::sync_layout(&agent);
        }
    }
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_event_records_focused_agent_cwd_and_ignores_shells() {
        let _guard = crate::state::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("dv-track-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("HERDR_PLUGIN_STATE_DIR", &dir);

        std::env::set_var(
            "HERDR_PLUGIN_CONTEXT_JSON",
            r#"{"tab_id":"w9:t9","focused_pane_cwd":"/Users/u/topscan","focused_pane_agent":"opencode"}"#,
        );
        std::env::remove_var("HERDR_PLUGIN_EVENT_JSON");
        assert_eq!(track_event(), 0);
        assert_eq!(state::load_touched("w9:t9"), vec!["/Users/u/topscan"]);

        std::env::set_var(
            "HERDR_PLUGIN_CONTEXT_JSON",
            r#"{"tab_id":"w9:t9","focused_pane_cwd":"/tmp","focused_pane_agent":""}"#,
        );
        assert_eq!(track_event(), 0);
        assert_eq!(state::load_touched("w9:t9"), vec!["/Users/u/topscan"]);

        std::env::remove_var("HERDR_PLUGIN_STATE_DIR");
        std::env::remove_var("HERDR_PLUGIN_CONTEXT_JSON");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
