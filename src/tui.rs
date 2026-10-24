//! Terminal UI.
//!
//! Rows are pre-wrapped by [`crate::render`], so one row is always one screen
//! row. Scrolling, section jumps and search all work on that assumption, which
//! keeps the arithmetic honest.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
use ratatui::Frame;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::render::{self, OutlineEntry, Row};
use crate::rmd::{self, Document, Overrides};
use crate::theme::{TuiTheme, TuiThemeChoice};

/// Where the outline lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum OutlineMode {
    /// A popup you open with `o`.
    Popup,
    /// A column pinned to the left.
    Pinned,
    /// No outline at all.
    Off,
}

/// Settings the app needs from the command line.
#[derive(Debug, Clone)]
pub struct RunOptions {
    pub theme: TuiThemeChoice,
    /// The package's documents, when the reader was pointed at one.
    pub site: Vec<(String, PathBuf)>,
    pub overrides: Overrides,
    pub watch: bool,
    pub outline: OutlineMode,
}

const SIDEBAR_WIDTH: u16 = 30;

/// Which pane the arrow keys are working on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pane {
    #[default]
    Document,
    Index,
}

/// A line of the index column: a document of the package, or a heading of the
/// document being read.
#[derive(Debug, Clone, PartialEq, Eq)]
enum IndexItem {
    Document {
        path: PathBuf,
        label: String,
    },
    Section {
        row: usize,
        level: u8,
        title: String,
    },
}

struct Search {
    query: String,
    matches: Vec<usize>,
    selected: usize,
}

impl Search {
    fn current(&self) -> Option<usize> {
        self.matches.get(self.selected).copied()
    }
}

struct App {
    files: Vec<PathBuf>,
    picker: bool,
    /// Index into `files` of the highlighted document.
    picker_file: usize,
    path: PathBuf,
    doc: Document,
    rows: Vec<Row>,
    outline: Vec<OutlineEntry>,
    /// First row of each block of the current document.
    block_rows: Vec<usize>,
    /// What the index column lists.
    index_items: Vec<IndexItem>,
    index_cursor: usize,
    focus: Pane,
    /// How far the index column is scrolled.
    index_scroll: usize,
    offset: usize,
    content_width: usize,
    height: usize,
    /// The terminal width of the last frame, for the arrow keys.
    last_width: u16,
    theme: TuiTheme,
    options: RunOptions,
    popup: bool,
    popup_index: usize,
    search: Option<Search>,
    input: Option<String>,
    help: bool,
    watch_mtime: Option<SystemTime>,
    /// A problem worth telling the reader about, shown in the status line.
    status: Option<String>,
    should_quit: bool,
    want_editor: Option<usize>,
}

impl App {
    fn new(files: Vec<PathBuf>, options: RunOptions) -> io::Result<Self> {
        let path = files.first().cloned().unwrap_or_default();
        let mut app = Self {
            picker: false,
            picker_file: 0,
            files,
            path: path.clone(),
            doc: rmd::load(&path, &options.overrides)?,
            rows: Vec::new(),
            outline: Vec::new(),
            block_rows: Vec::new(),
            index_items: Vec::new(),
            index_cursor: 0,
            focus: Pane::Document,
            index_scroll: 0,
            offset: 0,
            content_width: 0,
            height: 0,
            last_width: 0,
            theme: TuiTheme::new(options.theme),
            options,
            popup: false,
            popup_index: 0,
            search: None,
            input: None,
            help: false,
            watch_mtime: mtime(&path),
            status: None,
            should_quit: false,
            want_editor: None,
        };
        app.rebuild(80, None);
        if app.files.len() > 1 {
            // Start on the first document the list actually shows.
            app.picker_file = first_listed(&app).unwrap_or(0);
        }
        Ok(app)
    }

    /// Parse the current file again, keeping the reading position.
    ///
    /// A file that cannot be read leaves what is already on screen alone: an
    /// editor that saves something unreadable must not end the session.
    fn reload(&mut self) {
        let anchor = self.rows.get(self.offset).map(|row| row.src);
        match rmd::load(&self.path, &self.options.overrides) {
            Ok(document) => {
                self.doc = document;
                self.watch_mtime = mtime(&self.path);
                self.status = None;
                self.rebuild(self.content_width, anchor);
                self.research();
            }
            Err(error) => self.status = Some(format!("could not read this file: {error}")),
        }
    }

    fn open(&mut self, path: PathBuf) {
        match rmd::load(&path, &self.options.overrides) {
            Ok(document) => {
                self.path = path;
                self.doc = document;
                self.picker = false;
                self.offset = 0;
                self.search = None;
                self.watch_mtime = mtime(&self.path);
                self.status = None;
                self.rebuild(self.content_width, None);
            }
            Err(error) => {
                self.status = Some(format!("could not read {}: {error}", path.display()));
            }
        }
    }

    /// Re-wrap at the current width, landing on `anchor_src` if given.
    fn rebuild(&mut self, width: usize, anchor_src: Option<usize>) {
        let width = width.max(24);
        let layout = render::build(&self.doc, self.theme, self.theme.code_theme(), width);
        self.block_rows = layout.block_rows;
        self.rows = layout.rows;
        self.outline = layout.outline;
        self.content_width = width;
        self.build_index_items();
        self.offset = match anchor_src {
            Some(src) => self.rows.iter().position(|row| row.src >= src).unwrap_or(0),
            None => self.offset.min(self.rows.len().saturating_sub(1)),
        };
        self.clamp_offset(self.height);
    }

    /// The documents of the package first, then this document's headings.
    fn build_index_items(&mut self) {
        let mut items = Vec::new();
        let current = std::path::absolute(&self.path).unwrap_or_else(|_| self.path.clone());
        for (label, path) in &self.options.site {
            if std::path::absolute(path).unwrap_or_else(|_| path.clone()) == current {
                continue;
            }
            items.push(IndexItem::Document {
                path: path.clone(),
                label: label.clone(),
            });
        }
        for entry in &self.outline {
            items.push(IndexItem::Section {
                row: entry.row,
                level: entry.level,
                title: entry.title.clone(),
            });
        }
        self.index_items = items;
        self.index_cursor = self
            .index_cursor
            .min(self.index_items.len().saturating_sub(1));
    }

    /// Move the index cursor, skipping nothing: every line is selectable.
    fn move_index_cursor(&mut self, forward: bool, height: usize) {
        if self.index_items.is_empty() {
            return;
        }
        self.index_cursor = if forward {
            (self.index_cursor + 1).min(self.index_items.len() - 1)
        } else {
            self.index_cursor.saturating_sub(1)
        };
        self.keep_cursor_visible(height);
    }

