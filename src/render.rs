use crate::git::{DLKind, DiffLine};
use crate::hl::{Span, ThemeId};
use crate::model::{FileView, Snapshot};
use crate::palette::{Palette, Rgb};
use crate::screen::{Line, Style};

const TAB: usize = 4;
const PAD: usize = 1;
const LIST_TOP: usize = 3;
pub const FIXED_ROWS: usize = 8;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Target {
    File(usize),
    Group,
    Close,
    ThemeBtn,
    Theme(usize),
}

pub enum Row {
    Header(usize),
    Blank(usize),
    Rule(usize),
    Gap(usize),
    Code {
        file: usize,
        lineno: u32,
        text: String,
        cont: bool,
        code_col: usize,
    },
}

impl Row {
    pub fn file(&self) -> usize {
        match self {
            Row::Header(f) | Row::Blank(f) | Row::Rule(f) | Row::Gap(f) => *f,
            Row::Code { file, .. } => *file,
        }
    }
}

pub struct Body {
    pub rows: Vec<Row>,
    pub lines: Vec<Line>,
    pub headers: Vec<usize>,
    pub names: Vec<String>,
}

impl Body {
    fn push(&mut self, row: Row, line: Line) {
        self.rows.push(row);
        self.lines.push(line);
    }

    pub fn start_of(&self, file: usize) -> usize {
        self.headers.get(file).map_or(0, |h| h.saturating_sub(1))
    }
}

pub enum Entry {
    File(usize),
    Group,
}

pub struct Geometry {
    pub list_top: usize,
    pub list_h: usize,
    pub body_top: usize,
    pub body_h: usize,
    pub body_start: usize,
    pub active: Option<usize>,
}

pub struct Hit {
    pub row: usize,
    pub from: usize,
    pub to: usize,
    pub target: Target,
}

pub struct Frame {
    pub lines: Vec<Line>,
    pub hits: Vec<Hit>,
    pub geo: Geometry,
}

impl Frame {
    pub fn hit(&self, row: usize, col: usize) -> Option<Target> {
        self.hits
            .iter()
            .find(|h| h.row == row && col >= h.from && col < h.to)
            .map(|h| h.target)
    }

    pub fn in_list(&self, row: usize) -> bool {
        row >= self.geo.list_top && row < self.geo.list_top + self.geo.list_h
    }

    pub fn body_row(&self, row: usize) -> Option<usize> {
        let g = &self.geo;
        (row >= g.body_top && row < g.body_top + g.body_h).then(|| g.body_start + row - g.body_top)
    }

    pub fn body_row_clamped(&self, row: usize) -> usize {
        let g = &self.geo;
        let last = (g.body_top + g.body_h).saturating_sub(1).max(g.body_top);
        g.body_start + row.clamp(g.body_top, last) - g.body_top
    }
}

pub struct View<'a> {
    pub snap: &'a Snapshot,
    pub body: &'a Body,
    pub show_session: bool,
    pub width: usize,
    pub height: usize,
    pub offset: usize,
    pub list_off: usize,
    pub hover: Option<Target>,
    pub sel: Option<((usize, usize), (usize, usize))>,
    pub pal: &'a Palette,
    pub menu: Option<MenuView<'a>>,
}

#[derive(Clone, Copy)]
pub struct MenuView<'a> {
    pub items: &'a [ThemeId],
    pub sel: usize,
    pub off: usize,
    pub current: ThemeId,
}

pub fn file_at(snap: &Snapshot, i: usize) -> &FileView {
    if i < snap.main.len() {
        &snap.main[i]
    } else {
        &snap.session[i - snap.main.len()]
    }
}

pub fn entries(snap: &Snapshot, show_session: bool) -> Vec<Entry> {
    let mut out: Vec<Entry> = (0..snap.main.len()).map(Entry::File).collect();
    if !snap.session.is_empty() {
        out.push(Entry::Group);
        if show_session {
            out.extend((0..snap.session.len()).map(|j| Entry::File(snap.main.len() + j)));
        }
    }
    out
}

pub fn entry_pos(entries: &[Entry], file: usize) -> Option<usize> {
    entries
        .iter()
        .position(|e| matches!(e, Entry::File(i) if *i == file))
}

