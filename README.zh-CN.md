# rmdt

[English](README.md) | 中文

在终端里阅读 R Markdown，或者为浏览器生成页面，而不运行其中任何代码。

渲染（knit）一篇 vignette 会执行它里面的代码：装包、读文件，甚至删东西。`rmdt` 全部跳过。
它显示文档本身，标出哪些 chunk 没有运行，并在图本该出现的位置放上编号图注和占位框。

## 绝不运行你的代码

文档内容没有任何执行路径。本程序唯一会启动的进程是你的 `$EDITOR`（`e` 键，用来编辑你正在读的
那一行）和 `--html` 的浏览器打开器（macOS 上是 `open`，其他地方是 `xdg-open`）。这两条都有测试
守着：前端里完全没有 `Command::new`，其余代码中只有这两处调用，参数是字面量或 `$EDITOR`。

## 安装

```sh
cargo build --release          # 2.3 MB 二进制，冷编译约 25 秒
ln -sf "$PWD/target/release/rmdt" ~/.local/bin/rmdt
```

## 用法

```sh
rmdt ~/r_package/bears                # 建站，然后在终端里阅读
rmdt vignettes/intro.Rmd              # 读单个文档
rmdt --watch vignettes/intro.Rmd      # 边改边读
rmdt ~/r_package/bears --html         # 只生成页面（加 --open 才开浏览器）
rmdt vignettes/intro.Rmd --md         # 过滤后的 Markdown 输出到 stdout
rmdt vignettes/intro.Rmd --md | mdt   # 交给别的终端阅读器
```

不带参数的 `rmdt` 会从当前目录向上找到最近的包（有 `DESCRIPTION` 文件或 `vignettes/` 目录）。
传入目录参数时，就对这个目录做同样的事。

## 包会变成一个站点

把 `rmdt` 指向一个包，它会建一个小型文档站点，然后在终端里读它：

```sh
cd ~/r_package/bears
rmdt                      # 建站，然后在这里浏览
rmdt --html               # 只建站就停；加 --open 才开浏览器
rmdt --html -d /tmp/site  # 建到别处
```

站点写到 `<包目录>/.rmdview/`，源码树不动。它包含：

- 首页：包名、`DESCRIPTION` 里的 `Title` 和 `Description`，以及按 `Vignettes` 和
  `Other documents` 分组的文档清单
- 每一页顶部的导航条，链接包首页和所有同级文档，所以这些页面读起来是一个站，而不是散落的文件
- 每页自己的浮动目录和章节编号，来自该文档的 front matter

深层页面按需要回退 `../`，测试会检查建出的站点里每一条相对链接都指向真实存在的文件。

终端里列的是同一批文档、同样的分组，所以在列表第一项上按 `Enter` 就打开第一篇 vignette。
按 `b` 回到列表。

终端视图需要 stdin 和 stdout 都是终端。没有终端时它会说明情况并指出已经写好的页面，而不是
抛一个终端错误：

```
$ rmdt ~/r_package/bears | cat
site: 3 page(s) -> /home/me/r_package/bears/.rmdview/index.html
rmdt: the terminal view needs a terminal on stdin and stdout. The pages are
already written: .../.rmdview/index.html. Use --html to write pages instead, or
run this in a terminal.
```

### 选项

| 选项 | 作用 |
| --- | --- |
| `--theme rmarkdown\|github\|plain` | 外观，两个后端同时生效。`rmarkdown` 按 Bootstrap 3 的度量，也就是 `rmarkdown::render` 产出的样子 |
| `--toc` / `--no-toc` | 强制打开或关闭目录 |
| `--toc-depth N` | 目录列到第几级标题 |
| `--number-sections` / `--no-number-sections` | 强制打开或关闭章节编号 |
| `--outline popup\|pinned\|off` | 大纲初始形态：弹窗、常驻侧栏、关闭 |
| `--watch` | 文件一变就重新读取 |
| `--no-banner` | 去掉页面顶部那行「code not executed」 |
| `--no-math` | 页面里不引入 MathJax |
| `-o PATH`、`-d DIR` | 写到哪个文件或哪个目录 |
| `--open` | 用浏览器打开写出的页面。只写页面不会自己打开 |
| `--no-open` | 明确表示什么都不打开，这也是默认行为 |

front matter 会先被读取，所以文档只要在 `output:` 下写了 `toc: true`、`toc_depth: 3`、
`number_sections: yes`，不需要任何参数就能生效。命令行参数优先于 front matter。

### 按键