    /// Scroll the column so the cursor stays on screen.
    ///
    /// `index_scroll` is the index of the item at the top of the column; the
    /// column also draws a title and, between documents and headings, a rule.
    fn keep_cursor_visible(&mut self, height: usize) {
        let height = height.max(3);
        if self.index_cursor < self.index_scroll {
            self.index_scroll = self.index_cursor;
            return;
        }
        let mut lines = 1; // the title
        for index in self.index_scroll..=self.index_cursor {
            lines += 1 + usize::from(separator_before(self, index));
        }
        if lines <= height {
            return;
        }
        // Scroll down just enough that the cursor is the last line that fits.
        let mut start = self.index_cursor;
        let mut needed = 2; // the title and the cursor line
        while start > 0 {
            let above = start - 1;
            let cost = 1 + usize::from(separator_before(self, above));
            if needed + cost > height {
                break;
            }
            needed += cost;
            start = above;
        }
        self.index_scroll = start;
    }

    /// What Enter does with the highlighted line.
    fn enter_index(&mut self) {
        match self.index_items.get(self.index_cursor).cloned() {
            Some(IndexItem::Document { path, .. }) => self.open(path),
            Some(IndexItem::Section { row, .. }) => {
                self.offset = row.min(self.max_offset());
                self.focus = Pane::Document;
            }
            None => {}
        }
    }

    fn clamp_offset(&mut self, height: usize) {
        let max = self.rows.len().saturating_sub(height.max(1));
        self.offset = self.offset.min(max);
    }

    fn max_offset(&self) -> usize {
        self.rows.len().saturating_sub(self.height.max(1))
    }

    fn sidebar_width(&self, terminal_width: u16) -> u16 {
        match self.options.outline {
            OutlineMode::Pinned if terminal_width >= 80 && !self.outline.is_empty() => {
                SIDEBAR_WIDTH
            }
            _ => 0,
        }
    }

    fn current_section(&self) -> Option<&OutlineEntry> {
        self.outline
            .iter()
            .rev()
            .find(|entry| entry.row <= self.offset)
            .or_else(|| self.outline.first())
    }

    fn jump_section(&mut self, forward: bool) {
        let target = if forward {
            self.outline
                .iter()
                .find(|entry| entry.row > self.offset)
                .map(|entry| entry.row)
        } else {
            self.outline
                .iter()
                .rev()
                .find(|entry| entry.row < self.offset)
                .map(|entry| entry.row)
        };
        if let Some(row) = target {
            self.offset = row;
            self.clamp_offset(self.height);
        }
    }

    fn research(&mut self) {
        if let Some(search) = &self.search {
            let query = search.query.clone();
            let selected = search.selected;
            self.apply_search(query, selected);
        }
    }

    fn apply_search(&mut self, query: String, keep: usize) {
        if query.trim().is_empty() {
            self.search = None;
            return;
        }
        let needle = query.to_lowercase();
        let matches: Vec<usize> = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.text().to_lowercase().contains(&needle))
            .map(|(index, _)| index)
            .collect();
        let selected = keep.min(matches.len().saturating_sub(1));
        if let Some(row) = matches.get(selected) {
            self.offset = (*row).min(self.max_offset());
        }
        self.search = Some(Search {
            query,
            matches,
            selected,
        });
    }

    fn search_step(&mut self, forward: bool) {
        let height = self.height.max(1);
        let Some(search) = self.search.as_mut() else {
            return;
        };
        if search.matches.is_empty() {
            return;
        }
        search.selected = if forward {
            (search.selected + 1) % search.matches.len()
        } else if search.selected == 0 {
            search.matches.len() - 1
        } else {
            search.selected - 1
        };
        if let Some(row) = search.current() {
            self.offset = row.min(self.rows.len().saturating_sub(1));
            self.clamp_offset(height);
        }
    }

    fn key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }
        if self.picker {
            self.picker_key(key);
            return;
        }
        if self.help {
            self.help = false;
            return;
        }
        if let Some(mut query) = self.input.take() {
            match key.code {
                KeyCode::Esc => {}
                KeyCode::Enter => {
                    let keep = self.search.as_ref().map(|s| s.selected).unwrap_or(0);
                    self.apply_search(query, keep);
                }
                KeyCode::Backspace => {
                    query.pop();
                    self.input = Some(query);
                }
                KeyCode::Char(ch) => {
                    query.push(ch);
                    self.input = Some(query);
                }
                _ => self.input = Some(query),
            }
            return;
        }
        if self.popup {
            self.popup_key(key);
            return;
        }

        let page = self.height.saturating_sub(2).max(1);
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Esc => {
                self.search = None;
            }
            // The arrows pick a pane and move inside it: left goes to the index
            // column, right back to the document, up and down work on whichever
            // one is focused, and Enter takes the highlighted line.
            KeyCode::Down => match self.focus {
                Pane::Index => self.move_index_cursor(true, self.height),
                Pane::Document => self.offset = (self.offset + 1).min(self.max_offset()),
            },
            KeyCode::Up => match self.focus {
                Pane::Index => self.move_index_cursor(false, self.height),
                Pane::Document => self.offset = self.offset.saturating_sub(1),
            },
            KeyCode::Left if self.sidebar_width(self.last_width) > 0 => self.focus = Pane::Index,
            KeyCode::Right => self.focus = Pane::Document,
            KeyCode::Enter if self.focus == Pane::Index => self.enter_index(),
            KeyCode::Char('j') => self.offset = (self.offset + 1).min(self.max_offset()),
            KeyCode::Char('k') => self.offset = self.offset.saturating_sub(1),
            KeyCode::Char('d') | KeyCode::PageDown | KeyCode::Char(' ') => {
                self.offset = (self.offset + page).min(self.max_offset())
            }
            KeyCode::Char('u') | KeyCode::PageUp => self.offset = self.offset.saturating_sub(page),
            KeyCode::Char('l') => self.offset = (self.offset + page / 2).min(self.max_offset()),
            KeyCode::Char('h') => self.offset = self.offset.saturating_sub(page / 2),
            KeyCode::Char('g') => self.offset = 0,
            KeyCode::Char('G') => self.offset = self.max_offset(),
            KeyCode::Char('[') => self.jump_section(false),
            KeyCode::Char(']') => self.jump_section(true),
            KeyCode::Char('t') => {
                self.options.outline = if self.options.outline == OutlineMode::Pinned {
                    OutlineMode::Popup
                } else {
                    OutlineMode::Pinned
                };
                self.rebuild(
                    self.content_width,
                    self.rows.get(self.offset).map(|row| row.src),
                );
            }
            KeyCode::Char('o') | KeyCode::Tab if !self.outline.is_empty() => {
                self.popup = !self.popup;
                self.popup_index = self
                    .outline
                    .iter()
                    .rposition(|entry| entry.row <= self.offset)
                    .unwrap_or(0);
            }
            KeyCode::Char('/') => self.input = Some(String::new()),
            KeyCode::Char('n') => self.search_step(true),
            KeyCode::Char('N') => self.search_step(false),
            KeyCode::Char('r') => self.reload(),
            KeyCode::Char('b') if self.files.len() > 1 => {
                self.picker = true;
                self.picker_file = self
                    .files
                    .iter()
                    .position(|path| *path == self.path)
                    .unwrap_or(0);
            }
            KeyCode::Char('e') => {
                self.want_editor = self.rows.get(self.offset).map(|row| row.src);
            }
            KeyCode::Char('?') => self.help = true,
            _ => {}
        }
    }

    fn popup_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('o') | KeyCode::Tab | KeyCode::Char('q') => {
                self.popup = false
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.popup_index = (self.popup_index + 1).min(self.outline.len().saturating_sub(1))
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.popup_index = self.popup_index.saturating_sub(1)
            }
            KeyCode::Char('g') => self.popup_index = 0,
            KeyCode::Char('G') => self.popup_index = self.outline.len().saturating_sub(1),
            KeyCode::Enter => {
                if let Some(entry) = self.outline.get(self.popup_index) {
                    self.offset = entry.row;
                    self.clamp_offset(self.height);
                }
                self.popup = false;
            }
            _ => {}
        }
    }

    /// Step through the documents in the order they are shown.
    fn picker_move(&mut self, forward: bool) {
        let listed: Vec<usize> = picker_rows(self)
            .iter()
            .filter_map(|row| match row {
                PickerRow::File(index) => Some(*index),
                PickerRow::Group(_) => None,
            })
            .collect();
        if listed.is_empty() {
            return;
        }
        let position = listed
            .iter()
            .position(|index| *index == self.picker_file)
            .unwrap_or(0);
        let next = if forward {
            (position + 1).min(listed.len() - 1)
        } else {
            position.saturating_sub(1)
        };
        self.picker_file = listed[next];
    }

    fn picker_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('j') | KeyCode::Down => self.picker_move(true),
            KeyCode::Char('k') | KeyCode::Up => self.picker_move(false),
            KeyCode::Enter => {
                if let Some(path) = self.files.get(self.picker_file).cloned() {
                    self.open(path);
                }
            }
            _ => {}
        }
    }

    fn tick(&mut self) {
        if !self.options.watch {
            return;
        }
        let now = mtime(&self.path);
        if now != self.watch_mtime && now.is_some() {
            self.reload();
        }
    }

    fn launch_editor(&self, line: usize) {
        let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
        let mut command = Command::new(editor);
        if line > 0 {
            command.arg(format!("+{line}"));
        }
        command.arg(&self.path);
        let _ = command.status();
    }
}

