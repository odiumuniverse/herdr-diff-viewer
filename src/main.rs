mod ctx;
mod git;
mod group;
mod herdr_cli;
mod hl;
mod journal;
mod model;
mod palette;
mod procs;
mod render;
mod screen;
mod state;
mod tty;

use std::env;

fn main() {
    let code = match env::args().nth(1).as_deref() {
        Some("toggle") => toggle(),
        Some("viewer") => viewer(),
        Some("theme") => theme_cmd(),
        Some("themes") => themes(),
        Some("journals") => journals(),
        _ => {
            eprintln!("usage: diff-viewer <toggle|viewer|theme [name]|themes|journals AGENT [CWD] [SESSION]>");
            2
        }
    };
    std::process::exit(code);
}

fn journals() -> i32 {
    let agent = env::args().nth(2).unwrap_or_default();
    let cwd = env::args().nth(3).unwrap_or_default();
    let session = env::args().nth(4).unwrap_or_default();
    let Some(adapter) = journal::adapter_for(&agent) else {
        eprintln!("no journal adapter for agent '{agent}'");
        return 1;
    };
    let Some(sess) = adapter.resolve(&cwd, &session) else {
        eprintln!("no session resolved (cwd='{cwd}' session='{session}')");
        return 1;
    };
    println!("id: {}", sess.id);
    println!("cwd: {:?}", sess.cwd);
    for src in &sess.sources {
        println!("source: {}", src.display());
    }
    match adapter.edits(&sess, None) {
        Ok(e) => {
            println!("paths: {}", e.paths.len());
            for p in e.paths.iter().take(20) {
                println!("  {}", p.display());
            }
            0
        }
        Err(err) => {
            eprintln!("edits error: {err}");
            1
        }
    }
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
