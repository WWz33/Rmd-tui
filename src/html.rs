//! HTML backend.
//!
//! Emits the same shape the earlier Python tool produced: a standalone page with
//! a stylesheet, an optional floating table of contents, chunk header bars, a
//! note for every chunk that was not shown, and a placeholder where a figure
//! would have been. MathJax is only pulled in when the document has math.

use pulldown_cmark::{html, Options, Parser};

use crate::rmd::{Block, Document};
use crate::theme::{stylesheet, ThemeName};

/// Choices the front matter does not carry.
#[derive(Debug, Clone)]
pub struct HtmlOptions {
    pub theme: ThemeName,
    /// Show the "code not executed" strip at the top of the page.
    pub banner: bool,
    /// Allow the MathJax bundle. It is still only added when math is present.
    pub math: bool,
    /// Set when the document is part of a package site: a navigation bar is then
    /// put on every page, linking it to its siblings and to the index.
    pub site: Option<Site>,
}

impl Default for HtmlOptions {
    fn default() -> Self {
        Self {
            theme: ThemeName::default(),
            banner: true,
            math: true,
            site: None,
        }
    }
}

/// One page of a package site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteEntry {
    /// Link target relative to the page being written.
    pub href: String,
    pub label: String,
    pub source: String,
    pub group: SiteGroup,
}

/// Which section of the site a page belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteGroup {
    Vignettes,
    Other,
}

impl SiteGroup {
    fn title(self) -> &'static str {
        match self {
            SiteGroup::Vignettes => "Vignettes",
            SiteGroup::Other => "Other documents",
        }
    }
}

/// The package a site belongs to, and where its index sits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    pub package: String,
    pub title: Option<String>,
    pub description: Option<String>,
    /// Link to the index from a page at `depth` levels below the site root.
    pub index_href: String,
    /// The page being rendered, so it is not linked to itself.
    pub current: String,
    pub siblings: Vec<SiteEntry>,
}

const MATH_HEAD: &str = r#"<script>
window.MathJax = { tex: { inlineMath: [['\\(', '\\)']], displayMath: [['$$', '$$']] },
  options: { skipHtmlTags: ['script', 'noscript', 'style', 'textarea', 'pre', 'code'] } };
</script>
<script defer src="https://cdn.jsdelivr.net/npm/mathjax@3/es5/tex-mml-chtml.js"></script>
"#;

/// Highlights the section you are reading in the floating table of contents.
const TOC_SCRIPT: &str = r#"<script>
(function () {
  var links = Array.prototype.slice.call(document.querySelectorAll('nav.toc a'));
  if (!links.length || !('IntersectionObserver' in window)) { return; }
  var byId = {};
  links.forEach(function (link) { byId[link.getAttribute('href').slice(1)] = link; });
  var targets = Object.keys(byId).map(function (id) { return document.getElementById(id); })
    .filter(Boolean);
  var observer = new IntersectionObserver(function (entries) {
    entries.forEach(function (entry) {
      if (!entry.isIntersecting) { return; }
      links.forEach(function (link) { link.classList.remove('active'); });
      if (byId[entry.target.id]) { byId[entry.target.id].classList.add('active'); }
    });
  }, { rootMargin: '0px 0px -75% 0px' });
  targets.forEach(function (target) { observer.observe(target); });
})();
</script>
"#;

const SHELL: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title}</title>
<style>{css}</style>
{math_head}</head>
<body>
{banner}{content}
{scripts}</body>
</html>
"#;

