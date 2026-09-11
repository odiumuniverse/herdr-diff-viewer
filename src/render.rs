use crate::git::DLKind;
use crate::hl::Span;
use crate::model::Snapshot;

const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";
const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
pub const ADD_BG: &str = "\x1b[48;5;22m";
pub const DEL_BG: &str = "\x1b[48;5;52m";
const EMPTY: Vec<Span> = Vec::new();

/// Screen columns before the code starts: `old new ± `. The selection drag
/// clamps to this so gutter digits never leak into the agent payload.
pub const GUTTER_W: usize = 12;

/// One selectable content row for the mouse drag -> send-text path.
pub struct ContentRow {
    pub row: u32,
    pub file: String,
    pub lineno: u32,
    pub text: String,
}

pub struct ClickMap {
    /// (screen row, path) for the file list — click jumps the section.
    pub file_rows: Vec<(u32, String)>,
    /// (path, section start row) for scroll targets.
    pub section_rows: Vec<(String, u32)>,
    /// Screen row of the collapsed `N tests/generated (show)` toggle.
    pub group_row: Option<u32>,
    pub content: Vec<ContentRow>,
}

/// Full content render (no scrolling applied — the tty loop slices). Returns
/// the body plus hit-test maps in screen-row coordinates.
pub fn render(snap: &Snapshot, show_session: bool, width: usize) -> (String, ClickMap) {
    let w = width.max(20);
    let mut out = String::new();
    let mut map = ClickMap {
        file_rows: Vec::new(),
        section_rows: Vec::new(),
        group_row: None,
        content: Vec::new(),
    };
    let mut row: u32 = 0;

    // Header: `5 files changed +43 -5`.
    out.push_str(&format!(
        "{BOLD}{} files{RESET} changed {GREEN}+{}{RESET} {RED}-{}{RESET}\n",
        snap.files, snap.adds, snap.dels
    ));
    row += 1;

    let list_row =
        |path: &str, adds: u32, dels: u32, map: &mut ClickMap, out: &mut String, row: &mut u32| {
            map.file_rows.push((*row, path.to_string()));
            let stats = format!("{GREEN}+{adds}{RESET} {RED}-{dels}{RESET}");
            // Pad between path and stats; stats contain invisible escapes so pad
            // on the visible path width plus a fixed gap.
            let gap = w.saturating_sub(path.chars().count() + 12).max(2);
            out.push_str(&format!("{path}{}{stats}\n", " ".repeat(gap)));
            *row += 1;
        };

    for f in &snap.main {
        list_row(&f.display, f.adds, f.dels, &mut map, &mut out, &mut row);
    }
    if !snap.session.is_empty() {
        let label = if show_session { "hide" } else { "show" };
        map.group_row = Some(row);
        out.push_str(&format!(
            "{DIM}{} tests/generated ({label}){RESET}\n",
            snap.session.len()
        ));
        row += 1;
        if show_session {
            for f in &snap.session {
                list_row(&f.display, f.adds, f.dels, &mut map, &mut out, &mut row);
            }
        }
    }

    out.push('\n');
    row += 1;

    let visible: Vec<_> = snap
        .main
        .iter()
        .chain(if show_session {
            snap.session.iter()
        } else {
            [].iter()
        })
        .collect();
    for f in visible {
        map.section_rows.push((f.display.clone(), row));
        out.push_str(&format!("{BOLD}{}{RESET}\n", trunc(&f.display, w)));
        row += 1;
        for (h, spans) in f
            .hunks
            .iter()
            .zip(f.hl.iter().chain(std::iter::repeat(&EMPTY)))
        {
            let (bg, mark, color) = match h.kind {
                DLKind::Add => (ADD_BG, "+", GREEN),
                DLKind::Del => (DEL_BG, "-", RED),
                DLKind::Ctx => ("", " ", RESET),
            };
            let old = h.old.map(|n| n.to_string()).unwrap_or_default();
            let new = h.new.map(|n| n.to_string()).unwrap_or_default();
            let lineno = h.new.or(h.old).unwrap_or(0);
            let budget = w.saturating_sub(GUTTER_W);
            let code = colorize(spans, &h.text, budget);
            let plain_code = trunc(&h.text, budget);
            let line = format!("{old:>4} {new:>4} {color}{mark}{RESET} {code}");
            let line = pad_visible(&line, &format!("{old:>4} {new:>4} {mark} {plain_code}"), w);
            if bg.is_empty() {
                out.push_str(&format!("{line}\n"));
            } else {
                out.push_str(&format!("{bg}{line}{RESET}\n"));
            }
            map.content.push(ContentRow {
                row,
                file: f.display.clone(),
                lineno,
                text: h.text.clone(),
            });
            row += 1;
        }
    }
    (out, map)
}