fn mtime(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path)
        .ok()
        .and_then(|meta| meta.modified().ok())
}

/// Why the event loop handed control back.
enum Outcome {
    Quit,
    Edit(usize),
}

/// Run the reader until the user quits.
pub fn run(files: Vec<PathBuf>, options: RunOptions) -> io::Result<()> {
    let mut app = App::new(files, options)?;
    if app.files.len() > 1 {
        app.picker = true;
        // Start on the first document the list actually shows.
        app.picker_file = first_listed(&app).unwrap_or(0);
    }
    loop {
        // `ratatui::init` panics when there is no terminal; report it instead.
        let mut terminal = ratatui::try_init()?;
        let outcome = event_loop(&mut terminal, &mut app);
        ratatui::restore();
        match outcome? {
            Outcome::Quit => return Ok(()),
            Outcome::Edit(line) => app.launch_editor(line),
        }
    }
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> io::Result<Outcome> {
    terminal.clear()?;
    loop {
        terminal.draw(|frame| draw(frame, app))?;
        if event::poll(Duration::from_millis(200))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => app.key(key),
                Event::Resize(width, height) => {
                    let sidebar = app.sidebar_width(width);
                    let width = width.saturating_sub(sidebar) as usize;
                    app.height = height.saturating_sub(1) as usize;
                    app.rebuild(width, app.rows.get(app.offset).map(|row| row.src));
                }
                _ => {}
            }
        }
        app.tick();
        if app.should_quit {
            return Ok(Outcome::Quit);
        }
        if let Some(line) = app.want_editor.take() {
            return Ok(Outcome::Edit(line));
        }
    }
}

fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    if area.height < 3 || area.width < 24 {
        return;
    }
    if app.picker {
        draw_picker(frame, app, area);
        return;
    }

    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(area);
    let body = chunks[0];
    let status = chunks[1];

    app.last_width = area.width;
    let sidebar = app.sidebar_width(area.width);
    let doc_area = if sidebar > 0 {
        let split = Layout::horizontal([
            Constraint::Length(sidebar),
            Constraint::Length(1),
            Constraint::Min(20),
        ])
        .split(body);
        draw_sidebar(frame, app, split[0]);
        draw_divider(frame, app, split[1]);
        split[2]
    } else {
        body
    };

    let width = doc_area.width as usize;
    app.height = doc_area.height as usize;
    if app.content_width != width && width >= 24 {
        app.rebuild(width, app.rows.get(app.offset).map(|row| row.src));
    }
    app.clamp_offset(app.height);
    draw_document(frame, app, doc_area);
    draw_status(frame, app, status);
    if app.popup {
        draw_outline_popup(frame, app, area);
    }
    if app.help {
        draw_help(frame, app, area);
    }
}

fn draw_document(frame: &mut Frame, app: &App, area: Rect) {
    frame.render_widget(
        Paragraph::new(Text::from(visible_rows(app, area.height as usize))),
        area,
    );
}

/// The rows that belong on screen right now, with search hits marked.
fn visible_rows(app: &App, height: usize) -> Vec<Line<'static>> {
    let end = (app.offset + height).min(app.rows.len());
    let slice = app.rows.get(app.offset..end).unwrap_or(&[]);
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(slice.len());
    for (index, row) in slice.iter().enumerate() {
        let row_index = app.offset + index;
        let mut line = row.line.clone();
        if let Some(list) = &app.search {
            // Reverse video and an underline rather than a fixed highlight
            // colour, which would fight the terminal's own background.
            if Some(row_index) == list.current() {
                line.style = line.style.add_modifier(Modifier::REVERSED);
            } else if list.matches.contains(&row_index) {
                line.style = line.style.add_modifier(Modifier::UNDERLINED);
            }
        }
        lines.push(line);
    }
    lines
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    frame.render_widget(Paragraph::new(status_line(app)), area);
}

