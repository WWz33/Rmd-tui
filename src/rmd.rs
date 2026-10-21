//! R Markdown front end.
//!
//! Turns `.Rmd` source into a [`Document`] of blocks. Chunk options that change
//! what the *source* looks like (`echo`, `include`, `eval`, `child`) are
//! honoured. Options that only shape output are irrelevant here, because no
//! chunk is ever evaluated.
//!
//! This crate has no execution path: it never spawns a process and never
//! evaluates anything. The point of the tool is to show you an R Markdown
//! document without running the code inside it.

use std::path::{Path, PathBuf};

/// Invisible separator that marks an inline R expression in prose.
pub const INLINE_R_MARKER: char = '\u{2063}';

/// How deep `child = "other.Rmd"` inclusion may nest before it is cut off.
const MAX_CHILD_DEPTH: usize = 8;

const DEFAULT_TOC_DEPTH: usize = 3;

/// Values read from the YAML front matter.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Meta {
    pub title: Option<String>,
    pub author: Option<String>,
    pub date: Option<String>,
    pub toc: Option<bool>,
    pub toc_depth: Option<usize>,
    pub number_sections: Option<bool>,
}

/// Command line choices that override the front matter.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Overrides {
    pub toc: Option<bool>,
    pub toc_depth: Option<usize>,
    pub number_sections: Option<bool>,
}

/// Front matter and command line resolved into the values a backend needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub toc: bool,
    pub toc_depth: usize,
    pub number_sections: bool,
}

/// One piece of the document.
///
/// Prose stays as CommonMark text so a markdown engine does the layout work;
/// everything the Rmd layer has an opinion about gets its own variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Heading {
        level: u8,
        text: String,
        anchor: String,
        number: Option<String>,
        src: usize,
    },
    Markdown {
        text: String,
        src: usize,
    },
    Code {
        engine: String,
        lang: String,
        label: String,
        code: String,
        src: usize,
    },
    /// A one line explanation of code that is not shown, such as `echo=FALSE`.
    Note {
        text: String,
        src: usize,
    },
    /// A `fig.cap` caption standing in for a figure that was never drawn.
    Figure {
        caption: String,
        number: usize,
        src: usize,
    },
    /// A ```` ```{=html} ```` block, passed through untouched.
    RawHtml {
        text: String,
        src: usize,
    },
}

/// A parsed document, ready for either backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    pub meta: Meta,
    pub layout: Layout,
    pub blocks: Vec<Block>,
    pub source: PathBuf,
}

/// Read and parse a file.
pub fn load(path: &Path, overrides: &Overrides) -> std::io::Result<Document> {
    let text = std::fs::read_to_string(path)?;
    Ok(parse(&text, path, overrides))
}

/// Parse source text that came from `source`.
pub fn parse(text: &str, source: &Path, overrides: &Overrides) -> Document {
    let (meta, body, consumed) = split_front_matter(text);
    let base = source.parent().unwrap_or_else(|| Path::new("."));
    let blocks = parse_owned(&body, base, 0, consumed);
    finish(meta, blocks, source, overrides)
}

/// Split off a leading YAML block.
///
/// rmarkdown nests the options this tool cares about under `output:`:
///
/// ```yaml
/// output:
///   html_document:
///     toc: true
///     number_sections: yes
/// ```
///
/// so indented keys are read too. A top level key wins over an indented one.
/// Front matter, the body, and how many lines the front matter occupied.
///
/// The count matters: source line numbers are reported relative to the file, so
/// every block has to be shifted by the lines the header took.
pub fn split_front_matter(text: &str) -> (Meta, Vec<String>, usize) {
    let normalized = text.replace("\r\n", "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();
    let mut meta = Meta::default();
    let opener = lines.first().map(|line| line.trim()).unwrap_or("");
    if opener != "---" && opener != "----" {
        return (meta, lines.iter().map(|line| line.to_string()).collect(), 0);
    }
    let mut nested = Meta::default();
    let mut end = lines.len();
    for (index, line) in lines.iter().enumerate().skip(1) {
        let trimmed = line.trim();
        if trimmed == "---" || trimmed == "..." {
            end = index + 1;
            break;
        }
        if let Some((key, value)) = top_level_key(line) {
            apply_key(&mut meta, &key, &value);
        } else if let Some((key, value)) = indented_key(line) {
            apply_key(&mut nested, &key, &value);
        }
    }
    merge(&mut meta, nested);
    let consumed = end.min(lines.len());
    let body = lines[consumed..]
        .iter()
        .map(|line| line.to_string())
        .collect();
    (meta, body, consumed)
}

fn top_level_key(line: &str) -> Option<(String, String)> {
    if line.starts_with([' ', '\t']) {
        return None;
    }
    split_key_value(line)
}

fn indented_key(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim_start();
    if trimmed.len() == line.len() || trimmed.is_empty() {
        return None;
    }
    split_key_value(trimmed)
}

fn split_key_value(line: &str) -> Option<(String, String)> {
    let (key, value) = line.split_once(':')?;
    let key = key.trim();
    let value = value.trim();
    if key.is_empty() || value.is_empty() {
        return None;
    }
    if !key
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
    {
        return None;
    }
    Some((key.to_string(), unquote(value)))
}

fn unquote(value: &str) -> String {
    let value = value.trim();
    let mut chars = value.chars();
    if let (Some(first), Some(last)) = (chars.next(), value.chars().last()) {
        if (first == '"' || first == '\'') && first == last && value.len() >= 2 {
            return value[1..value.len() - 1].to_string();
        }
    }
    value.to_string()
}

fn apply_key(meta: &mut Meta, key: &str, value: &str) {
    match key {
        "title" => meta.title = Some(value.to_string()),
        "author" => meta.author = Some(value.to_string()),
        "date" => meta.date = Some(value.to_string()),
        "toc" => meta.toc = Some(as_flag(value)),
        "number_sections" => meta.number_sections = Some(as_flag(value)),
        "toc_depth" => meta.toc_depth = value.trim().parse().ok(),
        _ => {}
    }
}

fn merge(meta: &mut Meta, nested: Meta) {
    if meta.title.is_none() {
        meta.title = nested.title;
    }
    if meta.author.is_none() {
        meta.author = nested.author;
    }
    if meta.date.is_none() {
        meta.date = nested.date;
    }
    if meta.toc.is_none() {
        meta.toc = nested.toc;
    }
    if meta.number_sections.is_none() {
        meta.number_sections = nested.number_sections;
    }
    if meta.toc_depth.is_none() {
        meta.toc_depth = nested.toc_depth;
    }
}

/// `yes`, `true`, `t`, `y` mean on. Front matter is written both ways.
pub fn as_flag(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "yes" | "true" | "t" | "y" | "on"
    )
}

