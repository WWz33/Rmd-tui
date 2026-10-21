//! Command line behaviour, exercised through the built binary.

use std::path::{Path, PathBuf};
use std::process::Command;

fn rmdt() -> Command {
    Command::new(env!("CARGO_BIN_EXE_rmdt"))
}

/// A scratch package that cleans up after itself.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("rmdt-cli-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Self(dir)
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("fixture dir");
        }
        std::fs::write(&path, text).expect("write fixture");
        path
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn assert_ok(output: &std::process::Output, what: &str) {
    assert!(
        output.status.success(),
        "{what} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn help_is_available() {
    let output = rmdt().arg("--help").output().expect("run rmdt --help");
    assert_ok(&output, "--help");
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("--html"));
    assert!(text.contains("--md"));
    assert!(text.contains("--watch"));
}

#[test]
fn a_single_file_writes_html_beside_it() {
    let scratch = Scratch::new("single");
    let source = scratch.write("note.Rmd", "---\ntitle: T\n---\n\nhi\n");
    let output = rmdt()
        .arg(&source)
        .args(["--html", "--no-open"])
        .output()
        .expect("run");
    assert_ok(&output, "--html");
    let target = scratch.path().join("note.html");
    assert!(target.is_file());
    let page = std::fs::read_to_string(&target).expect("read page");
    assert!(page.contains("<title>T</title>"));
    assert!(page.contains("code not executed"));
}

#[test]
fn markdown_goes_to_stdout_and_writes_no_file() {
    let scratch = Scratch::new("md");
    let source = scratch.write("note.Rmd", "```{r keep}\nx <- 1\n```\n");
    let output = rmdt().arg(&source).arg("--md").output().expect("run");
    assert_ok(&output, "--md");
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("```r"));
    assert!(text.contains("x <- 1"));
    assert!(!scratch.path().join("note.md").exists());
    assert!(!scratch.path().join("note.html").exists());
}

#[test]
fn outdir_redirects_the_output() {
    let scratch = Scratch::new("outdir");
    let source = scratch.write("note.Rmd", "hi\n");
    let outdir = scratch.path().join("build");
    let output = rmdt()
        .arg(&source)
        .args(["--html", "--no-open", "-d"])
        .arg(&outdir)
        .output()
        .expect("run");
    assert_ok(&output, "-d");
    assert!(outdir.join("note.html").is_file());
}

#[test]
fn output_path_rejects_several_inputs() {
    let scratch = Scratch::new("two-inputs");
    let a = scratch.write("a.Rmd", "x\n");
    let b = scratch.write("b.Rmd", "y\n");
    let output = rmdt()
        .arg(&a)
        .arg(&b)
        .args(["--html", "-o"])
        .arg(scratch.path().join("out.html"))
        .output()
        .expect("run");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("single file"));
}

#[test]
fn a_directory_builds_an_index_and_leaves_the_sources_alone() {
    let scratch = Scratch::new("package");
    scratch.write("DESCRIPTION", "Package: mypkg\nVersion: 0.1\n");
    scratch.write(
        "vignettes/intro.Rmd",
        "---\ntitle: \"Intro vignette\"\ntoc: true\n---\n\n# Start\n\n## Deeper\n\ntext\n",
    );
    scratch.write("vignettes/extra.Rmd", "---\ntitle: \"Extra\"\n---\n\nhi\n");
    scratch.write("README.Rmd", "---\ntitle: \"Readme\"\n---\n\nreadme text\n");
    let before: Vec<String> = std::fs::read_dir(scratch.path())
        .expect("read dir")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect();

    let output = rmdt()
        .arg(scratch.path())
        .args(["--html", "--no-open"])
        .output()
        .expect("run");
    assert_ok(&output, "directory mode");

    let outdir = scratch.path().join(".rmdview");
    assert!(outdir.join("index.html").is_file());
    assert!(outdir.join("vignettes/intro.html").is_file());
    assert!(outdir.join("vignettes/extra.html").is_file());
    assert!(outdir.join("README.html").is_file());
    let index = std::fs::read_to_string(outdir.join("index.html")).expect("read index");
    assert!(index.contains("mypkg"));
    assert!(index.contains("Vignettes"));
    assert!(index.contains("href=\"vignettes/intro.html\""));
    assert!(index.contains("Intro vignette"));

    let after: Vec<String> = std::fs::read_dir(scratch.path())
        .expect("read dir")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| name != ".rmdview")
        .collect();
    let mut before_sorted = before;
    let mut after_sorted = after;
    before_sorted.sort();
    after_sorted.sort();
    assert_eq!(before_sorted, after_sorted, "source tree changed");
}

