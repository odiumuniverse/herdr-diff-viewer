use std::io::{Read, Write};

use crate::herdr_cli;
use crate::model::{self, Snapshot};
use crate::render::{self, GUTTER_W};
use crate::session::SessionRef;

/// Raw terminal + SGR mouse owner. Drop restores everything best-effort
/// (panic unwinds through here too, so the pane never stays raw).
pub struct Term {
    saved: String,
}

impl Term {
    pub fn enter() -> Result<Term, String> {
        let saved = stty(&["-g"])?;
        stty(&["-echo", "-icanon", "min", "1"])?;
        let mut o = std::io::stdout();
        let _ = o.write_all(b"\x1b[?1049h\x1b[?1000h\x1b[?1006h\x1b[?25l\x1b[H");
        let _ = o.flush();
        Ok(Term { saved })
    }
}

impl Drop for Term {
    fn drop(&mut self) {
        let mut o = std::io::stdout();
        let _ = o.write_all(b"\x1b[?1000l\x1b[?1006l\x1b[?25h\x1b[?1049l");
        let _ = o.flush();
        // stty wants one flag per argv (like `stty $(stty -g)`), not the
        // whole -g blob as a single arg — otherwise restore fails silently.
        let args: Vec<&str> = self.saved.split_whitespace().collect();
        let _ = stty(&args);
    }
}

fn stty(args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("stty")
        .args(args)
        .output()
        .map_err(|e| format!("stty failed: {e}"))?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn size() -> (u16, u16) {
    // Given a pane tty When asking its size Then rows/cols or a sane fallback.
    let Ok(out) = stty(&["size"]) else {
        return (24, 100);
    };
    let mut it = out.split_whitespace();
    let rows = it.next().and_then(|s| s.parse().ok()).unwrap_or(24);
    let cols = it.next().and_then(|s| s.parse().ok()).unwrap_or(100);
    (rows.max(10), cols.max(40))
}

pub enum Ev {
    Quit,
    Refresh,
    ToggleGroup,
    Scroll(i16),
    Top,
    Bottom,
    Click(u16, u16),
    Drag(u16, u16),
    Release(u16, u16),
    Esc,
    Unknown,
}

/// `(button, x, y)` from an SGR body (`<Cb;Cx;Cy`) + final byte.
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
    Release,
    WheelUp,
    WheelDown,
}

/// Pure SGR button decode: bit 6 = wheel (direction by bit 0, so
/// shift+wheel 68/69 still resolves), low 2 bits 3 = release,
/// bit 5 (32) = drag, else press.
pub fn decode_button(cb: u8) -> Mouse {
    if cb & 64 != 0 {
        if cb & 1 == 0 {
            Mouse::WheelUp
        } else {
            Mouse::WheelDown
        }
    } else if cb & 3 == 3 {
        Mouse::Release
    } else if cb & 32 != 0 {
        Mouse::Drag
    } else {
        Mouse::Press
    }
}

struct Reader<'a> {
    stdin: std::io::StdinLock<'a>,
    pushback: Vec<u8>,
}

impl Reader<'_> {
    /// None only on EOF — the caller quits instead of hot-spinning.
    fn byte(&mut self) -> Option<u8> {
        if let Some(b) = self.pushback.pop() {
            return Some(b);
        }
        let mut buf = [0u8; 1];
        loop {
            match self.stdin.read_exact(&mut buf) {
                Ok(()) => return Some(buf[0]),
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return None,
                Err(_) => continue,
            }
        }
    }
}

fn next_ev(r: &mut Reader) -> Ev {
    let Some(b) = r.byte() else {
        return Ev::Quit;
    };
    if b != 0x1b {
        return match b {
            b'q' | 0x03 => Ev::Quit,
            b'r' => Ev::Refresh,
            b't' => Ev::ToggleGroup,
            b'j' => Ev::Scroll(1),
            b'k' => Ev::Scroll(-1),
            b'g' => Ev::Top,
            b'G' => Ev::Bottom,
            b' ' => Ev::Scroll(20),
            _ => Ev::Unknown,
        };
    }
    let Some(b2) = r.byte() else {
        return Ev::Quit;
    };
    if b2 != b'[' {
        r.pushback.push(b2);
        return Ev::Esc;
    }
    let mut body = String::new();
    loop {
        let Some(c) = r.byte() else {
            return Ev::Quit;
        };
        if (0x40..=0x7E).contains(&c) {
            if body.starts_with('<') {
                if let Some((cb, x, y)) = parse_sgr(&body, c) {
                    let (row, col) = (y.saturating_sub(1), x.saturating_sub(1));
                    // `m` is always a release (terminals vary the button code:
                    // xterm sends 3, others echo 0) — never decode it as press.
                    if c == b'm' {
                        return Ev::Release(row, col);
                    }
                    return match decode_button(cb) {
                        Mouse::Press => Ev::Click(row, col),
                        Mouse::Drag => Ev::Drag(row, col),
                        Mouse::Release => Ev::Release(row, col),
                        Mouse::WheelUp => Ev::Scroll(-3),
                        Mouse::WheelDown => Ev::Scroll(3),
                    };
                }
                return Ev::Unknown;
            }
            return match c {
                b'A' => Ev::Scroll(-1),
                b'B' => Ev::Scroll(1),
                _ => Ev::Unknown,
            };
        }
        body.push(c as char);
        if body.len() > 24 {
            return Ev::Unknown;
        }
    }
}

