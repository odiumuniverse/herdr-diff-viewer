use std::io::Cursor;
use std::sync::OnceLock;

use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme as SynTheme, ThemeSet};
use syntect::parsing::SyntaxSet;

pub struct Span {
    pub fg: (u8, u8, u8),
    pub text: String,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ThemeId {
    ClaudeDark,
    ClaudeLight,
    ViewerDark,
    ViewerLight,
    Catppuccin,
    CatppuccinLatte,
    Terminal,
    TokyoNight,
    TokyoNightDay,
    Dracula,
    Nord,
    Gruvbox,
    GruvboxLight,
    OneDark,
    OneLight,
    Solarized,
    SolarizedLight,
    Kanagawa,
    KanagawaLotus,
    RosePine,
    RosePineDawn,
    Vesper,
}

impl ThemeId {
    pub fn all() -> Vec<ThemeId> {
        vec![
            ThemeId::ClaudeDark,
            ThemeId::ClaudeLight,
            ThemeId::ViewerDark,
            ThemeId::ViewerLight,
            ThemeId::Catppuccin,
            ThemeId::CatppuccinLatte,
            ThemeId::Terminal,
            ThemeId::TokyoNight,
            ThemeId::TokyoNightDay,
            ThemeId::Dracula,
            ThemeId::Nord,
            ThemeId::Gruvbox,
            ThemeId::GruvboxLight,
            ThemeId::OneDark,
            ThemeId::OneLight,
            ThemeId::Solarized,
            ThemeId::SolarizedLight,
            ThemeId::Kanagawa,
            ThemeId::KanagawaLotus,
            ThemeId::RosePine,
            ThemeId::RosePineDawn,
            ThemeId::Vesper,
        ]
    }

    pub fn name(self) -> &'static str {
        match self {
            ThemeId::ClaudeDark => "claude-code-dark",
            ThemeId::ClaudeLight => "claude-code-light",
            ThemeId::ViewerDark => "diff-viewer",
            ThemeId::ViewerLight => "diff-viewer-light",
            ThemeId::Catppuccin => "catppuccin",
            ThemeId::CatppuccinLatte => "catppuccin-latte",
            ThemeId::Terminal => "terminal",
            ThemeId::TokyoNight => "tokyo-night",
            ThemeId::TokyoNightDay => "tokyo-night-day",
            ThemeId::Dracula => "dracula",
            ThemeId::Nord => "nord",
            ThemeId::Gruvbox => "gruvbox",
            ThemeId::GruvboxLight => "gruvbox-light",
            ThemeId::OneDark => "one-dark",
            ThemeId::OneLight => "one-light",
            ThemeId::Solarized => "solarized",
            ThemeId::SolarizedLight => "solarized-light",
            ThemeId::Kanagawa => "kanagawa",
            ThemeId::KanagawaLotus => "kanagawa-lotus",
            ThemeId::RosePine => "rose-pine",
            ThemeId::RosePineDawn => "rose-pine-dawn",
            ThemeId::Vesper => "vesper",
        }
    }

    pub fn is_dark(self) -> bool {
        !matches!(
            self,
            ThemeId::ClaudeLight
                | ThemeId::ViewerLight
                | ThemeId::CatppuccinLatte
                | ThemeId::TokyoNightDay
                | ThemeId::GruvboxLight
                | ThemeId::OneLight
                | ThemeId::SolarizedLight
                | ThemeId::KanagawaLotus
                | ThemeId::RosePineDawn
        )
    }

    pub fn from_name(name: &str) -> Option<ThemeId> {
        match name.to_lowercase().replace([' ', '_'], "-").as_str() {
            "claude-code-dark" | "claude-dark" | "claude" | "claude-code" => {
                Some(ThemeId::ClaudeDark)
            }
            "claude-code-light" | "claude-light" => Some(ThemeId::ClaudeLight),
            "diff-viewer" | "viewer" | "viewer-dark" => Some(ThemeId::ViewerDark),
            "diff-viewer-light" | "viewer-light" => Some(ThemeId::ViewerLight),
            "catppuccin" | "catppuccin-mocha" => Some(ThemeId::Catppuccin),
            "catppuccin-latte" | "latte" => Some(ThemeId::CatppuccinLatte),
            "terminal" => Some(ThemeId::Terminal),
            "tokyo-night" | "tokyonight" => Some(ThemeId::TokyoNight),
            "tokyo-night-day" | "tokyo-day" | "tokyonight-day" => Some(ThemeId::TokyoNightDay),
            "dracula" => Some(ThemeId::Dracula),
            "nord" => Some(ThemeId::Nord),
            "gruvbox" | "gruvbox-dark" => Some(ThemeId::Gruvbox),
            "gruvbox-light" => Some(ThemeId::GruvboxLight),
            "one-dark" | "onedark" => Some(ThemeId::OneDark),
            "one-light" | "onelight" => Some(ThemeId::OneLight),
            "solarized" | "solarized-dark" => Some(ThemeId::Solarized),
            "solarized-light" => Some(ThemeId::SolarizedLight),
            "kanagawa" => Some(ThemeId::Kanagawa),
            "kanagawa-lotus" | "lotus" => Some(ThemeId::KanagawaLotus),
            "rose-pine" | "rosepine" => Some(ThemeId::RosePine),
            "rose-pine-dawn" | "rosepine-dawn" | "dawn" => Some(ThemeId::RosePineDawn),
            "vesper" => Some(ThemeId::Vesper),
            _ => None,
        }
    }