#[test]
fn a_package_becomes_a_site_whose_links_all_resolve() {
    let scratch = Scratch::new("site");
    scratch.write(
        "DESCRIPTION",
        "Package: sitepkg\nType: Package\nTitle: A Title From Description\n\
         Description: A description that spans\n    more than one line.\nVersion: 0.1\n",
    );
    scratch.write("README.Rmd", "---\ntitle: \"Readme\"\n---\n\nreadme text\n");
    scratch.write(
        "vignettes/deep/intro.Rmd",
        "---\ntitle: \"Intro vignette\"\n---\n\n# Start\n\ntext\n",
    );
    scratch.write("vignettes/other.Rmd", "---\ntitle: \"Other\"\n---\n\nhi\n");

    let output = rmdt()
        .arg(scratch.path())
        .args(["--html"])
        .output()
        .expect("run");
    assert_ok(&output, "site build");

    let site = scratch.path().join(".rmdview");
    let pages: Vec<PathBuf> = walk_html(&site);
    assert_eq!(pages.len(), 4, "index plus three pages: {pages:?}");

    // Every relative link on every page points at a page that exists.
    let mut broken = Vec::new();
    for page in &pages {
        let text = std::fs::read_to_string(page).expect("read page");
        for href in links(&text) {
            if href.starts_with("http") {
                continue;
            }
            let target = page.parent().expect("parent").join(&href);
            if !target.exists() {
                broken.push(format!("{} -> {href}", page.display()));
            }
        }
    }
    assert!(broken.is_empty(), "broken links: {broken:?}");

    // The landing page reads like a package page.
    let index = std::fs::read_to_string(site.join("index.html")).expect("index");
    assert!(index.contains("sitepkg"));
    assert!(index.contains("A Title From Description"));
    assert!(index.contains("A description that spans more than one line."));
    assert!(index.contains("Vignettes"));
    assert!(index.contains("Other documents"));
    assert!(index.contains("vignettes/deep/intro.html"));

    // A page two directories down still links home and to its siblings.
    let deep = std::fs::read_to_string(site.join("vignettes/deep/intro.html")).expect("deep page");
    let nav = deep
        .split("<header class=\"site-nav\">")
        .nth(1)
        .and_then(|rest| rest.split("</header>").next())
        .expect("a navigation bar")
        .to_string();
    assert!(nav.contains(r#"href="../../index.html""#), "{nav}");
    assert!(nav.contains("other.html"), "{nav}");
    assert!(nav.contains("site-current"), "{nav}");

    // And the document's own contents and numbering survived the trip.
    assert!(deep.contains("<nav class=\"toc\">") || deep.contains("<h1"));
}

fn walk_html(dir: &Path) -> Vec<PathBuf> {
    let mut pages = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "html") {
                pages.push(path);
            }
        }
    }
    pages.sort();
    pages
}

fn links(html: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find("href=\"") {
        let after = &rest[start + 6..];
        let Some(end) = after.find('"') else { break };
        let href = &after[..end];
        if !href.starts_with('#') {
            found.push(href.to_string());
        }
        rest = &after[end..];
    }
    found
}

#[test]
fn without_a_terminal_it_explains_instead_of_crashing() {
    let scratch = Scratch::new("no-terminal");
    scratch.write("DESCRIPTION", "Package: nopty\nTitle: T\n");
    scratch.write("vignettes/a.Rmd", "# One\n\ntext\n");
    scratch.write("README.Rmd", "# Readme\n");

    // `.output()` pipes stdout, so the process has no terminal.
    let output = rmdt().arg(scratch.path()).output().expect("run");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("panicked"),
        "a panic is not an error report: {stderr}"
    );
    assert!(stderr.contains("needs a terminal"), "{stderr}");
    assert!(!output.status.success(), "the terminal view did not open");
    // The pages it had already written are named in the message.
    assert!(scratch.path().join(".rmdview/index.html").is_file());
    assert!(stderr.contains(".rmdview"), "{stderr}");

    let single = scratch.write("solo.Rmd", "hi\n");
    let output = rmdt().arg(&single).output().expect("run");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("panicked"), "{stderr}");
    assert!(stderr.contains("needs a terminal"), "{stderr}");
}

#[test]
fn a_directory_rejects_markdown_output() {
    let scratch = Scratch::new("dir-md");
    scratch.write("vignettes/a.Rmd", "hi\n");
    let output = rmdt()
        .arg(scratch.path())
        .arg("--md")
        .output()
        .expect("run");
    assert!(!output.status.success());
}

#[test]
fn an_empty_directory_says_so() {
    let scratch = Scratch::new("empty");
    let output = rmdt().arg(scratch.path()).output().expect("run");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("no .Rmd files"));
}