/// Render a document to a standalone page, or to a page of a package site.
pub fn render(doc: &Document, options: &HtmlOptions) -> String {
    let mut body = String::new();

    for block in &doc.blocks {
        match block {
            Block::Heading {
                level,
                text,
                anchor,
                number,
                ..
            } => {
                let number = match number {
                    Some(number) => format!(r#"<span class="secnum">{number}</span>"#),
                    None => String::new(),
                };
                body.push_str(&format!(
                    "<h{level} id=\"{anchor}\">{number}{}</h{level}>\n",
                    inline_html(text, options.math)
                ));
            }
            Block::Markdown { text, .. } => {
                body.push_str(&prose_html(text, options.math));
            }
            Block::Code {
                engine,
                lang,
                label,
                code,
                ..
            } => {
                let class = if lang.is_empty() {
                    "chunk".to_string()
                } else {
                    format!("chunk lang-{lang}")
                };
                let head = if engine.is_empty() {
                    String::new()
                } else {
                    let label = if label.is_empty() {
                        String::new()
                    } else {
                        format!(r#"<span class="chunk-label">{}</span>"#, escape(label))
                    };
                    format!(
                        r#"<div class="chunk-head"><span class="chunk-lang">{}</span>{label}</div>"#,
                        escape(engine)
                    )
                };
                body.push_str(&format!(
                    "<div class=\"{class}\">{head}<pre><code>{}</code></pre></div>\n",
                    escape(code)
                ));
            }
            Block::Note { text, .. } => {
                body.push_str(&format!(
                    "<div class=\"chunk-note\">{}</div>\n",
                    escape(text)
                ));
            }
            Block::Figure {
                caption, number, ..
            } => {
                body.push_str(&format!(
                    "<figure>\n<div class=\"missing-figure\">figure not rendered \
                     (code was not run)</div>\n<figcaption>Figure {number}: {}</figcaption>\n</figure>\n",
                    inline_html(caption, options.math)
                ));
            }
            Block::RawHtml { text, .. } => {
                body.push_str(text.trim_end());
                body.push('\n');
            }
        }
    }

    let mut header = String::new();
    let title = doc
        .meta
        .title
        .clone()
        .unwrap_or_else(|| file_stem(&doc.source));
    let mut meta_lines = String::new();
    for value in [&doc.meta.author, &doc.meta.date].into_iter().flatten() {
        meta_lines.push_str(&format!(
            "<div>{}</div>\n",
            inline_html(value, options.math)
        ));
    }
    if doc.meta.title.is_some() || !meta_lines.is_empty() {
        header = format!(
            "<div class=\"doc-meta\">\n<div class=\"title\">{}</div>\n{meta_lines}</div>\n",
            inline_html(&title, options.math)
        );
    }

    let page = format!("<div class=\"page\">\n{header}{body}\n</div>");
    let nav = options.site.as_ref().map(nav_bar).unwrap_or_default();
    let toc = toc_html(doc);
    let content = if toc.is_empty() {
        page
    } else {
        format!("<div class=\"layout\">\n{toc}\n{page}\n</div>")
    };

    let banner = if options.banner {
        format!(
            "<div class=\"banner\">Rendered from {} &middot; code not executed</div>\n",
            escape(&file_name(&doc.source))
        )
    } else {
        String::new()
    };
    // The page itself is the source of truth: math can sit in a heading or a
    // figure caption just as easily as in a paragraph.
    let has_math = content.contains(MATH_MARKER);
    let math_head = if options.math && has_math {
        MATH_HEAD
    } else {
        ""
    };
    let scripts = if toc.is_empty() { "" } else { TOC_SCRIPT };

    fill(
        SHELL,
        &[
            ("{title}", &escape(&title)),
            ("{css}", &stylesheet(options.theme)),
            ("{math_head}", math_head),
            ("{banner}", &format!("{nav}{banner}")),
            ("{content}", &content),
            ("{scripts}", scripts),
        ],
    )
}

/// Put the pieces into the page template in a single pass.
///
/// Substituting one placeholder at a time would scan text that was inserted by
/// an earlier step, so a document that happens to print `{scripts}` would lose
/// those characters.
fn fill(template: &str, values: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len() + 4096);
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        match values.iter().find(|(name, _)| tail.starts_with(name)) {
            Some((name, value)) => {
                out.push_str(value);
                rest = &tail[name.len()..];
            }
            None => {
                out.push('{');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The floating table of contents, or an empty string when there is nothing to list.
fn toc_html(doc: &Document) -> String {
    if !doc.layout.toc {
        return String::new();
    }
    let entries: Vec<(u8, &str, String)> = doc
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Heading {
                level,
                anchor,
                text,
                number,
                ..
            } if (*level as usize) <= doc.layout.toc_depth => {
                let title = match number {
                    Some(number) => format!("{number} {text}"),
                    None => text.clone(),
                };
                Some((*level, anchor.as_str(), title))
            }
            _ => None,
        })
        .collect();
    if entries.len() < 2 {
        return String::new();
    }
    let mut out = String::from("<nav class=\"toc\">\n<div class=\"toc-title\">Contents</div>\n");
    for (level, anchor, title) in entries {
        out.push_str(&format!(
            "<a class=\"toc-h{level}\" href=\"#{anchor}\">{}</a>\n",
            inline_html(&title, false)
        ));
    }
    out.push_str("</nav>");
    out
}

/// The landing page of a package site.
pub fn render_index(site: &Site, entries: &[SiteEntry], theme: ThemeName) -> String {
    let mut body = String::new();
    if let Some(description) = &site.description {
        body.push_str(&format!(
            "<p class=\"site-description\">{}</p>\n",
            escape(description)
        ));
    }
    for group in [SiteGroup::Vignettes, SiteGroup::Other] {
        let in_group: Vec<&SiteEntry> = entries
            .iter()
            .filter(|entry| entry.group == group)
            .collect();
        if in_group.is_empty() {
            continue;
        }
        body.push_str(&format!(
            "<h2>{}</h2>\n<ul class=\"site-list\">\n",
            group.title()
        ));
        for entry in in_group {
            body.push_str(&format!(
                "<li><a href=\"{}\">{}</a> <span class=\"path\">{}</span></li>\n",
                escape(&entry.href),
                escape(&entry.label),
                escape(&entry.source)
            ));
        }
        body.push_str("</ul>\n");
    }
    body.push_str(
        "<p class=\"path\">Rendered from source. No code was run, so figures drawn by \
         chunks appear as captions and placeholders.</p>\n",
    );
    let header = format!(
        "<div class=\"doc-meta\">\n<div class=\"title\">{}</div>\n{}\n</div>\n",
        escape(&site.package),
        escape(site.title.as_deref().unwrap_or("Package documentation")),
    );
    fill(
        SHELL,
        &[
            ("{title}", &escape(&site.package)),
            ("{css}", &stylesheet(theme)),
            ("{math_head}", ""),
            ("{banner}", ""),
            (
                "{content}",
                &format!("<div class=\"page\">\n{header}{body}\n</div>"),
            ),
            ("{scripts}", ""),
        ],
    )
}

/// The bar at the top of every page of a site: the package, its index, and the
/// other documents.
fn nav_bar(site: &Site) -> String {
    let mut links = format!(
        "<a class=\"site-home\" href=\"{}\">{}</a>",
        escape(&site.index_href),
        escape(&site.package)
    );
    for entry in &site.siblings {
        if entry.href == site.current {
            links.push_str(&format!(
                "<span class=\"site-current\">{}</span>",
                escape(&entry.label)
            ));
        } else {
            links.push_str(&format!(
                "<a href=\"{}\">{}</a>",
                escape(&entry.href),
                escape(&entry.label)
            ));
        }
    }
    format!("<header class=\"site-nav\">{links}</header>\n")
}

/// Render markdown to HTML, including any inline markup a block carries.
fn prose_html(markdown: &str, math: bool) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_MATH);
    let mut out = String::new();
    html::push_html(&mut out, Parser::new_ext(markdown, options));
    let mut out = out.replace(
        &format!("<code>{marker}", marker = crate::rmd::INLINE_R_MARKER),
        "<code class=\"rinline\" title=\"R expression, not evaluated\">",
    );
    if out.contains(crate::rmd::INLINE_R_MARKER) {
        out = out.replace(crate::rmd::INLINE_R_MARKER, "");
    }
    fix_math(&out, math)
}

