#!/usr/bin/env python3
"""Build the user's manual: one Markdown source, a typeset PDF and a web page.

Question this answers
---------------------
    How do the PDF and the GitHub Pages manual stay saying the same thing?

By not being written twice. `manual.md` is the only copy anyone edits. This emits
`slice-manual.tex` and `slice-manual.pdf` from it, and `index.md` with the front matter
GitHub Pages wants. A manual maintained as two hand-written documents drifts, and a manual
that disagrees with itself is worse than one that is merely out of date.

Pandoc would be the obvious tool and is not installed here, so this implements the small
subset of Markdown the manual actually uses: ATX headings, paragraphs, fenced and indented
code, bullet lists, pipe tables, block quotes, and inline `code`, **bold**, *italic* and
[links](url). It refuses to guess: anything it does not recognise is reported rather than
passed through silently, so a new construct in the manual fails the build instead of
vanishing from the PDF.

Usage
-----
    docs/manual/build.py             # write the .tex, the .pdf and index.md
    docs/manual/build.py --check     # fail if any committed output is stale
    docs/manual/build.py --no-pdf    # skip the LaTeX run

Needs `pdflatex` for the PDF; `--no-pdf` skips it. The .tex is written either way, so the
PDF can be built elsewhere.

`--check` is deliberately a local step: CI does not run it, and the build machine carries
no TeX installation. Run it before committing a change to `manual.md`. (It compares the
committed files and never invokes pdflatex, so it needs no TeX itself -- but keeping the
whole manual pipeline off CI keeps the CI image small.)
"""

from __future__ import annotations

import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
SOURCE = HERE / "manual.md"
TEX = HERE / "slice-manual.tex"
PDF = HERE / "slice-manual.pdf"
PAGE = HERE.parent / "index.md"


# ----------------------------------------------------------------------- the parser

def parse(text: str) -> tuple[dict, list[dict]]:
    """Split the source into a metadata dict and a list of block dicts."""
    meta: dict[str, str] = {}
    lines = text.split("\n")

    blocks: list[dict] = []
    i = 0
    while i < len(lines):
        line = lines[i]

        # `% key: value` metadata, anywhere in the preamble.
        if line.startswith("% ") and ":" in line:
            key, _, value = line[2:].partition(":")
            meta[key.strip()] = value.strip()
            i += 1
            continue

        if not line.strip():
            i += 1
            continue

        if line.startswith("#"):
            level = len(line) - len(line.lstrip("#"))
            blocks.append({"kind": "heading", "level": level,
                           "text": line[level:].strip()})
            i += 1
            continue

        if line.startswith("```"):
            i += 1
            body = []
            while i < len(lines) and not lines[i].startswith("```"):
                body.append(lines[i])
                i += 1
            i += 1
            blocks.append({"kind": "code", "lines": body})
            continue

        # An indented code block: four spaces, and not a list continuation.
        if line.startswith("    ") and line.strip():
            body = []
            while i < len(lines) and (lines[i].startswith("    ") or not lines[i].strip()):
                body.append(lines[i][4:] if lines[i].startswith("    ") else "")
                i += 1
            while body and not body[-1].strip():
                body.pop()
            blocks.append({"kind": "code", "lines": body})
            continue

        if line.startswith("> "):
            body = []
            while i < len(lines) and lines[i].startswith(">"):
                body.append(lines[i].lstrip(">").strip())
                i += 1
            blocks.append({"kind": "quote", "text": " ".join(body)})
            continue

        if line.startswith("| "):
            rows = []
            while i < len(lines) and lines[i].startswith("|"):
                rows.append([c.strip() for c in lines[i].strip().strip("|").split("|")])
                i += 1
            # Row 1 is the header, row 2 the alignment rule.
            if len(rows) < 2 or not all(set(c) <= set("-: ") for c in rows[1]):
                raise SystemExit(f"table without an alignment row near: {line!r}")
            blocks.append({"kind": "table", "head": rows[0], "body": rows[2:]})
            continue

        if line.startswith("- "):
            items, current = [], ""
            while i < len(lines) and (lines[i].startswith("- ") or
                                      (lines[i].startswith("  ") and lines[i].strip())):
                if lines[i].startswith("- "):
                    if current:
                        items.append(current)
                    current = lines[i][2:].strip()
                else:
                    current += " " + lines[i].strip()
                i += 1
            if current:
                items.append(current)
            blocks.append({"kind": "list", "items": items})
            continue

        if re.match(r"^\d+\. ", line):
            items, current = [], ""
            while i < len(lines) and (re.match(r"^\d+\. ", lines[i]) or
                                      (lines[i].startswith("   ") and lines[i].strip())):
                if re.match(r"^\d+\. ", lines[i]):
                    if current:
                        items.append(current)
                    current = re.sub(r"^\d+\. ", "", lines[i]).strip()
                else:
                    current += " " + lines[i].strip()
                i += 1
            if current:
                items.append(current)
            blocks.append({"kind": "ordered", "items": items})
            continue

        # Otherwise a paragraph: everything up to the next blank line.
        body = []
        while i < len(lines) and lines[i].strip() and not lines[i].startswith(
                ("#", "```", "|", "- ", "> ")) and not re.match(r"^\d+\. ", lines[i]):
            body.append(lines[i].strip())
            i += 1
        blocks.append({"kind": "para", "text": " ".join(body)})

    return meta, blocks


