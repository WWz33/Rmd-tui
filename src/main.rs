//! `rmdt` reads R Markdown in the terminal, or writes a page for a browser.
//!
//! It never runs the code in the document. Chunks that `knitr` would evaluate
//! are shown as source, or reported as hidden, and figures that would have been
//! drawn by that code get a placeholder with their caption.

mod html;
mod render;
mod rmd;
mod theme;
mod tui;

use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

use clap::Parser;

use crate::html::HtmlOptions;
use crate::rmd::Overrides;
use crate::theme::{ThemeName, TuiThemeChoice};
use crate::tui::{OutlineMode, RunOptions};

#[derive(Parser, Debug)]
#[command(
    name = "rmdt",
    version,
    about = "Read R Markdown without running any code",
    long_about = "Read R Markdown without running any code.\n\n\
        Point it at .Rmd files, or at a package directory, or run it bare inside\n\
        a package to list that package's vignettes. The default view is the\n\
        terminal; --html writes a page for a browser instead."
)]
struct Cli {
    /// .Rmd files, or a package directory (default: the package you are in)
    #[arg(value_name = "PATH")]
    inputs: Vec<PathBuf>,

    /// Write a standalone HTML page instead of opening the terminal view
    #[arg(long)]
    html: bool,

    /// Write the filtered Markdown to stdout (GitHub previews it, `mdt` reads it)
    #[arg(long)]
    md: bool,

    /// Output path (single file input)
    #[arg(long, short = 'o', value_name = "PATH")]
    output: Option<PathBuf>,

    /// Directory for the written files
    #[arg(long, short = 'd', value_name = "DIR")]
    outdir: Option<PathBuf>,

    /// Theme for the written pages, in a browser
    #[arg(long, value_enum, default_value_t = ThemeName::Rmarkdown)]
    theme: ThemeName,

    /// Theme for the terminal: glow's palette, dark or light, or minimal
    #[arg(long, value_enum, default_value_t = TuiThemeChoice::GlowDark)]
    tui_theme: TuiThemeChoice,

    /// Force the table of contents on
    #[arg(long)]
    toc: bool,

    /// Force the table of contents off
    #[arg(long)]
    no_toc: bool,

    /// Heading levels listed in the table of contents
    #[arg(long, value_name = "N")]
    toc_depth: Option<usize>,

    /// Force section numbering on
    #[arg(long)]
    number_sections: bool,

    /// Force section numbering off
    #[arg(long)]
    no_number_sections: bool,

    /// Drop the "code not executed" strip from the HTML page
    #[arg(long)]
    no_banner: bool,

    /// Leave MathJax out of the HTML page
    #[arg(long)]
    no_math: bool,

    /// Read the file again whenever it changes
    #[arg(long)]
    watch: bool,

    /// Where the index column starts: pinned, popup, or off
    #[arg(long, value_enum, default_value_t = OutlineMode::Pinned)]
    outline: OutlineMode,

    /// Open the written page in a browser
    #[arg(long)]
    open: bool,

    /// Never open anything (the default, kept so it can be stated explicitly)
    #[arg(long)]
    no_open: bool,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("rmdt: {error}");
        std::process::exit(1);
    }
}

