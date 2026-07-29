# kb

[日本語](README.ja.md)

A fast Markdown knowledge base. Notes are plain files in git repositories, and
`kb` speaks [`nb`](https://github.com/xwmx/nb)'s command language — same
subcommands, same aliases, same ids, same file formats — while being roughly two
orders of magnitude faster.

```sh
kb search "AI Search"       # full-text across every notebook, in ~16 ms
kb 3                        # show item 3
kb home:knowledge/12        # scope to a notebook and folder
kb new -t "Title" --folder knowledge --content - <<< "body"
kb open knowledge/          # pick with fzf, preview with glow, edit
kb sync                     # commit Markdown, pull, push
```

## Install

### Nix flake (how it is used here)

Add it as an input and put the package on your path:

```nix
# flake.nix
inputs.kb = {
  url = "github:Koutaro-Hanabusa/kb";
  inputs.nixpkgs.follows = "nixpkgs";
};

# home-manager
home.packages = [ kb.packages.${system}.default ];
```

Update with `nix flake update kb`.

### Cargo

```sh
cargo install --git https://github.com/Koutaro-Hanabusa/kb kb-cli
```

### From source

```sh
git clone https://github.com/Koutaro-Hanabusa/kb && cd kb
cargo build --release      # target/release/kb
cargo test
```

Nothing else is required to run. `git` is needed for `sync`, `status`, and
`history`; `fzf`, `glow`, `bat`, `openssl`, and `gpg` are used when present and
skipped when not. `kb env --long` reports which of them it found.

### First run

Point `kb` at an existing `nb` knowledge base and it just works — same layout,
same ids:

```sh
KB_ROOT=~/.nb kb ls         # ~/.kb is the default for new kb installations
```

Starting fresh instead:

```sh
kb init                     # creates ~/.kb/home as a git repository
```

## Speed

Measured on 811 notes / 13.5 MB, macOS, warm cache. `kb` is the mean of 50 runs;
`nb` is a single run because at these times it does not need averaging.

| | `nb` 7.25.4 | `kb` | |
| --- | ---: | ---: | ---: |
| startup only (`--version`) | 0.30 s | **0.013 s** | 23× |
| list one notebook | 1.04 s | **0.016 s** | 65× |
| search one notebook | 8.21 s | **0.016 s** | **513×** |

The data was never the problem — 13.5 MB is nothing. `nb` is a 27,000-line Bash
script that reparses itself on every invocation (hence the 0.3 s floor) and forks
`git`, `sed`, and `awk` per note while searching. During an 18.5 s search its CPU
usage was 38%: it was waiting on processes, not reading text. `kb` reads each
file once and scans it with ripgrep's matcher.

There is no index and no database. Every command walks the tree. At the size a
personal knowledge base reaches, an index would cost more in bookkeeping than it
saves.

## Layout

A knowledge base is a directory of *notebooks*; each notebook is a directory of
Markdown, normally a git repository:

```
~/.kb/                  # or $KB_ROOT
├── .current            # notebook selected by `kb use`
├── home/               # a notebook
│   ├── .index          # ids for this directory
│   └── knowledge/
│       ├── .index
│       └── *.md
└── work/
```

## Compatibility with `nb`

Existing habits, scripts, and notebooks carry over. Every `nb` subcommand
resolves, with the same short aliases.

Compatibility is a claim about behaviour, so it is checked by running both tools
over the same operations and diffing what each produced:

```sh
./scripts/compat-check.sh     # requires nb on PATH
```

It covers filenames, note bodies, id resolution and retirement, bookmark and
todo formats, `.index` / `.pindex` / `.archived`, and the settings file — 17
comparisons, all matching as of the current version.

**Item references.** Commands take `[<notebook>:][<folder>/]<id|filename|title>`:

```sh
kb 3                     # item 3 in the current notebook
kb home:knowledge/12     # scoped to a notebook and folder
kb show "My Note Title"  # or named by title
kb                       # no argument lists the current notebook
```

Ids come from the `.index` file in each directory: an item's id is its line
number. Deleting an item blanks its line rather than removing it, so ids are
never reused and a reference written down last year still points at the same
note. Verified against `nb` itself, including the gaps.

**Filenames.** A title becomes a filename the way `nb` does it: lowercase ASCII,
whitespace and `: / \ ? *` replaced by underscores, everything else left alone.
`日本語UIライティング - 句点のルール` → `日本語uiライティング_-_句点のルール.md`.

**Files.** Notes are Markdown; bookmarks are `*.bookmark.md`; todos are
`*.todo.md` with a `# [ ]` / `# [x]` heading; encrypted items are `*.enc`
(OpenSSL AES-256 with `-md sha256`, or GPG when configured). Pinned entries live
in `.pindex`, archived notebooks carry `.archived`. A note encrypted by `kb`
decrypts with `nb` and the reverse, verified both ways.

**Settings.** `kb` reads `~/.kbrc` and falls back to `~/.nbrc`, in the same
`export NAME="${NAME:-value}"` shell format, with the same twelve setting names.
Both files are *sourced*, not parsed, so they can branch at run time.

## Differences from `nb`

### Additions

- **Frontmatter.** Notes carry `title` / `tags` / `created` / `updated`, so
  listings sort by date and filters work on metadata. `nb` reads these files
  unchanged — it just ignores the header.
- **`kb migrate`** — backfill frontmatter onto notes that predate it. Insertion
  only: it never rewrites prose.
- **`kb tags`** — every tag and how many notes carry it.
- **`kb pick`** — choose a note with fzf and a `glow` preview. `kb open` and
  `kb peek` do the same when given a folder.
- **`kb reconcile`** — rebuild `.index` from what is actually on disk.
- **`--json`** on `search` and `ls`.
- **Shared filters** — `-n/--notebook`, `-t/--tag`, `-s/--since 7d`, `--limit`
  work across the commands that select sets of notes.

### Coverage

Every `nb` subcommand and sub-subcommand resolves — 44 of the latter, checked
mechanically against `nb help` rather than by eye.

Two do less than their `nb` counterparts, because there is nothing for them to
do here:

- **`env install` / `env update`** fetch assets for `nb`'s web UI. `kb browse` is
  self-contained, so `kb env check` reports on external tools instead.
- **`notebooks select`** resolves a selector to its notebook and prints it.
  `nb`'s version sets the current notebook without persisting, which only ever
  meant "for the rest of this one invocation".

### Behavioural differences

- **`browse`** is a smaller web application. It renders notes, resolves
  `[[wiki links]]` and `#tags`, searches, and offers add / edit / delete — but it
  is not a reimplementation of `nb`'s 677-line embedded UI.
- **`kb open <folder>`** opens the fzf picker and then your editor. `nb` reaches
  for a file browser (`ranger`, `mc`, …). `kb show <folder>` still lists.
- **No terminal, no editor.** `kb` checks for a tty before launching `$EDITOR`
  and reports the path instead of hanging an automated session.
- **`kb sync`** stages Markdown only, and refuses to commit when something else
  is already staged, rather than sweeping it in.
- **Colour output** is minimal: paths, titles, and line numbers. No themes.

## Commands

| Command | Purpose |
| --- | --- |
| `add` (`a`, `new`, `+`) | Create a note. A bare argument is a path if it contains `/` or has an extension, else content |
| `ls` / `list` | List items |
| `search` (`q`, `grep`) | Search note bodies. Regex by default, smart case |
| `show` (`s`) / `peek` (`p`) / `open` (`o`) | Display an item, or hand it to the system |
| `edit` (`e`) | Append, prepend, or overwrite; or open `$EDITOR` |
| `delete` (`d`, `rm`) / `move` (`mv`) / `copy` (`cp`) | Manage items, keeping ids straight |
| `bookmark` (`bm`) | Bookmark a URL, fetching its title |
| `todo` (`todos`, `tasks`) / `do` / `undo` | Todos and their checkboxes |
| `pin` / `unpin` / `archive` / `unarchive` | Ordering and notebook state |
| `browse` | The embedded web app |
| `notebooks` / `use` (`u`) / `count` / `folders` | Notebooks and structure |
| `sync` / `status` / `git` / `history` / `remote` | Git |
| `settings` / `set` / `unset` / `env` | Configuration |
| `plugins` | Install and run `*.kb-plugin` / `*.nb-plugin` |
| `import` / `export` / `run` / `shell` / `completions` | Odds and ends |
| `migrate` / `pick` / `tags` / `reconcile` | Additions (see above) |

Filters shared by the commands that select sets of notes:

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
them, deriving each field mechanically:

- **title** — the first heading or line of prose, condensed at the first
  sentence break if it runs long; the filename as a last resort
- **tags** — the note's top-level directory
- **created** — an `nb`-style timestamp filename, else the first commit
- **updated** — the last commit touching the file

It never rewrites prose. Existing frontmatter keys are left byte-for-byte intact
and only missing ones are appended, so the diff is pure insertion. The run is a
preview until you pass `--apply`, and it refuses a dirty working tree unless you
pass `--allow-dirty`. Migrating 797 notes here produced 5,569 insertions and zero
deletions.

## Configuration

Settings live in `~/.kbrc` and are managed with `kb settings`, `kb set`, and
`kb unset`. `~/.nbrc` is read as a fallback, so an existing `nb` configuration
keeps working; `kb` writes only to its own file.

```sh
kb settings list          # every setting and its value
kb set default_extension org
kb set 5                  # settings can be named or numbered
```

The file is sourced, so it can decide at run time:

```bash
# ~/.kbrc
if [[ -n "${CLAUDECODE:-}" ]]; then
  export KB_EDITOR="cat"   # never block on an editor in an automated session
else
  export KB_EDITOR="nvim"
fi
```

Environment variables take precedence over the file:

| Variable | Effect |
| --- | --- |
| `KB_ROOT` | Knowledge base location (default `~/.kb`) |
| `KB_NOTEBOOK` | Notebook for new notes |
| `KBRC_PATH` / `NBRC_PATH` | Settings file locations |
| `KB_*` / `NB_*` | Any setting, e.g. `KB_ENCRYPTION_TOOL=gpg` |
| `NO_COLOR` | Disable ANSI styling |

Without `KB_NOTEBOOK`, commands act on the notebook `kb use` selected; failing
that, `work` when `~/.is_work_pc` exists and `home` otherwise — work and personal
notes live in separate repositories, and putting one in the other is tedious to
undo.

## Structure

- **`kb-core`** — the library: notes, ids, search, git, bookmarks, todos,
  rendering, encryption, plugins
- **`kb-cli`** — the `kb` binary

Both the CLI and the `browse` web application are thin shells over `kb-core`.

## Licence and attribution

`kb` is licensed under the **GNU Affero General Public License v3.0 or later**
(see [LICENSE](LICENSE)).

It reimplements the command surface of [`nb`](https://github.com/xwmx/nb) by
William Melody, which is itself AGPLv3. No `nb` code was copied — `kb` is written
from scratch in Rust — but it deliberately reproduces `nb`'s commands,
identifiers, and on-disk formats so the two interoperate, and one detail (the
exact OpenSSL invocation used for encrypted notes, needed to decrypt existing
`.enc` files) was taken from reading `nb`'s source. `kb` is therefore licensed
under the same terms as the work it is compatible with.

Thanks to William Melody for `nb`, which this replaces only because 811 notes
made its startup cost impossible to ignore.