| 按键 | 动作 |
| --- | --- |
| `j` `k`、上下方向键 | 一行 |
| `d` `u`、空格、翻页键 | 一页 |
| `h` `l` | 半页 |
| `g` `G` | 到顶、到底 |
| `[` `]` | 上一节、下一节 |
| `o`、Tab | 大纲弹窗，`Enter` 跳过去 |
| `t` | 固定或取消固定大纲侧栏 |
| `/` | 搜索，`n` `N` 跳到下一个、上一个命中 |
| `e` | 用 `$EDITOR` 打开当前源码行 |
| `r` | 重新读取文件 |
| `b` | 回到文件列表 |
| `?` | 按键说明 |
| `q`、`Ctrl-C` | 退出 |

弹层会先吃掉关闭它的那个按键，所以在大纲弹窗或按键说明里退出要按 `Esc` 再按 `q`。

## chunk 怎么处理

| chunk 选项 | 你看到什么 |
| --- | --- |
| `echo=FALSE` | 一行提示：`chunk name: code hidden (echo=FALSE)` |
| `include=FALSE` | 什么都没有 |
| `eval=FALSE` | 源码，外加一行「未运行」的说明 |
| `child="other.Rmd"` | 把那个文档内联进来 |
| `fig.cap="..."` | 占位框下面是 `Figure N: ...` |
| `#| echo: false` | 同样识别，用于 Quarto 风格的文档 |
| ```` ```{=html} ```` | 原样透传为 HTML |

行内 R（`` `r nrow(df)` ``）按表达式显示，永不求值。在 HTML 输出里它保留 `rinline` 类，这样你能
看出哪些值本来是代码算出来的；终端里就是一个普通的行内代码。

Markdown 由 `pulldown-cmark` 解析，所以 CommonMark 的行为是正经的：列表的 lazy continuation、
表格、脚注、alert 都正常。

## 不处理的情况

Sweave 的 `.Rnw` vignette。那种文档是 LaTeX 加 R chunk，不是 Markdown，需要本工具没有的 LaTeX
前端。指到一个 vignette 全是 `.Rnw` 的包时，它会明说而不是空手而归：

```
rmdt: no .Rmd files under PopGenome. It holds Sweave .Rnw files such as
.../vignettes/Integration_of_new_Methods.Rnw, which rmdt does not read: ...
```

行内 R 只显示不求值，所以像日期这样的算出来的值，显示出来的是产出它的那个表达式。

源码位置属于被打开的那个文件。在通过 `child="other.Rmd"` 进来的 chunk 上按 `e`，会在当前文档里
打开 child 的行号，那里并不是这个 chunk 所在的位置。child 文档的其它方面（正文、在大纲里的位置）
都是对的。

## 内部结构

```
.Rmd  ->  前端  ->  带源码行的 block 列表  ->  终端行  ->  ratatui
                                          ->  HTML 页面
```

前端（`src/rmd.rs`）把 R Markdown 变成一串 block。正文保留为 CommonMark，排版交给 markdown
引擎；标题、代码、提示、图各自成块，这样两个后端都能按自己的方式渲染，大纲也能指到它们。

`src/render.rs` 逐块把 block 变成终端行，并记下每一行来自哪一行源码。大纲、跳节、保留阅读位置的
重载、`e` 键都建立在这张映射上。行在生成时就折过行，所以一个行永远等于一个屏幕行，滚动算术很直接。

渲染由 [`tui-markdown`](https://github.com/joshka/tui-markdown)（`pulldown-cmark` 加 `syntect`）
完成，终端主题是它的一套 `StyleSheet` 实现。HTML 后端自己写标记，这样才能把章节号、chunk 头、
未运行提示、图注放在跟早先那个 Python 工具相同的位置。

## 测试

```sh
cargo test                    # 89 条单元测试
cargo clippy --all-targets    # 无告警
```

单元测试覆盖 chunk 选项规则、front matter、Markdown 输出、HTML 输出、折行与源码映射，以及通过
`ratatui` 的 `TestBackend` 断言的终端画面。畸形文档也在其中：未闭合的围栏和 front matter、
自己包含自己或互相成环的 child 文档、空标题和七个井号的标题、CRLF 输入，以及不含合法 UTF-8 的
文件（这类会明确报错，而不是猜）。

渲染结果跟它取代的那个 Python 工具做过对比，用的是四篇真实 vignette 和一个覆盖全部特性的夹具：
按级别统计的标题数、chunk 块、提示、章节号、目录链接、表格、图注、公式、`rinline` 类全部一致。

## 与 Python 版的差别

- 默认命令是在终端里读。`--html` 是浏览器那条路，它只写页面，除非你传 `--open`。
- HTML 正文与 Python 版不是逐字节相同：段落和列表项内部的换行跟随源码，表格属性的空格也不同。
  渲染结果一致。
- 三套样式表和 chunk 头部的标记原样沿用。

## 许可证

MIT OR Apache-2.0。
