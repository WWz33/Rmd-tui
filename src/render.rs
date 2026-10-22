//! Blocks to terminal rows.
//!
//! The renderer in `tui-markdown` returns one flat `Text` for a whole document,
//! which loses any connection between a rendered row and the source line it came
//! from. Building rows one block at a time restores that connection, and it is
//! what makes the outline, "jump to section", position preserving reloads and a
//! future figure overlay possible.
//!
//! Nothing here knows about R Markdown beyond the [`Block`] variants handed to
//! it, so the row and source map machinery is portable.

use ratatui_core::style::{Color, Modifier, Style};
use ratatui_core::text::{Line, Span};
use tui_markdown::{from_str_with_options, CodeTheme, Options, StyleSheet};
use unicode_width::UnicodeWidthChar;

use crate::rmd::{Block, Document, INLINE_R_MARKER};
use crate::theme::{Depth, TuiTheme};

/// What a row is. Decides how it wraps and whether it gets a gutter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowKind {
    Text,
    Heading(u8),
    CodeHeader,
    Code,
    Note,
    Figure,
    Caption,
    Raw,
}

impl RowKind {
    /// Code has to break mid token: a long token has to go somewhere.
    fn breaks_anywhere(self) -> bool {
        self == RowKind::Code || self == RowKind::Raw
    }

    /// Chunks, notes and figures are boxed in the HTML themes, so they get a
    /// gutter here too.
    fn is_boxed(self) -> bool {
        matches!(
            self,
            RowKind::Code | RowKind::Note | RowKind::Figure | RowKind::Caption | RowKind::Raw
        )
    }
}

/// One displayed row, already wrapped to the content width.
#[derive(Debug, Clone)]
pub struct Row {
    pub line: Line<'static>,
    /// One based source line this row came from.
    pub src: usize,
}

impl Row {
    pub fn text(&self) -> String {
        self.line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }
}

/// The rows of a document, and where each block landed in them.
#[derive(Debug, Clone, Default)]
pub struct Layout {
    pub rows: Vec<Row>,
    pub outline: Vec<OutlineEntry>,
    /// First row of each block, in the order the blocks were given.
    pub block_rows: Vec<usize>,
}

/// A heading, with the row it starts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineEntry {
    pub row: usize,
    pub level: u8,
    pub title: String,
}

