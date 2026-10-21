# rmdt

Read R Markdown in the terminal, or write a page for a browser, without running
any code.

Knitting a vignette runs its code: installing packages, reading files, possibly
deleting things. `rmdt` skips all of that. It shows you the document, says which
chunks it did not run, and puts a caption and a placeholder where a figure would
have been drawn.

## Never runs your code

There is no execution path for document content. The only processes this program
ever starts are your `$EDITOR` (on the `e` key, so you can edit the line you are
reading) and the browser opener for `--html` (`open` on macOS, `xdg-open`
elsewhere). Both are guards in the test suite: the front end contains no
`Command::new` at all, and the only two call sites in the rest of the crate take
a literal or `$EDITOR`.

## Install

```sh
cargo build --release          # 2.3 MB binary, about 25 seconds cold
ln -sf "$PWD/target/release/rmdt" ~/.local/bin/rmdt
```

## Use

```sh
rmdt ~/r_package/bears                # build that package's site, then read it here
rmdt vignettes/intro.Rmd              # read one document
rmdt --watch vignettes/intro.Rmd      # read it while you edit it
rmdt ~/r_package/bears --html         # only build the pages (add --open for a browser)
rmdt vignettes/intro.Rmd --md         # filtered Markdown on stdout
rmdt vignettes/intro.Rmd --md | mdt   # hand it to another terminal viewer
```

A bare `rmdt` walks up from the current directory to the nearest package (a
`DESCRIPTION` file or a `vignettes/` directory). A directory argument does the
same for that directory.

## A package becomes a site

Pointing `rmdt` at a package builds a small documentation site and then reads it
in the terminal:

```sh
cd ~/r_package/bears
rmdt                      # build the site, then browse it here
rmdt --html               # build it and stop; add --open for a browser
rmdt --html -d /tmp/site  # build it somewhere else
```

The site goes to `<package>/.rmdview/` and the source tree is left alone. It
carries:

- an index with the package name, its `Title` and `Description` from
  `DESCRIPTION`, and its documents grouped into `Vignettes` and
  `Other documents`
- a navigation bar on every page, linking the package home page and every
  sibling document, so the pages read as one site rather than loose files
- each page's own floating contents and section numbering, from its front matter

Deep pages link with `../` as needed, and the test suite checks that every
relative link in a built site resolves to a file that exists.

The terminal view of a package lists the same documents in the same sections, so
`Enter` on the first entry opens the first vignette. `b` returns to the list.

The terminal view needs a terminal on both stdin and stdout. When it does not
have one, it says so and names the pages it wrote, rather than failing with a
terminal error:

```
$ rmdt ~/r_package/bears | cat
site: 3 page(s) -> /home/me/r_package/bears/.rmdview/index.html
rmdt: the terminal view needs a terminal on stdin and stdout. The pages are
already written: .../.rmdview/index.html. Use --html to write pages instead, or
run this in a terminal.
```

### Options

| Option | Effect |
| --- | --- |
| `--theme rmarkdown\|github\|plain` | the look, in both backends. `rmarkdown` follows Bootstrap 3 metrics, which is what `rmarkdown::render` produces |
| `--toc` / `--no-toc` | force the table of contents on or off |
| `--toc-depth N` | how deep the contents go |
| `--number-sections` / `--no-number-sections` | force section numbering |
| `--outline popup\|pinned\|off` | where the outline starts |
| `--watch` | read the file again whenever it changes |
| `--no-banner` | drop the "code not executed" strip from the page |
| `--no-math` | leave MathJax out of the page |
| `-o PATH`, `-d DIR` | where to write, for a file or for a directory |
| `--open` | open the written page in a browser. Writing a page does not open one by itself |
| `--no-open` | state that nothing should be opened, which is the default |

Front matter is read first, so a document that already declares `toc: true`,
`toc_depth: 3` and `number_sections: yes` under `output:` gets those without any
flags. Flags win over front matter.

### Keys

| Key | Action |
| --- | --- |
| `j` `k`, arrows | one row |
| `d` `u`, space, page keys | one page |
| `h` `l` | half a page |
| `g` `G` | top, bottom |
| `[` `]` | previous, next section |
| `o`, tab | outline popup, `Enter` jumps |
| `t` | pin or unpin the outline column |
| `/` | search, `n` `N` for the next and previous hit |
| `e` | open the current source line in `$EDITOR` |
| `r` | read the file again |
| `b` | back to the file list |
| `?` | keys |
| `q`, `Ctrl-C` | quit |