pub fn geometry(entries: usize, body: &Body, height: usize, offset: usize) -> Geometry {
    let list_h = entries.min((height.saturating_sub(FIXED_ROWS) / 3).max(1));
    let body_top = LIST_TOP + list_h + 4;
    let body_h = height.saturating_sub(body_top);
    let active = body.rows.get(offset).map(|r| r.file());
    let mut body_start = offset;
    if let Some(f) = active {
        while matches!(body.rows.get(body_start), Some(Row::Rule(x) | Row::Header(x)) if *x == f) {
            body_start += 1;
        }
    }
    Geometry {
        list_top: LIST_TOP,
        list_h,
        body_top,
        body_h,
        body_start,
        active,
    }
}

pub fn max_offset(body: &Body, body_h: usize) -> usize {
    body.rows.len().saturating_sub(body_h.max(1))
}

pub fn body(snap: &Snapshot, show_session: bool, width: usize, pal: &Palette) -> Body {
    let base = Style::new(pal.fg, pal.bg);
    let mut b = Body {
        rows: Vec::new(),
        lines: Vec::new(),
        headers: Vec::new(),
        names: Vec::new(),
    };
    let files = snap.main.iter().chain(if show_session {
        snap.session.iter()
    } else {
        [].iter()
    });
    for (fi, f) in files.enumerate() {
        if fi > 0 {
            b.push(Row::Blank(fi - 1), Line::blank(width, base));
        }
        b.push(Row::Rule(fi), rule(width, pal));
        b.headers.push(b.rows.len());
        b.names.push(f.display.clone());
        b.push(
            Row::Header(fi),
            stat_line(&f.display, f.adds, f.dels, width, pal, base.bold()),
        );
        b.push(Row::Rule(fi), rule(width, pal));
        code_rows(&mut b, fi, f, width, pal);
    }
    b
}

fn code_rows(b: &mut Body, fi: usize, f: &FileView, w: usize, pal: &Palette) {
    let max_no = f.hunks.iter().filter_map(|h| h.new.or(h.old)).max();
    let lnw = max_no.unwrap_or(0).to_string().len();
    let code_col = PAD + lnw + 2;
    let budget = w.saturating_sub(code_col + PAD).max(1);
    let mut last: (Option<u32>, Option<u32>) = (None, None);
    for (i, h) in f.hunks.iter().enumerate() {
        if is_gap(last, h) {
            let dim = Style::new(pal.dim, pal.bg);
            let mut l = Line::blank(w, dim);
            l.put(PAD + lnw.saturating_sub(1), "⋯", dim);
            b.push(Row::Gap(fi), l);
        }
        last = (h.old.or(last.0), h.new.or(last.1));
        let cells = expand(f.hl.get(i).map_or(&[][..], |s| &s[..]), &h.text, pal.fg);
        let (bg, mark, no_fg) = match h.kind {
            DLKind::Add => (pal.add_bg, "+", pal.fg),
            DLKind::Del => (pal.del_bg, "-", pal.fg),
            DLKind::Ctx => (pal.bg, " ", pal.dim),
        };
        let lineno = h.new.or(h.old).unwrap_or(0);
        let chunks: Vec<&[(char, Rgb)]> = if cells.is_empty() {
            vec![&[]]
        } else {
            cells.chunks(budget).collect()
        };
        for (ci, chunk) in chunks.iter().enumerate() {
            let mut l = Line::blank(w, Style::new(pal.fg, pal.bg));
            l.set_bg(PAD, w.saturating_sub(PAD), bg);
            if ci == 0 {
                let no = lineno.to_string();
                l.put(
                    PAD + lnw.saturating_sub(no.len()),
                    &no,
                    Style::new(no_fg, bg),
                );
            }
            l.put(PAD + lnw + 1, mark, Style::new(pal.fg, bg));
            for (k, (ch, fg)) in chunk.iter().enumerate() {
                l.put_char(code_col + k, *ch, Style::new(*fg, bg));
            }
            b.push(
                Row::Code {
                    file: fi,
                    lineno,
                    text: chunk.iter().map(|c| c.0).collect(),
                    cont: ci > 0,
                    code_col,
                },
                l,
            );
        }
    }
}

fn is_gap(last: (Option<u32>, Option<u32>), h: &DiffLine) -> bool {
    let jump = |prev: Option<u32>, cur: Option<u32>| matches!((prev, cur), (Some(p), Some(c)) if c != p + 1);
    jump(last.0, h.old) || jump(last.1, h.new)
}