/// Build the wrapped rows and the outline for a document.
pub fn build(doc: &Document, theme: TuiTheme, code_theme: CodeTheme, width: usize) -> Layout {
    let width = width.max(24);
    let options = Options::new(theme)
        .table_width(width.min(u16::MAX as usize) as u16)
        .table_row_dividers(true)
        .code_theme(code_theme);

    let mut rows: Vec<Row> = Vec::new();
    let mut outline: Vec<OutlineEntry> = Vec::new();
    let mut block_rows: Vec<usize> = Vec::with_capacity(doc.blocks.len());

    for block in &doc.blocks {
        block_rows.push(rows.len());
        match block {
            Block::Heading {
                level,
                text,
                number,
                src,
                ..
            } => {
                let title = match number {
                    Some(number) => format!("{number} {text}"),
                    None => text.clone(),
                };
                let markdown = format!("{} {title}", "#".repeat(*level as usize));
                let rendered = render_markdown(&markdown, &options, theme);
                let start = rows.len();
                push_lines(
                    &mut rows,
                    rendered,
                    RowKind::Heading(*level),
                    *src,
                    width,
                    &theme,
                );
                outline.push(OutlineEntry {
                    row: start,
                    level: *level,
                    title,
                });
            }
            Block::Markdown { text, src } => {
                let rendered = render_markdown(text, &options, theme);
                push_lines(&mut rows, rendered, RowKind::Text, *src, width, &theme);
            }
            Block::Code {
                engine,
                lang,
                label,
                code,
                src,
            } => {
                if !engine.is_empty() {
                    let mut header = String::new();
                    if !label.is_empty() {
                        header.push_str(label);
                        header.push_str("  ");
                    }
                    header.push_str(engine);
                    push_lines(
                        &mut rows,
                        vec![Line::from(Span::styled(
                            header,
                            theme.chrome().add_modifier(Modifier::BOLD),
                        ))],
                        RowKind::CodeHeader,
                        *src,
                        width,
                        &theme,
                    );
                }
                let fence = crate::rmd::fence_for(code);
                let markdown = format!("{fence}{lang}\n{code}\n{fence}");
                let rendered = render_markdown(&markdown, &options, theme);
                push_lines(&mut rows, rendered, RowKind::Code, *src, width, &theme);
                rows.push(blank(*src));
            }
            Block::Note { text, src } => {
                push_lines(
                    &mut rows,
                    vec![Line::from(Span::styled(
                        text.clone(),
                        theme.chrome().add_modifier(Modifier::ITALIC),
                    ))],
                    RowKind::Note,
                    *src,
                    width,
                    &theme,
                );
                rows.push(blank(*src));
            }
            Block::Figure {
                caption,
                number,
                src,
            } => {
                push_lines(
                    &mut rows,
                    vec![Line::from(Span::styled(
                        "figure not rendered (code was not run)",
                        theme.chrome().add_modifier(Modifier::ITALIC),
                    ))],
                    RowKind::Figure,
                    *src,
                    width,
                    &theme,
                );
                push_lines(
                    &mut rows,
                    vec![Line::from(Span::styled(
                        format!("Figure {number}: {caption}"),
                        theme.chrome(),
                    ))],
                    RowKind::Caption,
                    *src,
                    width,
                    &theme,
                );
                rows.push(blank(*src));
            }
            Block::RawHtml { text, src } => {
                let lines: Vec<Line<'static>> = text
                    .lines()
                    .map(|line| Line::from(Span::styled(line.to_string(), theme.html_style())))
                    .collect();
                push_lines(&mut rows, lines, RowKind::Raw, *src, width, &theme);
                rows.push(blank(*src));
            }
        }
    }

    fit_rows(&mut rows, theme.depth);

    Layout {
        rows,
        outline,
        block_rows,
    }
}

/// Round every colour in the rows to one the terminal understands.
///
/// The syntax colours come back from the highlighting theme as truecolour
/// values. A terminal that only knows the 256 colour cube drops those, so an
/// entire code block lands on the default foreground.
fn fit_rows(rows: &mut [Row], depth: Depth) {
    if depth == Depth::TrueColor {
        return;
    }
    // The palette is small enough that a handful of colours repeat across every
    // row, so the nearest one is worth remembering.
    let mut seen: Vec<(Color, Color)> = Vec::new();
    let mut fit = |color: Color| match seen.iter().find(|(from, _)| *from == color) {
        Some((_, to)) => *to,
        None => {
            let to = depth.fit(color);
            seen.push((color, to));
            to
        }
    };
    for row in rows.iter_mut() {
        for span in row.line.spans.iter_mut() {
            span.style.fg = span.style.fg.map(&mut fit);
            span.style.bg = span.style.bg.map(&mut fit);
        }
        row.line.style.fg = row.line.style.fg.map(&mut fit);
        row.line.style.bg = row.line.style.bg.map(&mut fit);
    }
}

fn blank(src: usize) -> Row {
    Row {
        line: Line::default(),
        src,
    }
}

fn push_lines(
    rows: &mut Vec<Row>,
    lines: Vec<Line<'static>>,
    kind: RowKind,
    src: usize,
    width: usize,
    theme: &TuiTheme,
) {
    let gutter = if kind.is_boxed() {
        Some(theme.chrome())
    } else {
        None
    };
    for line in lines {
        rows.extend(wrap(line, src, width, kind.breaks_anywhere(), gutter));
    }
}

/// Run a fragment of markdown through the renderer and take ownership of it.
fn render_markdown(
    markdown: &str,
    options: &Options<TuiTheme>,
    theme: TuiTheme,
) -> Vec<Line<'static>> {
    let math = [theme.math_inline(), theme.math_display()];
    let rendered = from_str_with_options(markdown, options);
    rendered
        .lines
        .into_iter()
        .map(|line| Line {
            spans: line
                .spans
                .into_iter()
                .map(|span| {
                    let text = span.content.replace(INLINE_R_MARKER, "");
                    // The delimiters are noise in a terminal: the style already
                    // says this is a formula, and a line of `$$` does not read
                    // as math.
                    let text = if math.contains(&span.style) {
                        text.trim_matches('$').to_string()
                    } else {
                        text
                    };
                    Span::styled(text, span.style)
                })
                .collect(),
            style: line.style,
            alignment: line.alignment,
        })
        .collect()
}

