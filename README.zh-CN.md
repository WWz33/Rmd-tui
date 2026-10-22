# rmdt

[English](README.md) | 中文

在终端里阅读 R Markdown，或者为浏览器生成页面，而不运行其中任何代码。

渲染（knit）一篇 vignette 会执行它里面的代码：装包、读文件，甚至删东西。`rmdt` 全部跳过。
它显示文档本身，标出哪些 chunk 没有运行，并在图本该出现的位置放上编号图注和占位框。

![rmdt 正在读一篇 vignette，左边是索引栏，右边是正文](assets/screenshot.png)

## 绝不运行你的代码

文档内容没有任何执行路径。本程序唯一会启动的进程是你的 `$EDITOR`（`e` 键，用来编辑你正在读的
那一行）和 `--html` 的浏览器打开器（macOS 上是 `open`，其他地方是 `xdg-open`）。

## 安装

```sh
cargo build --release                                # 编译
ln -sf "$PWD/target/release/rmdt" ~/.local/bin/rmdt  # 放进 PATH
```

## 用法

```sh
rmdt ~/my_package                    # 指向一个包：建站点，然后在终端里读
rmdt vignettes/intro.Rmd             # 单个文档
rmdt --watch vignettes/intro.Rmd     # 文件一变就重新读取
rmdt ~/my_package --html             # 只写页面，不在终端里读
rmdt vignettes/intro.Rmd --md        # 写出过滤后的 Markdown
rmdt vignettes/intro.Rmd --md | mdt  # 交给别的渲染器
```

不带参数的 `rmdt` 会从当前目录向上找到最近的包（有 `DESCRIPTION` 文件或 `vignettes/` 目录）。
传入目录参数时，就对这个目录做同样的事。

## 包会变成一个站点

把 `rmdt` 指向一个包，它会建一个小型文档站点，然后在终端里读它：

```sh
cd ~/my_package           # 进入包目录
rmdt                      # 建站点，然后在终端里读
rmdt --html               # 只写页面
rmdt --html -d /tmp/site  # 写到别处
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
$ rmdt ~/my_package | cat
site: 3 page(s) -> /home/me/my_package/.rmdview/index.html
rmdt: the terminal view needs a terminal on stdin and stdout. The pages are
already written: .../.rmdview/index.html. Use --html to write pages instead, or
run this in a terminal.
```

### 选项

| 选项 | 作用 |
| --- | --- |
| `--theme rmarkdown\|github\|plain` | 写出的页面的外观 |
| `--tui-theme glow-dark\|glow-light\|minimal` | 终端里的外观：[glow](https://github.com/charmbracelet/glow) 的配色，分深色底和浅色底两套，`minimal` 则用终端自己的颜色 |
| `--toc` / `--no-toc` | 强制打开或关闭目录 |
| `--toc-depth N` | 目录列到第几级标题 |
| `--number-sections` / `--no-number-sections` | 强制打开或关闭章节编号 |
| `--outline pinned\|popup\|off` | 索引栏初始形态 |
| `--watch` | 文件一变就重新读取 |
| `--no-banner` | 去掉页面顶部那行「code not executed」 |
| `--no-math` | 页面里不引入 MathJax |
| `-o PATH`、`-d DIR` | 写到哪个文件或哪个目录 |
| `--html` | 生成页面，而不是在终端里读 |
| `--md` | 把过滤后的 Markdown 写到 stdout，或者写到 `-o` |
| `--open` | 用浏览器打开写出的页面。只写页面不会自己打开 |
| `--no-open` | 明确表示什么都不打开，这也是默认行为 |

### 按键

| 按键 | 动作 |
| --- | --- |
| `j` `k` | 一行 |
| `←` `→` | 在索引栏和正文之间切换 |
| `↑` `↓` | 在获得方向键的那个窗格里移动 |
| `Enter` | 在索引栏里：打开该文档，或进入该章节 |
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

### 终端里的配色

配色取自 `glow`：markdown 部分来自 glamour 的样式文件，代码部分来自那些文件里带的 chroma 色表。
`glow-dark` 与 `glow-light` 的区别只是为哪种底色而画，`minimal` 用终端自身的前景色，靠字重区分标题，
适合 glow 的固定颜色不合口味的环境。

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
未运行提示、图注放在该在的位置。

## 许可证

MIT OR Apache-2.0。