struct App {
    agent: String,
    root: String,
    session: Option<SessionRef>,
    sess_cache: crate::session::SessCache,
    snap: Snapshot,
    offset: usize,
    show_session: bool,
    anchor: Option<(u32, u16)>,
    cur: Option<(u32, u16)>,
    msg: String,
    /// Render cache: (body, hit-maps, width, snapshot generation). Scrolling
    /// re-slices it; only rebuild/resize re-render.
    frame: Option<(String, render::ClickMap, usize, u64)>,
    gen: u64,
}

impl App {
    fn rebuild(&mut self) {
        match model::detect(&self.root).and_then(|s| model::build(&s)) {
            Ok(mut snap) => {
                let note = self.apply_session_filter(&mut snap);
                self.msg = if note.is_empty() {
                    format!("{} files +{} -{}", snap.files, snap.adds, snap.dels)
                } else {
                    format!(
                        "{} files +{} -{} · {note}",
                        snap.files, snap.adds, snap.dels
                    )
                };
                self.snap = snap;
                self.gen += 1;
            }
            Err(e) => self.msg = e,
        }
    }

    /// Narrow the snapshot to this session's footprint. Empty mined set
    /// shows everything (a fresh session has no edits yet) with a note.
    /// Transcript reads are (size, mtime)-cached in SessCache.
    fn apply_session_filter(&mut self, snap: &mut Snapshot) -> String {
        let Some(s) = &self.session else {
            return "no session — showing all changes".to_string();
        };
        let touched = crate::session::edited_files_cached(s, &mut self.sess_cache);
        if touched.is_empty() {
            return "session has no mined edits yet — showing all".to_string();
        }
        model::retain_session(snap, &touched);
        let short: String = s.id.chars().take(12).collect();
        format!("session {} {short}: {} files", s.agent, snap.files)
    }

    fn max_offset(&self, lines: usize, height: usize) -> usize {
        lines.saturating_sub(height)
    }

    /// Release the drag: rows between anchor and cursor become a
    /// `file:line` payload. Pure — sending stays with the caller so the
    /// frame borrow never fights `&mut self`.
    fn selection_payload(map: &render::ClickMap, a: (u32, u16), c: (u32, u16)) -> Option<String> {
        let (r1, c1, r2, c2) = if (a.0, a.1) <= (c.0, c.1) {
            (a.0, a.1, c.0, c.1)
        } else {
            (c.0, c.1, a.0, a.1)
        };
        if r1 == r2 && c1 == c2 {
            return None; // plain click, not a selection
        }
        let mut payload = String::new();
        let mut last_file = String::new();
        let mut picked = 0usize;
        for cr in &map.content {
            if cr.row < r1 || cr.row > r2 {
                continue;
            }
            let (lo, hi) = if r1 == r2 {
                (c1 as usize, c2 as usize)
            } else if cr.row == r1 {
                (c1 as usize, usize::MAX)
            } else if cr.row == r2 {
                (0, c2 as usize)
            } else {
                (0, usize::MAX)
            };
            let total = cr.text.chars().count();
            let from = lo.saturating_sub(GUTTER_W).min(total);
            let to = hi.saturating_sub(GUTTER_W).min(total);
            if from >= to {
                continue;
            }
            let frag: String = cr.text.chars().skip(from).take(to - from).collect();
            if frag.trim().is_empty() {
                continue;
            }
            if cr.file != last_file {
                last_file = cr.file.clone();
                payload.push_str(&format!("{}:{}\n", cr.file, cr.lineno));
            }
            payload.push_str(&frag);
            payload.push('\n');
            picked += 1;
        }
        if picked == 0 {
            return None;
        }
        payload.truncate(payload.trim_end().len());
        Some(payload)
    }
}