    fn asset(self) -> &'static [u8] {
        match self {
            ThemeId::ClaudeDark => include_bytes!("../assets/claude-code-dark.tmTheme"),
            ThemeId::ClaudeLight => include_bytes!("../assets/claude-code-light.tmTheme"),
            ThemeId::ViewerDark => include_bytes!("../assets/diff-viewer.tmTheme"),
            ThemeId::ViewerLight => include_bytes!("../assets/diff-viewer-light.tmTheme"),
            ThemeId::Catppuccin => include_bytes!("../assets/catppuccin.tmTheme"),
            ThemeId::CatppuccinLatte => include_bytes!("../assets/catppuccin-latte.tmTheme"),
            ThemeId::Terminal => include_bytes!("../assets/terminal.tmTheme"),
            ThemeId::TokyoNight => include_bytes!("../assets/tokyo-night.tmTheme"),
            ThemeId::TokyoNightDay => include_bytes!("../assets/tokyo-night-day.tmTheme"),
            ThemeId::Dracula => include_bytes!("../assets/dracula.tmTheme"),
            ThemeId::Nord => include_bytes!("../assets/nord.tmTheme"),
            ThemeId::Gruvbox => include_bytes!("../assets/gruvbox.tmTheme"),
            ThemeId::GruvboxLight => include_bytes!("../assets/gruvbox-light.tmTheme"),
            ThemeId::OneDark => include_bytes!("../assets/one-dark.tmTheme"),
            ThemeId::OneLight => include_bytes!("../assets/one-light.tmTheme"),
            ThemeId::Solarized => include_bytes!("../assets/solarized.tmTheme"),
            ThemeId::SolarizedLight => include_bytes!("../assets/solarized-light.tmTheme"),
            ThemeId::Kanagawa => include_bytes!("../assets/kanagawa.tmTheme"),
            ThemeId::KanagawaLotus => include_bytes!("../assets/kanagawa-lotus.tmTheme"),
            ThemeId::RosePine => include_bytes!("../assets/rose-pine.tmTheme"),
            ThemeId::RosePineDawn => include_bytes!("../assets/rose-pine-dawn.tmTheme"),
            ThemeId::Vesper => include_bytes!("../assets/vesper.tmTheme"),
        }
    }
}

pub struct Theme {
    pub id: ThemeId,
    pub syn: SynTheme,
}

static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
static QUERIED_LIGHT: OnceLock<Option<bool>> = OnceLock::new();

fn syntaxes() -> &'static SyntaxSet {
    SYNTAXES.get_or_init(SyntaxSet::load_defaults_nonewlines)
}

pub fn resolve() -> Theme {
    let id = match std::env::var("DIFF_THEME").ok().as_deref() {
        Some("dark") => ThemeId::ClaudeDark,
        Some("light") => ThemeId::ClaudeLight,
        Some(name) => ThemeId::from_name(name).unwrap_or_else(default_id),
        None => default_id(),
    };
    Theme { id, syn: load(id) }
}

fn default_id() -> ThemeId {
    if queried_light() {
        ThemeId::ClaudeLight
    } else {
        ThemeId::ClaudeDark
    }
}

fn queried_light() -> bool {
    (*QUERIED_LIGHT.get_or_init(query_terminal)).unwrap_or(false)
}

fn load(id: ThemeId) -> SynTheme {
    let fallback = if id.is_dark() {
        "base16-ocean.dark"
    } else {
        "base16-ocean.light"
    };
    match ThemeSet::load_from_reader(&mut Cursor::new(id.asset())) {
        Ok(t) => t,
        Err(_) => ThemeSet::load_defaults().themes[fallback].clone(),
    }
}