fn status_line(app: &App) -> Line<'static> {
    let theme = app.theme;
    let section = app
        .current_section()
        .map(|entry| entry.title.clone())
        .unwrap_or_default();
    let percent = if app.rows.is_empty() {
        100
    } else {
        ((app.offset + app.height) * 100 / app.rows.len().max(1)).min(100)
    };

    let mut spans = vec![
        Span::styled(
            app.path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  {percent}%  "), theme.chrome()),
    ];
    if let Some(query) = &app.input {
        spans.push(Span::styled("search: ", theme.chrome()));
        spans.push(Span::styled(
            query.clone(),
            Style::default().fg(theme.accent_color()),
        ));
        spans.push(Span::styled(
            "_",
            Style::default().add_modifier(Modifier::SLOW_BLINK),
        ));
    } else if let Some(search) = &app.search {
        let total = search.matches.len();
        let position = if total == 0 { 0 } else { search.selected + 1 };
        spans.push(Span::styled(
            format!("/{} {position}/{total}  ", search.query),
            Style::default().fg(theme.accent_color()),
        ));
    } else if let Some(message) = &app.status {
        spans.push(Span::styled(
            message.clone(),
            Style::default().fg(theme.fit(Color::Rgb(200, 90, 90))),
        ));
    } else if !section.is_empty() {
        spans.push(Span::styled(section, theme.chrome()));
    }
    if app.focus == Pane::Index {
        spans.push(Span::styled(
            "  index  ↑↓ pick  Enter opens  → back",
            theme.index_heading(),
        ));
    }
    Line::from(spans).style(theme.status_bar())
}

fn draw_sidebar(frame: &mut Frame, app: &App, area: Rect) {
    frame.render_widget(
        Paragraph::new(Text::from(sidebar_lines(app, area.height as usize)))
            .wrap(Wrap { trim: false }),
        area,
    );
}

/// A vertical rule, so the index reads as its own pane.
fn draw_divider(frame: &mut Frame, app: &App, area: Rect) {
    let lines: Vec<Line<'static>> = (0..area.height)
        .map(|_| Line::from(Span::styled("│", app.theme.divider())))
        .collect();
    frame.render_widget(Paragraph::new(Text::from(lines)), area);
}

/// Cut a label to fit the columns it has, marking that it was cut.
///
/// The width is counted in screen columns, not characters: a CJK character
/// takes two columns, and a label that wraps would move the rows the cursor
/// arithmetic counts on.
fn shorten(label: &str, room: usize) -> String {
    if room == 0 {
        return String::new();
    }
    if label.width() <= room {
        return label.to_string();
    }
    // The ellipsis takes one column of the room.
    let mut kept = String::new();
    let mut used = 0usize;
    for ch in label.chars() {
        let width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + width > room.saturating_sub(1) {
            break;
        }
        kept.push(ch);
        used += width;
    }
    format!("{}…", kept.trim_end())
}

fn index_title(app: &App) -> String {
    if app.options.site.is_empty() {
        "Contents".to_string()
    } else {
        "Documents".to_string()
    }
}

/// A rule is drawn where the document list becomes the heading list.
fn separator_before(app: &App, index: usize) -> bool {
    if index == 0 {
        return false;
    }
    matches!(
        (&app.index_items[index - 1], &app.index_items[index]),
        (IndexItem::Document { .. }, IndexItem::Section { .. })
    )
}

fn sidebar_lines(app: &App, height: usize) -> Vec<Line<'static>> {
    let theme = app.theme;
    let visible = height.max(2);
    let mut lines: Vec<Line<'static>> = vec![Line::from(Span::styled(
        index_title(app),
        theme.index_heading(),
    ))];
    for (index, item) in app
        .index_items
        .iter()
        .enumerate()
        .skip(app.index_scroll)
        .take(visible.saturating_sub(1))
    {
        if separator_before(app, index) {
            lines.push(Line::from(Span::styled(
                "─".repeat(SIDEBAR_WIDTH as usize - 3),
                theme.divider(),
            )));
        }
        let (label, indent) = match item {
            IndexItem::Document { label, .. } => (label.clone(), 0usize),
            IndexItem::Section { level, title, .. } => (title.clone(), *level as usize),
        };
        let selected = index == app.index_cursor;
        let style = match (selected, app.focus) {
            (true, Pane::Index) => theme.index_selected(),
            (true, Pane::Document) => theme.index_current(),
            (false, _) => theme.index_idle(),
        };
        let prefix = if indent == 0 {
            String::new()
        } else {
            "  ".repeat(indent.saturating_sub(1))
        };
        // One row per entry: a wrapped label would move the rows the cursor
        // arithmetic counts on.
        let room = (SIDEBAR_WIDTH as usize).saturating_sub(prefix.chars().count() + 1);
        lines.push(Line::from(Span::styled(
            format!("{prefix}{}", shorten(&label, room)),
            style,
        )));
    }
    lines
}

fn draw_outline_popup(frame: &mut Frame, app: &App, area: Rect) {
    let width = (area.width.saturating_sub(8)).min(72);
    let height = (area.height.saturating_sub(4)).min(24);
    let popup = centered(area, width, height);
    frame.render_widget(Clear, popup);
    let block = Block::bordered().title(" Outline  Enter jumps, Esc closes ");
    frame.render_widget(
        Paragraph::new(Text::from(outline_lines(
            app,
            height.saturating_sub(2) as usize,
        )))
        .block(block),
        popup,
    );
}

fn outline_lines(app: &App, visible: usize) -> Vec<Line<'static>> {
    let start = if app.popup_index >= visible {
        app.popup_index + 1 - visible
    } else {
        0
    };
    let mut lines: Vec<Line<'static>> = Vec::new();
    for (index, entry) in app.outline.iter().enumerate().skip(start).take(visible) {
        let indent = "  ".repeat(entry.level.saturating_sub(1) as usize);
        let style = if index == app.popup_index {
            Style::default()
                .fg(app.theme.accent_color())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        lines.push(Line::from(Span::styled(
            format!(" {indent}{}", entry.title),
            style,
        )));
    }
    lines
}

fn draw_help(frame: &mut Frame, app: &App, area: Rect) {
    let popup = centered(area, 66.min(area.width), 20.min(area.height));
    frame.render_widget(Clear, popup);
    let block = Block::bordered().title(" Keys ");
    frame.render_widget(
        Paragraph::new(Text::from(help_lines(app))).block(block),
        popup,
    );
}

fn help_lines(app: &App) -> Vec<Line<'static>> {
    let rows = [
        ("j / k", "one row down / up"),
        ("d / u, space", "one page down / up"),
        ("h / l", "half page up / down"),
        ("g / G", "top / bottom"),
        ("[ / ]", "previous / next section"),
        ("o, tab", "outline popup"),
        ("t", "pin or unpin the outline column"),
        ("/  n  N", "search, next hit, previous hit"),
        ("e", "open the source line in $EDITOR"),
        ("r", "read the file again"),
        ("b", "back to the file list"),
        ("q", "quit"),
    ];
    rows.iter()
        .map(|(key, what)| {
            Line::from(vec![
                Span::styled(
                    format!(" {key:<14}"),
                    Style::default()
                        .fg(app.theme.accent_color())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(*what, Style::default()),
            ])
        })
        .collect()
}