fn expand(spans: &[Span], plain: &str, fg: Rgb) -> Vec<(char, Rgb)> {
    let mut out = Vec::new();
    if spans.is_empty() {
        for ch in plain.chars() {
            push_cell(&mut out, ch, fg);
        }
    } else {
        for s in spans {
            for ch in s.text.chars() {
                push_cell(&mut out, ch, s.fg);
            }
        }
    }
    out
}

fn push_cell(out: &mut Vec<(char, Rgb)>, ch: char, fg: Rgb) {
    match ch {
        '\t' => {
            let n = TAB - out.len() % TAB;
            out.extend(std::iter::repeat_n((' ', fg), n));
        }
        c if c.is_control() => out.push((' ', fg)),
        c => out.push((c, fg)),
    }
}

fn rule(w: usize, pal: &Palette) -> Line {
    let mut l = Line::blank(w, Style::new(pal.fg, pal.bg));
    l.put(
        PAD,
        &"─".repeat(w.saturating_sub(2 * PAD)),
        Style::new(pal.sep, pal.bg),
    );
    l
}

fn stat_line(path: &str, adds: u32, dels: u32, w: usize, pal: &Palette, st: Style) -> Line {
    let mut l = Line::blank(w, Style::new(pal.fg, pal.bg));
    let a = format!("+{adds}");
    let d = format!("-{dels}");
    let stats = a.len() + 1 + d.len();
    l.put(PAD, &trunc(path, w.saturating_sub(2 * PAD + stats + 2)), st);
    let c = l.put(
        w.saturating_sub(PAD + stats),
        &a,
        Style::new(pal.add_fg, pal.bg),
    );
    l.put(c + 1, &d, Style::new(pal.del_fg, pal.bg));
    l
}

fn trunc(s: &str, w: usize) -> String {
    if s.chars().count() <= w {
        return s.to_string();
    }
    s.chars().take(w.saturating_sub(1)).collect::<String>() + "…"
}

pub fn frame(v: &View) -> Frame {
    let w = v.width;
    let pal = v.pal;
    let base = Style::new(pal.fg, pal.bg);
    let dim = Style::new(pal.dim, pal.bg);
    let ents = entries(v.snap, v.show_session);
    let geo = geometry(ents.len(), v.body, v.height, v.offset);
    let mut lines = Vec::with_capacity(v.height);
    let mut hits = Vec::new();

    lines.push(Line::blank(w, base));

    let mut head = Line::blank(w, base);
    let c = head.put(PAD, &format!("{} files", v.snap.files), base.bold());
    let c = head.put(c, " changed ", base);
    let c = head.put(
        c,
        &format!("+{}", v.snap.adds),
        Style::new(pal.add_fg, pal.bg),
    );
    let c = head.put(
        c + 1,
        &format!("-{}", v.snap.dels),
        Style::new(pal.del_fg, pal.bg),
    );
    let c = head.put(c + 1, &format!("· watching {}", v.snap.repos), dim);
    head.put(c + 1, &format!("· {}", v.snap.theme.name()), dim);
    let close = w.saturating_sub(PAD + 1);
    let btn = close.saturating_sub(2);
    head.put(close, "×", dim.underlined(v.hover == Some(Target::Close)));
    head.put(btn, "◑", dim.underlined(v.hover == Some(Target::ThemeBtn)));
    hits.push(Hit {
        row: 1,
        from: btn.saturating_sub(1),
        to: btn + 1,
        target: Target::ThemeBtn,
    });
    hits.push(Hit {
        row: 1,
        from: close.saturating_sub(1),
        to: w,
        target: Target::Close,
    });
    lines.push(head);
    lines.push(Line::blank(w, base));

    let list_off = v.list_off.min(ents.len().saturating_sub(geo.list_h));
    for (k, e) in ents.iter().skip(list_off).take(geo.list_h).enumerate() {
        let (mut line, target, text_len) = match e {
            Entry::File(i) => {
                let f = file_at(v.snap, *i);
                let mut l = stat_line(&f.display, f.adds, f.dels, w, pal, base);
                if geo.active == Some(*i) {
                    l.set_bg(PAD, w.saturating_sub(PAD), pal.active_bg);
                }
                (l, Target::File(*i), f.display.chars().count())
            }
            Entry::Group => {
                let label = format!(
                    "{} tests/generated ({})",
                    v.snap.session.len(),
                    if v.show_session { "hide" } else { "show" }
                );
                let mut l = Line::blank(w, base);
                l.put(PAD, &label, dim);
                (l, Target::Group, label.chars().count())
            }
        };
        if v.hover == Some(target) {
            line.underline(PAD, PAD + text_len);
        }
        hits.push(Hit {
            row: geo.list_top + k,
            from: 0,
            to: w,
            target,
        });
        lines.push(line);
    }

    lines.push(Line::blank(w, base));
    lines.push(rule(w, pal));
    lines.push(match geo.active {
        Some(f) => v.body.lines[v.body.headers[f]].clone(),
        None => {
            let mut l = Line::blank(w, base);
            l.put(
                PAD,
                &format!("no changes · watching {}", v.snap.repos),
                dim,
            );
            l
        }
    });
    lines.push(rule(w, pal));

    let sel = v.sel.map(|(a, b)| if a <= b { (a, b) } else { (b, a) });
    for k in 0..geo.body_h {
        let idx = geo.body_start + k;
        let mut l = v
            .body
            .lines
            .get(idx)
            .cloned()
            .unwrap_or_else(|| Line::blank(w, base));
        if let (Some(((r1, c1), (r2, c2))), Some(Row::Code { code_col, text, .. })) =
            (sel, v.body.rows.get(idx))
        {
            if idx >= r1 && idx <= r2 {
                let lo = if idx == r1 { c1 } else { 0 };
                let hi = if idx == r2 { c2 } else { w };
                let from = lo.max(*code_col);
                let to = hi.min(code_col + text.chars().count());
                if from < to {
                    l.set_bg(from, to, pal.sel_bg);
                }
            }
        }
        lines.push(l);
    }

    if let Some(m) = v.menu {
        draw_menu(&mut lines, &mut hits, &geo, m, w, pal, v.hover);
    }

    Frame { lines, hits, geo }
}

