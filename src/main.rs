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
        Some("themes") => themes(),
        _ => {
            eprintln!("usage: diff-viewer <toggle|viewer|themes>");
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
    let code = tty::run_viewer(&agent, &repo_src, me.clone());
    if code == 0 {
        if let Some(me) = me {
            state::remove(&tab);
            let _ = herdr_cli::close_pane(&me);
        }
    }
    code
}
