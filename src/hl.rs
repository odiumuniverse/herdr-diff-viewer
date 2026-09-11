use std::io::Cursor;
use std::sync::OnceLock;

use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::SyntaxSet;

/// One syntax-colored token: foreground RGB + source slice.
pub struct Span {
    pub fg: (u8, u8, u8),
    pub text: String,
}

static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
static THEME: OnceLock<Theme> = OnceLock::new();

fn syntaxes() -> &'static SyntaxSet {
    SYNTAXES.get_or_init(SyntaxSet::load_defaults_nonewlines)
}

fn theme() -> &'static Theme {
    THEME.get_or_init(|| {
        let bytes = include_bytes!("../assets/diff-viewer.tmTheme");
        // Primary: our hand-tuned theme. Fallback: bundled dark theme.
        match ThemeSet::load_from_reader(&mut Cursor::new(bytes)) {
            Ok(t) => t,
            Err(_) => ThemeSet::load_defaults().themes["base16-ocean.dark"].clone(),
        }
    })
}

/// Highlight `lines` of `path` (syntax by extension). Unknown extensions
/// fall back to plain text — never an error, never a panic.
pub fn highlight(path: &str, lines: &[String]) -> Vec<Vec<Span>> {
    let ss = syntaxes();
    let syntax = ss
        .find_syntax_for_file(path)
        .ok()
        .flatten()
        .unwrap_or_else(|| ss.find_syntax_plain_text());
    let mut hl = HighlightLines::new(syntax, theme());
    lines
        .iter()
        .map(|line| {
            hl.highlight_line(line, ss)
                .unwrap_or_default()
                .into_iter()
                .map(|(style, text)| Span {
                    fg: (style.foreground.r, style.foreground.g, style.foreground.b),
                    text: text.to_string(),
                })
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_keyword_gets_color() {
        // Given a Rust fn line When highlighting Then `fn` is pink, joined text roundtrips.
        let out = highlight("src/a.rs", &["fn complement(c: Rgb) -> Rgb {".into()]);
        assert_eq!(out.len(), 1);
        let joined: String = out[0].iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, "fn complement(c: Rgb) -> Rgb {");
        assert!(
            out[0].iter().any(|s| s.fg == (0xFF, 0x79, 0xC6)),
            "keyword pink missing: {:?}",
            out[0].iter().map(|s| s.fg).collect::<Vec<_>>()
        );
    }

    #[test]
    fn unknown_extension_falls_back_to_plain() {
        // Given an unknown extension When highlighting Then text survives whole.
        let out = highlight("file.zzzunknown", &["hello world".into()]);
        let joined: String = out[0].iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, "hello world");
    }
}