fn draw_menu(
    lines: &mut [Line],
    hits: &mut Vec<Hit>,
    geo: &Geometry,
    m: MenuView,
    w: usize,
    pal: &Palette,
    hover: Option<Target>,
) {
    let base = Style::new(pal.fg, pal.bg);
    let sep = Style::new(pal.sep, pal.bg);
    let mut inner = 0;
    for t in m.items {
        inner = inner.max(t.name().chars().count());
    }
    inner += 4;
    let right = w.saturating_sub(PAD);
    let left = right.saturating_sub(inner + 2);
    let count = m
        .items
        .len()
        .saturating_sub(m.off)
        .min(geo.body_h.saturating_sub(2));
    if count == 0 || left + 2 >= right || geo.body_top >= lines.len() {
        return;
    }
    let mut top = Line::blank(w, base);
    top.put(left, &format!("┌{}┐", "─".repeat(inner)), sep);
    lines[geo.body_top] = top;
    for k in 0..count {
        let i = m.off + k;
        let row = geo.body_top + 1 + k;
        if row >= lines.len() {
            break;
        }
        let bg = if i == m.sel { pal.active_bg } else { pal.bg };
        let mut l = Line::blank(w, base);
        l.set_bg(left, right, bg);
        l.put(left, "│", Style::new(pal.sep, bg));
        l.put(right - 1, "│", Style::new(pal.sep, bg));
        let mark = if m.items[i] == m.current { "●" } else { "○" };
        let after_mark = l.put(left + 2, mark, Style::new(pal.dim, bg));
        let name_from = after_mark + 1;
        l.put(name_from, m.items[i].name(), Style::new(pal.fg, bg));
        if hover == Some(Target::Theme(i)) {
            l.underline(name_from, name_from + m.items[i].name().chars().count());
        }
        hits.push(Hit {
            row,
            from: left,
            to: right,
            target: Target::Theme(i),
        });
        lines[row] = l;
    }
    let brow = geo.body_top + 1 + count;
    if brow < lines.len() {
        let mut bot = Line::blank(w, base);
        bot.put(left, &format!("└{}┘", "─".repeat(inner)), sep);
        lines[brow] = bot;
    }
}