# ------------------------------------------------------------------------ to LaTeX

LATEX_ESCAPES = {
    "\\": r"\textbackslash{}", "&": r"\&", "%": r"\%", "$": r"\$", "#": r"\#",
    "_": r"\_", "{": r"\{", "}": r"\}", "~": r"\textasciitilde{}",
    "^": r"\textasciicircum{}",
}


def tex_escape(text: str) -> str:
    return "".join(LATEX_ESCAPES.get(c, c) for c in text)


def tex_inline(text: str) -> str:
    r"""Inline markup to LaTeX.

    Anything that expands to a LaTeX *command* is stashed behind a placeholder before the
    text is escaped and put back afterwards. Escaping after substitution would turn the
    backslash of the command just emitted into `\textbackslash{}`, which is how the first
    build of this manual printed every hyperlink as literal `\href{...}{...}` text.
    """
    protected: list[str] = []

    def keep(latex: str) -> str:
        protected.append(latex)
        return f"\x00{len(protected) - 1}\x00"

    # Code spans first, so a URL or an asterisk inside one is left alone.
    text = re.sub(r"`([^`]+)`",
                  lambda m: keep(f"\\texttt{{{tex_escape(m.group(1))}}}"), text)
    text = re.sub(
        r"\[([^\]]+)\]\(([^)]+)\)",
        lambda m: keep("\\href{%s}{%s}" % (
            # Only `#` and `%` actually need escaping inside a URL argument.
            m.group(2).replace("#", r"\#").replace("%", r"\%"),
            tex_escape(m.group(1)),
        )),
        text,
    )
    text = tex_escape(text)
    text = re.sub(r"\*\*([^*]+)\*\*", r"\\textbf{\1}", text)
    text = re.sub(r"(?<!\*)\*([^*]+)\*(?!\*)", r"\\emph{\1}", text)
    # An en dash typed as a spaced hyphen, and a literal em dash.
    text = text.replace("—", "---")
    # A straight `"` sets as a *closing* quote in LaTeX, so an opening one comes out
    # backwards. Pair them off: odd occurrences open, even ones close.
    parts = text.split('"')
    if len(parts) > 1:
        text = parts[0]
        for n, part in enumerate(parts[1:]):
            text += ("``" if n % 2 == 0 else "''") + part
    text = text.replace("\u2019", "'").replace("\u201c", "``").replace("\u201d", "''")
    return re.sub(r"\x00(\d+)\x00", lambda m: protected[int(m.group(1))], text)


