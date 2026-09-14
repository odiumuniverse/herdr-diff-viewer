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
    let evt = std::env::var("HERDR_PLUGIN_EVENT_JSON").unwrap_or_default();
    let ctx = std::env::var("HERDR_PLUGIN_CONTEXT_JSON").unwrap_or_default();
    let event = std::env::var("HERDR_PLUGIN_EVENT").unwrap_or_default();
    if event == "pane.closed" || event == "pane.exited" {
        if let Some(pane) = ctx::find_str(&evt, "pane_id") {
            state::remove_entry(&pane);
        }
        return 0;
    }
    if event == "tab.closed" {
        if let Some(tab) = ctx::find_str(&evt, "tab_id") {
            state::remove_tab(&tab);
        }
        return 0;
    }
    let pane = ctx::find_str(&evt, "pane_id")
        .or_else(|| ctx::find_str(&ctx, "focused_pane_id"))
        .or_else(|| {
            std::env::var("HERDR_PANE_ID")
                .ok()
                .filter(|p| !p.is_empty())
        });
    let Some(pane) = pane else {
        return 0;
    };
    let Some(info) = herdr_cli::pane_info(&pane) else {
        return 0;
    };
    if info.agent.is_empty() {
        state::remove_entry(&pane);
        return 0;
    }
    let live = state::LivePane {
        pane: info.pane_id.clone(),
        tab: info.tab_id.clone(),
        agent: info.agent.clone(),
        session: info.session.clone(),
    };
    if event != "pane.focused" {
        if let Ok(out) = herdr_cli::run(&["pane", "list"]) {
            state::prune(&to_live(&herdr_cli::parse_panes(&out)));
        }
    }
    let procs = herdr_cli::pane_processes(&pane);
    state::observe(&live, &procs);
    0
}

fn to_live(panes: &[herdr_cli::PaneInfo]) -> Vec<state::LivePane> {
    panes
        .iter()
        .map(|p| state::LivePane {
            pane: p.pane_id.clone(),
            tab: p.tab_id.clone(),
            agent: p.agent.clone(),
            session: p.session.clone(),
        })
        .collect()
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
    let me = env::var("HERDR_PANE_ID")
        .ok()
        .filter(|p| !p.is_empty() && *p != agent);
    let code = tty::run_viewer(&agent, me.clone());
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
    fn track_event_ignores_shell_panes_and_close_events() {
        let _guard = crate::state::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("dv-track-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("HERDR_PLUGIN_STATE_DIR", &dir);
        std::env::remove_var("HERDR_PLUGIN_EVENT");
        std::env::remove_var("HERDR_PLUGIN_CONTEXT_JSON");

        std::env::remove_var("HERDR_PLUGIN_EVENT_JSON");
        std::env::remove_var("HERDR_PANE_ID");
        assert_eq!(track_event(), 0);

        std::env::set_var("HERDR_PLUGIN_EVENT", "pane.closed");
        std::env::set_var("HERDR_PLUGIN_EVENT_JSON", r#"{"pane_id":"w9:p9"}"#);
        assert_eq!(track_event(), 0);
        std::env::set_var("HERDR_PLUGIN_EVENT", "tab.closed");
        std::env::set_var("HERDR_PLUGIN_EVENT_JSON", r#"{"tab_id":"w9:t9"}"#);
        assert_eq!(track_event(), 0);

        std::env::remove_var("HERDR_PLUGIN_STATE_DIR");
        std::env::remove_var("HERDR_PLUGIN_EVENT");
        std::env::remove_var("HERDR_PLUGIN_EVENT_JSON");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