/// Write to stdout, and treat a closed pipe as the end of the work.
///
/// The `print!` macros panic when the reader has gone away, which
/// `rmdt --md doc.Rmd | head` does on purpose.
fn say(text: &str) {
    let mut out = io::stdout().lock();
    let _ = out.write_all(text.as_bytes());
    let _ = out.flush();
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let overrides = Overrides {
        toc: pick(cli.toc, cli.no_toc),
        toc_depth: cli.toc_depth,
        number_sections: pick(cli.number_sections, cli.no_number_sections),
    };
    // Writing a page does not open anything unless it is asked for.
    let want_open = cli.open && !cli.no_open;

    let sources = if cli.inputs.is_empty() {
        vec![package_root(&std::env::current_dir()?)]
    } else {
        cli.inputs.clone()
    };
    let single_directory = sources.len() == 1 && sources[0].is_dir();
    if single_directory && (cli.md || cli.output.is_some()) {
        return Err("--md and -o work on files, not directories".into());
    }
    if cli.output.is_some() && sources.len() > 1 {
        return Err("-o works with a single file input".into());
    }
    if cli.output.is_some() && !cli.html && !cli.md {
        return Err("--output needs --html or --md".into());
    }

    if single_directory {
        let root = sources[0].clone();
        let files = collect_rmd(&root);
        if files.is_empty() {
            return Err(missing_documents(&root).into());
        }
        let outdir = cli.outdir.clone().unwrap_or_else(|| root.join(".rmdview"));
        let package = package_title(&root);
        let description = package_description(&root);

        // Parse each document once: the label for the index and the page itself
        // come from the same parse.
        let mut plan = Vec::with_capacity(files.len());
        for path in &files {
            plan.push(SitePage::new(
                rmd::load(path, &overrides)?,
                path,
                &root,
                &outdir,
            ));
        }
        let entries: Vec<html::SiteEntry> = plan.iter().map(SitePage::entry).collect();

        // A document named index.Rmd already owns index.html; the generated
        // index goes beside it, so that page is not overwritten.
        let index_name = if plan.iter().any(|page| page.target_rel == "index.html") {
            "site-index.html"
        } else {
            "index.html"
        };

        for page in &plan {
            // Every link on a page is relative to that page's own directory.
            let siblings: Vec<html::SiteEntry> = entries
                .iter()
                .map(|entry| html::SiteEntry {
                    href: relative_href(&page.target_rel, &entry.href),
                    ..entry.clone()
                })
                .collect();
            let site = html::Site {
                package: package.clone(),
                title: description.0.clone(),
                description: description.1.clone(),
                index_href: relative_href(&page.target_rel, index_name),
                current: relative_href(&page.target_rel, &page.target_rel),
                siblings,
            };
            write_html(&page.document, &page.target, &cli, Some(&site))?;
            say(&format!(
                "{} -> {}\n",
                page.document.source.display(),
                page.target.display()
            ));
        }

        let index = outdir.join(index_name);
        let index_site = html::Site {
            package: package.clone(),
            title: description.0.clone(),
            description: description.1.clone(),
            index_href: index_name.to_string(),
            current: index_name.to_string(),
            siblings: entries.clone(),
        };
        std::fs::write(&index, html::render_index(&index_site, &entries, cli.theme))?;
        say(&format!(
            "site: {} page(s) -> {}\n",
            entries.len() + 1,
            index.display()
        ));

        if cli.html {
            if want_open {
                opened(&index, &opener(&index));
            }
            return Ok(());
        }
        // Reading a package in the terminal browses the same set of documents,
        // listed in the index column.
        let documents: Vec<(String, PathBuf)> = plan
            .iter()
            .map(|page| (page.label.clone(), page.document.source.clone()))
            .collect();
        return start_terminal(
            files,
            run_options(&cli, overrides, &documents),
            Some(&index),
        );
    }

    let files: Vec<PathBuf> = sources
        .iter()
        .filter(|path| {
            if path.is_file() {
                true
            } else {
                eprintln!("rmdt: skipping {} (not a file)", path.display());
                false
            }
        })
        .cloned()
        .collect();
    if files.is_empty() {
        return Err("nothing to read".into());
    }

    if cli.md {
        // Each branch reads every document exactly once.
        match (&cli.output, &cli.outdir) {
            (Some(target), _) => {
                let mut out = String::new();
                for path in &files {
                    out.push_str(&rmd::to_markdown(&rmd::load(path, &overrides)?));
                }
                std::fs::write(target, &out)?;
                say(&format!("{}\n", target.display()));
            }
            (None, Some(dir)) => {
                for path in &files {
                    let target = target_in(dir, &files, path, "md");
                    if let Some(parent) = target.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(&target, rmd::to_markdown(&rmd::load(path, &overrides)?))?;
                    say(&format!("{} -> {}\n", path.display(), target.display()));
                }
            }
            (None, None) => {
                for path in &files {
                    say(&rmd::to_markdown(&rmd::load(path, &overrides)?));
                }
            }
        }
        return Ok(());
    }

    if cli.html {
        for path in &files {
            let target = match (&cli.output, &cli.outdir) {
                (Some(target), _) => target.clone(),
                (None, Some(dir)) => target_in(dir, &files, path, "html"),
                (None, None) => path.with_extension("html"),
            };
            write_html(&rmd::load(path, &overrides)?, &target, &cli, None)?;
            say(&format!("{} -> {}\n", path.display(), target.display()));
            if want_open {
                opened(&target, &opener(&target));
            }
        }
        return Ok(());
    }

    start_terminal(files, run_options(&cli, overrides, &[]), None)
}

/// Where one input lands under `dir`.
///
/// Two inputs can share a file name, for example `a/x.Rmd` and `b/x.Rmd`. Both
/// would be written to `dir/x.html` and the second would replace the first, so
/// a name that repeats keeps its parent directory in front of it.
fn target_in(dir: &Path, files: &[PathBuf], path: &Path, extension: &str) -> PathBuf {
    let name = path
        .file_name()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("document"));
    let repeats = files
        .iter()
        .filter(|other| other.file_name() == path.file_name())
        .count()
        > 1;
    let name = if repeats {
        match path.parent().and_then(|parent| parent.file_name()) {
            Some(parent) => PathBuf::from(parent).join(name),
            None => name,
        }
    } else {
        name
    };
    dir.join(name).with_extension(extension)
}