A modal swallows the first key that closes it, so quitting from the outline
popup or the key list takes `Esc` then `q`.

## What it does with chunks

| Chunk option | What you see |
| --- | --- |
| `echo=FALSE` | a one line note: `chunk name: code hidden (echo=FALSE)` |
| `include=FALSE` | nothing |
| `eval=FALSE` | the source, with a note that it was not evaluated |
| `child="other.Rmd"` | that document, inlined |
| `fig.cap="..."` | `Figure N: ...` under a placeholder where the figure would be |
| `#| echo: false` | read as well, for Quarto style documents |
| ```` ```{=html} ```` | passed through as raw HTML |

Inline R (`` `r nrow(df)` ``) is shown as an expression, never evaluated. In the
HTML output it keeps a `rinline` class so you can see which values the code would
have produced; the terminal shows it as a plain code span.

Markdown is parsed by `pulldown-cmark`, so CommonMark behaviour is the real
thing: lazy list continuation, tables, footnotes and alerts all work.

## Not handled

Sweave `.Rnw` vignettes. They are LaTeX with R chunks, not Markdown, so they need
a LaTeX front end this tool does not have. Asking for a package whose vignettes
are all `.Rnw` says so instead of coming up empty:

```
rmdt: no .Rmd files under PopGenome, but 3 Sweave .Rnw file(s) are there. ...
```

Inline R is shown but never evaluated, so a computed value such as a date appears
as the expression that would have produced it.

Source positions belong to the file that was opened. Pressing `e` on a chunk that
came in through `child="other.Rmd"` opens this document at the child's line
number, which is not where that chunk lives. Everything else about a child
document, including its text and its place in the outline, is correct.

## How it is put together

```
.Rmd  ->  front end  ->  blocks with source lines  ->  terminal rows  ->  ratatui
                                                  ->  HTML page
```

The front end (`src/rmd.rs`) turns R Markdown into a list of blocks. Prose stays
as CommonMark so a markdown engine does the layout; headings, code, notes and
figures become their own blocks so both backends can render them their way and
the outline can point at them.

`src/render.rs` turns blocks into terminal rows one block at a time and keeps the
source line each row came from. That map is what the outline, the section jumps,
the position preserving reloads and the `e` key are built on. Since rows are
pre-wrapped, one row is always one screen row, and the scroll arithmetic stays
simple.

Rendering is done by [`tui-markdown`](https://github.com/joshka/tui-markdown)
(`pulldown-cmark` plus `syntect`), with a `StyleSheet` implementation as the
terminal theme. The HTML backend writes its own markup so it can put the section
numbers, chunk headers, notes and figure captions where the earlier Python tool
put them.

## Tests

```sh
cargo test                    # 89 unit tests, 15 command line tests
cargo clippy --all-targets    # clean
```

The unit tests cover the chunk option rules, the front matter, the markdown
output, the HTML output, row wrapping and the source map, and the terminal
screens through `ratatui`'s `TestBackend`. Malformed documents are in there too:
unterminated fences and front matter, a child document that includes itself or
forms a cycle, empty and seven hash headings, CRLF input, and a file with no
valid UTF-8, which is reported rather than guessed at.

The command line tests run the built binary: writing a page beside a source file
without opening a browser, Markdown to stdout and to a file, redirecting output
with `-d`, a package building a site whose every link resolves without touching
the sources, being run without a terminal (which has to explain itself rather
than panic), forcing the contents and section numbering either way, and the
argument combinations that should fail. Browser opening is checked with a fake `open` on `PATH`, so the test
asserts the real behaviour without launching anything.

Rendering was compared against the Python tool this replaces on four real
vignettes and one fixture covering every feature: heading counts by level, chunk
blocks, notes, section numbers, contents links, tables, figure captions, math and
the `rinline` class all match exactly.

## Differences from the Python tool

- The default command reads the terminal. `--html` is the browser path, and it
  writes the page without opening a browser unless you pass `--open`.
- HTML bodies are not byte identical to the Python output: line breaks inside
  paragraphs and list items follow the source, and table attributes are spaced
  differently. The rendered result is the same.
- The three stylesheets and the chunk header markup are carried over unchanged.

## License

MIT OR Apache-2.0.