fn query_terminal() -> Option<bool> {
    use std::io::{Read, Write};
    let set = |a: &[&str]| {
        std::process::Command::new("stty")
            .args(a)
            .status()
            .ok()
            .filter(|s| s.success())
    };
    let saved = std::process::Command::new("stty")
        .arg("-g")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())?;
    set(&["-echo", "-icanon"])?;
    set(&["min", "0", "time", "1"])?;
    let _ = write!(std::io::stdout(), "\x1b]11;?\x1b\\");
    let _ = std::io::stdout().flush();
    let mut buf = Vec::new();
    let mut tmp = [0u8; 64];
    let mut stdin = std::io::stdin();
    for _ in 0..8 {
        match stdin.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                if buf.ends_with(b"\x07") || buf.ends_with(b"\x1b\\") {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let args: Vec<&str> = saved.split_whitespace().collect();
    let _ = set(&args);
    parse_bg(&String::from_utf8_lossy(&buf))
}

pub fn parse_bg(reply: &str) -> Option<bool> {
    let i = reply.find("rgb:")?;
    let mut parts = reply[i + 4..].split('/');
    let comp = |s: Option<&str>| {
        let h = s?.trim().trim_end_matches(['\x07', '\x1b', '\\']);
        let v = u32::from_str_radix(h, 16).ok()?;
        Some(v as f64 / (16u32.pow(h.len() as u32) - 1) as f64)
    };
    let (r, g, b) = (
        comp(parts.next())?,
        comp(parts.next())?,
        comp(parts.next())?,
    );
    Some(0.2126 * r + 0.7152 * g + 0.0722 * b > 0.5)
}

pub fn row_bg(dark: bool) -> (&'static str, &'static str) {
    if dark {
        ("\x1b[48;5;22m", "\x1b[48;5;52m")
    } else {
        ("\x1b[48;5;194m", "\x1b[48;5;224m")
    }
}

pub fn highlight(path: &str, lines: &[String], theme: &Theme) -> Vec<Vec<Span>> {
    let ss = syntaxes();
    let syntax = ss
        .find_syntax_for_file(path)
        .ok()
        .flatten()
        .unwrap_or_else(|| ss.find_syntax_plain_text());
    let mut hl = HighlightLines::new(syntax, &theme.syn);
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

    fn claude_dark() -> Theme {
        Theme {
            id: ThemeId::ClaudeDark,
            syn: load(ThemeId::ClaudeDark),
        }
    }

    #[test]
    fn registry_covers_all_herdr_names() {
        assert_eq!(ThemeId::all().len(), 22);
        for name in [
            "catppuccin",
            "catppuccin-latte",
            "terminal",
            "tokyo-night",
            "tokyo-night-day",
            "dracula",
            "nord",
            "gruvbox",
            "gruvbox-light",
            "one-dark",
            "one-light",
            "solarized",
            "solarized-light",
            "kanagawa",
            "kanagawa-lotus",
            "rose-pine",
            "rose-pine-dawn",
            "vesper",
        ] {
            assert!(ThemeId::from_name(name).is_some(), "{name}");
        }
    }

    #[test]
    fn aliases_resolve() {
        assert_eq!(ThemeId::from_name("tokyonight"), Some(ThemeId::TokyoNight));
        assert_eq!(ThemeId::from_name("dawn"), Some(ThemeId::RosePineDawn));
        assert_eq!(ThemeId::from_name("latte"), Some(ThemeId::CatppuccinLatte));
        assert_eq!(ThemeId::from_name("claude"), Some(ThemeId::ClaudeDark));
        assert_eq!(ThemeId::from_name("nope"), None);
    }

    #[test]
    fn every_bundled_theme_loads() {
        for id in ThemeId::all() {
            assert!(load(id).name.is_some(), "{}", id.name());
        }
    }

    #[test]
    fn rust_keyword_gets_color() {
        let out = highlight(
            "src/a.rs",
            &["fn complement(c: Rgb) -> Rgb {".into()],
            &claude_dark(),
        );
        assert_eq!(out.len(), 1);
        let joined: String = out[0].iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, "fn complement(c: Rgb) -> Rgb {");
        assert!(
            out[0].iter().any(|s| s.fg == (0xD9, 0x77, 0x57)),
            "claude orange keyword missing: {:?}",
            out[0].iter().map(|s| s.fg).collect::<Vec<_>>()
        );
    }

    #[test]
    fn unknown_extension_falls_back_to_plain() {
        let out = highlight("file.zzzunknown", &["hello world".into()], &claude_dark());
        let joined: String = out[0].iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, "hello world");
    }

    #[test]
    fn bg_reply_parses_and_thresholds() {
        assert_eq!(parse_bg("\x1b]11;rgb:ffff/ffff/ffff\x1b\\"), Some(true));
        assert_eq!(parse_bg("\x1b]11;rgb:0000/0000/0000\x07"), Some(false));
        assert_eq!(parse_bg("\x1b]11;rgb:1e1e/1e1e/1e1e\x1b\\"), Some(false));
        assert_eq!(parse_bg("nonsense"), None);
    }
}
