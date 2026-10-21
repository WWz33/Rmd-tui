#!/usr/bin/env python3
"""rmdview - render R Markdown without running any code.

Knits nothing. Reads .Rmd source and produces standalone HTML (default) or plain
Markdown (--md, which GitHub previews natively). Chunk options that affect the
source listing (echo, include, eval, child) are honoured; output-producing
options are irrelevant because no chunk is ever evaluated.

Usage:
    python3 rmdview.py report.Rmd              # -> report.html
    python3 rmdview.py report.Rmd --open
    python3 rmdview.py report.Rmd --md -o README.md
    python3 rmdview.py *.Rmd --no-math
"""

import argparse
import functools
import html
import re
import sys
import threading
import time
import webbrowser
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

# ---------------------------------------------------------------- chunk options


def split_top_level(text):
    """Split on commas that are not inside quotes or parentheses."""
    parts, buf, quote, depth = [], [], None, 0
    for ch in text:
        if quote:
            buf.append(ch)
            if ch == quote:
                quote = None
        elif ch in "\"'":
            quote = ch
            buf.append(ch)
        elif ch in "([":
            depth += 1
            buf.append(ch)
        elif ch in ")]":
            depth -= 1
            buf.append(ch)
        elif ch == "," and depth == 0:
            parts.append("".join(buf))
            buf = []
        else:
            buf.append(ch)
    parts.append("".join(buf))
    return [p.strip() for p in parts if p.strip()]


def clean_value(value):
    """Unquote only. Booleans stay strings so that ``title: T`` survives and
    ``echo=FALSE`` is decided in one place, option_off()."""
    value = value.strip()
    if len(value) >= 2 and value[0] == value[-1] and value[0] in "\"'":
        value = value[1:-1]
    return value


def parse_chunk_info(info):
    """`` ```{r label, echo=FALSE} `` -> ('r', 'label', {'echo': False}).

    knitr allows the header with or without commas around options, so
    ``{r child="x.Rmd"}`` and ``{r, child="x.Rmd"}`` both work. Anything
    without an ``=`` that is not the engine name is the chunk label.
    """
    info = info.strip()
    if not (info.startswith("{") and info.endswith("}")):
        return (info.split()[0] if info else ""), "", {}

    inner = info[1:-1].strip()
    engine, label, opts = "", "", {}
    if not inner:
        return engine, label, opts
    if inner.startswith("="):                       # pandoc raw block, {=html}
        return inner, label, opts
    match = re.match(r"^([\w.+-]+)\s*(.*)$", inner)
    engine, rest = match.group(1), match.group(2)
    for token in split_top_level(rest):
        if "=" in token:
            key, value = token.split("=", 1)
            opts[key.strip()] = clean_value(value)
        else:
            label = token.strip()
    return engine, label, opts


def fence_end(marker):
    """Closing fence must repeat the opening marker at least as many times."""
    return re.compile(r"^\s*%s{%d,}\s*$" % (re.escape(marker[0]), len(marker)))


def pull_hash_pipe_options(body):
    """Quarto-style ``#| echo: false`` lines at the top of a chunk body."""
    opts, rest = {}, []
    for line in body:
        match = re.match(r"^\s*#\|\s*([\w.]+)\s*:\s*(.*?)\s*$", line)
        if match and not rest:
            opts[match.group(1)] = clean_value(match.group(2))
        else:
            rest.append(line)
    return opts, rest


def option_off(value):
    """knitr splits these three ways: FALSE disables, TRUE enables, and
    results='hide' / 'asis' are strings."""
    if value is False:
        return True
    if isinstance(value, str):
        return value.strip().lower() in ("false", "f", "no", "hide")
    return False


def chunk_children(opts, base_dir, depth):
    """Inline ``child="other.Rmd"`` chunk inclusions."""
    child = opts.get("child")
    if not isinstance(child, str) or depth > 8:
        return ""
    path = (base_dir / child) if base_dir else Path(child)
    if not path.is_file():
        return f'<p class="note">missing child document: {html.escape(str(path))}</p>\n'
    return render_blocks(read_front_matter(path.read_text(encoding="utf-8"))[1],
                         path.parent, depth + 1)


# ---------------------------------------------------------------- inline markup


class Stash:
    """Hold generated HTML aside behind inert tokens so later regexes skip it.

    One stash is shared by nested inline() calls, so a link label containing an
    image resolves both levels. restore() loops because a token can reappear
    inside an already-restored item.
    """

    def __init__(self):
        self.items = []

    def add(self, markup):
        self.items.append(markup)
        return "\x00%d\x00" % (len(self.items) - 1)

    def restore(self, text):
        for _ in range(16):
            restored = re.sub(r"\x00(\d+)\x00", lambda m: self.items[int(m.group(1))], text)
            if restored == text:
                break
            text = restored
        return text