fn trunc(s: &str, w: usize) -> String {
    if s.chars().count() <= w {
        return s.to_string();
    }
    s.chars().take(w.saturating_sub(1)).collect::<String>() + "…"
}

/// Pad with spaces to full width, measuring the escape-free reference.
/// Syntax-color `plain` (char budget so ANSI never gets cut), falling back
/// to uncolored text when no spans were produced for the row.
fn colorize(spans: &[Span], plain: &str, budget: usize) -> String {
    if spans.is_empty() {
        return trunc(plain, budget);
    }
    let mut out = String::new();
    let mut left = budget;
    for s in spans {
        if left == 0 {
            break;
        }
        let take: String = s.text.chars().take(left).collect();
        left -= take.chars().count();
        let (r, g, b) = s.fg;
        out.push_str(&format!("\x1b[38;2;{r};{g};{b}m{take}"));
    }
    out.push_str(RESET);
    out
}

fn pad_visible(colored: &str, plain: &str, w: usize) -> String {
    let n = plain.chars().count();
    if n >= w {
        return colored.to_string();
    }
    format!("{colored}{}", " ".repeat(w - n))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{DLKind, DiffLine};
    use crate::group::Group;
    use crate::model::{FileView, Snapshot};

    fn snap() -> Snapshot {
        Snapshot {
            main: vec![FileView {
                path: "src/color.rs".into(),
                display: "src/color.rs".into(),
                abs: "/r/src/color.rs".into(),
                adds: 21,
                dels: 0,
                group: Group::Main,
                hunks: vec![DiffLine {
                    old: None,
                    new: Some(112),
                    kind: DLKind::Add,
                    text: "pub fn complement(c: Rgb) -> Rgb {".into(),
                }],
                hl: vec![vec![
                    crate::hl::Span {
                        fg: (0xFF, 0x79, 0xC6),
                        text: "pub ".into(),
                    },
                    crate::hl::Span {
                        fg: (0xD4, 0xD4, 0xD4),
                        text: "fn complement(c: Rgb) -> Rgb {".into(),
                    },
                ]],
            }],
            session: vec![FileView {
                path: "tests/color.rs".into(),
                display: "tests/color.rs".into(),
                abs: "/r/tests/color.rs".into(),
                adds: 25,
                dels: 0,
                group: Group::Session,
                hunks: vec![],
                hl: vec![],
            }],
            files: 2,
            adds: 46,
            dels: 0,
        }
    }

    #[test]
    fn header_counts_and_hit_maps() {
        // Given a two-file snapshot When rendering collapsed Then the header
        // counts match and the group toggle row exists.
        let (body, map) = render(&snap(), false, 80);
        assert!(
            body.contains("2 files") && body.contains("+46"),
            "header: {body}"
        );
        assert!(map.group_row.is_some());
        assert_eq!(map.file_rows.len(), 1);
        assert_eq!(map.section_rows.len(), 1);
        assert!(body.contains("1 tests/generated (show)"));
    }

    #[test]
    fn expanded_session_lists_and_sections() {
        // Given expanded session When rendering Then both files list + section.
        let (body, map) = render(&snap(), true, 80);
        assert_eq!(map.file_rows.len(), 2);
        assert_eq!(map.section_rows.len(), 2);
        assert!(body.contains("(hide)"));
    }

    #[test]
    fn add_rows_carry_syntax_colors() {
        // Given an add hunk with spans When rendering Then truecolor codes land on the row.
        let (body, _) = render(&snap(), false, 80);
        assert!(body.contains(ADD_BG), "add bg missing:\n{body}");
        assert!(
            body.contains("\x1b[38;2;255;121;198mpub "),
            "pink keyword missing:\n{body}"
        );
    }
}
