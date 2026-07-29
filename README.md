# kb

A fast Markdown knowledge base — search, list, and write notes stored as plain
files in git repositories.

## Why

`kb` replaces [`nb`](https://github.com/xwmx/nb), which is a 27,000-line Bash
script. Measured over the same 787 notes (13.5 MB):

| Command | `nb` | `kb` |
| --- | ---: | ---: |
| startup only (`--version`) | 0.42 s | 0.017 s |
| list a notebook | 3.28 s | 0.073 s |
| search a notebook | 18.5 s | **0.090 s** |

The data was never the problem. `nb` forks `git`, `sed`, and `awk` per note, so
searching spent its time waiting on processes rather than reading text — its CPU
usage during that 18.5 s search was 38%. `kb` reads each file once and scans it
with ripgrep's matcher.

## Layout

A knowledge base is a directory of *notebooks*; each notebook is a directory of
Markdown, normally a git repository:

```
~/.nb/                  # or $KB_ROOT
├── home/               # a notebook
│   └── knowledge/*.md
└── work/
    └── knowledge/*.md
```

Notes carry YAML frontmatter:

```yaml
---
title: Cloudflare で nb データの RAG を構築する設計パターン
tags: [knowledge]
created: 2026-04-20T13:34:09+09:00
updated: 2026-07-17T09:12:44+09:00
---
```

There is no index and no database. Every command walks the tree, which stays in
the tens of milliseconds at the sizes a personal knowledge base reaches.

## Commands

| Command | Purpose |
| --- | --- |
| `kb search <pattern>` | Search note bodies. Regex by default, smart case |
| `kb ls` | List notes, newest first |
| `kb tags` | Tags and how many notes carry them |
| `kb new <title>` | Create a note with frontmatter and open `$EDITOR` |
| `kb open` | Pick a note with fzf, read it with glow |
| `kb sync` | Commit Markdown changes, then pull and push |
| `kb migrate` | Backfill frontmatter onto notes that lack it |

Filters shared by `search`, `ls`, `tags`, and `open`:

```
-n, --notebook <NAME>   limit to one notebook
-t, --tag <TAG>         require a tag; repeat to require several
-s, --since <WHEN>      touched since a date or duration (7d, 3w, 2026-01-01)
    --limit <N>         maximum results
```

`search` also takes `-F` (literal), `-i`/`-s` (case), `-m` (matches per note),
`-l` (paths only), and `--json`.

### Migration

`kb migrate` adds `title`, `tags`, `created`, and `updated` to notes that lack
them. It derives each field mechanically:

- **title** — the first heading or line of prose, condensed at the first
  sentence break if it runs long; the filename as a last resort
- **tags** — the note's top-level directory
- **created** — an `nb`-style timestamp filename, else the first commit
- **updated** — the last commit touching the file

It never rewrites prose. Existing frontmatter keys are left byte-for-byte
intact and only missing ones are appended, so the diff is pure insertion. The
run is a preview until you pass `--apply`, and it refuses a dirty working tree
unless you pass `--allow-dirty`.

## Configuration

| Variable | Effect |
| --- | --- |
| `KB_ROOT` | Knowledge base location (default `~/.nb`) |
| `KB_NOTEBOOK` | Notebook for new notes |
| `NO_COLOR` | Disable ANSI styling |

Without `KB_NOTEBOOK`, new notes go to `work` when `~/.is_work_pc` exists and
`home` otherwise — work and personal notes live in separate repositories, and
putting one in the other is tedious to undo.

`kb open` uses `fzf`, and prefers `glow` for rendering when it is installed.

## Structure

- **`kb-core`** — the library: note discovery, frontmatter, search, git, migration
- **`kb-cli`** — the `kb` binary

Both the CLI and any future MCP server are thin shells over `kb-core`.

## Build

```sh
cargo build --release   # target/release/kb
cargo test
```
