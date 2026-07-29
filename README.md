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

## Compatibility with `nb`

`kb` implements `nb`'s command surface, so existing habits and scripts carry
over. Every `nb` subcommand resolves, with the same short aliases.

**Item references.** Commands take `[<notebook>:][<folder>/]<id|filename|title>`:

```sh
kb 3                     # show item 3 in the current notebook
kb home:knowledge/12     # scope to a notebook and folder
kb show "My Note Title"  # or name it by title
kb                       # no argument lists the current notebook
```

Ids come from the `.index` file in each directory: an item's id is its line
number. Deleting an item blanks its line rather than removing it, so ids are
never reused and a reference written down last year still points at the same
note — the same guarantee `nb` gives.

**Filenames.** A title becomes a filename the way `nb` does it: lowercase ASCII,
with whitespace and `: / \ ? *` replaced by underscores, everything else left
alone. `日本語UIライティング - 句点のルール` → `日本語uiライティング_-_句点のルール.md`.

**Files.** Notes are Markdown, bookmarks are `*.bookmark.md`, todos are
`*.todo.md` with a `# [ ]` / `# [x]` heading, encrypted items are `*.enc`
(OpenSSL AES-256, or GPG when configured). Pinned entries live in `.pindex`,
archived notebooks carry `.archived`, and settings go in `.nbrc` as
`export NB_NAME="${NB_NAME:-value}"` — which means one config file serves both
tools, in both directions.

## Commands

| Command | Purpose |
| --- | --- |
| `add` (`a`, `new`, `+`) | Create a note; a bare argument is a filename if it has an extension, else content |
| `ls` / `list` | List items |
| `search` (`q`, `grep`) | Search note bodies. Regex by default, smart case |
| `show` (`s`) / `peek` (`p`) / `open` (`o`) | Display an item, or hand it to the system |
| `edit` (`e`) | Append, prepend, or overwrite; or open `$EDITOR` |
| `delete` (`d`, `rm`) / `move` (`mv`) / `copy` (`cp`) | Manage items, keeping ids straight |
| `bookmark` (`bm`) | Bookmark a URL, fetching its title |
| `todo` (`todos`, `tasks`) / `do` / `undo` | Todos and their checkboxes |
| `pin` / `unpin` / `archive` / `unarchive` | Ordering and notebook state |
| `browse` | The embedded web app: render notes, follow `[[links]]` and `#tags` |
| `notebooks` / `use` (`u`) / `count` / `folders` | Notebooks and structure |
| `sync` / `status` / `git` / `history` / `remote` | Git |
| `settings` / `set` / `unset` / `env` | Configuration |
| `plugins` | Install and run `*.kb-plugin` / `*.nb-plugin` |
| `import` / `export` / `run` / `shell` / `completions` | Odds and ends |
| `migrate` / `pick` / `tags` / `reconcile` | `kb` additions (see below) |

Filters shared by the commands that select sets of notes:

```
-n, --notebook <NAME>   limit to one notebook
-t, --tag <TAG>         require a tag; repeat to require several
-s, --since <WHEN>      touched since a date or duration (7d, 3w, 2026-01-01)
    --limit <N>         maximum results
```

`search` also takes `-F` (literal), `-i`/`-s` (case), `-m` (matches per note),
`-l` (paths only), and `--json`.

### Beyond `nb`

- **`migrate`** — backfill frontmatter onto notes that predate it
- **`tags`** — every tag and how many notes carry it
- **`pick`** — choose a note with fzf and a `glow` preview
- **`reconcile`** — rebuild `.index` from what is actually on disk
- **Frontmatter** — notes carry `title` / `tags` / `created` / `updated`, so
  listings sort by date and filters work on metadata. `nb` reads these files
  unchanged; it just ignores the header.

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

Settings live in `.nbrc` at the knowledge base root and are managed with
`kb settings`, `kb set`, and `kb unset`. The file is the one `nb` uses, in the
same format, so either tool can read what the other wrote.

```sh
kb settings list          # every setting and its value
kb set default_extension org
kb set 5                  # settings can be named or numbered
```

Environment variables take precedence over the file:

| Variable | Effect |
| --- | --- |
| `KB_ROOT` / `NB_DIR` | Knowledge base location (default `~/.nb`) |
| `KB_NOTEBOOK` | Notebook for new notes |
| `NBRC_PATH` | Settings file location |
| `NB_*` | Any setting, e.g. `NB_ENCRYPTION_TOOL=gpg` |
| `NO_COLOR` | Disable ANSI styling |

Without `KB_NOTEBOOK`, commands act on the notebook `kb use` selected; failing
that, `work` when `~/.is_work_pc` exists and `home` otherwise — work and
personal notes live in separate repositories, and putting one in the other is
tedious to undo.

External tools are looked up on `PATH` and used when present: `git` (required
for sync and history), `fzf` (`pick`), `glow` or `bat` (rendering), `openssl` or
`gpg` (encryption).

## Structure

- **`kb-core`** — the library: notes, ids, search, git, bookmarks, todos,
  rendering, encryption, plugins
- **`kb-cli`** — the `kb` binary

Both the CLI and the `browse` web application are thin shells over `kb-core`.

## Build

```sh
cargo build --release   # target/release/kb
cargo test
```