CODE_SPAN = re.compile(r"(?<!`)(`+)(?!`)(.+?)(?<!`)\1(?!`)", re.S)
MATH_INLINE = re.compile(r"(?<!\\)\$(?!\$)((?:\\.|[^$\\])+?)(?<!\\)\$", re.S)
IMAGE = re.compile(r"!\[([^\]]*)\]\(\s*([^)\s]+)(?:\s+\"([^\"]*)\")?\s*\)")
LINK = re.compile(r"(?<!!)\[([^\]]*)\]\(\s*([^)\s]+)(?:\s+\"([^\"]*)\")?\s*\)")
BOLD = re.compile(r"\*\*(?=\S)(.+?)(?<=\S)\*\*|__(?=\S)(.+?)(?<=\S)__", re.S)
ITALIC = re.compile(r"(?<![*\w])\*(?=\S)([^*]+?)(?<=\S)\*(?!\*)|(?<![_\w])_(?=\S)([^_]+?)(?<=\S)_(?!_)", re.S)
STRIKE = re.compile(r"~~(?=\S)(.+?)(?<=\S)~~", re.S)
AUTOLINK = re.compile(r"<(https?://[^>\s]+)>")
HTML_INLINE = re.compile(r"</?[a-zA-Z][\w-]*(?:\s[^<>]*)?/?>")
HTML_COMMENT = re.compile(r"<!--.*?-->", re.S)


def inline(text, math=True, stash=None):
    """Render one run of inline markdown. Input is raw source text."""
    if stash is None:
        stash = Stash()

    def code_span(match):
        body = match.group(2)
        stripped = body.strip()
        if stripped.startswith("r ") or stripped == "r":
            expr = stripped[1:].strip()
            return stash.add(
                '<code class="rinline" title="R expression, not evaluated">%s</code>'
                % html.escape(expr))
        return stash.add("<code>%s</code>" % html.escape(stripped))

    text = CODE_SPAN.sub(code_span, text)

    def math_span(match):
        return stash.add('<span class="math">\\(%s\\)</span>' % html.escape(match.group(1)))

    if math:
        text = MATH_INLINE.sub(math_span, text)

    text = AUTOLINK.sub(
        lambda m: stash.add('<a href="%s">%s</a>'
                            % (html.escape(m.group(1), quote=True), html.escape(m.group(1)))), text)
    text = HTML_COMMENT.sub(lambda m: stash.add(""), text)
    text = HTML_INLINE.sub(lambda m: stash.add(m.group(0)), text)

    def image(match):
        alt, src, title = match.group(1), match.group(2), match.group(3)
        attr = ' title="%s"' % html.escape(title, quote=True) if title else ""
        return stash.add('<img src="%s" alt="%s"%s>'
                         % (html.escape(src, quote=True), html.escape(alt, quote=True), attr))

    def link(match):
        label, href, title = match.group(1), match.group(2), match.group(3)
        attr = ' title="%s"' % html.escape(title, quote=True) if title else ""
        return stash.add('<a href="%s"%s>%s</a>'
                         % (html.escape(href, quote=True), attr, inline(label, math, stash)))

    text = IMAGE.sub(image, text)
    text = LINK.sub(link, text)

    text = html.escape(text, quote=False)
    text = BOLD.sub(lambda m: "<strong>%s</strong>" % (m.group(1) or m.group(2)), text)
    text = ITALIC.sub(lambda m: "<em>%s</em>" % (m.group(1) or m.group(2)), text)
    text = STRIKE.sub(r"<del>\1</del>", text)
    return stash.restore(text)


# ---------------------------------------------------------------- block parser


HTML_TAGS = {
    "a", "abbr", "b", "blockquote", "br", "center", "code", "col", "colgroup", "dd",
    "details", "div", "dl", "dt", "em", "figcaption", "figure", "font", "h1", "h2",
    "h3", "h4", "h5", "h6", "hr", "i", "iframe", "img", "input", "kbd", "li", "mark",
    "ol", "p", "pre", "q", "s", "samp", "script", "section", "small", "span", "strong",
    "style", "sub", "summary", "sup", "table", "tbody", "td", "tfoot", "th", "thead",
    "tr", "u", "ul", "var", "video",
}

FENCE = re.compile(r"^(\s*)(`{3,}|~{3,})\s*(.*?)\s*$")
HEADING = re.compile(r"^(#{1,6})\s+(.*?)\s*#*\s*$")
HR = re.compile(r"^\s{0,3}((\*\s*){3,}|(-\s*){3,}|(_\s*){3,})$")
BULLET = re.compile(r"^(\s*)([-*+])\s+(.*)$")
ORDERED = re.compile(r"^(\s*)(\d+)[.)]\s+(.*)$")
TABLE_ROW = re.compile(r"^\s*\|.*\|\s*$")


def slug(text):
    return re.sub(r"[^a-z0-9]+", "-", re.sub(r"<[^>]+>", "", text).lower()).strip("-") or "section"


