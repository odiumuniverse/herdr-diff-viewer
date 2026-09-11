use crate::git::DLKind;
use crate::hl::Span;
use crate::model::Snapshot;

const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";
const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const EMPTY: Vec<Span> = Vec::new();

pub const GUTTER_W: usize = 12;

pub struct ContentRow {
    pub row: u32,
    pub file: String,
    pub lineno: u32,
    pub text: String,
}

pub struct ClickMap {
    pub file_rows: Vec<(u32, String)>,
    pub section_rows: Vec<(String, u32)>,
    pub group_row: Option<u32>,
    pub content: Vec<ContentRow>,
}

pub fn render(
    snap: &Snapshot,
    show_session: bool,
    width: usize,
    theme: crate::hl::ThemeId,
) -> (String, ClickMap) {
    let w = width.max(20);
    let mut out = String::new();
    let mut map = ClickMap {
        file_rows: Vec::new(),
        section_rows: Vec::new(),
        group_row: None,
        content: Vec::new(),
    };
    let mut row: u32 = 0;

    out.push_str(&format!(
        "{BOLD}{} files{RESET} changed {GREEN}+{}{RESET} {RED}-{}{RESET}\n",
        snap.files, snap.adds, snap.dels
    ));
    row += 1;

    let list_row =
        |path: &str, adds: u32, dels: u32, map: &mut ClickMap, out: &mut String, row: &mut u32| {
            map.file_rows.push((*row, path.to_string()));
            let stats = format!("{GREEN}+{adds}{RESET} {RED}-{dels}{RESET}");
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
    let (add_bg, del_bg) = crate::hl::row_bg(theme.is_dark());
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
                DLKind::Add => (add_bg, "+", GREEN),
                DLKind::Del => (del_bg, "-", RED),
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
        let (body, map) = render(&snap(), false, 80, crate::hl::ThemeId::ViewerDark);
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
        let (body, map) = render(&snap(), true, 80, crate::hl::ThemeId::ViewerDark);
        assert_eq!(map.file_rows.len(), 2);
        assert_eq!(map.section_rows.len(), 2);
        assert!(body.contains("(hide)"));
    }

    #[test]
    fn add_rows_carry_syntax_colors() {
        let (body, _) = render(&snap(), false, 80, crate::hl::ThemeId::ViewerDark);
        let (add_bg, _) = crate::hl::row_bg(true);
        assert!(body.contains(add_bg), "add bg missing:\n{body}");
        assert!(
            body.contains("\x1b[38;2;255;121;198mpub "),
            "pink keyword missing:\n{body}"
        );
    }

    #[test]
    fn light_mode_uses_tinted_backgrounds() {
        let (body, _) = render(&snap(), false, 80, crate::hl::ThemeId::ViewerLight);
        let (add_bg, _) = crate::hl::row_bg(false);
        assert!(body.contains(add_bg), "light add bg missing:\n{body}");
        assert!(
            !body.contains(crate::hl::row_bg(true).0),
            "dark bg leaked into light render:\n{body}"
        );
    }
}