PREAMBLE = r"""\documentclass[11pt,a4paper]{article}
\usepackage[utf8]{inputenc}
\usepackage[T1]{fontenc}
\usepackage{lmodern}
\usepackage[margin=2.6cm]{geometry}
\usepackage{longtable}
\usepackage{booktabs}
\usepackage{array}
\usepackage{ragged2e}
\usepackage{fancyvrb}
\usepackage{xcolor}
\usepackage{titlesec}
\usepackage{enumitem}
\usepackage[hidelinks]{hyperref}

\definecolor{rule}{gray}{0.6}
\definecolor{shade}{gray}{0.96}

\titleformat{\section}{\Large\bfseries}{\thesection}{0.7em}{}
\titleformat{\subsection}{\large\bfseries}{\thesubsection}{0.6em}{}
\setlist[itemize]{leftmargin=1.3em,itemsep=0.25em,topsep=0.4em}
\setlist[enumerate]{leftmargin=1.6em,itemsep=0.25em,topsep=0.4em}
\setlength{\parskip}{0.55em}
\setlength{\parindent}{0pt}
\renewcommand{\arraystretch}{1.25}

% Code blocks: shaded, no frame, small.
\DefineVerbatimEnvironment{code}{Verbatim}{%
  fontsize=\small,xleftmargin=1em,frame=leftline,framerule=0.8pt,%
  rulecolor=\color{rule},framesep=8pt}

\newenvironment{callout}%
  {\begin{quote}\itshape\small}%
  {\end{quote}}

\hypersetup{pdftitle={Slice --- User's Manual},pdfauthor={Slice}}

% Make the output byte-reproducible. pdfTeX otherwise stamps a creation date, a
% producer banner and a random trailer /ID, so two builds of an unchanged manual
% differ and a committed PDF cannot be diffed or checked in CI. The bits below
% suppress each of those; `SOURCE_DATE_EPOCH` in the environment pins what is left.
\ifdefined\pdfvariable
  \pdfvariable suppressoptionalinfo \numexpr 1+2+4+8+16+32+64+128+256+512\relax
\else
  \ifdefined\pdfsuppressptexinfo \pdfsuppressptexinfo=-1 \fi
\fi
\ifdefined\pdftrailerid \pdftrailerid{} \fi
"""


def to_latex(meta: dict, blocks: list[dict]) -> str:
    out = [PREAMBLE, r"\begin{document}"]

    title = meta.get("title", "Slice --- User's Manual")
    out += [
        r"\begin{titlepage}", r"\centering", r"\vspace*{5cm}",
        r"{\Huge\bfseries " + tex_escape(title) + r"\par}", r"\vspace{1cm}",
    ]
    if meta.get("subtitle"):
        out.append(r"{\Large " + tex_escape(meta["subtitle"]) + r"\par}")
    out.append(r"\vspace{2cm}")
    if meta.get("version"):
        out.append(r"{\large Version " + tex_escape(meta["version"]) + r"\par}")
    out += [r"\vfill", r"\end{titlepage}", r"\tableofcontents", r"\newpage"]

    first_heading = True
    for block in blocks:
        kind = block["kind"]

        if kind == "heading":
            # The document title is the first `#`; the rest map onto sections.
            if block["level"] == 1 and first_heading:
                first_heading = False
                continue
            command = {1: "section", 2: "section", 3: "subsection",
                       4: "subsubsection"}[min(block["level"], 4)]
            out.append(f"\\{command}{{{tex_inline(block['text'])}}}")

        elif kind == "para":
            out.append(tex_inline(block["text"]))

        elif kind == "code":
            out.append(r"\begin{code}")
            out.extend(block["lines"])
            out.append(r"\end{code}")

        elif kind == "quote":
            out.append(r"\begin{callout}")
            out.append(tex_inline(block["text"]))
            out.append(r"\end{callout}")

        elif kind in ("list", "ordered"):
            env = "itemize" if kind == "list" else "enumerate"
            out.append(f"\\begin{{{env}}}")
            out += [f"  \\item {tex_inline(item)}" for item in block["items"]]
            out.append(f"\\end{{{env}}}")

        elif kind == "table":
            columns = len(block["head"])
            # Size the columns by what is actually in them. Equal widths wrap a
            # two-word label onto three lines while a sentence beside it has room to
            # spare, which is how the first draft of this table read.
            longest = [
                max([len(row[c]) for row in [block["head"], *block["body"]]
                     if c < len(row)] or [1])
                for c in range(columns)
            ]
            # Clamp before normalising: one very long cell should not starve the rest.
            clamped = [min(w, 60) for w in longest]
            total = sum(clamped)
            widths = [max(0.09, 0.97 * w / total) for w in clamped]
            scale = 0.97 / sum(widths)
            widths = [w * scale for w in widths]
            spec = "".join(f">{{\\RaggedRight\\arraybackslash}}p{{{w}\\linewidth}}"
                           for w in widths)
            out.append(f"\\begin{{longtable}}{{{spec}}}")
            out.append(r"\toprule")
            out.append(" & ".join(f"\\textbf{{{tex_inline(c)}}}"
                                  for c in block["head"]) + r" \\")
            out.append(r"\midrule\endhead")
            for row in block["body"]:
                cells = (row + [""] * columns)[:columns]
                out.append(" & ".join(tex_inline(c) for c in cells) + r" \\")
            out.append(r"\bottomrule")
            out.append(r"\end{longtable}")

        else:  # pragma: no cover - the parser only emits the kinds above
            raise SystemExit(f"no LaTeX rule for block kind {kind!r}")

        out.append("")

    out.append(r"\end{document}")
    return "\n".join(out) + "\n"