/// One page of the site built for a package directory.
struct SitePage {
    document: rmd::Document,
    target: PathBuf,
    /// Path of the source as written in the index.
    rel: String,
    /// Path of the page relative to the site root, used as its link target.
    target_rel: String,
    label: String,
    group: html::SiteGroup,
}

impl SitePage {
    fn new(document: rmd::Document, source: &Path, root: &Path, outdir: &Path) -> Self {
        let relative = source.strip_prefix(root).unwrap_or(source);
        let target = outdir.join(relative).with_extension("html");
        let target_rel = target
            .strip_prefix(outdir)
            .unwrap_or(&target)
            .display()
            .to_string();
        let group = if relative.starts_with("vignettes") {
            html::SiteGroup::Vignettes
        } else {
            html::SiteGroup::Other
        };
        let label = match document.meta.title.clone() {
            Some(title) => title,
            None => source
                .file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
                .unwrap_or_default(),
        };
        Self {
            document,
            target,
            rel: relative.display().to_string(),
            target_rel,
            label,
            group,
        }
    }

    fn entry(&self) -> html::SiteEntry {
        html::SiteEntry {
            href: self.target_rel.clone(),
            label: self.label.clone(),
            source: self.rel.clone(),
            group: self.group,
        }
    }
}

/// A link from one page of a site to another, both named from the site root.
fn relative_href(from_page: &str, to_page: &str) -> String {
    let from_dir: Vec<&str> = match from_page.rsplit_once('/') {
        Some((dir, _)) => dir.split('/').collect(),
        None => Vec::new(),
    };
    let to_parts: Vec<&str> = to_page.split('/').collect();
    let to_dir = &to_parts[..to_parts.len().saturating_sub(1)];

    let mut common = 0;
    while common < from_dir.len() && common < to_dir.len() && from_dir[common] == to_dir[common] {
        common += 1;
    }
    let mut parts: Vec<String> = vec!["..".to_string(); from_dir.len() - common];
    parts.extend(to_dir[common..].iter().map(|part| part.to_string()));
    if let Some(name) = to_parts.last() {
        parts.push(name.to_string());
    }
    parts.join("/")
}

/// `Title` and `Description` from DESCRIPTION, the two lines a package site
/// shows above its contents.
fn package_description(root: &Path) -> (Option<String>, Option<String>) {
    let Ok(text) = std::fs::read_to_string(root.join("DESCRIPTION")) else {
        return (None, None);
    };
    (
        description_field(&text, "Title:"),
        description_field(&text, "Description:"),
    )
}

/// Read one DESCRIPTION field, joining its continuation lines.
fn description_field(text: &str, field: &str) -> Option<String> {
    let mut collected = String::new();
    let mut inside = false;
    for line in text.lines() {
        if inside {
            let starts_field = !line.starts_with([' ', '\t'])
                && line.contains(':')
                && line.starts_with(|c: char| c.is_ascii_alphabetic());
            if starts_field {
                break;
            }
            if !line.trim().is_empty() {
                collected.push(' ');
                collected.push_str(line.trim());
            }
        } else if let Some(rest) = line.strip_prefix(field) {
            inside = true;
            collected.push_str(rest.trim());
        }
    }
    let joined = collected.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.is_empty() {
        None
    } else {
        Some(joined)
    }
}

/// Open the terminal view, or explain why it cannot be opened.
///
/// Anything already written stays where it is; the message points at it.
fn start_terminal(
    files: Vec<PathBuf>,
    options: RunOptions,
    written: Option<&Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    if !(io::stdin().is_terminal() && io::stdout().is_terminal()) {
        let where_are_the_pages = match written {
            Some(index) => format!(" The pages are already written: {}.", index.display()),
            None => String::new(),
        };
        return Err(format!(
            "the terminal view needs a terminal on stdin and stdout.{where_are_the_pages} \
             Use --html to write pages instead, or run this in a terminal."
        )
        .into());
    }
    tui::run(files, options)?;
    Ok(())
}

fn pick(on: bool, off: bool) -> Option<bool> {
    match (on, off) {
        (true, _) => Some(true),
        (_, true) => Some(false),
        _ => None,
    }
}