pub fn selection_payload(body: &Body, a: (usize, usize), c: (usize, usize)) -> Option<String> {
    let ((r1, c1), (r2, c2)) = if a <= c { (a, c) } else { (c, a) };
    if (r1, c1) == (r2, c2) {
        return None;
    }
    let mut out = String::new();
    let mut last: Option<(usize, u32)> = None;
    let mut last_file: Option<usize> = None;
    for idx in r1..=r2 {
        let Some(Row::Code {
            file,
            lineno,
            text,
            cont,
            code_col,
        }) = body.rows.get(idx)
        else {
            continue;
        };
        let lo = if idx == r1 { c1 } else { 0 };
        let hi = if idx == r2 { c2 } else { usize::MAX };
        let n = text.chars().count();
        let from = lo.saturating_sub(*code_col).min(n);
        let to = hi.saturating_sub(*code_col).min(n);
        if from >= to {
            continue;
        }
        let frag: String = text.chars().skip(from).take(to - from).collect();
        let same_line = *cont && last == Some((*file, *lineno));
        if !same_line && frag.trim().is_empty() {
            continue;
        }
        if last_file != Some(*file) {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&format!("{}:{}\n", body.names[*file], lineno));
            last_file = Some(*file);
        } else if !same_line {
            out.push('\n');
        }
        out.push_str(&frag);
        last = Some((*file, *lineno));
    }
    let trimmed = out.trim_end();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

pub fn anchor(body: &Body, offset: usize) -> Option<(String, u32)> {
    let top = body.rows.get(offset)?;
    let lineno = match top {
        Row::Code { lineno, .. } => *lineno,
        _ => 0,
    };
    Some((body.names[top.file()].clone(), lineno))
}

