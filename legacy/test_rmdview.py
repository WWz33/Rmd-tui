#!/usr/bin/env python3
"""Checks for rmdview. Run: python3 test_rmdview.py"""

import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
SCRIPT = HERE / "rmdview.py"
sys.path.insert(0, str(HERE))

import rmdview  # noqa: E402


def render(text, name="doc.Rmd", as_markdown=False, scratch=None, **options):
    scratch = scratch or Path(tempfile.mkdtemp())
    path = scratch / name
    path.write_text(text, encoding="utf-8")
    if as_markdown:
        return rmdview.build_markdown(path, text)
    return rmdview.build_html(path, text, True, **options)


class ChunkOptions(unittest.TestCase):
    def test_echo_false_hides_code(self):
        out = render("```{r private, echo=FALSE}\nsecret_call()\n```\n")
        self.assertNotIn("secret_call", out)
        self.assertIn("code hidden (echo=FALSE)", out)

    def test_include_false_drops_chunk_entirely(self):
        out = render("```{sql, include=FALSE}\nSELECT 1;\n```\n")
        self.assertNotIn("SELECT 1", out)
        self.assertNotIn('<div class="chunk-note">', out)

    def test_eval_false_keeps_source(self):
        out = render("```{r slow, eval=FALSE}\ntrain_model()\n```\n")
        self.assertIn("train_model()", out)
        self.assertIn("not evaluated (eval=FALSE)", out)

    def test_default_chunk_is_shown_with_label(self):
        out = render("```{r my-label}\nx <- 1\n```\n")
        self.assertIn("x &lt;- 1", out)
        self.assertIn("my-label", out)
        self.assertIn('class="chunk lang-r"', out)

    def test_quarto_hash_pipe_options(self):
        out = render("```{r}\n#| echo: false\nhidden_here\n```\n")
        self.assertNotIn("hidden_here", out)
        self.assertIn("code hidden", out)

    def test_engine_without_label(self):
        out = render("```{python}\nprint(1)\n```\n")
        self.assertIn('class="chunk lang-python"', out)

    def test_child_document_inlined(self):
        scratch = Path(tempfile.mkdtemp())
        (scratch / "setup.Rmd").write_text("```{r setup}\nlibrary(dplyr)\n```\n", encoding="utf-8")
        out = render('```{r child="setup.Rmd"}\n```\n', scratch=scratch)
        self.assertIn("library(dplyr)", out)
        self.assertNotIn("child=", out)

    def test_pandoc_raw_block_passes_through(self):
        out = render("```{=html}\n<div class='raw'>hi</div>\n```\n")
        self.assertIn("<div class='raw'>hi</div>", out)

    def test_four_backtick_fence_wrapping_three(self):
        out = render("````\n```{r}\ninner\n```\n````\n")
        self.assertIn("```{r}", out)
        self.assertIn("inner", out)


class NoExecution(unittest.TestCase):
    def test_source_has_no_execution_primitive(self):
        source = SCRIPT.read_text(encoding="utf-8")
        for pattern in (r"\bimport\s+subprocess", r"\bsubprocess\.", r"\beval\s*\(", r"\bexec\s*\(",
                        r"\bos\.system", r"\bimport\s+os\b"):
            self.assertIsNone(re.search(pattern, source), "found %s" % pattern)

    def test_inline_r_not_evaluated(self):
        out = render("Value: `r 1 + 1` here.\n")
        self.assertIn("1 + 1", out)
        self.assertIn('class="rinline"', out)
        self.assertNotIn(">2<", out)