fn draw_picker(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::bordered().title(format!(
        " {}  {} document(s)  Enter opens, q quits ",
        picker_label(app),
        app.files.len()
    ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(Text::from(picker_lines(app)))
            .scroll((picker_scroll(app, inner.height as usize) as u16, 0)),
        inner,
    );
}

/// A row of the document list: a section heading, or a document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PickerRow {
    Group(&'static str),
    File(usize),
}

/// The documents, in the same sections the written site uses.
fn picker_rows(app: &App) -> Vec<PickerRow> {
    let mut rows = Vec::new();
    for group in ["Vignettes", "Other documents"] {
        let mut heading = false;
        for (index, path) in app.files.iter().enumerate() {
            if picker_group(path) == group {
                if !heading {
                    rows.push(PickerRow::Group(group));
                    heading = true;
                }
                rows.push(PickerRow::File(index));
            }
        }
    }
    rows
}

fn picker_group(path: &Path) -> &'static str {
    let in_vignettes = path
        .components()
        .any(|part| part.as_os_str() == "vignettes");
    if in_vignettes {
        "Vignettes"
    } else {
        "Other documents"
    }
}

/// The first document in display order, which is grouped by section.
fn first_listed(app: &App) -> Option<usize> {
    picker_rows(app).iter().find_map(|row| match row {
        PickerRow::File(index) => Some(*index),
        PickerRow::Group(_) => None,
    })
}

/// Where the highlighted document sits in the list, headings included.
fn picker_position(app: &App) -> usize {
    picker_rows(app)
        .iter()
        .position(|row| matches!(row, PickerRow::File(index) if *index == app.picker_file))
        .unwrap_or(0)
}

fn picker_scroll(app: &App, height: usize) -> usize {
    picker_position(app).saturating_sub(height / 2)
}

fn picker_lines(app: &App) -> Vec<Line<'static>> {
    let cwd = std::env::current_dir().unwrap_or_default();
    picker_rows(app)
        .iter()
        .map(|row| match row {
            PickerRow::Group(title) => Line::from(Span::styled(
                title.to_string(),
                app.theme.chrome().add_modifier(Modifier::BOLD),
            )),
            PickerRow::File(index) => {
                let path = &app.files[*index];
                let label = path
                    .strip_prefix(&cwd)
                    .map(|rel| rel.display().to_string())
                    .unwrap_or_else(|_| path.display().to_string());
                let style = if *index == app.picker_file {
                    Style::default()
                        .fg(app.theme.accent_color())
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                Line::from(Span::styled(format!("  {label}"), style))
            }
        })
        .collect()
}