def read_front_matter(text):
    """Strip a leading YAML block, returning (meta, body lines).

    rmarkdown nests the options that matter under `output:`, as in
      output:
        html_document:
          toc: true
          number_sections: yes
    so indented keys are read too. A top-level key wins over an indented one.
    """
    lines = text.replace("\r\n", "\n").split("\n")
    meta = {}
    if lines and lines[0].strip() in ("---", "----"):
        for index, line in enumerate(lines[1:], start=1):
            if line.strip() in ("---", "..."):
                return meta, lines[index + 1:]
            top = re.match(r"^([\w.-]+)\s*:\s*(.*)$", line)
            if top and top.group(2):
                meta[top.group(1)] = clean_value(top.group(2))
                continue
            nested = re.match(r"^\s+([\w.-]+)\s*:\s*(\S.*)$", line)
            if nested:
                meta.setdefault(nested.group(1), clean_value(nested.group(2)))
    return meta, lines


def starts_new_block(line):
    return bool(FENCE.match(line) or HEADING.match(line) or HR.match(line)
                or TABLE_ROW.match(line) or line.lstrip().startswith((">", "$$")))


def render_blocks(lines, base_dir=None, depth=0, source=None):
    out = []
    index = 0
    while index < len(lines):
        line = lines[index]

        if not line.strip():
            index += 1
            continue

        fence = FENCE.match(line)
        if fence:
            marker = fence.group(2)
            info = fence.group(3)
            closed = fence_end(marker)
            body, index = [], index + 1
            while index < len(lines) and not closed.match(lines[index]):
                body.append(lines[index])
                index += 1
            index += 1
            out.append(render_fence(info, body, base_dir, depth, source))
            continue

        heading = HEADING.match(line)
        if heading:
            level = len(heading.group(1))
            out.append('<h%d id="%s">%s</h%d>' % (level, slug(heading.group(2)),
                                                 inline(heading.group(2), math_on), level))
            index += 1
            continue

        if HR.match(line):
            out.append("<hr>")
            index += 1
            continue

        if line.lstrip().startswith("$$"):
            body, index = [], index
            first = line.lstrip()[2:]
            if first.rstrip().endswith("$$") and len(first.rstrip()) > 2:
                body = [first.rstrip()[:-2]]
                index += 1
            else:
                if first.strip():
                    body.append(first)
                index += 1
                while index < len(lines) and "$$" not in lines[index]:
                    body.append(lines[index])
                    index += 1
                if index < len(lines):
                    tail = lines[index].split("$$")[0]
                    if tail.strip():
                        body.append(tail)
                    index += 1
            out.append('<div class="math-display">$$%s$$</div>' % html.escape("\n".join(body).strip()))
            continue

        if line.lstrip().startswith(">"):
            body, index = [], index
            while index < len(lines) and (lines[index].lstrip().startswith(">") or not lines[index].strip()):
                if not lines[index].strip():
                    if index + 1 < len(lines) and lines[index + 1].lstrip().startswith(">"):
                        body.append("")
                        index += 1
                        continue
                    break
                body.append(re.sub(r"^\s*>\s?", "", lines[index]))
                index += 1
            out.append("<blockquote>\n%s\n</blockquote>" % render_blocks(body, base_dir, depth, source))
            continue

        tag = re.match(r"^\s*<([a-zA-Z][\w-]*)[\s/>]", line)
        if tag and tag.group(1).lower() in HTML_TAGS:
            body, index = [], index
            while index < len(lines) and lines[index].strip():
                body.append(lines[index])
                index += 1
            out.append("\n".join(body))
            continue

        if TABLE_ROW.match(line) and index + 1 < len(lines) \
                and re.match(r"^\s*\|?[\s:|-]+\|[\s:|-]*$", lines[index + 1]):
            rows, index = [], index
            while index < len(lines) and TABLE_ROW.match(lines[index]):
                rows.append([c.strip() for c in lines[index].strip().strip("|").split("|")])
                index += 1
            out.append(render_table(rows))
            continue

        if BULLET.match(line) or ORDERED.match(line):
            body, index = [], index
            while index < len(lines):
                if BULLET.match(lines[index]) or ORDERED.match(lines[index]):
                    body.append(lines[index])
                    index += 1
                elif lines[index].strip() and body and not starts_new_block(lines[index]):
                    # Wrapped continuation, indented or lazy: join it to the item.
                    body[-1] += " " + lines[index].strip()
                    index += 1
                else:
                    break
            out.append(render_list(body, base_dir, depth, source))
            continue

        body, index = [], index
        while index < len(lines) and lines[index].strip() and not starts_new_block(lines[index]) \
                and not BULLET.match(lines[index]) and not ORDERED.match(lines[index]):
            body.append(lines[index])
            index += 1
        out.append("<p>%s</p>" % inline(" ".join(part.strip() for part in body), math_on))

    return "\n".join(out)