/// How far a tab moves the cursor, the usual terminal setting. The terminal
/// itself would honour this, but the row has to be measured here first.
const TAB_STOP: usize = 8;

/// Wrap one line to `width` columns, optionally behind a two column gutter.
fn wrap(
    line: Line<'static>,
    src: usize,
    width: usize,
    hard: bool,
    gutter: Option<Style>,
) -> Vec<Row> {
    let prefix_width = if gutter.is_some() { 2 } else { 0 };
    let inner = width.saturating_sub(prefix_width).max(8);
    let line_style = line.style;

    let cells: Vec<(char, Style)> = line
        .spans
        .iter()
        .flat_map(|span| {
            span.content
                .chars()
                .map(move |ch| (ch, span.style))
                .collect::<Vec<_>>()
        })
        .collect();
    if cells.is_empty() {
        return vec![finish_row(Vec::new(), line_style, src, gutter)];
    }

    let mut rows: Vec<Row> = Vec::new();
    let mut current: Vec<(char, Style)> = Vec::new();
    let mut current_width = 0usize;
    let mut last_space: Option<usize> = None;

    for (ch, style) in cells {
        // A tab carries no width of its own and ratatui drops zero width
        // characters, so it is expanded to the next stop here, the way the
        // terminal would have drawn it.
        let tab = ch == '\t';
        let char_width = if tab {
            TAB_STOP - current_width % TAB_STOP
        } else {
            UnicodeWidthChar::width(ch).unwrap_or(0)
        };
        if current_width + char_width > inner && !current.is_empty() {
            let break_at = if hard { None } else { last_space };
            match break_at {
                Some(split) => {
                    let tail = current.split_off(split);
                    trim_trailing_space(&mut current);
                    if !current.is_empty() {
                        rows.push(finish_row(
                            std::mem::take(&mut current),
                            line_style,
                            src,
                            gutter,
                        ));
                    }
                    current = tail;
                    if current.first().map(|(ch, _)| *ch == ' ').unwrap_or(false) {
                        current.remove(0);
                    }
                }
                None => {
                    trim_trailing_space(&mut current);
                    // A row of nothing but spaces is not worth a line.
                    if !current.is_empty() {
                        rows.push(finish_row(
                            std::mem::take(&mut current),
                            line_style,
                            src,
                            gutter,
                        ));
                    }
                }
            }
            current_width = current
                .iter()
                .map(|(ch, _)| UnicodeWidthChar::width(*ch).unwrap_or(0))
                .sum();
            last_space = current.iter().rposition(|(ch, _)| *ch == ' ');
        }
        if ch == ' ' || tab {
            last_space = Some(current.len());
        }
        current_width += char_width;
        if tab {
            current.extend(std::iter::repeat_n((' ', style), char_width));
        } else {
            current.push((ch, style));
        }
    }
    if !current.is_empty() {
        trim_trailing_space(&mut current);
        rows.push(finish_row(current, line_style, src, gutter));
    }
    if rows.is_empty() {
        rows.push(finish_row(Vec::new(), line_style, src, gutter));
    }
    rows
}

fn trim_trailing_space(cells: &mut Vec<(char, Style)>) {
    while cells.last().map(|(ch, _)| *ch == ' ').unwrap_or(false) {
        cells.pop();
    }
}

fn finish_row(
    cells: Vec<(char, Style)>,
    line_style: Style,
    src: usize,
    gutter: Option<Style>,
) -> Row {
    let mut spans = Vec::new();
    if let Some(style) = gutter {
        spans.push(Span::styled("│ ", style));
    }
    spans.extend(cells_to_spans(&cells));
    Row {
        line: Line::from(spans).style(line_style),
        src,
    }
}