/// The package the listed documents belong to, taken from their shared parent.
fn picker_label(app: &App) -> String {
    let mut shared: Option<Vec<String>> = None;
    for path in &app.files {
        let parts: Vec<String> = path
            .parent()
            .map(|dir| {
                dir.components()
                    .map(|part| part.as_os_str().to_string_lossy().to_string())
                    .collect()
            })
            .unwrap_or_default();
        shared = Some(match shared {
            None => parts,
            Some(current) => current
                .into_iter()
                .zip(parts)
                .take_while(|(left, right)| left == right)
                .map(|(left, _)| left)
                .collect(),
        });
    }
    shared
        .and_then(|parts| parts.last().cloned())
        .unwrap_or_else(|| "R Markdown".to_string())
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Three sections with four paragraphs each, so the document is comfortably
    /// taller than the test viewport.
    const DOC: &str = "\
# One

alpha paragraph

beta paragraph

gamma paragraph

delta paragraph

## Two

epsilon paragraph

zeta paragraph

eta paragraph

theta paragraph

# Three

iota paragraph

kappa paragraph

lambda paragraph

mu paragraph
";

    /// A document on disk that cleans up after itself.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str, text: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("rmdt-tui-{}-{}", name, std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("scratch dir");
            std::fs::write(dir.join("doc.Rmd"), text).expect("write doc");
            Self(dir)
        }

        fn doc(&self) -> PathBuf {
            self.0.join("doc.Rmd")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn options() -> RunOptions {
        RunOptions {
            theme: TuiThemeChoice::GlowDark,
            site: Vec::new(),
            overrides: Overrides::default(),
            watch: false,
            outline: OutlineMode::Pinned,
        }
    }

    fn app_for(scratch: &Scratch) -> App {
        let mut app = App::new(vec![scratch.doc()], options()).expect("app");
        // A viewport shorter than the document, and a terminal wide enough for
        // the index column, which is what a draw would have recorded.
        app.height = 8;
        app.last_width = 120;
        app
    }

    fn press(app: &mut App, code: KeyCode) {
        app.key(KeyEvent::from(code));
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    fn screen(lines: &[Line<'_>]) -> String {
        lines.iter().map(line_text).collect::<Vec<_>>().join("\n")
    }

    fn status(app: &App) -> String {
        line_text(&status_line(app))
    }

    #[test]
    fn the_terminal_theme_carries_glow_colours() {
        use crate::theme::{Depth, TuiThemeChoice};
        use tui_markdown::StyleSheet;
        let dark = TuiTheme::with_depth(TuiThemeChoice::GlowDark, Depth::TrueColor);
        assert_eq!(
            dark.heading(2).fg,
            Some(Color::Indexed(39)),
            "glamour's heading blue"
        );
        assert_eq!(
            dark.heading(1).bg,
            Some(Color::Indexed(63)),
            "glow's purple bar behind the first heading"
        );
        assert_eq!(
            dark.code().bg,
            Some(Color::Indexed(236)),
            "the inline code chip"
        );
        assert_eq!(dark.index_selected().fg, Some(Color::Rgb(0xEC, 0xFD, 0x65)));

        let light = TuiTheme::with_depth(TuiThemeChoice::GlowLight, Depth::TrueColor);
        assert_eq!(light.heading(2).fg, Some(Color::Indexed(27)));

        let minimal = TuiTheme::with_depth(TuiThemeChoice::Minimal, Depth::TrueColor);
        assert_eq!(
            minimal.heading(1).fg,
            Some(Color::Reset),
            "no fixed colours"
        );
    }

    #[test]
    fn the_fixture_is_taller_than_the_viewport() {
        let scratch = Scratch::new("fixture", DOC);
        let app = app_for(&scratch);
        assert!(
            app.max_offset() >= app.outline.last().expect("sections").row,
            "rows {} height {} outline {:?}",
            app.rows.len(),
            app.height,
            app.outline
                .iter()
                .map(|entry| entry.row)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn the_document_shows_up_with_its_headings() {
        let scratch = Scratch::new("render", DOC);
        let app = app_for(&scratch);
        let text = screen(&visible_rows(&app, 30));
        assert!(text.contains("One"));
        assert!(text.contains("alpha paragraph"));
        assert!(text.contains("Three"));
    }

    #[test]
    fn keys_scroll_page_and_jump() {
        let scratch = Scratch::new("scroll", DOC);
        let mut app = app_for(&scratch);

        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.offset, 1);
        press(&mut app, KeyCode::Char('k'));
        assert_eq!(app.offset, 0);
        press(&mut app, KeyCode::Char('G'));
        assert_eq!(app.offset, app.max_offset());
        press(&mut app, KeyCode::Char('g'));
        assert_eq!(app.offset, 0);
        press(&mut app, KeyCode::Char('}'));
        assert_eq!(app.offset, 0, "an unbound key changes nothing");
        press(&mut app, KeyCode::Char('d'));
        assert!(app.offset > 0);
        press(&mut app, KeyCode::Char('u'));
        assert_eq!(app.offset, 0);
        press(&mut app, KeyCode::Char('l'));
        assert!(app.offset > 0, "half page down");
        press(&mut app, KeyCode::Char('h'));
        assert_eq!(app.offset, 0);
    }

    #[test]
    fn brackets_walk_between_sections() {
        let scratch = Scratch::new("sections", DOC);
        let mut app = app_for(&scratch);
        press(&mut app, KeyCode::Char(']'));
        assert_eq!(app.offset, app.outline[1].row);
        press(&mut app, KeyCode::Char(']'));
        assert_eq!(app.offset, app.outline[2].row);
        press(&mut app, KeyCode::Char('['));
        assert_eq!(app.offset, app.outline[1].row);
    }

    #[test]
    fn the_status_line_reports_the_file_and_the_section() {
        let scratch = Scratch::new("status", DOC);
        let mut app = app_for(&scratch);
        let text = status(&app);
        assert!(text.contains("doc.Rmd"));
        assert!(text.contains('%'));
        assert!(text.contains("One"), "{text}");

        app.offset = app.outline[1].row;
        assert!(status(&app).contains("Two"), "{}", status(&app));
        app.offset = app.outline[2].row;
        assert!(status(&app).contains("Three"), "{}", status(&app));
    }

    #[test]
    fn the_outline_popup_lists_headings_and_jumps() {
        let scratch = Scratch::new("outline", DOC);
        let mut app = app_for(&scratch);
        press(&mut app, KeyCode::Char('o'));
        assert!(app.popup);
        let listed = screen(&outline_lines(&app, 20));
        assert!(listed.contains("One"));
        assert!(listed.contains("Three"));

        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.popup_index, 1);
        press(&mut app, KeyCode::Enter);
        assert!(!app.popup);
        assert_eq!(app.offset, app.outline[1].row);

        press(&mut app, KeyCode::Char('o'));
        press(&mut app, KeyCode::Esc);
        assert!(!app.popup, "escape closes the popup");
    }

    #[test]
    fn the_index_column_is_pinned_by_default() {
        let scratch = Scratch::new("pinned", DOC);
        let mut app = App::new(vec![scratch.doc()], options()).expect("app");
        assert_eq!(
            app.sidebar_width(100),
            SIDEBAR_WIDTH,
            "the index column is there to be navigated"
        );
        assert_eq!(app.sidebar_width(60), 0, "too narrow for a column");
        let listed = screen(&sidebar_lines(&app, 20));
        assert!(listed.contains("Contents"));
        assert!(listed.contains("One"), "{listed}");
        press(&mut app, KeyCode::Char('t'));
        assert_eq!(app.sidebar_width(100), 0, "t switches to the popup");
        press(&mut app, KeyCode::Char('t'));
        assert_eq!(app.sidebar_width(100), SIDEBAR_WIDTH, "and back");
    }

    #[test]
    fn search_counts_matches_and_moves_the_view() {
        let scratch = Scratch::new("search", DOC);
        let mut app = app_for(&scratch);
        press(&mut app, KeyCode::Char('/'));
        for ch in "paragraph".chars() {
            press(&mut app, KeyCode::Char(ch));
        }
        assert!(status(&app).contains("search: paragraph"));
        press(&mut app, KeyCode::Enter);

        let search = app.search.as_ref().expect("a search");
        assert_eq!(search.matches.len(), 12);
        let first = app.offset;
        assert!(status(&app).contains("/paragraph 1/12"), "{}", status(&app));

        press(&mut app, KeyCode::Char('n'));
        assert_ne!(app.offset, first, "the view follows the hit");
        assert!(status(&app).contains("2/12"));
        press(&mut app, KeyCode::Char('N'));
        assert!(status(&app).contains("1/12"));
        assert!(!visible_rows(&app, 30).is_empty());
    }

    #[test]
    fn search_ignores_case() {
        let scratch = Scratch::new("search-case", "text with Needle here\n");
        let mut app = app_for(&scratch);
        app.apply_search("needle".to_string(), 0);
        assert_eq!(app.search.as_ref().expect("search").matches.len(), 1);
        app.apply_search("absent".to_string(), 0);
        assert_eq!(app.search.as_ref().expect("search").matches.len(), 0);
    }

    #[test]
    fn escape_clears_the_search() {
        let scratch = Scratch::new("search-esc", DOC);
        let mut app = app_for(&scratch);
        app.apply_search("paragraph".to_string(), 0);
        assert!(app.search.is_some());
        press(&mut app, KeyCode::Esc);
        assert!(app.search.is_none());
    }

    #[test]
    fn the_help_overlay_opens_and_closes() {
        let scratch = Scratch::new("help", DOC);
        let mut app = app_for(&scratch);
        press(&mut app, KeyCode::Char('?'));
        assert!(app.help);
        let text = screen(&help_lines(&app));
        assert!(text.contains("outline popup"));
        assert!(text.contains("quit"));
        press(&mut app, KeyCode::Char('x'));
        assert!(!app.help);
    }

    #[test]
    fn q_quits_and_ctrl_c_quits() {
        let scratch = Scratch::new("quit", DOC);
        let mut app = app_for(&scratch);
        press(&mut app, KeyCode::Char('q'));
        assert!(app.should_quit);
        let mut app = app_for(&scratch);
        app.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.should_quit);
    }

    #[test]
    fn e_asks_for_the_editor_at_the_current_source_line() {
        let scratch = Scratch::new("editor", DOC);
        let mut app = app_for(&scratch);
        app.offset = app.outline[1].row;
        press(&mut app, KeyCode::Char('e'));
        assert_eq!(app.want_editor, Some(app.rows[app.offset].src));
        assert!(app.want_editor.expect("line") > 1);
    }

    #[test]
    fn reloading_keeps_the_reading_position() {
        let scratch = Scratch::new("reload", DOC);
        let mut app = app_for(&scratch);
        app.offset = app.outline[2].row;
        let anchor = app.rows[app.offset].src;
        std::fs::write(scratch.doc(), format!("{DOC}\n# Four\n\nnu paragraph\n")).expect("rewrite");
        app.reload();
        assert_eq!(app.rows[app.offset].src, anchor);
        assert!(app.outline.iter().any(|entry| entry.title == "Four"));
        assert_eq!(app.outline.len(), 4);
    }

    #[test]
    fn several_files_start_in_the_picker() {
        let scratch = Scratch::new("picker", DOC);
        let other = scratch.0.join("second.Rmd");
        std::fs::write(&other, "# Second\n").expect("write");
        let app = App::new(vec![scratch.doc(), other], options()).expect("app");
        assert_eq!(app.files.len(), 2);
        let listed = screen(&picker_lines(&app));
        assert!(listed.contains("doc.Rmd"));
    }

    #[test]
    fn the_picker_is_grouped_like_the_written_site() {
        let scratch = Scratch::new("picker-groups", DOC);
        let vignette = scratch.0.join("vignettes");
        std::fs::create_dir_all(&vignette).expect("vignettes dir");
        let inner = vignette.join("intro.Rmd");
        std::fs::write(&inner, "# Intro\n").expect("write");
        // Sorted order puts the top level file first, the display order does not.
        let app = App::new(vec![scratch.doc(), inner], options()).expect("app");
        let rows = screen(&picker_lines(&app));
        assert!(rows.contains("Vignettes"), "{rows}");
        assert!(rows.contains("Other documents"), "{rows}");
        assert!(
            rows.find("Vignettes").expect("heading")
                < rows.find("Other documents").expect("heading"),
            "{rows}"
        );

        // Moving down walks the documents in the order they are shown.
        let mut app = app;
        assert_eq!(
            app.picker_file,
            first_listed(&app).expect("a listed file"),
            "the highlight starts on the first row shown"
        );
        let listed: Vec<usize> = picker_rows(&app)
            .iter()
            .filter_map(|row| match row {
                PickerRow::File(index) => Some(*index),
                PickerRow::Group(_) => None,
            })
            .collect();
        app.picker_move(true);
        assert_eq!(app.picker_file, listed[1]);
        app.picker_move(false);
        assert_eq!(app.picker_file, listed[0]);
        app.picker_move(false);
        assert_eq!(app.picker_file, listed[0], "stops at the top");
    }

    #[test]
    fn an_unreadable_save_keeps_the_page_on_screen() {
        let scratch = Scratch::new("bad-reload", DOC);
        let mut app = app_for(&scratch);
        app.offset = app.outline[1].row;
        let before = app.rows.len();

        std::fs::write(scratch.doc(), b"# One\n\n\xff\xfe not utf8\n").expect("rewrite");
        app.reload();

        assert_eq!(app.rows.len(), before, "the page was replaced by nothing");
        assert!(app.status.is_some(), "the reader was not told");
        assert!(
            app.rows.iter().any(|row| row.text().contains("One")),
            "the text on screen was lost"
        );
        assert!(status(&app).contains("could not read"), "{}", status(&app));

        // A good save clears the message.
        std::fs::write(scratch.doc(), DOC).expect("rewrite");
        app.reload();
        assert!(app.status.is_none());
        assert!(!status(&app).contains("could not read"));
    }

    /// A reader opened on a package: this document plus one other.
    fn app_with_site(scratch: &Scratch) -> App {
        let other = scratch.0.join("other.Rmd");
        std::fs::write(&other, "# Another document\n\ntext\n").expect("write");
        let mut app = App::new(
            vec![scratch.doc()],
            RunOptions {
                // Both documents, the way a package hands them over.
                site: vec![
                    ("This document".to_string(), scratch.doc()),
                    ("Another document".to_string(), other),
                ],
                ..options()
            },
        )
        .expect("app");
        app.height = 8;
        app.last_width = 120;
        app
    }

    #[test]
    fn the_index_column_lists_the_package_then_the_sections() {
        let scratch = Scratch::new("index-items", DOC);
        let app = app_with_site(&scratch);
        assert_eq!(
            app.index_items[0],
            IndexItem::Document {
                path: scratch.0.join("other.Rmd"),
                label: "Another document".to_string()
            },
            "the other document, not the one being read"
        );
        assert!(matches!(app.index_items[1], IndexItem::Section { .. }));
        assert_eq!(index_title(&app), "Documents");

        // A rule is drawn where the document list ends.
        let drawn = screen(&sidebar_lines(&app, 20));
        assert!(drawn.contains("Documents"), "{drawn}");
        assert!(drawn.contains("Another document"), "{drawn}");
        assert!(drawn.contains("One"), "{drawn}");
        assert!(
            drawn.contains('─'),
            "no rule between the two groups: {drawn}"
        );
    }

    #[test]
    fn arrows_pick_a_pane_and_enter_enters_a_section() {
        let scratch = Scratch::new("arrows", DOC);
        let mut app = app_with_site(&scratch);
        assert_eq!(app.focus, Pane::Document, "the document starts focused");

        press(&mut app, KeyCode::Left);
        assert_eq!(app.focus, Pane::Index);
        let first_section = app
            .index_items
            .iter()
            .position(|item| matches!(item, IndexItem::Section { .. }))
            .expect("a section");
        app.index_cursor = first_section;
        press(&mut app, KeyCode::Down);
        assert_eq!(app.index_cursor, first_section + 1, "down moves the cursor");

        let target = match app.index_items[app.index_cursor] {
            IndexItem::Section { row, .. } => row,
            _ => panic!("expected a section"),
        };
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.offset, target, "Enter enters the section");
        assert_eq!(app.focus, Pane::Document, "and hands the arrows back");

        press(&mut app, KeyCode::Up);
        assert_eq!(app.focus, Pane::Document);
        assert!(
            app.offset < target,
            "up scrolls the document when it has focus"
        );
    }

    #[test]
    fn enter_on_a_document_opens_it() {
        let scratch = Scratch::new("index-open", DOC);
        let mut app = app_with_site(&scratch);
        press(&mut app, KeyCode::Left);
        app.index_cursor = 0;
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.path, scratch.0.join("other.Rmd"));
        assert!(
            app.rows
                .iter()
                .any(|row| row.text().contains("Another document")),
            "the opened document is on screen"
        );
        assert!(
            !app.index_items.iter().any(|item| matches!(
                item,
                IndexItem::Document { label, .. } if label == "Another document"
            )),
            "the column no longer offers the document being read"
        );
        assert_eq!(
            app.index_items
                .iter()
                .filter_map(|item| match item {
                    IndexItem::Document { label, .. } => Some(label.as_str()),
                    IndexItem::Section { .. } => None,
                })
                .collect::<Vec<_>>(),
            vec!["This document"],
            "the column now offers the document that was opened first"
        );
    }

    #[test]
    fn the_cursor_stays_visible_when_the_column_scrolls() {
        let mut source = String::new();
        for index in 0..30 {
            source.push_str(&format!("# Section {index}\n\nparagraph {index}\n\n"));
        }
        let scratch = Scratch::new("index-scroll", &source);
        let mut app = app_for(&scratch);
        app.height = 8;
        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Char('G'));
        for _ in 0..40 {
            press(&mut app, KeyCode::Down);
        }
        assert_eq!(app.index_cursor, app.index_items.len() - 1);
        assert!(app.index_scroll > 0, "the column scrolled");
        assert!(
            app.index_scroll <= app.index_cursor,
            "scroll {} is past the cursor {}",
            app.index_scroll,
            app.index_cursor
        );
        let drawn = screen(&sidebar_lines(&app, app.height));
        assert!(
            drawn.lines().any(|line| line.contains("Section 29")),
            "the cursor item is not on screen:\n{drawn}"
        );

        // Walking back up must bring the cursor item with it, all the way to
        // the first entry.
        for _ in 0..40 {
            press(&mut app, KeyCode::Up);
        }
        assert_eq!(app.index_cursor, 0);
        assert_eq!(app.index_scroll, 0, "the column did not return to the top");
        let drawn = screen(&sidebar_lines(&app, app.height));
        assert!(drawn.lines().any(|line| line.contains("Section 0")));
    }

    #[test]
    fn the_cursor_stays_visible_with_a_rule_above_it() {
        let scratch = Scratch::new("index-rule-scroll", &"# Heading\n\ntext\n".repeat(30));
        let mut app = app_with_site(&scratch);
        app.height = 6;
        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Char('G'));
        for _ in 0..40 {
            press(&mut app, KeyCode::Down);
        }
        let drawn = screen(&sidebar_lines(&app, app.height));
        let cursor = app.index_cursor;
        let label = match &app.index_items[cursor] {
            IndexItem::Section { title, .. } => title.clone(),
            IndexItem::Document { label, .. } => label.clone(),
        };
        assert!(
            drawn.lines().any(|line| line.contains(&label)),
            "cursor item {label:?} is not on screen:\n{drawn}"
        );
        assert!(
            drawn.lines().count() <= app.height,
            "the column drew more lines than it has room for"
        );
    }

    #[test]
    fn the_outline_can_be_switched_off() {
        let scratch = Scratch::new("outline-off", DOC);
        let mut app = App::new(
            vec![scratch.doc()],
            RunOptions {
                outline: OutlineMode::Off,
                ..options()
            },
        )
        .expect("app");
        assert_eq!(app.sidebar_width(200), 0);
        press(&mut app, KeyCode::Char('t'));
        assert_eq!(
            app.sidebar_width(200),
            SIDEBAR_WIDTH,
            "t still pins it on request"
        );
        press(&mut app, KeyCode::Char('t'));
        assert_eq!(app.sidebar_width(200), 0);
    }

    #[test]
    fn drawing_survives_awkward_terminal_sizes() {
        let scratch = Scratch::new("sizes", DOC);
        for (width, height) in [(100u16, 30u16), (40, 10), (24, 4), (200, 60)] {
            let mut app = app_for(&scratch);
            let backend = ratatui::backend::TestBackend::new(width, height);
            let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
            terminal
                .draw(|frame| draw(frame, &mut app))
                .unwrap_or_else(|error| panic!("draw at {width}x{height}: {error}"));
            app.popup = true;
            app.help = true;
            terminal
                .draw(|frame| draw(frame, &mut app))
                .unwrap_or_else(|error| panic!("overlays at {width}x{height}: {error}"));
        }
    }

    #[test]
    fn the_index_column_reaches_the_screen() {
        let scratch = Scratch::new("column-screen", DOC);
        let mut app = app_for(&scratch);
        let backend = ratatui::backend::TestBackend::new(100, 24);
        let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
        terminal.draw(|frame| draw(frame, &mut app)).expect("draw");
        let shown = format!("{:?}", terminal.backend());
        assert!(shown.contains("Contents"), "the column is missing: {shown}");
        assert!(shown.contains('│'), "no divider between the panes: {shown}");
        assert!(shown.contains("alpha paragraph"), "{shown}");
    }

    #[test]
    fn the_overlays_reach_the_screen() {
        let scratch = Scratch::new("overlays", DOC);
        let mut app = app_for(&scratch);
        let backend = ratatui::backend::TestBackend::new(90, 28);
        let mut terminal = ratatui::Terminal::new(backend).expect("terminal");

        app.help = true;
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw help");
        let shown = format!("{:?}", terminal.backend());
        assert!(shown.contains("outline popup"), "help overlay: {shown}");
        assert!(shown.contains("open the source line in $EDITOR"), "{shown}");

        app.help = false;
        app.popup = true;
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw outline");
        let shown = format!("{:?}", terminal.backend());
        assert!(shown.contains("Outline"), "{shown}");
        assert!(shown.contains("Three"), "{shown}");
    }

    #[test]
    fn shorten_measures_columns_not_characters() {
        assert_eq!(shorten("Intro", 28), "Intro");
        assert_eq!(shorten("abc", 0), "");
        let cut = shorten("检索与过滤流程的详细说明与示例", 28);
        assert!(cut.width() <= 28, "{cut:?} is {} columns", cut.width());
        assert!(cut.ends_with('…'), "{cut:?}");
    }

    #[test]
    fn a_wide_heading_keeps_the_index_column_one_row_per_entry() {
        let scratch = Scratch::new("wide-label", "# 检索与过滤流程的详细说明与示例\n\ntext\n");
        let app = app_for(&scratch);
        let lines = sidebar_lines(&app, 20);
        for line in &lines {
            let text = line_text(line);
            let width = text.width();
            assert!(
                width <= SIDEBAR_WIDTH as usize,
                "{text:?} is {width} columns"
            );
        }
        assert!(screen(&lines).contains('…'), "the wide label should be cut");
    }

    #[test]
    fn the_rendered_screen_carries_the_document_text() {
        let scratch = Scratch::new("screen", DOC);
        let mut app = app_for(&scratch);
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
        terminal.draw(|frame| draw(frame, &mut app)).expect("draw");
        let shown = format!("{:?}", terminal.backend());
        assert!(shown.contains("alpha paragraph"), "{shown}");
        assert!(shown.contains("doc.Rmd"), "{shown}");
    }
}
