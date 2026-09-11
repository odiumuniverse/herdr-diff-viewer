use std::fmt::Write;

use crate::palette::Rgb;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Style {
    pub fg: Rgb,
    pub bg: Rgb,
    pub bold: bool,
    pub underline: bool,
}

impl Style {
    pub fn new(fg: Rgb, bg: Rgb) -> Style {
        Style {
            fg,
            bg,
            bold: false,
            underline: false,
        }
    }

    pub fn bold(mut self) -> Style {
        self.bold = true;
        self
    }

    pub fn underlined(mut self, on: bool) -> Style {
        self.underline = on;
        self
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Cell {
    pub ch: char,
    pub st: Style,
}

#[derive(Clone, Debug)]
pub struct Line {
    pub cells: Vec<Cell>,
}

impl Line {
    pub fn blank(width: usize, st: Style) -> Line {
        Line {
            cells: vec![Cell { ch: ' ', st }; width],
        }
    }

    pub fn put(&mut self, col: usize, text: &str, st: Style) -> usize {
        let mut c = col;
        for ch in text.chars() {
            if c >= self.cells.len() {
                break;
            }
            self.cells[c] = Cell { ch, st };
            c += 1;
        }
        c
    }

    pub fn put_char(&mut self, col: usize, ch: char, st: Style) {
        if let Some(cell) = self.cells.get_mut(col) {
            *cell = Cell { ch, st };
        }
    }

    pub fn set_bg(&mut self, from: usize, to: usize, bg: Rgb) {
        for cell in self.cells.iter_mut().take(to).skip(from) {
            cell.st.bg = bg;
        }
    }

    pub fn underline(&mut self, from: usize, to: usize) {
        for cell in self.cells.iter_mut().take(to).skip(from) {
            cell.st.underline = true;
        }
    }

    #[cfg(test)]
    pub fn text(&self) -> String {
        self.cells.iter().map(|c| c.ch).collect()
    }

    pub fn encode(&self, out: &mut String, default_bg: Option<Rgb>) {
        let mut cur: Option<(Style, bool)> = None;
        for cell in &self.cells {
            let plain = default_bg.is_some_and(|d| cell.st.bg == d);
            if cur != Some((cell.st, plain)) {
                push_sgr(out, cell.st, plain);
                cur = Some((cell.st, plain));
            }
            out.push(cell.ch);
        }
        out.push_str("\x1b[0m");
    }
}

fn push_sgr(out: &mut String, st: Style, default_bg: bool) {
    let (fr, fg, fb) = st.fg;
    let (br, bg, bb) = st.bg;
    let bold = if st.bold { "1;" } else { "" };
    let under = if st.underline { "4;" } else { "" };
    if default_bg {
        let _ = write!(out, "\x1b[0;{bold}{under}38;2;{fr};{fg};{fb};49m");
    } else {
        let _ = write!(
            out,
            "\x1b[0;{bold}{under}38;2;{fr};{fg};{fb};48;2;{br};{bg};{bb}m"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_keeps_background_through_color_changes() {
        let bg = (0, 97, 0);
        let mut l = Line::blank(6, Style::new((1, 1, 1), bg));
        l.put(0, "ab", Style::new((9, 9, 9), bg));
        let mut out = String::new();
        l.encode(&mut out, None);
        assert_eq!(out.matches("48;2;0;97;0").count(), 2);
        assert!(out.ends_with("    \x1b[0m"), "{out:?}");
    }

    #[test]
    fn encode_emits_default_bg_for_transparent_base() {
        let bg = (38, 38, 38);
        let mut l = Line::blank(4, Style::new((232, 232, 232), bg));
        l.put(0, "ab", Style::new((255, 80, 80), (112, 22, 30)));
        let mut out = String::new();
        l.encode(&mut out, Some(bg));
        assert!(out.contains(";49m"), "base cells must use default bg: {out:?}");
        assert!(!out.contains("48;2;38;38;38"), "{out:?}");
        assert!(out.contains("48;2;112;22;30"), "hunk bg still paints: {out:?}");
    }

    #[test]
    fn underline_marks_only_the_given_range() {
        let st = Style::new((0, 0, 0), (0, 0, 0));
        let mut l = Line::blank(4, st);
        l.underline(0, 2);
        assert!(l.cells[..2].iter().all(|c| c.st.underline));
        assert!(!l.cells[2].st.underline);
    }

    #[test]
    fn put_clips_at_width() {
        let mut l = Line::blank(3, Style::new((0, 0, 0), (0, 0, 0)));
        assert_eq!(l.put(1, "xyz", Style::new((0, 0, 0), (0, 0, 0))), 3);
        assert_eq!(l.text(), " xy");
    }
}