fn run_options(cli: &Cli, overrides: Overrides, site: &[(String, PathBuf)]) -> RunOptions {
    RunOptions {
        theme: cli.tui_theme,
        site: site.to_vec(),
        overrides,
        watch: cli.watch,
        outline: cli.outline,
    }
}

fn write_html(
    document: &rmd::Document,
    target: &Path,
    cli: &Cli,
    site: Option<&html::Site>,
) -> Result<(), Box<dyn std::error::Error>> {
    let options = HtmlOptions {
        theme: cli.theme,
        banner: !cli.no_banner,
        math: !cli.no_math,
        site: site.cloned(),
    };
    if let Some(parent) = target.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(target, html::render(document, &options))?;
    Ok(())
}

/// Walk up from `start` looking for something that is clearly an R package.
fn package_root(start: &Path) -> PathBuf {
    let start = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    // `ancestors` yields the path itself first, so the start is checked once.
    for candidate in start.ancestors() {
        if candidate.join("DESCRIPTION").is_file() || candidate.join("vignettes").is_dir() {
            return candidate.to_path_buf();
        }
    }
    start
}

fn package_title(root: &Path) -> String {
    if let Ok(text) = std::fs::read_to_string(root.join("DESCRIPTION")) {
        for line in text.lines() {
            if let Some(value) = line.strip_prefix("Package:") {
                let value = value.trim();
                if !value.is_empty() {
                    return value.to_string();
                }
            }
        }
    }
    root.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "package".to_string())
}

/// Explain an empty package, naming the Sweave case because it is common.
fn missing_documents(root: &Path) -> String {
    match find_with_extension(root, "rnw") {
        None => format!("no .Rmd files under {}", root.display()),
        Some(example) => format!(
            "no .Rmd files under {}. It holds Sweave .Rnw files such as {}, which rmdt \
             does not read: an .Rnw document is LaTeX with R chunks and needs a front end \
             this tool does not have yet.",
            root.display(),
            example.display()
        ),
    }
}

/// The first file under `root` with this extension, if there is one.
///
/// The search stops at the first hit: this only runs to explain an empty
/// package, and walking a whole tree to count files would make the error message
/// as slow as the largest directory in the package.
fn find_with_extension(root: &Path, extension: &str) -> Option<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let hidden = path
                .file_name()
                .map(|name| name.to_string_lossy().starts_with('.'))
                .unwrap_or(false);
            if hidden {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if path
                .extension()
                .map(|found| found.to_string_lossy().eq_ignore_ascii_case(extension))
                .unwrap_or(false)
            {
                return Some(path);
            }
        }
    }
    None
}

/// The vignettes of a package, plus a top level README.
fn collect_rmd(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let vignettes = root.join("vignettes");
    if vignettes.is_dir() {
        walk(&vignettes, &mut files, 0);
    }
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && is_rmd(&path) {
                files.push(path);
            }
        }
    }
    files.sort();
    files.dedup();
    files
}

fn walk(dir: &Path, files: &mut Vec<PathBuf>, depth: usize) {
    if depth > 4 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let hidden = path
            .file_name()
            .map(|name| name.to_string_lossy().starts_with('.'))
            .unwrap_or(false);
        if hidden {
            continue;
        }
        if path.is_dir() {
            walk(&path, files, depth + 1);
        } else if is_rmd(&path) {
            files.push(path);
        }
    }
}

fn is_rmd(path: &Path) -> bool {
    path.extension()
        .map(|extension| extension.to_string_lossy().eq_ignore_ascii_case("rmd"))
        .unwrap_or(false)
}

/// Say so only when a browser really was asked for and started.
fn opened(target: &Path, result: &io::Result<()>) {
    match result {
        Ok(()) => say(&format!("opened {}\n", target.display())),
        Err(error) => eprintln!("rmdt: could not open {}: {error}", target.display()),
    }
}

fn opener(target: &Path) -> io::Result<()> {
    // A browser needs an absolute file URL; a relative one points nowhere.
    let absolute = std::path::absolute(target).unwrap_or_else(|_| target.to_path_buf());
    let url = format!("file://{}", absolute.display());
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(not(target_os = "macos"))]
    let mut command = std::process::Command::new("xdg-open");
    command.arg(url).status().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_index_column_is_the_default() {
        let cli = Cli::parse_from(["rmdt", "doc.Rmd"]);
        assert_eq!(cli.outline, OutlineMode::Pinned);
        assert_eq!(cli.tui_theme, TuiThemeChoice::GlowDark);
        let explicit = Cli::parse_from(["rmdt", "doc.Rmd", "--outline", "popup"]);
        assert_eq!(explicit.outline, OutlineMode::Popup);
    }
}