/// Markdown to HTML with the wrapping paragraph removed, for headings and captions.
fn inline_html(markdown: &str, math: bool) -> String {
    let rendered = prose_html(markdown, math);
    let trimmed = rendered.trim_end();
    let trimmed = trimmed.strip_prefix("<p>").unwrap_or(trimmed);
    let trimmed = trimmed.strip_suffix("</p>").unwrap_or(trimmed);
    trimmed.to_string()
}

/// Any rendered formula carries this class, whatever block it came from.
const MATH_MARKER: &str = r#"class="math"#;

const MATH_INLINE_OPEN: &str = r#"<span class="math math-inline">"#;
const MATH_DISPLAY_OPEN: &str = r#"<span class="math math-display">"#;

/// Restore the dollar delimiters MathJax expects.
///
/// `pulldown-cmark` writes math without its delimiters, so the page would show
/// raw TeX. Math bodies are escaped, so the first closing span after an opening
/// span is always the matching one.
fn fix_math(html: &str, math: bool) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    loop {
        let inline_at = rest.find(MATH_INLINE_OPEN);
        let display_at = rest.find(MATH_DISPLAY_OPEN);
        let (at, open, display) = match (inline_at, display_at) {
            (Some(a), Some(b)) if a < b => (a, MATH_INLINE_OPEN, false),
            (Some(_), Some(b)) => (b, MATH_DISPLAY_OPEN, true),
            (Some(a), None) => (a, MATH_INLINE_OPEN, false),
            (None, Some(b)) => (b, MATH_DISPLAY_OPEN, true),
            (None, None) => {
                out.push_str(rest);
                break;
            }
        };
        let body_at = at + open.len();
        let close_at = match rest[body_at..].find("</span>") {
            Some(offset) => body_at + offset,
            None => {
                out.push_str(rest);
                break;
            }
        };
        let body = &rest[body_at..close_at];
        out.push_str(&rest[..at]);
        if display {
            // A display formula is its own block: take it back out of the
            // paragraph pulldown-cmark wrapped around it.
            let class = if math { "math-display" } else { "math" };
            let markup = format!(r#"<span class="{class}">$${body}$$</span>"#);
            let after = close_at + "</span>".len();
            if out.ends_with("<p>") && rest[after..].starts_with("</p>") {
                out.truncate(out.len() - "<p>".len());
                rest = rest[after + "</p>".len()..].trim_start_matches('\n');
            } else {
                rest = &rest[after..];
            }
            out.push_str(&markup);
            continue;
        } else {
            let delimiter = if math { (r"\(", r"\)") } else { ("$", "$") };
            out.push_str(&format!(
                r#"<span class="math">{}{body}{}</span>"#,
                delimiter.0, delimiter.1
            ));
        }
        rest = &rest[close_at + "</span>".len()..];
    }
    out
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
    out
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn file_stem(path: &std::path::Path) -> String {
    path.file_stem()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "document".to_string())
}