/// ```` ```{r label, echo=FALSE} ```` into engine, label and options.
///
/// The header is accepted with or without commas, so `{r child="x.Rmd"}` and
/// `{r, child="x.Rmd"}` both parse. A token without `=` that is not the engine
/// is the chunk label.
pub fn parse_chunk_info(info: &str) -> (String, String, Vec<(String, String)>) {
    let info = info.trim();
    if !(info.starts_with('{') && info.ends_with('}')) {
        let engine = info.split_whitespace().next().unwrap_or("").to_string();
        return (engine, String::new(), Vec::new());
    }
    let inner = info[1..info.len() - 1].trim();
    if inner.is_empty() {
        return (String::new(), String::new(), Vec::new());
    }
    if inner.starts_with('=') {
        return (inner.to_string(), String::new(), Vec::new());
    }
    let (engine, rest) = split_engine(inner);
    let mut label = String::new();
    let mut options = Vec::new();
    for token in split_top_level(rest) {
        match token.split_once('=') {
            Some((key, value)) => options.push((key.trim().to_string(), unquote(value))),
            None => label = token.trim().to_string(),
        }
    }
    (engine, label, options)
}

fn split_engine(inner: &str) -> (String, &str) {
    let end = inner
        .char_indices()
        .take_while(|(_, c)| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-' | '_'))
        .map(|(index, c)| index + c.len_utf8())
        .last()
        .unwrap_or(0);
    (
        inner[..end].to_string(),
        inner[end..].trim_start_matches([',', ' ']),
    )
}

/// Split on commas that are not inside quotes or brackets.
fn split_top_level(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut depth = 0i32;
    for ch in text.chars() {
        match ch {
            _ if quote == Some(ch) => {
                quote = None;
                current.push(ch);
            }
            '\'' | '"' if quote.is_none() => {
                quote = Some(ch);
                current.push(ch);
            }
            '(' | '[' if quote.is_none() => {
                depth += 1;
                current.push(ch);
            }
            ')' | ']' if quote.is_none() => {
                depth -= 1;
                current.push(ch);
            }
            ',' if quote.is_none() && depth == 0 => {
                let part = current.trim().to_string();
                if !part.is_empty() {
                    parts.push(part);
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    let part = current.trim().to_string();
    if !part.is_empty() {
        parts.push(part);
    }
    parts
}

/// Quarto style `#| echo: false` lines at the top of a chunk body.
fn pull_hash_pipe(body: &[String]) -> (Vec<(String, String)>, Vec<String>) {
    let mut options = Vec::new();
    let mut rest = Vec::new();
    let mut leading = true;
    for line in body {
        if leading {
            let trimmed = line.trim_start();
            if let Some(rest_of_line) = trimmed.strip_prefix("#|") {
                if let Some((key, value)) = rest_of_line.split_once(':') {
                    options.push((key.trim().to_string(), unquote(value)));
                    continue;
                }
            }
            if !trimmed.is_empty() {
                leading = false;
            }
        }
        rest.push(line.clone());
    }
    (options, rest)
}

/// knitr splits these three ways: `FALSE` off, `TRUE` on, `'hide'` also off.
pub fn option_off(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "false" | "f" | "no" | "hide" | "off"
    )
}

fn option_is(options: &[(String, String)], key: &str, default: bool) -> bool {
    match options.iter().rev().find(|(name, _)| name == key) {
        Some((_, value)) => !option_off(value),
        None => default,
    }
}

fn option_value<'a>(options: &'a [(String, String)], key: &str) -> Option<&'a str> {
    options
        .iter()
        .rev()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

/// `offset` is the number of lines the front matter took, so block source lines
/// match the file the reader is looking at.
fn parse_owned(lines: &[String], base: &Path, depth: usize, offset: usize) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    let mut prose: Vec<&str> = Vec::new();
    let mut prose_start = 1usize;
    let mut index = 0usize;

    while index < lines.len() {
        let line = lines[index].as_str();

        if let Some((marker, info)) = fence_open(line) {
            flush_prose(&mut prose, &mut blocks, prose_start);
            let mut body = Vec::new();
            let mut scan = index + 1;
            while scan < lines.len() && !closes_fence(&lines[scan], &marker) {
                body.push(lines[scan].clone());
                scan += 1;
            }
            let src = index + 1 + offset;
            blocks.extend(chunk_blocks(&info, &body, base, depth, src));
            index = scan + 1;
            continue;
        }

        if let Some((level, text)) = atx_heading(line) {
            flush_prose(&mut prose, &mut blocks, prose_start);
            let text = strip_comments(text).trim().to_string();
            blocks.push(Block::Heading {
                level,
                anchor: slug(&clean_text(&text)),
                text,
                number: None,
                src: index + 1 + offset,
            });
            index += 1;
            continue;
        }

        if prose.is_empty() {
            prose_start = index + 1 + offset;
        }
        prose.push(line);
        index += 1;
    }
    flush_prose(&mut prose, &mut blocks, prose_start);
    blocks
}

fn flush_prose(prose: &mut Vec<&str>, blocks: &mut Vec<Block>, start: usize) {
    if prose.is_empty() {
        return;
    }
    let raw = prose.join("\n");
    prose.clear();
    let text = strip_comments(&unwrap_inline_r(&raw));
    let trimmed = text.trim_matches('\n');
    if !trimmed.trim().is_empty() {
        blocks.push(Block::Markdown {
            text: trimmed.to_string(),
            src: start,
        });
    }
}

/// Opening fence with its marker and info string.
fn fence_open(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim_start();
    let indent = line.len() - trimmed.len();
    if indent > 3 {
        return None;
    }
    let marker_char = trimmed.chars().next()?;
    if marker_char != '`' && marker_char != '~' {
        return None;
    }
    let count = trimmed.chars().take_while(|c| *c == marker_char).count();
    if count < 3 {
        return None;
    }
    let info = trimmed[count..].trim().to_string();
    Some((marker_char.to_string().repeat(count), info))
}

fn closes_fence(line: &str, marker: &str) -> bool {
    let trimmed = line.trim();
    let marker_char = marker.chars().next().unwrap_or('`');
    let count = trimmed.chars().take_while(|c| *c == marker_char).count();
    count >= marker.len() && trimmed[count..].trim().is_empty()
}

/// ATX heading: up to three spaces of indent, one to six hashes, then a space.
fn atx_heading(line: &str) -> Option<(u8, &str)> {
    let trimmed = line.trim_start();
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &trimmed[hashes..];
    if !rest.is_empty() && !rest.starts_with(' ') && !rest.starts_with('\t') {
        return None;
    }
    Some((hashes as u8, strip_closing_hashes(rest)))
}

/// Drop a trailing `#` run, which CommonMark only counts when a space
/// precedes it. `# C#` is the heading `C#`, `# C #` is the heading `C`.
fn strip_closing_hashes(text: &str) -> &str {
    let trimmed = text.trim();
    let hashes = trimmed.chars().rev().take_while(|c| *c == '#').count();
    if hashes == 0 {
        return trimmed;
    }
    let without = &trimmed[..trimmed.len() - hashes];
    match without.chars().last() {
        Some(c) if c.is_whitespace() => without.trim_end(),
        _ => trimmed,
    }
}

fn chunk_blocks(info: &str, body: &[String], base: &Path, depth: usize, src: usize) -> Vec<Block> {
    let (engine, label, mut options) = parse_chunk_info(info);
    let (hash_options, body) = pull_hash_pipe(body);
    options.extend(hash_options);
    let code = body.join("\n");
    let code = code.trim_end_matches('\n').to_string();

    if let Some(child) = option_value(&options, "child") {
        if depth < MAX_CHILD_DEPTH {
            let path = if base.as_os_str().is_empty() {
                PathBuf::from(child)
            } else {
                base.join(child)
            };
            match std::fs::read_to_string(&path) {
                Ok(text) => {
                    let (_, child_lines, child_consumed) = split_front_matter(&text);
                    let child_base = path.parent().unwrap_or(base).to_path_buf();
                    return parse_owned(&child_lines, &child_base, depth + 1, child_consumed);
                }
                Err(_) => {
                    return vec![Block::Note {
                        text: format!("missing child document: {}", path.display()),
                        src,
                    }]
                }
            }
        }
        return Vec::new();
    }

    if !option_is(&options, "include", true) {
        return Vec::new();
    }

    if engine.starts_with('=') {
        return vec![Block::RawHtml { text: code, src }];
    }

    let mut blocks = Vec::new();
    let label = label.trim().to_string();
    let named = if label.is_empty() {
        "unnamed".to_string()
    } else {
        label.clone()
    };
    let plain = engine.is_empty();

    if !plain && !option_is(&options, "echo", true) {
        blocks.push(Block::Note {
            text: format!("chunk {named}: code hidden (echo=FALSE)"),
            src,
        });
    } else {
        if !plain && !option_is(&options, "eval", true) {
            blocks.push(Block::Note {
                text: format!("chunk {named}: not evaluated (eval=FALSE), source only"),
                src,
            });
        }
        blocks.push(Block::Code {
            lang: language_for(&engine),
            engine: engine.clone(),
            label,
            code,
            src,
        });
    }

    if let Some(caption) = option_value(&options, "fig.cap") {
        blocks.push(Block::Figure {
            caption: strip_comments(caption).trim().to_string(),
            number: 0,
            src,
        });
    }
    blocks
}

fn language_for(engine: &str) -> String {
    match engine {
        "sh" | "shell" => "bash".to_string(),
        other => other.to_string(),
    }
}

/// Remove HTML comments. They are scaffolding, not content.
fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        match rest[start..].find("-->") {
            Some(end) => rest = &rest[start + end + 3..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// Drop the `r` from inline R code so an unevaluated `` `r x` `` reads as `x`.
fn unwrap_inline_r(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b'`' {
            out.push(bytes[index] as char);
            index += 1;
            continue;
        }
        let mut open = index;
        while open < bytes.len() && bytes[open] == b'`' {
            open += 1;
        }
        let fence = open - index;
        let mut close = open;
        let mut found = None;
        while close + fence <= bytes.len() {
            if bytes[close..close + fence].iter().all(|b| *b == b'`') {
                found = Some(close);
                break;
            }
            close += 1;
        }
        match found {
            Some(close) => {
                let content = &text[open..close];
                out.push_str(&"`".repeat(fence));
                out.push_str(&trim_inline_r(content));
                out.push_str(&"`".repeat(fence));
                index = close + fence;
            }
            None => {
                out.push_str(&text[index..]);
                break;
            }
        }
    }
    out
}

/// Mark inline R with an invisible separator, drop its `r` prefix.
///
/// The marker lets the HTML backend keep the `rinline` class, so a reader can
/// still see which values the code would have produced, while the terminal
/// strips it and shows a plain code span.
fn trim_inline_r(content: &str) -> String {
    let trimmed = content.trim();
    match trimmed.strip_prefix("r ") {
        Some(rest) => format!("{INLINE_R_MARKER}{}", rest.trim()),
        None => content.to_string(),
    }
}

/// Strip markdown emphasis and HTML tags from a heading or caption.
fn clean_text(text: &str) -> String {
    let text = strip_comments(text);
    let mut out = String::with_capacity(text.len());
    let mut in_tag = false;
    for ch in text.chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if in_tag => {}
            _ => out.push(ch),
        }
    }
    out.trim().to_string()
}

/// Stable heading anchor, mirroring what the HTML backend writes.
pub fn slug(text: &str) -> String {
    let mut out = String::new();
    let mut last_dash = true;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let slug = out.trim_matches('-').to_string();
    if slug.is_empty() {
        "section".to_string()
    } else {
        slug
    }
}

fn finish(meta: Meta, mut blocks: Vec<Block>, source: &Path, overrides: &Overrides) -> Document {
    let layout = Layout {
        toc: overrides.toc.or(meta.toc).unwrap_or(false),
        toc_depth: overrides
            .toc_depth
            .or(meta.toc_depth)
            .unwrap_or(DEFAULT_TOC_DEPTH),
        number_sections: overrides
            .number_sections
            .or(meta.number_sections)
            .unwrap_or(false),
    };

    let mut figure = 0usize;
    let mut counters = [0usize; 6];
    let mut seen_anchors: Vec<String> = Vec::new();
    for block in blocks.iter_mut() {
        match block {
            Block::Figure { number, .. } => {
                figure += 1;
                *number = figure;
            }
            Block::Heading {
                level,
                number,
                anchor,
                ..
            } => {
                // Two headings can slug the same, for example `#` and `##`,
                // which both reduce to nothing. Keep the ids distinct so
                // contents links land where they say they do.
                let base = anchor.clone();
                let mut candidate = base.clone();
                let mut suffix = 2;
                while seen_anchors.contains(&candidate) {
                    candidate = format!("{base}-{suffix}");
                    suffix += 1;
                }
                seen_anchors.push(candidate.clone());
                *anchor = candidate;
                if layout.number_sections {
                    let level = (*level as usize).clamp(1, 6);
                    counters[level - 1] += 1;
                    for counter in counters.iter_mut().skip(level) {
                        *counter = 0;
                    }
                    *number = Some(
                        counters[..level]
                            .iter()
                            .map(|count| count.to_string())
                            .collect::<Vec<_>>()
                            .join("."),
                    );
                } else {
                    *number = None;
                }
            }
            _ => {}
        }
    }

    Document {
        meta,
        layout,
        blocks,
        source: source.to_path_buf(),
    }
}

/// Filtered markdown, the shape GitHub previews and the terminal viewers read.
pub fn to_markdown(doc: &Document) -> String {
    let mut out = String::new();
    if let Some(title) = &doc.meta.title {
        out.push_str(&format!("# {title}\n"));
        for (label, value) in [("Author", &doc.meta.author), ("Date", &doc.meta.date)] {
            if let Some(value) = value {
                out.push_str(&format!("{label}: {value}\n"));
            }
        }
        out.push('\n');
    }
    for block in &doc.blocks {
        match block {
            Block::Heading {
                level,
                text,
                number,
                ..
            } => {
                out.push_str(&"#".repeat(*level as usize));
                out.push(' ');
                if let Some(number) = number {
                    out.push_str(number);
                    out.push(' ');
                }
                out.push_str(&text.replace(INLINE_R_MARKER, ""));
                out.push_str("\n\n");
            }
            Block::Markdown { text, .. } => {
                out.push_str(text.replace(INLINE_R_MARKER, "").trim_end());
                out.push_str("\n\n");
            }
            Block::Code { lang, code, .. } => {
                out.push_str(&format!("```{lang}\n{code}\n```\n\n"));
            }
            Block::RawHtml { text, .. } => {
                out.push_str(text.trim_end());
                out.push_str("\n\n");
            }
            // Notes and figure captions have no markdown equivalent worth
            // publishing: there is no code to run and no image to point at.
            Block::Note { .. } | Block::Figure { .. } => {}
        }
    }
    out.trim_end().to_string() + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html::{render, HtmlOptions};
    use crate::theme::ThemeName;

    fn parse_with(text: &str, overrides: Overrides) -> Document {
        parse(text, Path::new("/tmp/rmdt-test.Rmd"), &overrides)
    }

    fn doc(text: &str) -> Document {
        parse_with(text, Overrides::default())
    }

    fn html(text: &str) -> String {
        render(&doc(text), &HtmlOptions::default())
    }

    fn html_with(text: &str, options: HtmlOptions) -> String {
        render(&doc(text), &options)
    }

    fn md(text: &str) -> String {
        to_markdown(&doc(text))
    }

    /// A scratch directory that goes away when the test ends.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("rmdt-{}-{}", name, std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("scratch dir");
            Self(dir)
        }

        fn write(&self, name: &str, text: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, text).expect("write fixture");
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn echo_false_hides_the_code() {
        let out = html("```{r private, echo=FALSE}\nsecret_call()\n```\n");
        assert!(!out.contains("secret_call"));
        assert!(out.contains("code hidden (echo=FALSE)"));
    }

    #[test]
    fn include_false_drops_the_chunk_entirely() {
        let out = html("```{sql, include=FALSE}\nSELECT 1;\n```\n");
        assert!(!out.contains("SELECT 1"));
        assert!(!out.contains("<div class=\"chunk-note\">"));
    }

    #[test]
    fn eval_false_keeps_the_source() {
        let out = html("```{r slow, eval=FALSE}\ntrain_model()\n```\n");
        assert!(out.contains("train_model()"));
        assert!(out.contains("not evaluated (eval=FALSE)"));
    }

    #[test]
    fn a_plain_chunk_is_shown_with_its_label() {
        let out = html("```{r my-label}\nx <- 1\n```\n");
        assert!(out.contains("x &lt;- 1"));
        assert!(out.contains("my-label"));
        assert!(out.contains(r#"class="chunk lang-r""#));
    }

    #[test]
    fn quarto_hash_pipe_options_are_read() {
        let out = html("```{r}\n#| echo: false\nhidden_here\n```\n");
        assert!(!out.contains("hidden_here"));
        assert!(out.contains("code hidden"));
    }

    #[test]
    fn engine_without_label_still_gets_a_class() {
        let out = html("```{python}\nprint(1)\n```\n");
        assert!(out.contains(r#"class="chunk lang-python""#));
    }

    #[test]
    fn child_documents_are_inlined() {
        let scratch = Scratch::new("child");
        scratch.write("setup.Rmd", "```{r setup}\nlibrary(dplyr)\n```\n");
        let main = scratch.write("main.Rmd", "```{r child=\"setup.Rmd\"}\n```\n");
        let document = load(&main, &Overrides::default()).expect("load");
        let out = render(&document, &HtmlOptions::default());
        assert!(out.contains("library(dplyr)"));
        assert!(!out.contains("child="));
    }

    #[test]
    fn a_missing_child_is_reported_not_swallowed() {
        let scratch = Scratch::new("missing-child");
        let main = scratch.write("main.Rmd", "```{r child=\"nope.Rmd\"}\n```\n");
        let document = load(&main, &Overrides::default()).expect("load");
        let out = render(&document, &HtmlOptions::default());
        assert!(out.contains("missing child document"));
    }

    #[test]
    fn pandoc_raw_blocks_pass_through() {
        let out = html("```{=html}\n<div class='raw'>hi</div>\n```\n");
        assert!(out.contains("<div class='raw'>hi</div>"));
    }

    #[test]
    fn a_four_backtick_fence_may_wrap_three() {
        let out = html("````\n```{r}\ninner\n```\n````\n");
        assert!(out.contains("```{r}"));
        assert!(out.contains("inner"));
    }

    #[test]
    fn figure_captions_are_numbered() {
        let out = html(
            "```{r a, fig.cap=\"First plot\"}\nplot(1)\n```\n\n\
             ```{r b, fig.cap=\"Second plot\"}\nplot(2)\n```\n",
        );
        assert!(out.contains("<figcaption>Figure 1: First plot</figcaption>"));
        assert!(out.contains("<figcaption>Figure 2: Second plot</figcaption>"));
        assert_eq!(out.matches("figure not rendered").count(), 2);
    }

    #[test]
    fn a_comment_inside_a_chunk_is_not_a_heading() {
        let out = html("```{r}\n# not a heading\nx <- 1\n```\n");
        assert!(!out.contains("<h1"));
        assert!(out.contains("# not a heading"));
    }

    /// Everything above `#[cfg(test)]`, so a scan does not find its own literals.
    fn production_source(source: &str) -> &str {
        source.split("\n#[cfg(test)]").next().unwrap_or(source)
    }

    #[test]
    fn the_front_end_has_no_execution_primitive() {
        let source = production_source(include_str!("rmd.rs"));
        for forbidden in ["eval(", "exec(", "Command::new", "std::process"] {
            assert!(
                !source.contains(forbidden),
                "the front end must not contain {forbidden}"
            );
        }
    }

    #[test]
    fn only_the_editor_and_the_browser_opener_start_a_process() {
        let mut programs = Vec::new();
        let main = format!(
            "{}{}",
            production_source(include_str!("main.rs")),
            production_source(include_str!("tui.rs"))
        );
        for chunk in main.split("Command::new(").skip(1) {
            let argument = chunk.split(')').next().unwrap_or("").trim();
            programs.push(argument.to_string());
        }
        programs.sort();
        assert_eq!(
            programs,
            vec![
                "\"open\"".to_string(),
                "\"xdg-open\"".to_string(),
                "editor".to_string()
            ],
            "term calls: {:?}",
            programs
        );
        assert!(production_source(include_str!("tui.rs")).contains(r#"std::env::var("EDITOR")"#));
    }

    #[test]
    fn inline_r_is_not_evaluated() {
        let out = html("Value: `r 1 + 1` here.\n");
        assert!(out.contains("1 + 1"));
        assert!(out.contains(r#"class="rinline""#));
        assert!(!out.contains(">2<"));
    }

    #[test]
    fn the_inline_r_marker_never_reaches_the_reader() {
        let text = "A `r nrow(df)` value and `code` too.\n";
        let page = html(text);
        assert!(!page.contains(INLINE_R_MARKER));
        assert_eq!(page.matches(r#"class="rinline""#).count(), 1);
        assert!(!md(text).contains(INLINE_R_MARKER));
    }

    #[test]
    fn front_matter_becomes_the_document_header() {
        let out = html("---\ntitle: \"Report\"\nauthor: \"me\"\n---\n\n## Section\n");
        assert!(out.contains("<title>Report</title>"));
        assert!(out.contains(r#"<div class="title">Report</div>"#));
        assert!(out.contains("<div>me</div>"));
        assert!(out.contains(r#"<h2 id="section">Section</h2>"#));
    }

    #[test]
    fn nested_front_matter_options_are_read() {
        let text = "---\ntitle: T\noutput:\n  html_document:\n    toc: true\n    \
                    toc_depth: 2\n    number_sections: yes\n---\n\n# One\n\n## Two\n\n### Three\n";
        let document = doc(text);
        assert!(document.layout.toc);
        assert_eq!(document.layout.toc_depth, 2);
        assert!(document.layout.number_sections);
    }

    #[test]
    fn table_cells_keep_their_alignment() {
        let out = html("| a | b |\n|:--|--:|\n| 1 | 2 |\n");
        assert!(out.contains(r#"<th style="text-align: left">a</th>"#));
        assert!(out.contains(r#"<th style="text-align: right">b</th>"#));
    }

    #[test]
    fn nested_lists_nest() {
        let out = html("- top\n  - sub\n- top2\n");
        assert_eq!(out.matches("<ul>").count(), 2);
        assert_eq!(out.matches("<li>").count(), 3);
    }

    #[test]
    fn a_lazy_continuation_stays_in_its_item() {
        let out = html("- a bullet wrapped over\ntwo source lines\n- second bullet\n");
        assert_eq!(out.matches("<ul>").count(), 1);
        assert_eq!(out.matches("<li>").count(), 2);
        assert!(out.contains("a bullet wrapped over"));
        assert!(out.contains("two source lines"));
    }

    #[test]
    fn an_indented_continuation_stays_in_its_item() {
        let out = html("- wrapped\n  continuation line\n- next\n");
        assert_eq!(out.matches("<ul>").count(), 1);
        assert_eq!(out.matches("<li>").count(), 2);
    }

    #[test]
    fn a_heading_after_a_list_is_not_absorbed() {
        let out = html("- item\n\n## Next\n");
        assert_eq!(out.matches("<li>").count(), 1);
        assert!(out.contains(r#"<h2 id="next">Next</h2>"#));
    }

    #[test]
    fn math_keeps_the_delimiters_mathjax_needs() {
        let out = html("Inline $E = mc^2$ and:\n\n$$\n\\hat\\beta\n$$\n");
        assert!(out.contains(r#"<span class="math">\(E = mc^2\)</span>"#));
        assert!(out.contains(r#"<span class="math-display">$$"#));
        assert!(out.contains("mathjax"));
    }

    #[test]
    fn mathjax_is_left_out_when_asked() {
        let out = html_with(
            "Inline $E = mc^2$.\n",
            HtmlOptions {
                math: false,
                ..HtmlOptions::default()
            },
        );
        assert!(!out.contains("mathjax"));
        assert!(out.contains("<span class=\"math\">$E = mc^2$</span>"));
    }

    #[test]
    fn a_document_without_math_does_not_pull_mathjax_in() {
        let out = html("Just prose, no formulas, and a $5 price tag.\n");
        assert!(!out.contains("mathjax"));
    }

    #[test]
    fn autolinks_and_inline_html_survive() {
        let out = html("Press <kbd>Ctrl</kbd> and see <https://example.com/x>.\n");
        assert!(out.contains("<kbd>Ctrl</kbd>"));
        assert!(out.contains(r#"<a href="https://example.com/x">https://example.com/x</a>"#));
    }

    #[test]
    fn underscores_in_a_link_are_not_taken_for_emphasis() {
        let out = html("See [doc](https://example.com/a_b_c_d).\n");
        assert!(out.contains(r#"href="https://example.com/a_b_c_d""#));
    }

    #[test]
    fn a_badge_image_inside_a_link_keeps_both() {
        let out = html("[![coverage](https://img.shields.io/badge.svg)](https://codecov.io/x)\n");
        assert!(out.contains(r#"<a href="https://codecov.io/x">"#));
        assert!(out.contains(r#"alt="coverage""#));
        assert!(!out.contains(INLINE_R_MARKER));
    }

    #[test]
    fn html_comments_are_dropped() {
        let out = html("<!-- badges: start -->\n\ntext\n");
        assert!(!out.contains("badges: start"));
        assert!(out.contains("<p>text</p>"));
    }

    #[test]
    fn an_image_keeps_its_alt_text_and_title() {
        let out = html("![a caption](figure/plot.png \"optional title\")\n");
        assert!(out.contains(r#"alt="a caption""#));
        assert!(out.contains(r#"title="optional title""#));
    }

    #[test]
    fn html_inside_a_code_span_is_escaped() {
        let out = html("Use `<div>` tags.\n");
        assert!(out.contains("<code>&lt;div&gt;</code>"));
    }

    #[test]
    fn headings_may_carry_inline_html() {
        let out = html("# bears <img src='logo.png' align=\"right\" />\n");
        assert!(out.contains("logo.png"));
        assert!(out.contains("<h1"));
    }

    #[test]
    fn carriage_returns_are_tolerated() {
        let out = html("## Head\r\n\r\ntext\r\n");
        assert!(out.contains(r#"<h2 id="head">Head</h2>"#));
    }

    #[test]
    fn front_matter_turns_on_the_toc_and_numbering() {
        let out =
            html("---\ntoc: true\nnumber_sections: yes\n---\n\n# One\n\ntext\n\n## Sub\n\nmore\n");
        assert!(out.contains(r#"<nav class="toc">"#));
        assert!(out.contains(r#"<span class="secnum">1</span>One"#));
        assert!(out.contains(r#"<span class="secnum">1.1</span>Sub"#));
        assert!(out.contains(r#"class="toc-h2""#));
    }

    #[test]
    fn numbering_restarts_deeper_levels() {
        let numbers: Vec<String> = parse_with(
            "# A\n\n## B\n\n## C\n\n# D\n",
            Overrides {
                number_sections: Some(true),
                ..Overrides::default()
            },
        )
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Heading { number, .. } => number.clone(),
            _ => None,
        })
        .collect();
        assert_eq!(numbers, vec!["1", "1.1", "1.2", "2"]);
    }

    #[test]
    fn a_command_line_flag_beats_the_front_matter() {
        let text = "---\ntoc: true\n---\n\n# One\n\n## Two\n";
        let off = render(
            &parse_with(
                text,
                Overrides {
                    toc: Some(false),
                    ..Overrides::default()
                },
            ),
            &HtmlOptions::default(),
        );
        assert!(!off.contains("<nav class=\"toc\">"));
        let on = render(
            &parse_with(
                "# One\n\n## Two\n",
                Overrides {
                    toc: Some(true),
                    ..Overrides::default()
                },
            ),
            &HtmlOptions::default(),
        );
        assert!(on.contains(r#"<nav class="toc">"#));
    }

    #[test]
    fn the_toc_depth_limits_what_is_listed() {
        let out = render(
            &parse_with(
                "# One\n\n## Two\n\n### Three\n",
                Overrides {
                    toc: Some(true),
                    toc_depth: Some(2),
                    ..Overrides::default()
                },
            ),
            &HtmlOptions::default(),
        );
        assert!(out.contains(r##"href="#two""##));
        assert!(!out.contains(r##"href="#three""##));
    }

    #[test]
    fn a_single_heading_gets_no_toc() {
        let out = render(
            &parse_with(
                "# Only one\n\ntext\n",
                Overrides {
                    toc: Some(true),
                    ..Overrides::default()
                },
            ),
            &HtmlOptions::default(),
        );
        assert!(!out.contains("<nav class=\"toc\">"));
    }

    #[test]
    fn the_banner_can_be_dropped() {
        let with = html("text\n");
        assert!(with.contains("code not executed"));
        let without = html_with(
            "text\n",
            HtmlOptions {
                banner: false,
                ..HtmlOptions::default()
            },
        );
        assert!(!without.contains("code not executed"));
    }

    #[test]
    fn the_themes_have_distinct_metrics() {
        assert!(crate::theme::stylesheet(ThemeName::Rmarkdown).contains("--page-width:940px"));
        assert!(crate::theme::stylesheet(ThemeName::Github).contains("--page-width:820px"));
        assert!(crate::theme::stylesheet(ThemeName::Plain).contains("--page-width:760px"));
        assert!(crate::theme::stylesheet(ThemeName::Rmarkdown).contains("Helvetica Neue"));
    }

    #[test]
    fn anchors_are_stable_slugs() {
        assert_eq!(slug("Why"), "why");
        assert_eq!(slug("Installation and setup"), "installation-and-setup");
        assert_eq!(slug("Step 1: outline"), "step-1-outline");
        assert_eq!(slug("!!!"), "section");
    }

    #[test]
    fn markdown_keeps_chunks_and_filters_hidden_ones() {
        let out = md("```{r keep}\nx <- 1\n```\n\n```{r gone, echo=FALSE}\ny <- 2\n```\n");
        assert!(out.contains("```r\nx <- 1\n```"));
        assert!(!out.contains("y <- 2"));
        assert!(!out.contains("echo=FALSE"));
    }

    #[test]
    fn markdown_unwraps_inline_r() {
        let out = md("Count is `r n`.\n");
        assert!(out.contains("Count is `n`."));
        assert!(!out.contains(INLINE_R_MARKER));
    }

    #[test]
    fn markdown_carries_no_notes_or_figures() {
        let out = md("```{r a, eval=FALSE, fig.cap=\"Plot\"}\nplot(1)\n```\n");
        assert!(out.contains("plot(1)"));
        assert!(!out.contains("eval=FALSE, source only"));
        assert!(!out.contains("Figure 1"));
    }

    #[test]
    fn markdown_leads_with_the_title() {
        let out = md("---\ntitle: Report\nauthor: me\n---\n\n# One\n");
        assert!(out.starts_with("# Report\nAuthor: me\n"));
    }

    #[test]
    fn chunk_headers_parse_with_and_without_commas() {
        let (engine, label, options) = parse_chunk_info("{r setup, echo=FALSE, fig.width=6}");
        assert_eq!(engine, "r");
        assert_eq!(label, "setup");
        assert_eq!(options.len(), 2);
        let (engine, label, options) = parse_chunk_info("{r child=\"x.Rmd\"}");
        assert_eq!(engine, "r");
        assert_eq!(label, "");
        assert_eq!(options[0].0, "child");
        let (engine, ..) = parse_chunk_info("{python py-chunk}");
        assert_eq!(engine, "python");
        let (engine, ..) = parse_chunk_info("{=html}");
        assert_eq!(engine, "=html");
        let (engine, ..) = parse_chunk_info("r");
        assert_eq!(engine, "r");
    }

    #[test]
    fn option_values_keep_commas_inside_quotes() {
        let (_, _, options) = parse_chunk_info("{r shown, fig.cap=\"A caption, with a comma\"}");
        assert_eq!(options[0].1, "A caption, with a comma");
    }

    #[test]
    fn source_lines_are_numbered_in_the_file_not_the_body() {
        // Four lines of front matter, so the heading is on line 6 of the file.
        let text = "---\ntitle: T\ntoc: true\n---\n\n# Heading\n\ntext\n\n```{r a}\nx <- 1\n```\n";
        let document = doc(text);
        let heading = document.blocks.first().expect("a heading");
        assert_eq!(heading_src(heading), 6, "{:?}", document.blocks);
        let code = document
            .blocks
            .iter()
            .find(|block| matches!(block, Block::Code { .. }))
            .expect("a chunk");
        assert_eq!(heading_src(code), 10);
    }

    fn heading_src(block: &Block) -> usize {
        match block {
            Block::Heading { src, .. }
            | Block::Markdown { src, .. }
            | Block::Code { src, .. }
            | Block::Note { src, .. }
            | Block::Figure { src, .. }
            | Block::RawHtml { src, .. } => *src,
        }
    }

    #[test]
    fn a_document_without_front_matter_starts_at_line_one() {
        let document = doc("# Heading\n\ntext\n");
        assert_eq!(heading_src(document.blocks.first().expect("heading")), 1);
    }

    #[test]
    fn document_text_that_looks_like_a_placeholder_survives() {
        let out = html("```{python}\nprint(\"{scripts}\")\n```\n\nand `{content}` too.\n");
        assert!(out.contains("{scripts}"), "the page template ate the text");
        assert!(out.contains("{content}"));
        assert!(out.contains("<html lang=\"en\">"));
    }

    #[test]
    fn math_anywhere_on_the_page_pulls_mathjax_in() {
        for (label, text) in [
            ("paragraph", "Text with $a$ inside.\n"),
            ("heading", "# Energy $E = mc^2$\n"),
            (
                "caption",
                "```{r a, fig.cap=\"Plot of $x^2$\"}\nplot(1)\n```\n",
            ),
        ] {
            let out = html(text);
            assert!(out.contains("mathjax"), "{label} math got no bundle");
        }
        let plain = html("A $5 price and nothing else.\n");
        assert!(!plain.contains("mathjax"), "a dollar sign is not math");
    }

    #[test]
    fn a_heading_ending_in_a_hash_keeps_the_hash() {
        let out = html("# C#\n\n# A B #\n");
        assert!(out.contains(">C#<"), "{out}");
        assert!(out.contains(">A B<"));
        assert!(out.contains(r##"id="c""##), "{out}");
    }

    #[test]
    fn malformed_documents_do_not_panic() {
        let cases = [
            "text\n\n```{r}\nx <- 1\n",
            "---\ntitle: no closing fence\n\n# Heading\n",
            "text with `open span\n",
            "",
            "---\ntitle: T\n---\n",
            "#\n##\n###\n",
            "####### seven hashes\n",
            "````\n```{r}\ninner\n```\n\nstill inside\n````\n",
            "# Head",
            "---\r\ntitle: T\r\n---\r\n\r\n# H\r\n",
            "```{r , , echo = FALSE , }\nx\n```\n",
            "text $a + b\n",
            "```{}}\n\n```{ruby chapter='x'}\nputs 1\n```\n",
        ];
        for case in cases {
            let document = doc(case);
            let page = render(&document, &HtmlOptions::default());
            assert!(page.contains("</html>"), "case {case:?} produced no page");
            let _ = to_markdown(&document);
        }
    }

    #[test]
    fn a_child_document_that_includes_itself_terminates() {
        let scratch = Scratch::new("self-child");
        let main = scratch.write("self.Rmd", "```{r child=\"self.Rmd\"}\n```\n");
        let document = load(&main, &Overrides::default()).expect("load");
        assert!(document.blocks.is_empty());
    }

    #[test]
    fn a_child_cycle_terminates() {
        let scratch = Scratch::new("child-cycle");
        scratch.write("b.Rmd", "```{r child=\"a.Rmd\"}\n```\n");
        let main = scratch.write("a.Rmd", "```{r child=\"b.Rmd\"}\n```\n");
        let document = load(&main, &Overrides::default()).expect("load");
        let _ = render(&document, &HtmlOptions::default());
    }

    #[test]
    fn an_unterminated_fence_still_shows_its_code() {
        let out = html("text\n\n```{r}\nx <- 1\n");
        assert!(out.contains("x &lt;- 1"));
        assert!(out.contains("text"));
    }

    #[test]
    fn headings_that_slug_the_same_get_distinct_anchors() {
        let out = html("#\n\n##\n\n###\n");
        assert!(out.contains(r#"id="section""#));
        assert!(out.contains(r#"id="section-2""#));
        assert!(out.contains(r#"id="section-3""#));
    }

    #[test]
    fn two_real_headings_with_the_same_text_stay_distinct() {
        let out = html("# Notes\n\ntext\n\n# Notes\n");
        assert!(out.contains(r#"id="notes""#));
        assert!(out.contains(r#"id="notes-2""#));
    }

    #[test]
    fn knitr_truthiness_is_respected() {
        for off in ["FALSE", "false", "F", "no", "hide"] {
            assert!(option_off(off), "{off} should read as off");
        }
        for on in ["TRUE", "true", "yes", "1"] {
            assert!(!option_off(on), "{on} should read as on");
        }
        for on in ["yes", "true", "TRUE"] {
            assert!(as_flag(on), "{on} should read as a flag");
        }
    }
}