def render_fence(info, body, base_dir, depth, source):
    engine, label, opts = parse_chunk_info(info)
    hash_opts, body = pull_hash_pipe_options(body)
    opts = {**opts, **hash_opts}
    code = "\n".join(body).rstrip("\n")

    if "child" in opts:
        return chunk_children(opts, base_dir, depth)

    if "include" in opts and option_off(opts["include"]):
        return ""

    if engine in ("=html", "=latex"):
        return code

    note = ""
    hidden = bool(engine) and option_off(opts.get("echo", True))
    if hidden:
        note = '<div class="chunk-note">chunk %s: code hidden (echo=FALSE)</div>\n' % html.escape(label or "unnamed")
    elif engine and option_off(opts.get("eval", True)):
        note = '<div class="chunk-note">chunk %s: not evaluated (eval=FALSE), source only</div>\n' % html.escape(label or "unnamed")

    # A caption in fig.cap means the chunk was meant to draw something. The drawing
    # does not exist, so show the caption with a placeholder in place of the image.
    figure = ""
    caption = opts.get("fig.cap")
    if engine and isinstance(caption, str) and caption.strip():
        figure = ('<figure>\n<div class="missing-figure">figure not rendered (code was not run)</div>\n'
                  '<figcaption class="fig-cap">%s</figcaption>\n</figure>\n' % inline(caption, math_on))
    if hidden:
        return note + figure

    language = {"r": "r", "python": "python", "bash": "bash", "sh": "bash", "sql": "sql",
                "cpp": "cpp", "stan": "stan", "julia": "julia"}.get(engine, engine)
    classes = "chunk" + (" lang-%s" % language if language else "")
    header = ""
    if engine:
        header = '<div class="chunk-head"><span class="chunk-lang">%s</span>%s</div>' % (
            html.escape(engine), '<span class="chunk-label">%s</span>' % html.escape(label) if label else "")
    return '%s<div class="%s">%s<pre><code>%s</code></pre></div>\n%s' % (
        note, classes, header, html.escape(code), figure)


def render_table(rows):
    if len(rows) < 2:
        return ""
    aligns = []
    for cell in rows[1]:
        left, right = cell.startswith(":"), cell.endswith(":")
        aligns.append("center" if left and right else "right" if right else "left" if left else "")
    head, body = rows[0], rows[2:]
    parts = ['<table>\n<thead><tr>']
    for cell, align in zip(head, aligns + [""] * len(head)):
        parts.append('<th%s>%s</th>' % (' style="text-align:%s"' % align if align else "", inline(cell, math_on)))
    parts.append("</tr></thead>\n<tbody>")
    for row in body:
        parts.append("<tr>")
        for cell, align in zip(row, aligns + [""] * len(row)):
            parts.append('<td%s>%s</td>' % (' style="text-align:%s"' % align if align else "", inline(cell, math_on)))
        parts.append("</tr>")
    parts.append("</tbody></table>")
    return "\n".join(parts)