pub fn resolve_anchor(body: &Body, a: &(String, u32)) -> Option<usize> {
    let f = body.names.iter().position(|n| n == &a.0)?;
    let start = body.start_of(f);
    if a.1 == 0 {
        return Some(start);
    }
    let hit = body
        .rows
        .iter()
        .enumerate()
        .skip(start)
        .take_while(|(_, r)| r.file() == f)
        .find(|(_, r)| matches!(r, Row::Code { lineno, cont: false, .. } if *lineno >= a.1))
        .map(|(i, _)| i);
    Some(hit.unwrap_or(start))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::group::Group;
    use crate::palette::DARK;

    fn dl(old: Option<u32>, new: Option<u32>, kind: DLKind, text: &str) -> DiffLine {
        DiffLine {
            old,
            new,
            kind,
            text: text.into(),
        }
    }

    fn file(name: &str, hunks: Vec<DiffLine>) -> FileView {
        let adds = hunks.iter().filter(|h| h.kind == DLKind::Add).count() as u32;
        let dels = hunks.iter().filter(|h| h.kind == DLKind::Del).count() as u32;
        FileView {
            path: name.into(),
            display: name.into(),
            adds,
            dels,
            group: Group::Main,
            hunks,
            hl: vec![],
        }
    }

    fn snap(main: Vec<FileView>, session: Vec<FileView>) -> Snapshot {
        let all = || main.iter().chain(&session);
        let adds = all().map(|f| f.adds as u64).sum();
        let dels = all().map(|f| f.dels as u64).sum();
        let files = main.len() + session.len();
        Snapshot {
            main,
            session,
            files,
            adds,
            dels,
            repos: 1,
            theme: ThemeId::ClaudeDark,
        }
    }

    fn code_rows_of(b: &Body) -> Vec<(&Row, String)> {
        b.rows
            .iter()
            .zip(&b.lines)
            .filter(|(r, _)| matches!(r, Row::Code { .. }))
            .map(|(r, l)| (r, l.text()))
            .collect()
    }

    fn one_line(name: &str) -> FileView {
        file(name, vec![dl(None, Some(1), DLKind::Add, "x")])
    }

    fn view<'a>(s: &'a Snapshot, b: &'a Body, hover: Option<Target>) -> View<'a> {
        View {
            snap: s,
            body: b,
            show_session: false,
            width: 40,
            height: 30,
            offset: 0,
            list_off: 0,
            hover,
            sel: None,
            pal: &DARK,
            menu: None,
        }
    }

    #[test]
    fn changed_rows_paint_background_between_the_margins() {
        let s = snap(
            vec![file(
                "a.rs",
                vec![
                    dl(Some(1), None, DLKind::Del, "old"),
                    dl(None, Some(1), DLKind::Add, "new"),
                ],
            )],
            vec![],
        );
        let b = body(&s, false, 40, &DARK);
        let rows: Vec<&Line> = b
            .rows
            .iter()
            .zip(&b.lines)
            .filter(|(r, _)| matches!(r, Row::Code { .. }))
            .map(|(_, l)| l)
            .collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].cells.len(), 40);
        assert!(rows[0].cells[PAD..40 - PAD]
            .iter()
            .all(|c| c.st.bg == DARK.del_bg));
        assert_eq!(rows[0].cells[0].st.bg, DARK.bg, "left margin stays clean");
        assert_eq!(rows[0].cells[39].st.bg, DARK.bg, "right margin stays clean");
        assert!(rows[1].cells[PAD..40 - PAD]
            .iter()
            .all(|c| c.st.bg == DARK.add_bg));
        assert!(rows[1].text().starts_with(" 1 +new"));
    }

    #[test]
    fn file_name_sits_between_two_rules() {
        let s = snap(vec![one_line("a.rs"), one_line("b.rs")], vec![]);
        let b = body(&s, false, 40, &DARK);
        let head = b.headers[1];
        assert!(matches!(b.rows[head], Row::Header(1)));
        assert!(matches!(b.rows[head - 1], Row::Rule(1)), "rule above name");
        assert!(matches!(b.rows[head + 1], Row::Rule(1)), "rule below name");
        assert!(
            matches!(b.rows[head - 2], Row::Blank(0)),
            "gap closes the previous file"
        );
        let line = b.lines[head - 1].text();
        assert_eq!(line.chars().next(), Some(' '), "rule keeps the margin");
        assert_eq!(line.chars().nth(1), Some('─'));
        assert_eq!(line.chars().last(), Some(' '));
    }

    #[test]
    fn long_lines_wrap_repeating_mark_with_one_lineno() {
        let text = "a".repeat(90);
        let s = snap(
            vec![file("a.rs", vec![dl(None, Some(7), DLKind::Add, &text)])],
            vec![],
        );
        let b = body(&s, false, 40, &DARK);
        let code = code_rows_of(&b);
        assert_eq!(code.len(), 3);
        assert!(code[0].1.starts_with(" 7 +aaa"), "{:?}", code[0].1);
        assert!(code[1].1.starts_with("   +aaa"), "{:?}", code[1].1);
        assert!(matches!(code[2].0, Row::Code { cont: true, .. }));
    }

    #[test]
    fn gap_row_separates_distant_hunks() {
        let s = snap(
            vec![file(
                "a.rs",
                vec![
                    dl(Some(10), Some(10), DLKind::Ctx, "a"),
                    dl(Some(11), None, DLKind::Del, "b"),
                    dl(None, Some(11), DLKind::Add, "c"),
                    dl(Some(50), Some(50), DLKind::Ctx, "d"),
                ],
            )],
            vec![],
        );
        let b = body(&s, false, 40, &DARK);
        assert_eq!(
            b.rows.iter().filter(|r| matches!(r, Row::Gap(_))).count(),
            1
        );
    }

    #[test]
    fn tabs_expand_to_four_columns() {
        let s = snap(
            vec![file("a.go", vec![dl(None, Some(1), DLKind::Add, "\tx")])],
            vec![],
        );
        let b = body(&s, false, 40, &DARK);
        let code = code_rows_of(&b);
        assert!(matches!(code[0].0, Row::Code { text, .. } if text == "    x"));
    }

    #[test]
    fn sticky_band_follows_scroll_without_repeating_itself() {
        let s = snap(vec![one_line("a.rs"), one_line("b.rs")], vec![]);
        let b = body(&s, false, 40, &DARK);
        let second = b.start_of(1);
        let at_band = geometry(2, &b, 30, second);
        assert_eq!(at_band.active, Some(1));
        assert!(
            matches!(b.rows[at_band.body_start], Row::Code { file: 1, .. }),
            "band rows are not repeated under the sticky band"
        );
        assert_eq!(geometry(2, &b, 30, 0).active, Some(0));
        assert_eq!(
            geometry(2, &b, 30, second - 1).active,
            Some(0),
            "gap row belongs to the file above"
        );
    }

    #[test]
    fn list_is_capped_to_a_third_of_the_panel() {
        let empty = Body {
            rows: vec![],
            lines: vec![],
            headers: vec![],
            names: vec![],
        };
        assert_eq!(geometry(30, &empty, 40, 0).list_h, 10);
        assert_eq!(geometry(2, &empty, 40, 0).list_h, 2);
    }

    #[test]
    fn header_counts_close_and_list_targets() {
        let s = snap(
            vec![file(
                "a.rs",
                vec![
                    dl(None, Some(1), DLKind::Add, "x"),
                    dl(None, Some(2), DLKind::Add, "y"),
                ],
            )],
            vec![],
        );
        let b = body(&s, false, 40, &DARK);
        let f = frame(&view(&s, &b, None));
        assert_eq!(f.lines.len(), 30);
        assert_eq!(f.lines[0].text().trim(), "", "gap above the header");
        assert!(f.lines[1].text().starts_with(" 1 files changed +2 -0"));
        assert_eq!(f.hit(1, 38), Some(Target::Close));
        assert_eq!(f.hit(3, 5), Some(Target::File(0)));
    }

    #[test]
    fn hover_underlines_only_the_path() {
        let s = snap(vec![one_line("a.rs")], vec![]);
        let b = body(&s, false, 40, &DARK);
        let f = frame(&view(&s, &b, Some(Target::File(0))));
        let row = &f.lines[3].cells;
        assert!(!row[0].st.underline, "margin is not underlined");
        assert!(row[PAD..PAD + 4].iter().all(|c| c.st.underline));
        assert!(!row[PAD + 4].st.underline);
        assert!(row[PAD..40 - PAD].iter().all(|c| c.st.bg == DARK.active_bg));
    }

    #[test]
    fn group_row_toggles_session_files() {
        let s = snap(vec![one_line("a.rs")], vec![one_line("tests/a.rs")]);
        let b = body(&s, false, 40, &DARK);
        let f = frame(&view(&s, &b, None));
        assert!(f.lines[4].text().starts_with(" 1 tests/generated (show)"));
        assert_eq!(f.hit(4, 0), Some(Target::Group));
        assert_eq!(b.headers.len(), 1);
        assert_eq!(body(&s, true, 40, &DARK).headers.len(), 2);
    }

    #[test]
    fn selection_across_wrapped_rows_is_one_line() {
        let text: String = (0..90).map(|i| char::from(b'a' + (i % 26) as u8)).collect();
        let s = snap(
            vec![file("a.rs", vec![dl(None, Some(7), DLKind::Add, &text)])],
            vec![],
        );
        let b = body(&s, false, 40, &DARK);
        let first = b
            .rows
            .iter()
            .position(|r| matches!(r, Row::Code { .. }))
            .unwrap();
        let p = selection_payload(&b, (first, PAD + 3), (first + 2, 40)).unwrap();
        assert_eq!(p, format!("a.rs:7\n{text}"));
    }

    #[test]
    fn anchor_survives_new_file_above() {
        let two = |n: &str| {
            file(
                n,
                vec![
                    dl(None, Some(1), DLKind::Add, "x"),
                    dl(None, Some(2), DLKind::Add, "y"),
                ],
            )
        };
        let s1 = snap(vec![two("b.rs")], vec![]);
        let b1 = body(&s1, false, 40, &DARK);
        let second_code = b1.headers[0] + 3;
        let a = anchor(&b1, second_code).unwrap();
        assert_eq!(a, ("b.rs".to_string(), 2));
        let s2 = snap(vec![two("a.rs"), two("b.rs")], vec![]);
        let b2 = body(&s2, false, 40, &DARK);
        let o = resolve_anchor(&b2, &a).unwrap();
        assert!(matches!(
            &b2.rows[o],
            Row::Code {
                file: 1,
                lineno: 2,
                ..
            }
        ));
    }

    #[test]
    fn theme_menu_overlays_body_with_clickable_rows() {
        let s = snap(vec![one_line("a.rs")], vec![]);
        let b = body(&s, false, 40, &DARK);
        let items = [ThemeId::ClaudeDark, ThemeId::Transparent];
        let mut v = view(&s, &b, None);
        v.menu = Some(MenuView {
            items: &items,
            sel: 1,
            off: 0,
            current: ThemeId::ClaudeDark,
        });
        let f = frame(&v);
        assert_eq!(f.lines.len(), 30);
        assert_eq!(f.hit(1, 36), Some(Target::ThemeBtn));
        assert_eq!(f.hit(1, 38), Some(Target::Close));
        let mrow = f.geo.body_top + 2;
        assert_eq!(f.hit(mrow, 20), Some(Target::Theme(1)));
        assert!(f.lines[mrow].text().contains("transparent"));
    }
}
