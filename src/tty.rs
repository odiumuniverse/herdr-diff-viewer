use std::collections::VecDeque;
use std::io::{Read, Write};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use crate::herdr_cli;
use crate::hl::{self, ThemeId};
use crate::model::{self, Snapshot};
use crate::palette::{self, Palette};
use crate::render::{self, Body, Frame, Target};
use crate::screen::Line;
use crate::state;

const TICK: Duration = Duration::from_millis(1000);
const SIZE_POLL: Duration = Duration::from_millis(500);
const SEQ_WAIT: Duration = Duration::from_millis(50);
const WHEEL_IDLE: Duration = Duration::from_millis(250);
const GRAB_GRACE: Duration = Duration::from_millis(400);

pub fn open_ctty() -> Result<std::fs::File, String> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .map_err(|e| format!("no controlling terminal (/dev/tty): {e}"))
}

pub struct Term {
    saved: String,
    tty: std::fs::File,
}

impl Term {
    pub fn enter(tty: std::fs::File) -> Result<Term, String> {
        let saved = stty_on(&tty, &["-g"])?;
        stty_on(&tty, &["-echo", "-icanon", "-isig", "min", "1"])?;
        let mut o = std::io::stdout();
        let _ = o.write_all(
            b"\x1b[?1049h\x1b[?7l\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1006h\x1b[?25l\x1b[H",
        );
        let _ = o.flush();
        Ok(Term { saved, tty })
    }
}

impl Drop for Term {
    fn drop(&mut self) {
        let mut o = std::io::stdout();
        let _ = o
            .write_all(b"\x1b[?1003l\x1b[?1002l\x1b[?1000l\x1b[?1006l\x1b[?7h\x1b[?25h\x1b[?1049l");
        let _ = o.flush();
        if self.saved.split_whitespace().next().is_none() {
            return;
        }
        let args: Vec<&str> = self.saved.split_whitespace().collect();
        let _ = stty_on(&self.tty, &args);
    }
}