class Blocks(unittest.TestCase):
    def test_front_matter_title_and_heading_anchor(self):
        out = render('---\ntitle: "Report"\nauthor: "me"\n---\n\n## Section\n')
        self.assertIn("<title>Report</title>", out)
        self.assertIn('<div class="title">Report</div>', out)
        self.assertIn('<h2 id="section">Section</h2>', out)

    def test_table_alignment_and_cells(self):
        out = render("| a | b |\n|:--|--:|\n| 1 | 2 |\n")
        self.assertIn('<th style="text-align:left">a</th>', out)
        self.assertIn('<th style="text-align:right">b</th>', out)
        self.assertIn("<td style=\"text-align:left\">1</td>", out)

    def test_nested_lists(self):
        out = render("- top\n  - sub\n- top2\n")
        self.assertEqual(out.count("<ul>"), 2)
        self.assertEqual(out.count("<li>"), 3)

    def test_lazy_continuation_joins_wrapped_item(self):
        out = render("- a bullet wrapped over\n"
                     "two source lines\n"
                     "- second bullet\n")
        self.assertEqual(out.count("<ul>"), 1)
        self.assertEqual(out.count("<li>"), 2)
        self.assertIn("a bullet wrapped over two source lines", out)
        self.assertIn("second bullet", out)

    def test_indented_continuation_joins_wrapped_item(self):
        out = render("- wrapped\n  continuation line\n- next\n")
        self.assertEqual(out.count("<ul>"), 1)
        self.assertIn("wrapped continuation line", out)

    def test_heading_after_list_is_not_absorbed(self):
        out = render("- item\n\n## Next\n")
        self.assertIn("<li>item\n</li>", out)
        self.assertIn('<h2 id="next">Next</h2>', out)

    def test_math_is_preserved_for_mathjax(self):
        out = render("Inline $a<b$ and block:\n\n$$\n\\hat\\beta\n$$\n")
        self.assertIn("\\(a&lt;b\\)", out)
        self.assertIn("$$\\hat\\beta$$", out)
        self.assertIn("mathjax", out)

    def test_no_math_flag_drops_mathjax(self):
        out = rmdview.build_html(Path("/tmp/x.Rmd"), "text\n", False)
        self.assertNotIn("mathjax", out)

    def test_inline_html_and_autolink(self):
        out = render("Press <kbd>Ctrl</kbd> see <https://example.com/x>.\n")
        self.assertIn("<kbd>Ctrl</kbd>", out)
        self.assertIn('<a href="https://example.com/x">https://example.com/x</a>', out)

    def test_link_html_is_not_mangled_by_emphasis(self):
        out = render("See [doc](https://example.com/a_b_c_d).\n")
        self.assertIn('href="https://example.com/a_b_c_d"', out)

    def test_badge_image_nested_in_link(self):
        out = render("[![coverage](https://img.shields.io/badge.svg)](https://codecov.io/x)\n")
        self.assertIn('<a href="https://codecov.io/x">', out)
        self.assertIn('<img src="https://img.shields.io/badge.svg" alt="coverage">', out)
        self.assertNotIn("\x00", out)

    def test_html_comments_are_dropped(self):
        out = render("<!-- badges: start -->\n\ntext\n")
        self.assertNotIn("badges: start", out)
        self.assertIn("<p>text</p>", out)

    def test_image_keeps_caption_and_alt(self):
        out = render("![a caption](figure/plot.png \"optional title\")\n")
        self.assertIn('<img src="figure/plot.png" alt="a caption" title="optional title">', out)
        self.assertIn("missing-figure", out)          # placeholder for figures that never ran

    def test_heading_may_contain_inline_html(self):
        out = render("# bears <img src='logo.png' align=\"right\" height=\"139\" />\n")
        self.assertIn("<img src='logo.png' align=\"right\" height=\"139\" />", out)

    def test_emphasis_and_code(self):
        out = render("**bold** *it* `code` ~~gone~~\n")
        for expected in ("<strong>bold</strong>", "<em>it</em>", "<code>code</code>", "<del>gone</del>"):
            self.assertIn(expected, out)

    def test_code_span_containing_html_is_escaped(self):
        out = render("Use `<div>` tags.\n")
        self.assertIn("<code>&lt;div&gt;</code>", out)

    def test_crlf_input(self):
        out = render("## Head\r\n\r\ntext\r\n")
        self.assertIn('<h2 id="head">Head</h2>', out)