/// Main viewer loop. The scope gate runs BEFORE raw mode so git errors
/// print cleanly; the snapshot itself builds exactly once via rebuild().
pub fn run_viewer(agent: &str, root: &str) -> i32 {
    if let Err(e) = model::detect(root) {
        eprintln!("diff-viewer: {e}");
        return 1;
    }
    let mut app = App {
        agent: agent.to_string(),
        root: root.to_string(),
        session: crate::session::resolve(agent),
        sess_cache: crate::session::SessCache::new(),
        snap: Snapshot {
            main: Vec::new(),
            session: Vec::new(),
            files: 0,
            adds: 0,
            dels: 0,
        },
        offset: 0,
        show_session: false,
        anchor: None,
        cur: None,
        msg: String::new(),
        frame: None,
        gen: 0,
    };
    app.rebuild(); // single first build: filter + counts, no raw screen yet
    let _term = match Term::enter() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("diff-viewer: {e}");
            return 1;
        }
    };
    // The lock borrows a local Stdin that outlives the loop — no 'static hack.
    let stdin = std::io::stdin();
    let mut r = Reader {
        stdin: stdin.lock(),
        pushback: Vec::new(),
    };
    loop {
        let (rows, cols) = size();
        let height = rows as usize - 1;
        let width = cols as usize;
        let stale = app
            .frame
            .as_ref()
            .is_none_or(|f| f.2 != width || f.3 != app.gen);
        if stale {
            let (body, map) = render::render(&app.snap, app.show_session, width);
            app.frame = Some((body, map, width, app.gen));
        }
        let frame = app.frame.as_ref().expect("frame just rendered");
        let lines: Vec<&str> = frame.0.lines().collect();
        app.offset = app.offset.min(app.max_offset(lines.len(), height));
        // Screen coords -> content rows: the map is full-body indexed.
        let max_row = lines.len().saturating_sub(1) as u32;
        let abs = |r: u16| (r as u32 + app.offset as u32).min(max_row);
        let mut o = std::io::stdout();
        let _ = o.write_all(b"\x1b[H\x1b[J");
        for line in lines.iter().skip(app.offset).take(height) {
            let _ = o.write_all(line.as_bytes());
            let _ = o.write_all(b"\r\n");
        }
        let footer = format!(
            "\x1b[2m{}\x1b[0m",
            truncate(
                &format!(
                    "{} · click jump, drag send, t tests, r refresh, q quit",
                    app.msg
                ),
                width
            )
        );
        let _ = o.write_all(footer.as_bytes());
        let _ = o.flush();

        match next_ev(&mut r) {
            Ev::Quit => break,
            Ev::Refresh => app.rebuild(),
            Ev::ToggleGroup => {
                // No rebuild: render() already branches on the flag.
                app.show_session = !app.show_session;
                app.gen += 1;
            }
            Ev::Scroll(d) => {
                app.offset = app.offset.saturating_add_signed(d as isize);
            }
            Ev::Top => app.offset = 0,
            Ev::Bottom => app.offset = usize::MAX,
            Ev::Esc => {
                app.anchor = None;
                app.cur = None;
            }
            Ev::Click(row, col) => {
                let row = abs(row);
                let hit_group = app.frame.as_ref().and_then(|f| f.1.group_row);
                if hit_group == Some(row) {
                    app.show_session = !app.show_session;
                    app.gen += 1;
                    continue;
                }
                let jump = app.frame.as_ref().and_then(|f| {
                    f.1.file_rows
                        .iter()
                        .find(|(fr, _)| *fr == row)
                        .and_then(|(_, path)| {
                            f.1.section_rows
                                .iter()
                                .find(|(p, _)| p == path)
                                .map(|(_, sec)| *sec as usize)
                        })
                });
                if let Some(sec) = jump {
                    app.offset = sec;
                    continue;
                }
                app.anchor = Some((row, col));
                app.cur = Some((row, col));
            }
            Ev::Drag(row, col) => {
                if app.anchor.is_some() {
                    app.cur = Some((abs(row), col));
                }
            }
            Ev::Release(row, col) => {
                if let Some(a) = app.anchor {
                    app.anchor = None;
                    app.cur = None;
                    let payload = app
                        .frame
                        .as_ref()
                        .and_then(|f| App::selection_payload(&f.1, a, (abs(row), col)));
                    match payload {
                        None => {}
                        Some(p) => match herdr_cli::send_text(&app.agent, &p) {
                            Ok(()) => {
                                app.msg = format!("sent {} lines to agent", p.lines().count())
                            }
                            Err(e) => app.msg = format!("send failed: {e}"),
                        },
                    }
                }
            }
            Ev::Unknown => {}
        }
    }
    0
}

fn truncate(s: &str, w: usize) -> String {
    if s.chars().count() <= w {
        return s.to_string();
    }
    s.chars().take(w.saturating_sub(1)).collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sgr_press_drag_release_wheel_decode() {
        // Given SGR bodies When decoding Then each gesture maps correctly.
        assert!(matches!(decode_button(0), Mouse::Press));
        assert!(matches!(decode_button(32), Mouse::Drag));
        assert!(matches!(decode_button(3), Mouse::Release));
        assert!(matches!(decode_button(64), Mouse::WheelUp));
        assert!(matches!(decode_button(65), Mouse::WheelDown));
    }

    #[test]
    fn sgr_m_final_is_release_even_with_zero_button() {
        // Given a release with button code 0 (some terminals echo it).
        // When decoding the event Then it is a Release, never a Press.
        let stdin = std::io::stdin();
        let seq: Vec<u8> = b"\x1b[<0;30;14m".to_vec().into_iter().rev().collect();
        let mut r = Reader {
            stdin: stdin.lock(),
            pushback: seq,
        };
        assert!(matches!(next_ev(&mut r), Ev::Release(13, 29)));
    }

    #[test]
    fn sgr_parse_rejects_non_mouse_finals() {
        // Given an arrow final byte When parsing as mouse Then None.
        assert_eq!(parse_sgr("<0;10;20", b'M').unwrap(), (0, 10, 20));
        assert!(parse_sgr("<0;10;20", b'A').is_none());
        assert!(parse_sgr("0;10;20", b'M').is_none());
    }
}
