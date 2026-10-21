# legacy: the Python tool rmdt replaces

`rmdview.py` and `test_rmdview.py` are the original single file Python
implementation, kept for reference. It produced the same HTML and the same
filtered Markdown that `rmdt` now produces, and `rmdt` was checked against it on
four real vignettes before it was retired.

It is not installed any more. To run it:

    python3 legacy/rmdview.py path/to/doc.Rmd -o /tmp/doc.html
    python3 legacy/test_rmdview.py

`demo/` holds the fixture that exercises every feature: chunk options, `#|`
options, a child document, `fig.cap`, math, tables and raw HTML.