class MarkdownMode(unittest.TestCase):
    def test_markdown_keeps_chunks_and_filters_hidden(self):
        out = render("```{r keep}\nx <- 1\n```\n\n```{r gone, echo=FALSE}\ny <- 2\n```\n",
                     as_markdown=True)
        self.assertIn("```r\nx <- 1\n```", out)
        self.assertNotIn("y <- 2", out)
        self.assertNotIn("echo=FALSE", out)

    def test_markdown_inline_r_unwrapped(self):
        out = render("Count is `r n`.\n", as_markdown=True)
        self.assertIn("Count is `n`.", out)


class ThemesAndStructure(unittest.TestCase):
    def test_themes_have_distinct_metrics(self):
        self.assertIn("Helvetica Neue", rmdview.stylesheet("rmarkdown"))
        self.assertIn("--page-width:940px", rmdview.stylesheet("rmarkdown"))
        self.assertIn("--page-width:820px", rmdview.stylesheet("github"))
        self.assertIn("--page-width:760px", rmdview.stylesheet("plain"))

    def test_unknown_theme_falls_back_to_rmarkdown(self):
        self.assertEqual(rmdview.stylesheet("nope"), rmdview.stylesheet("rmarkdown"))

    def test_front_matter_enables_toc_and_numbering(self):
        out = render('---\ntoc: true\nnumber_sections: yes\n---\n\n# One\n\ntext\n\n## Sub\n\nmore\n')
        self.assertIn('<nav class="toc">', out)
        self.assertIn('<span class="secnum">1</span>One', out)
        self.assertIn('<span class="secnum">1.1</span>Sub', out)
        self.assertIn('href="#one"', out)
        self.assertIn('class="toc-h2"', out)

    def test_numbering_sequence_restarts_deeper_levels(self):
        body = rmdview.number_headings(
            '<h1 id="a">A</h1><h2 id="b">B</h2><h2 id="c">C</h2><h1 id="d">D</h1>')
        for expected in ('<span class="secnum">1</span>A', '<span class="secnum">1.1</span>B',
                         '<span class="secnum">1.2</span>C', '<span class="secnum">2</span>D'):
            self.assertIn(expected, body)

    def test_flag_overrides_front_matter(self):
        text = '---\ntoc: true\n---\n\n# One\n\n## Two\n'
        self.assertNotIn('<nav class="toc">', render(text, toc=False))
        self.assertIn('<nav class="toc">', render("# One\n\n## Two\n", toc=True))

    def test_toc_depth_limits_levels(self):
        text = "# One\n\n## Two\n\n### Three\n"
        out = render(text, toc=True, toc_depth=2)
        self.assertIn('href="#two"', out)
        self.assertNotIn('href="#three"', out)

    def test_single_heading_gets_no_toc(self):
        self.assertNotIn('<nav class="toc">', render("# Only one\n\ntext\n", toc=True))

    def test_fig_cap_becomes_numbered_caption(self):
        out = render('```{r a, fig.cap="First plot"}\nplot(1)\n```\n\n'
                     '```{r b, fig.cap="Second plot"}\nplot(2)\n```\n')
        self.assertIn("<figcaption>Figure 1: First plot</figcaption>", out)
        self.assertIn("<figcaption>Figure 2: Second plot</figcaption>", out)
        self.assertEqual(out.count('<div class="missing-figure">'), 2)

    def test_comment_inside_chunk_is_not_a_heading(self):
        out = render("```{r}\n# not a heading\nx <- 1\n```\n")
        self.assertNotIn("<h1", out)
        self.assertIn("# not a heading", out)

    def test_banner_can_be_dropped(self):
        self.assertIn("code not executed", render("text\n"))
        self.assertNotIn("code not executed", render("text\n", banner=False))


