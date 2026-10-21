//! Themes for both backends.
//!
//! The HTML themes are carried over from the earlier Python tool so the browser
//! output stays where it was. The terminal themes are the same three ideas
//! expressed in ratatui styles: headings, code, borders and links.

use ratatui_core::style::{Color, Modifier, Style};
use tui_markdown::StyleSheet;

/// The three looks the tool ships with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum ThemeName {
    /// Bootstrap 3 metrics, the shape `rmarkdown::render` produces.
    #[default]
    Rmarkdown,
    /// GitHub markdown look.
    Github,
    /// No chrome, for reading long prose.
    Plain,
}

const CSS_BASE: &str = r#"
* { box-sizing:border-box; }
html { scroll-behavior:smooth; }
body { margin:0; background:var(--page-bg); color:var(--fg); font:var(--size)/var(--lh) var(--font); }
h1,h2,h3,h4,h5,h6 { line-height:1.25; margin:1.5em 0 .6em; font-weight:600; }
h1 { font-size:1.9em; } h2 { font-size:1.5em; } h3 { font-size:1.22em; } h4 { font-size:1.05em; }
h5, h6 { font-size:1em; }
h1:first-child, h2:first-child { margin-top:0; }
h1, h2, h3, h4, h5, h6, .chunk { scroll-margin-top:12px; }
p, ul, ol, blockquote, table, pre, figure, .math-display { margin:0 0 1.1em; }
a { color:var(--accent); text-decoration:none; } a:hover { text-decoration:underline; }
code { font-family:var(--mono); font-size:.9em; background:var(--mark); border-radius:4px; padding:.15em .35em; }
pre { background:var(--code-bg); border:1px solid var(--code-border); border-radius:var(--radius);
  padding:14px 16px; overflow-x:auto; }
pre code { background:none; padding:0; font-size:.9em; line-height:1.5; }
.chunk { border:1px solid var(--code-border); border-radius:var(--radius); overflow:hidden;
  background:var(--bg); margin-bottom:1.1em; }
.chunk pre { margin:0; border:0; border-radius:0; }
.chunk-head { display:flex; gap:10px; align-items:center; padding:6px 12px; background:var(--code-bg);
  border-bottom:1px solid var(--code-border); font:600 .72em var(--mono); color:var(--muted);
  text-transform:uppercase; letter-spacing:.04em; }
.chunk-head .chunk-label { font-weight:400; text-transform:none; letter-spacing:0; }
.chunk-note { margin:0 0 1.1em; padding:8px 12px; border-left:3px solid var(--line);
  background:var(--code-bg); color:var(--muted); font-size:.86em; font-style:italic; }
blockquote { border-left:4px solid var(--line); padding:0 1em; color:var(--muted); }
blockquote > :last-child { margin-bottom:0; }
table { border-collapse:collapse; width:100%; }
th, td { border:1px solid var(--line); padding:6px 13px; }
th { background:var(--code-bg); font-weight:600; }
img { max-width:100%; }
hr { border:0; border-top:1px solid var(--line); margin:2em 0; }
.math-display { display:block; overflow-x:auto; text-align:center; padding:.4em 0; }
.doc-meta { color:var(--muted); border-bottom:1px solid var(--line); padding-bottom:1.2em;
  margin-bottom:2em; font-size:.95em; }
.doc-meta .title { font-size:1.9em; font-weight:700; color:var(--fg); line-height:1.25; margin:0 0 .3em; }
.missing-figure { border:1px dashed var(--line); border-radius:var(--radius); background:var(--code-bg);
  color:var(--muted); padding:18px 16px; font:italic .86em var(--mono); }
figure .missing-figure { margin:0; }
figcaption { color:var(--muted); font-size:.9em; margin-top:.5em; }
.secnum { margin-right:.5em; }
.path { color:var(--muted); font:.85em var(--mono); }
.banner { padding:9px 24px; color:var(--muted); font-size:.8em; background:var(--code-bg);
  border-bottom:1px solid var(--line); }
.site-nav { display:flex; flex-wrap:wrap; align-items:center; gap:18px; padding:12px 24px;
  border-bottom:1px solid var(--line); font-size:.9em; background:var(--bg); }
.site-nav .site-home { font-weight:600; color:var(--fg); }
.site-nav .site-current { color:var(--muted); font-weight:600; }
.site-description { color:var(--muted); }
.site-list { padding-left:1.2em; }
.site-list li { margin-bottom:.35em; }
.page { max-width:var(--page-width); margin:0 auto; padding:48px 56px 96px; background:var(--bg);
  box-shadow:var(--page-shadow); }
.layout { display:flex; align-items:flex-start; max-width:calc(var(--page-width) + var(--toc-width) + 56px);
  margin:0 auto; background:var(--bg); box-shadow:var(--page-shadow); }
.layout .page { box-shadow:none; }
nav.toc { flex:0 0 var(--toc-width); width:var(--toc-width); position:sticky; top:0; max-height:100vh;
  overflow-y:auto; padding:52px 20px 48px 24px; font-size:.86em; border-right:1px solid var(--line); }