# ------------------------------------------------------------------- to Pages markdown

def to_page(meta: dict, source: str) -> str:
    """The same text, with Jekyll front matter and the metadata lines removed.

    GitHub Pages renders GitHub-Flavoured Markdown, which the source already is, so this
    is deliberately almost a copy: the less transformation between what is written and
    what is published, the fewer ways they can differ.
    """
    body = "\n".join(line for line in source.split("\n")
                     if not (line.startswith("% ") and ":" in line))
    body = body.lstrip("\n")

    # Drop the leading `#`. A Pages theme renders the front-matter title as the page's
    # own heading, so leaving this in prints the title twice.
    if body.startswith("# "):
        body = body.split("\n", 1)[1].lstrip("\n")

    front = [
        "---",
        "layout: default",
        f'title: {meta.get("title", "Slice — User\'s Manual")}',
        f'description: {meta.get("subtitle", "")}',
        "---",
        "",
        "*Also available as a [PDF](manual/slice-manual.pdf).*",
        "",
        "",
    ]
    return "\n".join(front) + body


# ------------------------------------------------------------------------------ main

def build_pdf(tex_path: Path) -> None:
    if not shutil.which("pdflatex"):
        raise SystemExit("pdflatex is not installed; re-run with --no-pdf")
    # A fixed epoch, so the timestamp pdfTeX embeds is the same on every machine.
    # 2026-08-24, the day the manual was written; the value is arbitrary and only has
    # to be stable.
    env = {**os.environ, "SOURCE_DATE_EPOCH": "1787529600", "FORCE_SOURCE_DATE": "1"}
    with tempfile.TemporaryDirectory() as work:
        for _ in range(2):  # twice, so the table of contents resolves
            run = subprocess.run(
                ["pdflatex", "-interaction=nonstopmode", "-halt-on-error",
                 "-output-directory", work, str(tex_path)],
                capture_output=True, text=True, env=env,
            )
            if run.returncode != 0:
                tail = [l for l in run.stdout.split("\n") if l.startswith("!")][:8]
                raise SystemExit("pdflatex failed:\n  " + "\n  ".join(tail or
                                 run.stdout.split("\n")[-25:]))
        shutil.copy(Path(work) / (tex_path.stem + ".pdf"), PDF)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true",
                        help="fail if a committed output is out of date")
    parser.add_argument("--no-pdf", action="store_true")
    args = parser.parse_args()

    source = SOURCE.read_text()
    meta, blocks = parse(source)
    meta.setdefault("title", next(b["text"] for b in blocks if b["kind"] == "heading"))

    tex = to_latex(meta, blocks)
    page = to_page(meta, source)

    if args.check:
        stale = [p.name for p, wanted in ((TEX, tex), (PAGE, page))
                 if not p.exists() or p.read_text() != wanted]
        if not PDF.exists():
            stale.append(PDF.name)
        if stale:
            print(f"out of date: {', '.join(stale)}\n"
                  f"run docs/manual/build.py", file=sys.stderr)
            return 1
        print(f"the manual is up to date ({len(blocks)} blocks)")
        return 0

    TEX.write_text(tex)
    PAGE.write_text(page)
    print(f"wrote {TEX.relative_to(HERE.parent.parent)}")
    print(f"wrote {PAGE.relative_to(HERE.parent.parent)}")
    if not args.no_pdf:
        build_pdf(TEX)
        print(f"wrote {PDF.relative_to(HERE.parent.parent)} "
              f"({PDF.stat().st_size // 1024} KB)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