class PackageMode(unittest.TestCase):
    def _package(self):
        root = Path(tempfile.mkdtemp()) / "mypkg"
        (root / "vignettes").mkdir(parents=True)
        (root / "DESCRIPTION").write_text("Package: mypkg\nVersion: 0.1\n", encoding="utf-8")
        (root / "vignettes" / "intro.Rmd").write_text(
            '---\ntitle: "Intro vignette"\ntoc: true\n---\n\n# Start\n\n## Deeper\n\ntext\n', encoding="utf-8")
        (root / "vignettes" / "extra.Rmd").write_text('---\ntitle: "Extra"\n---\n\nhi\n', encoding="utf-8")
        (root / "README.Rmd").write_text('---\ntitle: "Readme"\n---\n\nreadme text\n', encoding="utf-8")
        return root

    def test_directory_input_builds_index_and_documents(self):
        root = self._package()
        result = subprocess.run([sys.executable, str(SCRIPT), str(root)], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        outdir = root / ".rmdview"
        self.assertTrue((outdir / "index.html").is_file())
        self.assertTrue((outdir / "vignettes" / "intro.html").is_file())
        self.assertTrue((outdir / "vignettes" / "extra.html").is_file())
        self.assertTrue((outdir / "README.html").is_file())
        index = (outdir / "index.html").read_text(encoding="utf-8")
        self.assertIn("mypkg", index)
        self.assertIn("Vignettes", index)
        self.assertIn('href="vignettes/intro.html"', index)
        self.assertIn("Intro vignette", index)
        self.assertIn('href="README.html"', index)

    def test_project_keeps_source_tree_clean(self):
        root = self._package()
        before = {path.name for path in root.iterdir()}
        subprocess.run([sys.executable, str(SCRIPT), str(root)], capture_output=True, text=True)
        after = {path.name for path in root.iterdir()} - {".rmdview"}
        self.assertEqual(before, after)

    def test_outdir_is_honoured(self):
        root = self._package()
        target = Path(tempfile.mkdtemp()) / "site"
        subprocess.run([sys.executable, str(SCRIPT), str(root), "-d", str(target)],
                       capture_output=True, text=True)
        self.assertTrue((target / "index.html").is_file())
        self.assertFalse((root / ".rmdview").exists())

    def test_directory_rejects_stdout_and_md(self):
        root = self._package()
        for flag in ("--stdout", "--md"):
            result = subprocess.run([sys.executable, str(SCRIPT), str(root), flag],
                                    capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0, flag)

    def test_single_file_outdir(self):
        scratch = Path(tempfile.mkdtemp())
        source = scratch / "note.Rmd"
        source.write_text("hi\n", encoding="utf-8")
        target = scratch / "build"
        subprocess.run([sys.executable, str(SCRIPT), str(source), "-d", str(target)], check=True)
        self.assertTrue((target / "note.html").is_file())


class Cli(unittest.TestCase):
    def test_cli_writes_file_beside_source(self):
        scratch = Path(tempfile.mkdtemp())
        source = scratch / "note.Rmd"
        source.write_text("---\ntitle: T\n---\n\nhi\n", encoding="utf-8")
        result = subprocess.run([sys.executable, str(SCRIPT), str(source)],
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        target = scratch / "note.html"
        self.assertTrue(target.is_file())
        self.assertIn("<title>T</title>", target.read_text(encoding="utf-8"))

    def test_cli_md_mode(self):
        scratch = Path(tempfile.mkdtemp())
        source = scratch / "note.Rmd"
        source.write_text("hi\n", encoding="utf-8")
        subprocess.run([sys.executable, str(SCRIPT), str(source), "--md"], check=True)
        self.assertTrue((scratch / "note.md").is_file())

    def test_cli_rejects_output_with_multiple_inputs(self):
        scratch = Path(tempfile.mkdtemp())
        for name in ("a.Rmd", "b.Rmd"):
            (scratch / name).write_text("x\n", encoding="utf-8")
        result = subprocess.run([sys.executable, str(SCRIPT), str(scratch / "a.Rmd"),
                                 str(scratch / "b.Rmd"), "-o", "out.html"],
                                capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)


if __name__ == "__main__":
    unittest.main(verbosity=2)