/// A PATH entry holding a fake browser opener, so opening can be observed
/// without launching anything.
fn fake_opener(dir: &Path) -> String {
    let bin = dir.join("fake-bin");
    std::fs::create_dir_all(&bin).expect("fake bin");
    for name in ["open", "xdg-open"] {
        let script = bin.join(name);
        std::fs::write(
            &script,
            format!("#!/bin/sh\necho \"$@\" >> {}/opened.log\n", dir.display()),
        )
        .expect("write fake opener");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&script).expect("stat").permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&script, permissions).expect("chmod");
        }
    }
    bin.display().to_string()
}

#[test]
fn writing_a_page_does_not_open_a_browser_unless_asked() {
    let scratch = Scratch::new("browser-default");
    let source = scratch.write("note.Rmd", "hi\n");
    let path = format!(
        "{}:{}",
        fake_opener(scratch.path()),
        std::env::var("PATH").unwrap_or_default()
    );
    let log = scratch.path().join("opened.log");

    let default = rmdt()
        .arg(&source)
        .args(["--html"])
        .env("PATH", &path)
        .output()
        .expect("run");
    assert_ok(&default, "--html");
    assert!(!log.exists(), "the default opened a browser");
    assert!(!String::from_utf8_lossy(&default.stdout).contains("opened"));

    let asked = rmdt()
        .arg(&source)
        .args(["--html", "--open"])
        .env("PATH", &path)
        .output()
        .expect("run");
    assert_ok(&asked, "--open");
    let recorded = std::fs::read_to_string(&log).expect("the opener ran");
    assert!(recorded.starts_with("file:///"), "relative url: {recorded}");
    let target = recorded
        .trim()
        .trim_start_matches("file://")
        .replace("%20", " ");
    assert!(
        Path::new(&target).is_file(),
        "url does not resolve: {recorded}"
    );
}

#[test]
fn the_contents_and_numbering_can_be_forced_either_way() {
    let scratch = Scratch::new("toc-flags");
    // The front matter asks for both; the flags have to win.
    let source = scratch.write(
        "note.Rmd",
        "---\ntoc: true\nnumber_sections: yes\n---\n\n# One\n\ntext\n\n## Two\n\nmore\n",
    );
    let on = rmdt().arg(&source).args(["--html"]).output().expect("run");
    assert_ok(&on, "default");
    let page = std::fs::read_to_string(scratch.path().join("note.html")).expect("page");
    assert!(page.contains("<nav class=\"toc\">"));
    assert!(page.contains("<span class=\"secnum\">1</span>"));

    let off = rmdt()
        .arg(&source)
        .args(["--html", "--no-toc", "--no-number-sections"])
        .output()
        .expect("run");
    assert_ok(&off, "--no-toc");
    let page = std::fs::read_to_string(scratch.path().join("note.html")).expect("page");
    assert!(!page.contains("<nav class=\"toc\">"));
    assert!(!page.contains("class=\"secnum\""));

    let forced = rmdt()
        .arg(&source)
        .args(["--html", "--toc-depth", "2"])
        .output()
        .expect("run");
    assert_ok(&forced, "--toc-depth");
}

#[test]
fn markdown_can_be_written_to_a_file() {
    let scratch = Scratch::new("md-file");
    let source = scratch.write("note.Rmd", "```{r keep}\nx <- 1\n```\n");
    let target = scratch.path().join("out.md");
    let output = rmdt()
        .arg(&source)
        .args(["--md", "-o"])
        .arg(&target)
        .output()
        .expect("run");
    assert_ok(&output, "--md -o");
    let text = std::fs::read_to_string(&target).expect("read markdown");
    assert!(text.contains("```r"));
}

#[test]
fn the_outline_mode_is_accepted() {
    let scratch = Scratch::new("outline-flag");
    let source = scratch.write("note.Rmd", "# One\n\ntext\n");
    for mode in ["popup", "pinned", "off"] {
        let output = rmdt()
            .arg(&source)
            .args(["--html", "--outline", mode])
            .output()
            .expect("run");
        assert_ok(&output, mode);
    }
    let bad = rmdt()
        .arg(&source)
        .args(["--outline", "sideways"])
        .output()
        .expect("run");
    assert!(!bad.status.success());
}

#[test]
fn an_unknown_theme_is_rejected() {
    let scratch = Scratch::new("theme");
    let source = scratch.write("note.Rmd", "hi\n");
    let output = rmdt()
        .arg(&source)
        .args(["--theme", "nope"])
        .output()
        .expect("run");
    assert!(!output.status.success());
}