nav.toc .toc-title { font-weight:600; margin-bottom:.7em; text-transform:uppercase; font-size:.78em;
  letter-spacing:.06em; color:var(--muted); }
nav.toc a { display:block; color:var(--muted); padding:3px 0; line-height:1.35; }
nav.toc a:hover { color:var(--accent); text-decoration:none; }
nav.toc a.active { color:var(--accent); font-weight:600; }
nav.toc a.toc-h2 { padding-left:12px; } nav.toc a.toc-h3 { padding-left:24px; }
nav.toc a.toc-h4 { padding-left:36px; } nav.toc a.toc-h5 { padding-left:48px; }
nav.toc a.toc-h6 { padding-left:60px; }
@media (max-width:1080px) {
  .layout { display:block; max-width:var(--page-width); box-shadow:none; background:none; }
  nav.toc { position:static; width:auto; max-height:none; border-right:0; background:var(--bg);
    border-bottom:1px solid var(--line); padding:24px 56px 16px; }
}
@media print {
  body { background:#fff; }
  .page, .layout { box-shadow:none; max-width:none; padding:0; display:block; background:#fff; }
  .banner, nav.toc { display:none; }
}
"#;

const CSS_RMARKDOWN_EXTRA: &str = r#"
h1 { font-size:2.57em; font-weight:500; }
h2 { font-size:2.14em; font-weight:500; }
h3 { font-size:1.71em; font-weight:500; }
h4 { font-size:1.28em; font-weight:500; }
h5 { font-size:1em; font-weight:700; }
h6 { font-size:.86em; font-weight:600; color:var(--muted); }
code { color:#c7254e; }
pre code { font-size:1em; }
.chunk-head { font-weight:400; text-transform:none; letter-spacing:0; }
.doc-meta .title { font-size:2.57em; font-weight:500; }
.doc-meta { font-size:1em; }
tr:nth-child(2n) td { background:#f9f9f9; }
"#;

const CSS_GITHUB_EXTRA: &str = r#"
h1 { border-bottom:1px solid var(--line); padding-bottom:.3em; }
h2 { border-bottom:1px solid var(--line); padding-bottom:.25em; }
code { color:inherit; }
.doc-meta .title { font-size:2em; }
tr:nth-child(2n) td { background:#fafbfc; }
"#;

const CSS_PLAIN_EXTRA: &str = r#"
h1 { font-size:1.6em; } h2 { font-size:1.3em; } h3 { font-size:1.12em; }
.chunk { border:0; border-left:3px solid var(--line); border-radius:0; }
.chunk-head { background:none; border-bottom:0; }
tr:nth-child(2n) td { background:#fafafa; }
"#;

const VARS_RMARKDOWN: &[(&str, &str)] = &[
    ("font", r#""Helvetica Neue",Helvetica,Arial,sans-serif"#),
    ("mono", r#"Menlo,Monaco,Consolas,"Courier New",monospace"#),
    ("size", "14px"),
    ("lh", "1.42857143"),
    ("fg", "#333"),
    ("muted", "#777"),
    ("line", "#ddd"),
    ("bg", "#fff"),
    ("page-bg", "#fff"),
    ("code-bg", "#f5f5f5"),
    ("code-border", "#ccc"),
    ("accent", "#337ab7"),
    ("mark", "#f9f2f4"),
    ("page-width", "940px"),
    ("radius", "4px"),
    ("toc-width", "240px"),
    ("page-shadow", "none"),
];

const VARS_GITHUB: &[(&str, &str)] = &[
    (
        "font",
        r#"-apple-system,BlinkMacSystemFont,"Segoe UI","Helvetica Neue",Arial,sans-serif"#,
    ),
    (
        "mono",
        r#"ui-monospace,SFMono-Regular,"SF Mono",Menlo,Consolas,monospace"#,
    ),
    ("size", "16px"),
    ("lh", "1.65"),
    ("fg", "#1f2328"),
    ("muted", "#656d76"),
    ("line", "#d8dee4"),
    ("bg", "#fff"),
    ("page-bg", "#f6f7f9"),
    ("code-bg", "#f6f8fa"),
    ("code-border", "#d8dee4"),
    ("accent", "#0969da"),
    ("mark", "rgba(175,184,193,.2)"),
    ("page-width", "820px"),
    ("radius", "8px"),
    ("toc-width", "250px"),
    ("page-shadow", "0 0 0 1px #d8dee4"),
];

const VARS_PLAIN: &[(&str, &str)] = &[
    (
        "font",
        r#"-apple-system,BlinkMacSystemFont,"Segoe UI","Helvetica Neue",Arial,sans-serif"#,
    ),
    (
        "mono",
        r#"ui-monospace,SFMono-Regular,"SF Mono",Menlo,Consolas,monospace"#,
    ),
    ("size", "16px"),
    ("lh", "1.6"),
    ("fg", "#111"),
    ("muted", "#666"),
    ("line", "#e5e5e5"),
    ("bg", "#fff"),
    ("page-bg", "#fff"),
    ("code-bg", "#fafafa"),
    ("code-border", "#ececec"),
    ("accent", "#0a58ca"),
    ("mark", "#f1f1f1"),
    ("page-width", "760px"),
    ("radius", "6px"),
    ("toc-width", "220px"),
    ("page-shadow", "none"),
];

/// The stylesheet for a theme: variables, shared rules, then theme overrides.
pub fn stylesheet(theme: ThemeName) -> String {
    let (vars, extra) = match theme {
        ThemeName::Rmarkdown => (VARS_RMARKDOWN, CSS_RMARKDOWN_EXTRA),
        ThemeName::Github => (VARS_GITHUB, CSS_GITHUB_EXTRA),
        ThemeName::Plain => (VARS_PLAIN, CSS_PLAIN_EXTRA),
    };
    let root = vars
        .iter()
        .map(|(name, value)| format!("--{name}:{value};"))
        .collect::<Vec<_>>()
        .join(" ");
    format!(":root {{{root}}}\n{CSS_BASE}{extra}")
}

/// Terminal styling, applied through `tui-markdown`'s style sheet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TuiTheme {
    pub name: ThemeName,
}

impl TuiTheme {
    pub fn new(name: ThemeName) -> Self {
        Self { name }
    }

    fn accent(&self) -> Color {
        match self.name {
            ThemeName::Rmarkdown => Color::Rgb(51, 122, 183),
            ThemeName::Github => Color::Rgb(9, 105, 218),
            ThemeName::Plain => Color::Reset,
        }
    }

    fn muted(&self) -> Color {
        match self.name {
            ThemeName::Rmarkdown => Color::Rgb(119, 119, 119),
            ThemeName::Github => Color::Rgb(101, 109, 118),
            ThemeName::Plain => Color::DarkGray,
        }
    }

    fn line(&self) -> Color {
        match self.name {
            ThemeName::Rmarkdown => Color::Rgb(221, 221, 221),
            ThemeName::Github => Color::Rgb(216, 222, 228),
            ThemeName::Plain => Color::DarkGray,
        }
    }

    fn code_fg(&self) -> Color {
        match self.name {
            ThemeName::Rmarkdown => Color::Rgb(199, 37, 78),
            ThemeName::Github => Color::Rgb(10, 48, 105),
            ThemeName::Plain => Color::Reset,
        }
    }

    fn heading_fg(&self, level: u8) -> Color {
        match self.name {
            ThemeName::Plain => Color::Reset,
            _ if level <= 2 => Color::Rgb(17, 17, 17),
            _ => self.accent(),
        }
    }

    fn base(&self) -> Style {
        Style::new().fg(Color::Reset)
    }

    /// Style for app chrome: chunk headers, notes, captions, status bar.
    pub fn chrome(&self) -> Style {
        self.base().fg(self.muted()).add_modifier(Modifier::DIM)
    }

    /// Style for raw HTML passed through by a `{=html}` chunk.
    pub fn html_style(&self) -> Style {
        self.base().fg(self.muted()).add_modifier(Modifier::DIM)
    }

    /// Accent colour, for the status bar and outlines.
    pub fn accent_color(&self) -> Color {
        self.accent()
    }
}

impl StyleSheet for TuiTheme {
    fn heading(&self, level: u8) -> Style {
        let weight = match self.name {
            ThemeName::Plain => Modifier::BOLD,
            _ if level <= 2 => Modifier::BOLD,
            _ => Modifier::BOLD,
        };
        self.base().fg(self.heading_fg(level)).add_modifier(weight)
    }

    /// Rendered documents do not show their markdown source markers.
    fn heading_marker(&self, _level: u8) -> &str {
        ""
    }

    /// Same for code fences: the chunk header line says what a block is.
    fn code_block_fence(&self) -> &str {
        ""
    }

    fn code(&self) -> Style {
        self.base().fg(self.code_fg())
    }

    fn link(&self) -> Style {
        self.base()
            .fg(self.accent())
            .add_modifier(Modifier::UNDERLINED)
    }

    fn blockquote(&self) -> Style {
        self.base().fg(self.muted()).add_modifier(Modifier::ITALIC)
    }

    fn list_marker(&self) -> Style {
        self.base().fg(self.accent())
    }

    fn table_border(&self) -> Style {
        self.base().fg(self.line())
    }

    fn table_header(&self) -> Style {
        self.base().add_modifier(Modifier::BOLD)
    }

    fn table_cell(&self) -> Style {
        self.base()
    }

    fn math_inline(&self) -> Style {
        self.base().fg(self.muted()).add_modifier(Modifier::ITALIC)
    }

    fn math_display(&self) -> Style {
        self.base().fg(self.muted()).add_modifier(Modifier::ITALIC)
    }

    fn html(&self) -> Style {
        self.base().fg(self.muted()).add_modifier(Modifier::DIM)
    }

    fn image_alt(&self) -> Style {
        self.base().fg(self.muted())
    }
}