fn stty_on(tty: &std::fs::File, args: &[&str]) -> Result<String, String> {
    let stdin = tty.try_clone().map_err(|e| format!("clone tty: {e}"))?;
    let out = std::process::Command::new("stty")
        .args(args)
        .stdin(stdin)
        .output()
        .map_err(|e| format!("spawn stty: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("stty {} failed: {}", args.join(" "), err.trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn size(tty: &std::fs::File) -> (usize, usize) {
    let Ok(out) = stty_on(tty, &["size"]) else {
        return (24, 100);
    };
    let mut it = out.split_whitespace();
    let rows: usize = it.next().and_then(|s| s.parse().ok()).unwrap_or(24);
    let cols: usize = it.next().and_then(|s| s.parse().ok()).unwrap_or(100);
    (rows.max(10), cols.max(40))
}

pub enum Msg {
    Byte(u8),
    InputClosed,
    Snap(Result<Snapshot, String>),
    Size(usize, usize),
    Gone,
}

pub enum Ev {
    Quit,
    Refresh,
    ToggleGroup,
    ThemeMenu,
    Enter,
    Scroll(i16),
    Top,
    Bottom,
    Press(u16, u16),
    Drag(u16, u16),
    Release(u16, u16),
    Move(u16, u16),
    Wheel(i16, u16),
    Esc,
    Unknown,
}

pub fn parse_sgr(body: &str, fin: u8) -> Option<(u8, u16, u16)> {
    let b = body.strip_prefix('<')?;
    let mut it = b.split(';');
    let cb: u8 = it.next()?.parse().ok()?;
    let x: u16 = it.next()?.parse().ok()?;
    let y: u16 = it.next()?.parse().ok()?;
    if fin != b'M' && fin != b'm' {
        return None;
    }
    Some((cb, x, y))
}

pub enum Mouse {
    Press,
    Drag,
    Move,
    Release,
    WheelUp,
    WheelDown,
    Other,
}

pub fn decode_button(cb: u8) -> Mouse {
    if cb & 64 != 0 {
        return if cb & 1 == 0 {
            Mouse::WheelUp
        } else {
            Mouse::WheelDown
        };
    }
    let button = cb & 3;
    if cb & 32 != 0 {
        return match button {
            0 => Mouse::Drag,
            3 => Mouse::Move,
            _ => Mouse::Other,
        };
    }
    match button {
        0 => Mouse::Press,
        3 => Mouse::Release,
        _ => Mouse::Other,
    }
}

fn mouse_ev(cb: u8, fin: u8, row: u16, col: u16) -> Ev {
    if fin == b'm' {
        return if cb & (3 | 64) == 0 {
            Ev::Release(row, col)
        } else {
            Ev::Unknown
        };
    }
    match decode_button(cb) {
        Mouse::Press => Ev::Press(row, col),
        Mouse::Drag => Ev::Drag(row, col),
        Mouse::Move => Ev::Move(row, col),
        Mouse::Release => Ev::Release(row, col),
        Mouse::WheelUp => Ev::Wheel(-3, row),
        Mouse::WheelDown => Ev::Wheel(3, row),
        Mouse::Other => Ev::Unknown,
    }
}

struct Reader {
    rx: Receiver<Msg>,
    stash: VecDeque<Msg>,
    pushback: Vec<u8>,
}

impl Reader {
    fn next(&mut self, timeout: Duration) -> Option<Msg> {
        if let Some(b) = self.pushback.pop() {
            return Some(Msg::Byte(b));
        }
        if let Some(m) = self.stash.pop_front() {
            return Some(m);
        }
        match self.rx.recv_timeout(timeout) {
            Ok(m) => Some(m),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => Some(Msg::InputClosed),
        }
    }

    fn byte(&mut self, timeout: Duration) -> Option<u8> {
        if let Some(b) = self.pushback.pop() {
            return Some(b);
        }
        let deadline = Instant::now() + timeout;
        loop {
            match self
                .rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(Msg::Byte(b)) => return Some(b),
                Ok(other) => self.stash.push_back(other),
                Err(_) => return None,
            }
        }
    }
}

fn parse(b: u8, r: &mut Reader) -> Ev {
    if b != 0x1b {
        return match b {
            b'q' | 0x03 => Ev::Quit,
            b'r' => Ev::Refresh,
            b't' => Ev::ToggleGroup,
            b'T' => Ev::ThemeMenu,
            b'\r' | b'\n' => Ev::Enter,
            b'j' => Ev::Scroll(1),
            b'k' => Ev::Scroll(-1),
            b'g' => Ev::Top,
            b'G' => Ev::Bottom,
            b' ' => Ev::Scroll(20),
            _ => Ev::Unknown,
        };
    }
    let Some(b2) = r.byte(SEQ_WAIT) else {
        return Ev::Esc;
    };
    if b2 != b'[' {
        r.pushback.push(b2);
        return Ev::Esc;
    }
    let mut body = String::new();
    loop {
        let Some(c) = r.byte(SEQ_WAIT) else {
            return Ev::Unknown;
        };
        if (0x40..=0x7E).contains(&c) {
            if body.starts_with('<') {
                return parse_sgr(&body, c)
                    .map(|(cb, x, y)| mouse_ev(cb, c, y.saturating_sub(1), x.saturating_sub(1)))
                    .unwrap_or(Ev::Unknown);
            }
            return match (body.as_str(), c) {
                ("", b'A') => Ev::Scroll(-1),
                ("", b'B') => Ev::Scroll(1),
                ("5", b'~') => Ev::Scroll(-20),
                ("6", b'~') => Ev::Scroll(20),
                _ => Ev::Unknown,
            };
        }
        body.push(c as char);
        if body.len() > 24 {
            return Ev::Unknown;
        }
    }
}

enum Job {
    Focus,
    Text(String),
}

enum Force {
    Refresh,
    Theme(ThemeId),
}

enum Flow {
    Idle,
    Redraw,
    Quit,
}

struct App {
    pal: &'static Palette,
    theme: ThemeId,
    menu: Option<ThemeMenu>,
    tty_size: (usize, usize),
    lay_size: Option<(usize, usize)>,
    jobs: Sender<Job>,
    snap: Snapshot,
    body: Option<(Body, usize, u64)>,
    gen: u64,
    offset: usize,
    list_off: usize,
    show_session: bool,
    hover: Option<Target>,
    anchor: Option<(usize, usize)>,
    cur: Option<(usize, usize)>,
    pending_anchor: Option<(String, u32)>,
    last_active: Option<usize>,
    grab: bool,
    grab_until: Instant,
    refocus_at: Option<Instant>,
}

struct ThemeMenu {
    items: Vec<ThemeId>,
    sel: usize,
    off: usize,
}

impl App {
    fn apply(&mut self, res: Result<Snapshot, String>) {
        let Ok(snap) = res else {
            return;
        };
        if snap.theme != self.theme {
            self.theme = snap.theme;
            self.pal = palette::for_theme(snap.theme);
            self.gen += 1;
        }
        if self.pending_anchor.is_none() {
            self.pending_anchor = self
                .body
                .as_ref()
                .and_then(|b| render::anchor(&b.0, self.offset));
        }
        self.snap = snap;
        self.anchor = None;
        self.cur = None;
        self.gen += 1;
    }

    fn effective(&self) -> (usize, usize) {
        match self.lay_size {
            Some((r, c)) => (self.tty_size.0.min(r), self.tty_size.1.min(c)),
            None => self.tty_size,
        }
    }

    fn draw(&mut self) -> Frame {
        let (h, w) = self.effective();
        let stale = self
            .body
            .as_ref()
            .is_none_or(|b| b.1 != w || b.2 != self.gen);
        if stale {
            let body = render::body(&self.snap, self.show_session, w, self.pal);
            if let Some(o) = self
                .pending_anchor
                .take()
                .and_then(|a| render::resolve_anchor(&body, &a))
            {
                self.offset = o;
            }
            self.body = Some((body, w, self.gen));
        }
        let body = &self.body.as_ref().expect("body built above").0;
        let ents = render::entries(&self.snap, self.show_session);
        let body_h = render::geometry(ents.len(), body, h, 0).body_h;
        self.offset = self.offset.min(render::max_offset(body, body_h));
        let geo = render::geometry(ents.len(), body, h, self.offset);
        if geo.active != self.last_active {
            self.last_active = geo.active;
            if let Some(pos) = geo.active.and_then(|f| render::entry_pos(&ents, f)) {
                if pos < self.list_off {
                    self.list_off = pos;
                } else if pos >= self.list_off + geo.list_h {
                    self.list_off = pos + 1 - geo.list_h;
                }
            }
        }
        self.list_off = self.list_off.min(ents.len().saturating_sub(geo.list_h));
        let frame = render::frame(&render::View {
            snap: &self.snap,
            body,
            show_session: self.show_session,
            width: w,
            height: h,
            offset: self.offset,
            list_off: self.list_off,
            hover: self.hover,
            sel: self.anchor.zip(self.cur),
            pal: self.pal,
            menu: self.menu.as_ref().map(|m| render::MenuView {
                items: &m.items,
                sel: m.sel,
                off: m.off,
                current: self.theme,
            }),
        });
        paint(&frame.lines, self.pal);
        frame
    }

    fn grabbed(&self) -> bool {
        self.grab || Instant::now() < self.grab_until
    }

    fn refocus(&mut self) {
        self.refocus_at = None;
        self.grab = false;
        self.grab_until = Instant::now() + GRAB_GRACE;
        let _ = self.jobs.send(Job::Focus);
    }

    fn forward(&mut self, c: char) {
        let _ = self.jobs.send(Job::Text(c.to_string()));
        if self.refocus_at.is_some() {
            self.refocus();
        }
    }

    fn toggle_group(&mut self) {
        self.pending_anchor = self
            .body
            .as_ref()
            .and_then(|b| render::anchor(&b.0, self.offset));
        self.show_session = !self.show_session;
        self.gen += 1;
    }

    fn scroll(&mut self, d: i16) {
        self.offset = self.offset.saturating_add_signed(d as isize);
    }

    fn handle(&mut self, ev: Ev, frame: Option<&Frame>, force: &Sender<Force>) -> Flow {
        if self.menu.is_some() {
            return self.handle_menu(ev, frame, force);
        }
        let keyish = matches!(
            ev,
            Ev::Quit
                | Ev::Refresh
                | Ev::ToggleGroup
                | Ev::ThemeMenu
                | Ev::Enter
                | Ev::Scroll(_)
                | Ev::Top
                | Ev::Bottom
                | Ev::Esc
        );
        if keyish && self.grabbed() {
            return Flow::Idle;
        }
        match ev {
            Ev::Quit => Flow::Quit,
            Ev::Refresh => {
                let _ = force.send(Force::Refresh);
                Flow::Idle
            }
            Ev::ToggleGroup => {
                self.toggle_group();
                Flow::Redraw
            }
            Ev::ThemeMenu => {
                self.open_menu();
                Flow::Redraw
            }
            Ev::Enter => Flow::Idle,
            Ev::Scroll(d) => {
                self.scroll(d);
                Flow::Redraw
            }
            Ev::Top => {
                self.offset = 0;
                Flow::Redraw
            }
            Ev::Bottom => {
                self.offset = usize::MAX;
                Flow::Redraw
            }
            Ev::Esc => {
                self.anchor = None;
                self.cur = None;
                Flow::Redraw
            }
            Ev::Press(row, col) => self.press(frame, row as usize, col as usize, force),
            Ev::Drag(row, col) => match frame {
                Some(f) if self.anchor.is_some() => {
                    self.cur = Some((f.body_row_clamped(row as usize), col as usize));
                    Flow::Redraw
                }
                _ => Flow::Idle,
            },
            Ev::Release(row, col) => {
                self.release(frame, row as usize, col as usize);
                Flow::Redraw
            }
            Ev::Move(row, col) => {
                let h = frame.and_then(|f| f.hit(row as usize, col as usize));
                if h == self.hover {
                    Flow::Idle
                } else {
                    self.hover = h;
                    Flow::Redraw
                }
            }
            Ev::Wheel(d, row) => {
                self.grab = true;
                self.refocus_at = Some(Instant::now() + WHEEL_IDLE);
                if frame.is_some_and(|f| f.in_list(row as usize)) {
                    self.list_off = self.list_off.saturating_add_signed(d as isize);
                } else {
                    self.scroll(d);
                }
                Flow::Redraw
            }
            Ev::Unknown => Flow::Idle,
        }
    }

    fn press(
        &mut self,
        frame: Option<&Frame>,
        row: usize,
        col: usize,
        force: &Sender<Force>,
    ) -> Flow {
        self.grab = true;
        let Some(f) = frame else {
            self.menu = None;
            return Flow::Idle;
        };
        match f.hit(row, col) {
            Some(Target::Close) => return Flow::Quit,
            Some(Target::ThemeBtn) => {
                if self.menu.is_some() {
                    self.menu = None;
                } else {
                    self.open_menu();
                }
                return Flow::Redraw;
            }
            Some(Target::Theme(i)) => {
                let id = self.menu.as_ref().and_then(|m| m.items.get(i).copied());
                self.menu = None;
                if let Some(id) = id {
                    self.apply_theme(id, force);
                }
                return Flow::Redraw;
            }
            Some(Target::Group) => {
                if self.menu.is_some() {
                    self.menu = None;
                    return Flow::Redraw;
                }
                self.toggle_group();
                return Flow::Redraw;
            }
            Some(Target::File(i)) => {
                if self.menu.is_some() {
                    self.menu = None;
                    return Flow::Redraw;
                }
                if let Some(start) = self.body.as_ref().map(|b| b.0.start_of(i)) {
                    self.offset = start;
                }
                return Flow::Redraw;
            }
            None => {
                if self.menu.is_some() {
                    self.menu = None;
                    return Flow::Redraw;
                }
            }
        }
        if let Some(r) = f.body_row(row) {
            self.anchor = Some((r, col));
            self.cur = Some((r, col));
        }
        Flow::Redraw
    }

    fn release(&mut self, frame: Option<&Frame>, row: usize, col: usize) {
        if let (Some(a), Some(f), Some(b)) = (self.anchor.take(), frame, self.body.as_ref()) {
            let end = (f.body_row_clamped(row), col);
            if let Some(p) = render::selection_payload(&b.0, a, end) {
                let _ = self.jobs.send(Job::Text(p));
            }
        }
        self.cur = None;
        self.refocus();
    }

    fn handle_menu(&mut self, ev: Ev, frame: Option<&Frame>, force: &Sender<Force>) -> Flow {
        match ev {
            Ev::Quit => Flow::Quit,
            Ev::Esc | Ev::ThemeMenu => {
                self.menu = None;
                Flow::Redraw
            }
            Ev::Enter => {
                self.select_menu(force);
                Flow::Redraw
            }
            Ev::Scroll(d) => {
                self.menu_move(d as isize);
                Flow::Redraw
            }
            Ev::Top => {
                self.menu_move_to(0);
                Flow::Redraw
            }
            Ev::Bottom => {
                self.menu_move_to(usize::MAX);
                Flow::Redraw
            }
            Ev::Press(row, col) => self.press(frame, row as usize, col as usize, force),
            Ev::Release(..) => Flow::Redraw,
            Ev::Move(row, col) => {
                let h = frame.and_then(|f| f.hit(row as usize, col as usize));
                if h == self.hover {
                    Flow::Idle
                } else {
                    self.hover = h;
                    Flow::Redraw
                }
            }
            Ev::Wheel(d, _) => {
                self.menu_move(d as isize);
                Flow::Redraw
            }
            _ => {
                self.menu = None;
                Flow::Redraw
            }
        }
    }

    fn open_menu(&mut self) {
        let items: Vec<ThemeId> = ThemeId::all()
            .into_iter()
            .filter(|t| t.is_dark() == self.theme.is_dark())
            .collect();
        let sel = items.iter().position(|t| *t == self.theme).unwrap_or(0);
        self.menu = Some(ThemeMenu { items, sel, off: 0 });
        let vis = self.menu_visible().max(1);
        if let Some(m) = &mut self.menu {
            m.off = sel.saturating_sub(vis.saturating_sub(1));
        }
    }

    fn menu_visible(&self) -> usize {
        match &self.body {
            Some(b) => {
                let ents = render::entries(&self.snap, self.show_session);
                render::geometry(ents.len(), &b.0, self.effective().0, self.offset)
                    .body_h
                    .saturating_sub(2)
            }
            None => 8,
        }
    }

    fn menu_move(&mut self, d: isize) {
        let (len, sel) = match &self.menu {
            Some(m) => (m.items.len(), m.sel),
            None => return,
        };
        self.menu_move_to(sel.saturating_add_signed(d).min(len.saturating_sub(1)));
    }

    fn menu_move_to(&mut self, sel: usize) {
        let vis = self.menu_visible().max(1);
        let Some(m) = &mut self.menu else { return };
        m.sel = sel.min(m.items.len().saturating_sub(1));
        if m.sel < m.off {
            m.off = m.sel;
        } else if m.sel >= m.off + vis {
            m.off = m.sel + 1 - vis;
        }
    }

    fn select_menu(&mut self, force: &Sender<Force>) {
        let id = self.menu.as_ref().and_then(|m| m.items.get(m.sel).copied());
        self.menu = None;
        if let Some(id) = id {
            self.apply_theme(id, force);
        }
    }

    fn apply_theme(&mut self, id: ThemeId, force: &Sender<Force>) {
        let _ = state::save_theme(id.name());
        self.pending_anchor = self
            .body
            .as_ref()
            .and_then(|b| render::anchor(&b.0, self.offset));
        self.theme = id;
        self.pal = palette::for_theme(id);
        self.gen += 1;
        let _ = force.send(Force::Theme(id));
    }
}

fn paint(lines: &[Line], pal: &Palette) {
    let mut s = String::from("\x1b[?2026h");
    let default_bg = pal.transparent.then_some(pal.bg);
    for (i, l) in lines.iter().enumerate() {
        s.push_str(&format!("\x1b[{};1H", i + 1));
        l.encode(&mut s, default_bg);
    }
    s.push_str("\x1b[?2026l");
    let mut o = std::io::stdout();
    let _ = o.write_all(s.as_bytes());
    let _ = o.flush();
}

fn spawn_input(mut tty: std::fs::File, tx: Sender<Msg>) {
    std::thread::spawn(move || {
        let mut buf = [0u8; 1024];
        loop {
            match tty.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    for &b in &buf[..n] {
                        if tx.send(Msg::Byte(b)).is_err() {
                            return;
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        let _ = tx.send(Msg::InputClosed);
    });
}

fn spawn_worker(agent: String, me: Option<String>, _tx: Sender<Msg>) -> Sender<Job> {
    let (jobs, rx) = mpsc::channel::<Job>();
    std::thread::spawn(move || {
        for job in rx {
            match job {
                Job::Focus => herdr_cli::focus_agent(&agent, me.as_deref()),
                Job::Text(t) => {
                    let _ = herdr_cli::send_text(&agent, &t);
                }
            }
        }
    });
    jobs
}

const RESCAN_EVERY: u32 = 10;

fn spawn_refresher(
    agent: String,
    theme: ThemeId,
    tx: Sender<Msg>,
    force: Receiver<Force>,
    tab: String,
    me: Option<String>,
) {
    std::thread::spawn(move || {
        let mut theme_id = theme;
        let mut theme = hl::theme(theme);
        let mut last: Option<u64> = None;
        let mut misses = 0;
        let mut forced = true;
        let mut ticks: u32 = 0;
        let mut scope_repos: Vec<String> = Vec::new();
        loop {
            ticks += 1;
            let mut scope_dirty = forced;
            match herdr_cli::run(&["pane", "list"]) {
                Ok(out) => {
                    let live: Vec<state::LivePane> = herdr_cli::parse_panes(&out)
                        .into_iter()
                        .map(|p| state::LivePane {
                            pane: p.pane_id,
                            tab: p.tab_id,
                            agent: p.agent,
                            session: p.session,
                        })
                        .collect();
                    if live.iter().any(|l| l.pane == agent) {
                        misses = 0;
                    } else {
                        misses += 1;
                        if misses >= 2 {
                            let _ = tx.send(Msg::Gone);
                            return;
                        }
                    }
                    if misses == 0 {
                        state::prune(&live);
                        for l in live.iter().filter(|l| l.tab == tab && !l.agent.is_empty()) {
                            let procs = herdr_cli::pane_processes(&l.pane);
                            state::observe(l, &procs);
                        }
                        let u = state::union_for_tab(&tab, Some(&live));
                        if u != scope_repos {
                            scope_repos = u;
                            scope_dirty = true;
                        }
                    }
                }
                Err(_) => {
                    misses += 1;
                    if misses >= 2 {
                        let _ = tx.send(Msg::Gone);
                        return;
                    }
                }
            }
            if scope_dirty || ticks.is_multiple_of(RESCAN_EVERY) {
                let scope = model::assemble(&scope_repos);
                let sig = model::signature(&scope.repos);
                if forced || last != Some(sig) {
                    last = Some(sig);
                    let res = model::build(&scope, &theme);
                    if tx.send(Msg::Snap(res)).is_err() {
                        return;
                    }
                }
            }
            if let Some(id) = state::load_theme().and_then(|n| hl::named(&n)) {
                if id != theme_id {
                    theme_id = id;
                    theme = hl::theme(id);
                    last = None;
                }
            }
            if ticks.is_multiple_of(2) {
                if let Some(me) = &me {
                    if let Ok(out) = herdr_cli::run(&["pane", "layout", "--pane", me]) {
                        if let Some((rows, cols)) = herdr_cli::pane_rect(&out, me) {
                            let _ = tx.send(Msg::Size(rows, cols));
                        }
                    }
                }
            }
            forced = match force.recv_timeout(TICK) {
                Ok(Force::Refresh) => true,
                Ok(Force::Theme(id)) => {
                    if id != theme_id {
                        theme_id = id;
                        theme = hl::theme(id);
                        last = None;
                    }
                    true
                }
                Err(RecvTimeoutError::Timeout) => false,
                Err(RecvTimeoutError::Disconnected) => return,
            };
        }
    });
}

pub fn run_viewer(agent: &str, me: Option<String>, tab: &str) -> i32 {
    let live: Option<Vec<state::LivePane>> = herdr_cli::run(&["pane", "list"]).ok().map(|out| {
        herdr_cli::parse_panes(&out)
            .into_iter()
            .map(|p| state::LivePane {
                pane: p.pane_id,
                tab: p.tab_id,
                agent: p.agent,
                session: p.session,
            })
            .collect()
    });
    if let Some(live) = &live {
        state::prune(live);
    }
    let scope = model::assemble(&state::union_for_tab(tab, live.as_deref()));
    let tty = match open_ctty() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("diff-viewer: {e}");
            return 1;
        }
    };
    let theme = hl::resolve_ctty(&tty);
    let snap = model::build(&scope, &theme).unwrap_or_else(|_| Snapshot {
        main: Vec::new(),
        session: Vec::new(),
        files: 0,
        adds: 0,
        dels: 0,
        repos: scope.repos.len(),
        theme: theme.id,
    });
    let lay = me.as_ref().and_then(|me| {
        herdr_cli::run(&["pane", "layout", "--pane", me])
            .ok()
            .and_then(|out| herdr_cli::pane_rect(&out, me))
    });
    let term = match Term::enter(tty) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("diff-viewer: {e}");
            return 1;
        }
    };
    let input = match term.tty.try_clone() {
        Ok(t) => t,
        Err(e) => {
            drop(term);
            eprintln!("diff-viewer: clone tty: {e}");
            return 1;
        }
    };
    let (tx, rx) = mpsc::channel();
    spawn_input(input, tx.clone());
    let (force_tx, force_rx) = mpsc::channel();
    let jobs = spawn_worker(agent.to_string(), me.clone(), tx.clone());
    spawn_refresher(
        agent.to_string(),
        theme.id,
        tx,
        force_rx,
        tab.to_string(),
        me,
    );

    let mut r = Reader {
        rx,
        stash: VecDeque::new(),
        pushback: Vec::new(),
    };
    let mut app = App {
        pal: palette::for_theme(theme.id),
        theme: theme.id,
        menu: None,
        tty_size: size(&term.tty),
        lay_size: lay,
        jobs,
        snap,
        body: None,
        gen: 0,
        offset: 0,
        list_off: 0,
        show_session: false,
        hover: None,
        anchor: None,
        cur: None,
        pending_anchor: None,
        last_active: None,
        grab: false,
        grab_until: Instant::now(),
        refocus_at: None,
    };
    let mut frame: Option<Frame> = None;
    let mut dirty = true;
    let mut size_checked = Instant::now();
    loop {
        if dirty {
            frame = Some(app.draw());
            dirty = false;
        }
        let wait = app
            .refocus_at
            .map_or(SIZE_POLL, |t| t.saturating_duration_since(Instant::now()))
            .min(SIZE_POLL);
        let msg = r.next(wait);
        if size_checked.elapsed() >= SIZE_POLL {
            size_checked = Instant::now();
            let s = size(&term.tty);
            if s != app.tty_size {
                let before = app.effective();
                app.tty_size = s;
                if app.effective() != before {
                    dirty = true;
                }
            }
        }
        if app.refocus_at.is_some_and(|t| Instant::now() >= t) {
            app.refocus();
        }
        match msg {
            None => {}
            Some(Msg::InputClosed) | Some(Msg::Gone) => return 0,
            Some(Msg::Snap(res)) => {
                app.apply(res);
                dirty = true;
            }
            Some(Msg::Size(rows, cols)) => {
                let before = app.effective();
                app.lay_size = Some((rows.max(10), cols.max(40)));
                if app.effective() != before {
                    dirty = true;
                }
            }
            Some(Msg::Byte(b)) => {
                if b != 0x1b && app.grabbed() {
                    if (0x20..0x7f).contains(&b) {
                        app.forward(b as char);
                    }
                    continue;
                }
                let ev = parse(b, &mut r);
                match app.handle(ev, frame.as_ref(), &force_tx) {
                    Flow::Quit => return 0,
                    Flow::Redraw => dirty = true,
                    Flow::Idle => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reader(bytes: &[u8]) -> Reader {
        let (_tx, rx) = mpsc::channel();
        Reader {
            rx,
            stash: VecDeque::new(),
            pushback: bytes.iter().rev().copied().collect(),
        }
    }

    fn ev(bytes: &[u8]) -> Ev {
        let mut r = reader(bytes);
        let b = r.byte(SEQ_WAIT).unwrap();
        parse(b, &mut r)
    }

    #[test]
    fn sgr_press_drag_move_release_wheel_decode() {
        assert!(matches!(decode_button(0), Mouse::Press));
        assert!(matches!(decode_button(32), Mouse::Drag));
        assert!(matches!(decode_button(35), Mouse::Move));
        assert!(matches!(decode_button(3), Mouse::Release));
        assert!(matches!(decode_button(64), Mouse::WheelUp));
        assert!(matches!(decode_button(65), Mouse::WheelDown));
        assert!(matches!(decode_button(2), Mouse::Other));
    }

    #[test]
    fn sgr_m_final_is_release_even_with_zero_button() {
        assert!(matches!(ev(b"\x1b[<0;30;14m"), Ev::Release(13, 29)));
    }

    #[test]
    fn motion_and_wheel_carry_position() {
        assert!(matches!(ev(b"\x1b[<35;5;3M"), Ev::Move(2, 4)));
        assert!(matches!(ev(b"\x1b[<65;5;3M"), Ev::Wheel(3, 2)));
    }

    #[test]
    fn lone_escape_and_keys() {
        assert!(matches!(ev(b"\x1b"), Ev::Esc));
        assert!(matches!(ev(b"q"), Ev::Quit));
        assert!(matches!(ev(b"T"), Ev::ThemeMenu));
        assert!(matches!(ev(b"\r"), Ev::Enter));
        assert!(matches!(ev(b"\x1b[6~"), Ev::Scroll(20)));
    }

    #[test]
    fn sgr_parse_rejects_non_mouse_finals() {
        assert_eq!(parse_sgr("<0;10;20", b'M').unwrap(), (0, 10, 20));
        assert!(parse_sgr("<0;10;20", b'A').is_none());
        assert!(parse_sgr("0;10;20", b'M').is_none());
    }

    fn test_app() -> (App, Sender<Force>, Receiver<Force>) {
        let (jobs, _dropped) = mpsc::channel();
        let (force_tx, force_rx) = mpsc::channel();
        let app = App {
            pal: palette::for_dark(true),
            theme: ThemeId::ClaudeDark,
            menu: None,
            tty_size: (30, 100),
            lay_size: None,
            jobs,
            snap: Snapshot {
                main: Vec::new(),
                session: Vec::new(),
                files: 0,
                adds: 0,
                dels: 0,
                repos: 0,
                theme: ThemeId::ClaudeDark,
            },
            body: None,
            gen: 0,
            offset: 0,
            list_off: 0,
            show_session: false,
            hover: None,
            anchor: None,
            cur: None,
            pending_anchor: None,
            last_active: None,
            grab: false,
            grab_until: Instant::now(),
            refocus_at: None,
        };
        (app, force_tx, force_rx)
    }

    #[test]
    fn theme_menu_select_applies_persists_and_notifies() {
        let dir = std::env::temp_dir().join(format!("dv-menu-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("DIFF_VIEWER_CONFIG_DIR", &dir);
        let (mut app, force_tx, force_rx) = test_app();
        app.open_menu();
        let n = app.menu.as_ref().unwrap().items.len();
        assert!(n > 1);
        assert!(app.menu.as_ref().unwrap().items.iter().all(|t| t.is_dark()));
        app.menu_move(1000);
        assert_eq!(app.menu.as_ref().unwrap().sel, n - 1);
        app.menu_move(-1000);
        assert_eq!(app.menu.as_ref().unwrap().sel, 0);
        app.menu_move(1);
        app.select_menu(&force_tx);
        assert!(app.menu.is_none());
        assert_eq!(app.theme, ThemeId::ViewerDark);
        assert_eq!(app.pal.bg, crate::palette::DARK.bg);
        assert_eq!(crate::state::load_theme().as_deref(), Some("diff-viewer"));
        assert!(matches!(
            force_rx.try_recv(),
            Ok(Force::Theme(ThemeId::ViewerDark))
        ));
        crate::state::clear_theme();
        assert!(crate::state::load_theme().is_none());
        std::env::remove_var("DIFF_VIEWER_CONFIG_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn effective_size_is_min_of_tty_and_layout() {
        let (mut app, _, _) = test_app();
        assert_eq!(app.effective(), (30, 100));
        app.lay_size = Some((58, 94));
        assert_eq!(app.effective(), (30, 94));
        app.lay_size = Some((20, 200));
        assert_eq!(app.effective(), (20, 100));
    }
}