fn cells_to_spans(cells: &[(char, Style)]) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buffer = String::new();
    let mut style = Style::default();
    for (ch, cell_style) in cells {
        if !buffer.is_empty() && *cell_style != style {
            spans.push(Span::styled(std::mem::take(&mut buffer), style));
        }
        style = *cell_style;
        buffer.push(*ch);
    }
    if !buffer.is_empty() {
        spans.push(Span::styled(buffer, style));
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rmd::{parse, Overrides};
    use crate::theme::TuiThemeChoice;
    use ratatui_core::style::Color;
    use std::path::Path;

    fn rows_of(source: &str, width: usize) -> (Vec<Row>, Vec<OutlineEntry>) {
        let document = parse(
            source,
            Path::new("/tmp/rmdt-render.Rmd"),
            &Overrides::default(),
        );
        let layout = build(
            &document,
            TuiTheme::with_depth(TuiThemeChoice::GlowDark, Depth::TrueColor),
            TuiThemeChoice::GlowDark.code_theme(),
            width,
        );
        (layout.rows, layout.outline)
    }

    #[test]
    fn every_row_fits_the_width() {
        let source = format!("{}\n", "wrapped words and more words ".repeat(20));
        for width in [30usize, 48, 72] {
            let (rows, _) = rows_of(&source, width);
            assert!(rows.len() > 3, "expected several rows at width {width}");
            for row in &rows {
                assert!(
                    row.line.width() <= width,
                    "row of {} columns at width {width}: {:?}",
                    row.line.width(),
                    row.text()
                );
            }
        }
    }

    #[test]
    fn a_long_token_is_broken_not_lost() {
        let token = "x".repeat(120);
        let (rows, _) = rows_of(&format!("```{{r}}\n{token}\n```\n"), 40);
        // The gutter sits between the halves of a row split mid token.
        let joined: String = rows
            .iter()
            .map(|row| row.text().trim_start_matches("│ ").to_string())
            .collect::<Vec<_>>()
            .join("");
        assert!(joined.contains(&token));
        for row in &rows {
            assert!(row.line.width() <= 40);
        }
    }

    #[test]
    fn rows_carry_the_source_line_they_came_from() {
        let (rows, outline) = rows_of("# One\n\nparagraph\n\n```{r a}\nx <- 1\n```\n", 60);
        assert_eq!(outline.len(), 1);
        assert_eq!(outline[0].title, "One");
        assert_eq!(rows[outline[0].row].src, 1);
        assert_eq!(rows[outline[0].row].text().trim(), "One");
        let code = rows
            .iter()
            .find(|row| row.text().contains("x <- 1"))
            .expect("the code row");
        assert_eq!(code.src, 5);
    }

    #[test]
    fn boxed_blocks_get_a_gutter() {
        let (rows, _) = rows_of("```{r a}\nx <- 1\n```\n", 60);
        let code = rows
            .iter()
            .find(|row| row.text().contains("x <- 1"))
            .expect("the code row");
        assert!(code.text().starts_with("│ "), "{:?}", code.text());
    }

    #[test]
    fn outline_rows_ascend_and_match_the_headings() {
        let (rows, outline) = rows_of("# A\n\n## B\n\ntext\n\n# C\n", 60);
        assert_eq!(outline.len(), 3);
        let indices: Vec<usize> = outline.iter().map(|entry| entry.row).collect();
        let mut sorted = indices.clone();
        sorted.sort_unstable();
        assert_eq!(indices, sorted);
        assert!(indices[2] < rows.len());
    }

    #[test]
    fn a_chunk_that_contains_a_fence_stays_one_code_block() {
        let (rows, _) = rows_of("````{r eval=FALSE}\n```\nexample fence\n```\n````\n", 60);
        let inside = rows
            .iter()
            .find(|row| row.text().contains("example fence"))
            .expect("the line inside the fence");
        // A code row carries the gutter; prose does not.
        assert!(inside.text().starts_with("│ "), "{:?}", inside.text());
        assert!(rows.iter().any(|row| row.text().contains("```")));
    }

    #[test]
    fn a_tab_becomes_spaces_instead_of_disappearing() {
        let (rows, _) = rows_of("```{r}\nif (x) {\n\tbody()\n}\n```\n", 60);
        let row = rows
            .iter()
            .find(|row| row.text().contains("body()"))
            .expect("the body row");
        assert!(!row.text().contains('\t'), "the tab is still there");
        assert!(
            row.text().starts_with("│         body()"),
            "{:?}",
            row.text()
        );
    }

    #[test]
    fn a_tab_in_prose_and_a_row_of_tabs_keep_the_width() {
        let (rows, _) = rows_of("one\ttwo\n\n```{r}\n\t\tdeep\n```\n", 30);
        for row in &rows {
            assert!(row.line.width() <= 30, "{:?}", row.text());
        }
        let prose = rows
            .iter()
            .find(|row| row.text().contains("one"))
            .expect("the prose row");
        assert_eq!(prose.text(), "one     two", "a tab moves to column eight");
    }

    #[test]
    fn the_inline_r_marker_never_shows_up_in_a_row() {
        let (rows, _) = rows_of("Value `r nrow(df)` here.\n", 60);
        for row in &rows {
            assert!(!row.text().contains(INLINE_R_MARKER));
        }
        assert!(rows.iter().any(|row| row.text().contains("nrow(df)")));
    }

    #[test]
    fn a_break_at_a_space_does_not_add_an_empty_row() {
        let (rows, _) = rows_of("word  double  spaced  words  and  more  here\n", 12);
        assert!(rows.len() > 1, "expected wrapping to happen");
        for row in &rows {
            assert!(
                !row.text().trim().is_empty(),
                "an empty row appeared: {:?}",
                rows.iter().map(|row| row.text()).collect::<Vec<_>>()
            );
        }
    }

    /// Colours of the spans in the fenced code block.
    fn code_colours(source: &str, choice: TuiThemeChoice) -> Vec<Color> {
        let document = parse(
            source,
            Path::new("/tmp/rmdt-render.Rmd"),
            &Overrides::default(),
        );
        let layout = build(
            &document,
            TuiTheme::with_depth(choice, Depth::TrueColor),
            choice.code_theme(),
            60,
        );
        layout
            .rows
            .iter()
            .filter(|row| row.text().contains("dplyr"))
            .flat_map(|row| row.line.spans.iter().map(|span| span.style.fg))
            .flatten()
            .collect()
    }

    #[test]
    fn the_code_palette_matches_the_terminal_background() {
        let source = "```{r demo}\nlibrary(dplyr)\n```\n\n```{python}\nprint(math.pi)\n```\n";
        // Base16 Ocean dark paints plain code text near-white.
        // chroma's function greens are the giveaway: neither built-in syntect
        // theme uses them.
        let dark = code_colours(source, TuiThemeChoice::GlowDark);
        assert!(
            dark.contains(&Color::Rgb(0x00, 0xD7, 0x87)),
            "glow's dark chroma palette should colour the tokens: {dark:?}"
        );
        let light = code_colours(source, TuiThemeChoice::GlowLight);
        assert!(
            light.contains(&Color::Rgb(0x01, 0x9F, 0x57)),
            "glow's light chroma palette should colour the tokens: {light:?}"
        );
    }

    /// Every (text, colour) pair of a chunk's code rows, in reading order.
    fn chunk_spans(
        code: &str,
        choice: TuiThemeChoice,
        depth: Depth,
    ) -> Vec<(String, Option<Color>)> {
        let source = format!("```{{r}}\n{code}\n```\n");
        let document = parse(
            &source,
            Path::new("/tmp/rmdt-render.Rmd"),
            &Overrides::default(),
        );
        let layout = build(
            &document,
            TuiTheme::with_depth(choice, depth),
            choice.code_theme(),
            70,
        );
        layout
            .rows
            .iter()
            .filter(|row| row.text().starts_with("│ "))
            .flat_map(|row| row.line.spans.iter())
            .map(|span| (span.content.to_string(), span.style.fg))
            .collect()
    }

    /// The colour of the first span whose text contains `token`.
    fn colour_of(spans: &[(String, Option<Color>)], token: &str) -> Option<Color> {
        spans
            .iter()
            .find(|(text, _)| text.contains(token))
            .and_then(|(_, colour)| *colour)
    }

    #[test]
    fn r_tokens_take_the_colours_glow_gives_them() {
        let spans = chunk_spans(
            "remotes::install_github(\"a/b\")\n# a comment\nx <- NA\n",
            TuiThemeChoice::GlowDark,
            Depth::TrueColor,
        );
        // A package qualifier is an operator, as it is in glow, not a keyword.
        assert_eq!(colour_of(&spans, "::"), Some(Color::Rgb(0xEF, 0x80, 0x80)));
        // A function is the function colour whether or not a package qualifies it.
        assert_eq!(
            colour_of(&spans, "install_github"),
            Some(Color::Rgb(0x00, 0xD7, 0x87))
        );
        // The quotes belong to the string they delimit, and the marker to its comment.
        assert_eq!(
            colour_of(&spans, "\"a/b\""),
            Some(Color::Rgb(0xC6, 0x96, 0x69))
        );
        assert_eq!(
            colour_of(&spans, "# a comment"),
            Some(Color::Rgb(0x67, 0x67, 0x67))
        );
        // NA is one of the named constants, not a number.
        assert_eq!(colour_of(&spans, "NA"), Some(Color::Rgb(0x00, 0xAA, 0xFF)));
    }

    #[test]
    fn a_call_is_not_all_function_colour() {
        let spans = chunk_spans(
            "library(dplyr)\n",
            TuiThemeChoice::GlowDark,
            Depth::TrueColor,
        );
        assert_eq!(
            colour_of(&spans, "library"),
            Some(Color::Rgb(0x00, 0xD7, 0x87))
        );
        assert_eq!(colour_of(&spans, "("), Some(Color::Rgb(0xE8, 0xE8, 0xA8)));
        assert_eq!(
            colour_of(&spans, "dplyr"),
            Some(Color::Rgb(0xC4, 0xC4, 0xC4))
        );
    }

    #[test]
    fn the_r_palette_follows_the_terminal_background() {
        let code = "remotes::install_github(\"a/b\")\n";
        let dark = chunk_spans(code, TuiThemeChoice::GlowDark, Depth::TrueColor);
        let light = chunk_spans(code, TuiThemeChoice::GlowLight, Depth::TrueColor);
        assert_eq!(colour_of(&dark, "::"), Some(Color::Rgb(0xEF, 0x80, 0x80)));
        assert_eq!(colour_of(&light, "::"), Some(Color::Rgb(0xFF, 0x26, 0x26)));
        assert_eq!(
            colour_of(&light, "install_github"),
            Some(Color::Rgb(0x01, 0x9F, 0x57))
        );
    }

    #[test]
    fn a_256_colour_terminal_still_gets_the_whole_palette() {
        // A terminal that only knows the cube drops a 24 bit request, so the
        // code would come out in one flat foreground. The numbers are the ones
        // glow prints on the same terminal.
        let spans = chunk_spans(
            "remotes::install_github(\"a/b\")\n# a comment\n",
            TuiThemeChoice::GlowDark,
            Depth::Ansi256,
        );
        assert_eq!(colour_of(&spans, "::"), Some(Color::Indexed(210)));
        assert_eq!(
            colour_of(&spans, "install_github"),
            Some(Color::Indexed(42))
        );
        assert_eq!(colour_of(&spans, "\"a/b\""), Some(Color::Indexed(173)));
        assert_eq!(colour_of(&spans, "# a comment"), Some(Color::Indexed(241)));
        assert!(
            !spans
                .iter()
                .any(|(_, colour)| matches!(colour, Some(Color::Rgb(..)))),
            "no truecolour left to be dropped: {spans:?}"
        );
    }

    #[test]
    fn figure_captions_and_notes_become_rows() {
        let (rows, _) = rows_of(
            "```{r a, fig.cap=\"A plot\"}\nplot(1)\n```\n\n```{r b, echo=FALSE}\nsecret()\n```\n",
            60,
        );
        let text: String = rows
            .iter()
            .map(|row| row.text())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Figure 1: A plot"));
        assert!(text.contains("code hidden (echo=FALSE)"));
        assert!(!text.contains("secret()"));
    }
}