def render_list(lines, base_dir, depth, source):
    """Build a nested list from flat source lines using indentation."""
    indent_unit = None
    for line in lines:
        match = BULLET.match(line) or ORDERED.match(line)
        if match and len(match.group(1)) > 0:
            indent_unit = len(match.group(1)) if indent_unit is None else min(indent_unit, len(match.group(1)))
    indent_unit = indent_unit or 2

    def build(items, index, level_indent):
        """items: list of (indent, ordered, text)."""
        ordered = items[index][1]
        tag = "ol" if ordered else "ul"
        parts = ["<%s>" % tag]
        while index < len(items):
            indent, is_ordered, text = items[index]
            if indent < level_indent:
                break
            if indent > level_indent:
                nested, index = build(items, index, indent)
                parts.append(nested)
                continue
            parts.append('<li>%s' % inline(text, math_on))
            index += 1
            if index < len(items) and items[index][0] > level_indent:
                nested, index = build(items, index, items[index][0])
                parts.append(nested)
            parts.append("</li>")
        parts.append("</%s>" % tag)
        return "\n".join(parts), index

    items = []
    for line in lines:
        match = BULLET.match(line) or ORDERED.match(line)
        if not match:
            continue
        items.append((len(match.group(1)) // indent_unit, bool(ORDERED.match(line)), match.group(3)))
    return build(items, 0, 0)[0]


# ---------------------------------------------------------------- markdown output


def to_markdown(lines, base_dir=None, depth=0):
    """Emit plain Markdown with chunks filtered the same way, for GitHub preview."""
    out = []
    index = 0
    while index < len(lines):
        fence = FENCE.match(lines[index])
        if fence:
            marker, info = fence.group(2), fence.group(3)
            closed = fence_end(marker)
            body, index = [], index + 1
            while index < len(lines) and not closed.match(lines[index]):
                body.append(lines[index])
                index += 1
            index += 1
            engine, label, opts = parse_chunk_info(info)
            hash_opts, body = pull_hash_pipe_options(body)
            opts = {**opts, **hash_opts}
            if "child" in opts:
                child = opts["child"]
                path = (base_dir / child) if base_dir else Path(child)
                if path.is_file() and depth < 8:
                    out.append(to_markdown(read_front_matter(path.read_text(encoding="utf-8"))[1],
                                           path.parent, depth + 1))
                continue
            if "include" in opts and option_off(opts["include"]):
                continue
            if engine and option_off(opts.get("echo", True)):
                continue
            if engine in ("=html", "=latex"):
                out.extend(body)
                continue
            language = engine if engine and not engine.startswith("=") else ""
            out.append("```%s" % language)
            out.extend(re.sub(r"^(\s*)`r\b", r"\1R", line) for line in body)
            out.append("```")
            continue
        out.append(re.sub(r"`r\s+([^`]*)`", r"`\1`", lines[index]))
        index += 1
    return "\n".join(out)


# ---------------------------------------------------------------- html shell

# ---------------------------------------------------------------- themes

# Concrete values live in THEMES; this sheet only reads them. Sizes and borders
# that differ per theme go in that theme's "extra", appended after these rules.
THEME_BASE = """
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
.math-display { overflow-x:auto; text-align:center; padding:.4em 0; }
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
"""

THEMES = {
    # Bootstrap 3 metrics, the shape rmarkdown::render's html_document has.
    "rmarkdown": {
        "vars": {
            "font": '"Helvetica Neue",Helvetica,Arial,sans-serif',
            "mono": 'Menlo,Monaco,Consolas,"Courier New",monospace',
            "size": "14px", "lh": "1.42857143", "fg": "#333", "muted": "#777",
            "line": "#ddd", "bg": "#fff", "page-bg": "#fff", "code-bg": "#f5f5f5",
            "code-border": "#ccc", "accent": "#337ab7", "mark": "#f9f2f4",
            "page-width": "940px", "radius": "4px", "toc-width": "240px",
            "page-shadow": "none",
        },
        "extra": """
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
""",
    },
    # GitHub markdown look.
    "github": {
        "vars": {
            "font": '-apple-system,BlinkMacSystemFont,"Segoe UI","Helvetica Neue",Arial,sans-serif',
            "mono": 'ui-monospace,SFMono-Regular,"SF Mono",Menlo,Consolas,monospace',
            "size": "16px", "lh": "1.65", "fg": "#1f2328", "muted": "#656d76",
            "line": "#d8dee4", "bg": "#fff", "page-bg": "#f6f7f9", "code-bg": "#f6f8fa",
            "code-border": "#d8dee4", "accent": "#0969da", "mark": "rgba(175,184,193,.2)",
            "page-width": "820px", "radius": "8px", "toc-width": "250px",
            "page-shadow": "0 0 0 1px #d8dee4",
        },
        "extra": """
h1 { border-bottom:1px solid var(--line); padding-bottom:.3em; }
h2 { border-bottom:1px solid var(--line); padding-bottom:.25em; }
code.rinline, code { color:inherit; }
.doc-meta .title { font-size:2em; }
tr:nth-child(2n) td { background:#fafbfc; }
""",
    },
    # No chrome, for reading long prose.
    "plain": {
        "vars": {
            "font": '-apple-system,BlinkMacSystemFont,"Segoe UI","Helvetica Neue",Arial,sans-serif',
            "mono": 'ui-monospace,SFMono-Regular,"SF Mono",Menlo,Consolas,monospace',
            "size": "16px", "lh": "1.6", "fg": "#111", "muted": "#666",
            "line": "#e5e5e5", "bg": "#fff", "page-bg": "#fff", "code-bg": "#fafafa",
            "code-border": "#ececec", "accent": "#0a58ca", "mark": "#f1f1f1",
            "page-width": "760px", "radius": "6px", "toc-width": "220px",
            "page-shadow": "none",
        },
        "extra": """
h1 { font-size:1.6em; } h2 { font-size:1.3em; } h3 { font-size:1.12em; }
.chunk { border:0; border-left:3px solid var(--line); border-radius:0; }
.chunk-head { background:none; border-bottom:0; }
tr:nth-child(2n) td { background:#fafafa; }
""",
    },
}


def stylesheet(theme):
    spec = THEMES.get(theme) or THEMES["rmarkdown"]
    root = ":root {%s}" % " ".join("--%s:%s;" % (name, value) for name, value in spec["vars"].items())
    return "\n".join([root, THEME_BASE, spec["extra"]]).strip()


# Figures produced by chunk code do not exist, since nothing ran. Swap each broken
# image for a placeholder that says so instead of leaving a broken-image icon.
FIGURE_SCRIPT = """<script>
(function () {
  function placeholder(img) {
    var box = document.createElement('div');
    box.className = 'missing-figure';
    box.textContent = 'figure not rendered (code was not run): ' + (img.getAttribute('src') || '');
    img.replaceWith(box);
  }
  document.querySelectorAll('img').forEach(function (img) {
    if (img.complete && img.naturalWidth === 0) { placeholder(img); return; }
    img.addEventListener('error', function () { placeholder(img); });
  });
})();
</script>
"""

HTML_SHELL = """<!DOCTYPE html>
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
"""

# Highlights the section you are reading in the floating TOC.
TOC_SCRIPT = """<script>
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
"""

MATH_HEAD = """<script>
window.MathJax = { tex: { inlineMath: [['\\\\(', '\\\\)']], displayMath: [['$$', '$$']] },
  options: { skipHtmlTags: ['script', 'noscript', 'style', 'textarea', 'pre', 'code'] } };
</script>
<script defer src="https://cdn.jsdelivr.net/npm/mathjax@3/es5/tex-mml-chtml.js"></script>
"""


HEADING_RE = re.compile(r'<h([1-6]) id="([^"]*)">(.*?)</h\1>', re.S)
FIGCAP_RE = re.compile(r'<figcaption class="fig-cap">(.*?)</figcaption>', re.S)


def as_flag(value):
    """Front matter writes these as yes/true/TRUE/no."""
    if value is True or value is False:
        return value
    if isinstance(value, str):
        return value.strip().lower() in ("yes", "true", "t", "y")
    return bool(value)


def collect_headings(body):
    return [(int(m.group(1)), m.group(2), m.group(3)) for m in HEADING_RE.finditer(body)]


def number_headings(body):
    """Prefix each heading with its section number, pandoc --number-sections style."""
    counters = [0] * 6

    def label(match):
        level = int(match.group(1))
        counters[level - 1] += 1
        for deeper in range(level, 6):
            counters[deeper] = 0
        number = ".".join(str(count) for count in counters[:level])
        return '<h%d id="%s"><span class="secnum">%s</span>%s</h%d>' % (
            level, match.group(2), number, match.group(3), level)

    return HEADING_RE.sub(label, body)


def number_figures(body):
    """Captions from fig.cap become "Figure N: ..." with no image behind them."""
    counter = [0]

    def label(match):
        counter[0] += 1
        return "<figcaption>Figure %d: %s</figcaption>" % (counter[0], match.group(1))

    return FIGCAP_RE.sub(label, body)


def toc_html(headings, depth):
    entries = [item for item in headings if item[0] <= depth]
    if len(entries) < 2:
        return ""
    parts = ['<nav class="toc">', '<div class="toc-title">Contents</div>']
    for level, anchor, text in entries:
        parts.append('<a class="toc-h%d" href="#%s">%s</a>'
                     % (level, anchor, re.sub(r"<[^>]+>", "", text)))
    parts.append("</nav>")
    return "\n".join(parts)


def build_html(source_path, text, math_on=True, theme="rmarkdown", toc=None, toc_depth=None,
               number_sections=None, banner=True, live=False):
    meta, body_lines = read_front_matter(text)
    body = render_blocks(body_lines, source_path.parent, 0, source_path)

    if toc is None:
        toc = as_flag(meta.get("toc"))
    if number_sections is None:
        number_sections = as_flag(meta.get("number_sections"))
    if toc_depth is None:
        declared = str(meta.get("toc_depth") or "")
        toc_depth = int(declared) if declared.isdigit() else 3

    headings = collect_headings(body)
    if number_sections:
        body = number_headings(body)
    body = number_figures(body)

    title = str(meta.get("title") or source_path.stem)
    bits = []
    for key in ("author", "date"):
        if meta.get(key):
            bits.append("<div>%s</div>" % inline(str(meta[key]), math_on))
    header = ""
    if meta.get("title") or bits:
        header = '<div class="doc-meta">\n<div class="title">%s</div>\n%s\n</div>\n' % (
            inline(title, math_on), "\n".join(bits))

    content = '<div class="page">\n%s%s\n</div>' % (header, body)
    if toc:
        sidebar = toc_html(headings, toc_depth)
        if sidebar:
            content = '<div class="layout">\n%s\n%s\n</div>' % (sidebar, content)

    rendered_banner = ""
    if banner:
        rendered_banner = ('<div class="banner">Rendered from %s &middot; code not executed</div>\n'
                           % html.escape(source_path.name))
    scripts = FIGURE_SCRIPT + (TOC_SCRIPT if toc else "")
    if live:
        scripts += LIVE_SCRIPT
    # Only pull in MathJax when there is math to typeset. Most vignettes have none,
    # and skipping the download is the difference between instant and a CDN wait.
    has_math = 'class="math"' in body or "math-display" in body
    math_head = MATH_HEAD if (math_on and has_math) else ""
    return HTML_SHELL.format(title=html.escape(title, quote=True), css=stylesheet(theme),
                             banner=rendered_banner, content=content, scripts=scripts,
                             math_head=math_head)


def build_markdown(source_path, text):
    meta, body_lines = read_front_matter(text)
    head = ""
    if meta.get("title"):
        head = "# %s\n" % meta["title"]
        for key in ("author", "date"):
            if meta.get(key):
                head += "%s: %s\n" % (key.capitalize(), meta[key])
        head += "\n"
    return head + to_markdown(body_lines, source_path.parent) + "\n"


# In --watch mode the page asks the little local server for a version token and
# reloads only when it changed, so an idle page never flickers.
LIVE_SCRIPT = """<script>
(function () {
  var seen = null;
  setInterval(function () {
    fetch('/__rmdview_version').then(function (response) { return response.text(); })
      .then(function (token) {
        if (seen === null) { seen = token; } else if (seen !== token) { location.reload(); }
      }).catch(function () {});
  }, 700);
})();
</script>
"""


class WatchHandler(SimpleHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/__rmdview_version":
            body = WATCH_STATE["version"].encode()
            self.send_response(200)
            self.send_header("Content-Type", "text/plain")
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            self.wfile.write(body)
            return
        SimpleHTTPRequestHandler.do_GET(self)

    def log_message(self, *args):
        pass


WATCH_STATE = {"version": "0"}


def start_watch_server(root):
    handler = functools.partial(WatchHandler, directory=str(root))
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, server.server_address[1]


def touch_version():
    WATCH_STATE["version"] = "%.6f" % time.time()


# ---------------------------------------------------------------- package mode


def collect_project(root):
    """Everything a package reader wants: its vignettes plus a top-level README."""
    found = []
    for pattern in ("vignettes/**/*.Rmd", "vignettes/**/*.rmd", "*.Rmd", "*.rmd"):
        found.extend(sorted(root.glob(pattern)))
    seen, docs = set(), []
    for path in found:
        resolved = path.resolve()
        if resolved in seen or not path.is_file():
            continue
        seen.add(resolved)
        docs.append(path)
    return docs


def project_title(root):
    description = root / "DESCRIPTION"
    if description.is_file():
        match = re.search(r"^Package:\s*(\S+)", description.read_text(encoding="utf-8"), re.M)
        if match:
            return match.group(1)
    return root.name


def build_index(title, entries, theme, banner_text=""):
    blocks = []
    for group in ("Vignettes", "Other"):
        in_group = [entry for entry in entries if entry["group"] == group]
        if not in_group:
            continue
        blocks.append("<h2>%s</h2>" % group)
        blocks.append("<ul>")
        for entry in in_group:
            blocks.append('<li><a href="%s">%s</a> <span class="path">%s</span></li>'
                          % (entry["target_rel"], html.escape(entry["title"]), html.escape(entry["rel"])))
        blocks.append("</ul>")
    body = ('<div class="doc-meta"><div class="title">%s</div><div>%d document(s), rendered '
            'without running code</div></div>\n%s' % (html.escape(title), len(entries), "\n".join(blocks)))
    rendered_banner = '<div class="banner">%s</div>\n' % html.escape(banner_text) if banner_text else ""
    return HTML_SHELL.format(title=html.escape(title, quote=True), css=stylesheet(theme),
                             banner=rendered_banner, content='<div class="page">\n%s\n</div>' % body,
                             scripts="", math_head="")


# ---------------------------------------------------------------- cli

math_on = True


def cli_render(source_path, text, args, live=False):
    return build_html(source_path, text, math_on=math_on, theme=args.theme, toc=args.toc,
                      toc_depth=args.toc_depth, number_sections=args.number_sections,
                      banner=args.banner, live=live)


def find_package_root(start):
    """Walk up from here looking for something that is clearly an R package."""
    current = start.resolve()
    for candidate in [current] + list(current.parents):
        if (candidate / "DESCRIPTION").is_file() or (candidate / "vignettes").is_dir():
            return candidate
    return current


def plan_project(root, outdir):
    plan = []
    for path in collect_project(root):
        rel = path.relative_to(root)
        target = outdir / rel.with_suffix(".html")
        plan.append({"source": path, "target": target, "rel": str(rel), "title": path.stem,
                     "target_rel": str(target.relative_to(outdir)),
                     "group": "Vignettes" if rel.parts[0] == "vignettes" else "Other"})
    return plan


def plan_files(paths, args):
    plan = []
    for path in paths:
        if not path.is_file():
            print("skip (not a file): %s" % path, file=sys.stderr)
            continue
        suffix = ".md" if args.md else ".html"
        if args.output:
            target = Path(args.output)
        elif args.outdir:
            target = Path(args.outdir) / path.with_suffix(suffix).name
        else:
            target = path.with_suffix(suffix)
        plan.append({"source": path, "target": target, "rel": path.name, "title": path.stem,
                     "target_rel": target.name, "group": "Other"})
    return plan


def render_entry(entry, args, live=False):
    text = entry["source"].read_text(encoding="utf-8")
    meta, _ = read_front_matter(text)
    entry["title"] = str(meta.get("title") or entry["source"].stem)
    entry["target"].parent.mkdir(parents=True, exist_ok=True)
    payload = build_markdown(entry["source"], text) if args.md \
        else cli_render(entry["source"], text, args, live=live)
    entry["target"].write_text(payload, encoding="utf-8")
    print("%s -> %s" % (entry["source"], entry["target"]))


def open_targets(plan, index_path, project, port):
    base = "http://127.0.0.1:%d/" % port if port else None
    if project:
        location = (base + "index.html") if base else index_path.resolve().as_uri()
        webbrowser.open(location)
        return
    for entry in plan:
        location = (base + entry["target_rel"]) if base else entry["target"].resolve().as_uri()
        webbrowser.open(location)


def watch_loop(plan, args, root, outdir, project, port):
    mtimes = {}
    for entry in plan:
        try:
            mtimes[entry["source"]] = entry["source"].stat().st_mtime
        except OSError:
            pass
    print("watching %d file(s): http://127.0.0.1:%d/  (ctrl-c to stop)" % (len(plan), port),
          flush=True)
    try:
        while True:
            time.sleep(0.6)
            if project:
                plan = plan_project(root, outdir)
            changed = []
            for entry in plan:
                try:
                    mtime = entry["source"].stat().st_mtime
                except OSError:
                    continue
                if mtimes.get(entry["source"]) != mtime:
                    mtimes[entry["source"]] = mtime
                    changed.append(entry)
            if not changed:
                continue
            for entry in changed:
                render_entry(entry, args, live=True)
            if project:
                (outdir / "index.html").write_text(
                    build_index(project_title(root), plan, args.theme), encoding="utf-8")
            touch_version()
            print("re-rendered %d file(s)" % len(changed))
    except KeyboardInterrupt:
        print("")
    return 0


def main(argv=None):
    global math_on
    parser = argparse.ArgumentParser(
        prog="rmdview",
        description="Render R Markdown to HTML without running code. Point it at .Rmd files, "
                    "at a package directory, or run it bare inside a package to read that "
                    "package's vignettes.")
    parser.add_argument("inputs", nargs="*",
                        help=".Rmd files, or a package directory (default: the package you are in)")
    parser.add_argument("-o", "--output", help="output path (single file input only)")
    parser.add_argument("-d", "--outdir", help="write into this directory "
                                               "(default: beside each source, or <dir>/.rmdview)")
    parser.add_argument("--theme", default="rmarkdown", choices=sorted(THEMES),
                        help="visual theme (default: rmarkdown, the html_document look)")
    parser.add_argument("--toc", dest="toc", action="store_true", default=None,
                        help="force the floating table of contents on")
    parser.add_argument("--no-toc", dest="toc", action="store_false", help="force it off")
    parser.add_argument("--toc-depth", type=int, default=None, help="heading levels in the TOC (default 3)")
    parser.add_argument("--number-sections", dest="number_sections", action="store_true", default=None,
                        help="force section numbering on")
    parser.add_argument("--no-number-sections", dest="number_sections", action="store_false",
                        help="force it off")
    parser.add_argument("--no-banner", dest="banner", action="store_false", default=True,
                        help="drop the 'code not executed' banner")
    parser.add_argument("--watch", action="store_true",
                        help="re-render on save and reload the open page")
    parser.add_argument("--md", action="store_true", help="write Markdown instead of HTML "
                                                          "(GitHub previews it)")
    parser.add_argument("--stdout", action="store_true", help="write to stdout, no file")
    parser.add_argument("--open", dest="open", action="store_true", default=None,
                        help="open in a browser (default when run in a terminal)")
    parser.add_argument("--no-open", dest="open", action="store_false", help="do not open anything")
    parser.add_argument("--no-math", action="store_true", help="skip the MathJax bundle")
    args = parser.parse_args(argv)
    math_on = not args.no_math
    if args.open is None:
        args.open = sys.stdout.isatty()

    if args.watch and (args.stdout or args.md):
        parser.error("--watch needs HTML output, so not --stdout or --md")

    sources = args.inputs or [str(find_package_root(Path.cwd()))]
    project = len(sources) == 1 and Path(sources[0]).is_dir()
    if args.output and (len(sources) > 1 or project):
        parser.error("-o works with a single file input")
    if project and (args.md or args.stdout):
        parser.error("--md and --stdout work on files, not directories")

    if args.stdout:
        for name in sources:
            path = Path(name)
            if not path.is_file():
                print("skip (not a file): %s" % name, file=sys.stderr)
                continue
            text = path.read_text(encoding="utf-8")
            sys.stdout.write(build_markdown(path, text) if args.md else cli_render(path, text, args))
        return 0

    if project:
        root = Path(sources[0])
        outdir = Path(args.outdir) if args.outdir else root / ".rmdview"
        plan = plan_project(root, outdir)
        if not plan:
            print("no .Rmd files under %s" % root, file=sys.stderr)
            return 1
        index_path = outdir / "index.html"
    else:
        plan = plan_files([Path(name) for name in sources], args)
        if not plan:
            return 1
        index_path = None

    port = None
    if args.watch:
        serve_root = outdir if project else plan[0]["target"].parent
        _, port = start_watch_server(serve_root)
        touch_version()

    started = time.time()
    for entry in plan:
        render_entry(entry, args, live=args.watch)
    if index_path is not None:
        index_path.write_text(build_index(project_title(root), plan, args.theme,
                                          "Rendered without running code"), encoding="utf-8")
        print("%d document(s) -> %s" % (len(plan), index_path))
    if not args.stdout:
        print("rendered in %.0f ms" % ((time.time() - started) * 1000))

    if args.open:
        open_targets(plan, index_path, project, port)
    if args.watch:
        return watch_loop(plan, args, root if project else None, outdir if project else None,
                          project, port)
    return 0


if __name__ == "__main__":
    sys.exit(main())