#!/usr/bin/env bash
#
# Run the same operations through nb and kb, then compare what each produced.
#
# Compatibility is a claim about behaviour, so it is checked by running both
# tools rather than by reading either one. Requires `nb` on PATH.
#
#   ./scripts/compat-check.sh
#   KB=./target/release/kb NB=/usr/local/bin/nb ./scripts/compat-check.sh
#
set -uo pipefail

KB="${KB:-kb}"
NB="${NB:-nb}"

command -v "$KB" >/dev/null || { echo "kb not found: $KB" >&2; exit 2; }
command -v "$NB" >/dev/null || { echo "nb not found: $NB — skipping" >&2; exit 0; }

NBDIR=$(mktemp -d)/nb
KBDIR=$(mktemp -d)/kb
export EDITOR=cat

nb_() { NB_DIR="$NBDIR" NBRC_PATH="$NBDIR/.nbrc" "$NB" "$@" 2>/dev/null; }
kb_() { KB_ROOT="$KBDIR" KBRC_PATH="$KBDIR/.kbrc" "$KB" "$@" 2>/dev/null; }

pass=0; fail=0
check() { # check <label> <expected> <actual>
  if [[ "$2" == "$3" ]]; then
    printf '  ✓ %s\n' "$1"; pass=$((pass+1))
  else
    printf '  ✗ %s\n      nb: %q\n      kb: %q\n' "$1" "$2" "$3"; fail=$((fail+1))
  fi
}

nb_ init >/dev/null; kb_ init >/dev/null

echo "── filenames and content ──"
nb_ add -t "UPPER lower - dash_under.dot" -c "body" >/dev/null
kb_ add -t "UPPER lower - dash_under.dot" -c "body" >/dev/null
nb_ add -t "日本語UIライティング - 句点のルール" -c "body" >/dev/null
kb_ add -t "日本語UIライティング - 句点のルール" -c "body" >/dev/null
nb_ add "content only, no extension" >/dev/null
kb_ add "content only, no extension" >/dev/null
nb_ add note.md -c "named" >/dev/null
kb_ add note.md -c "named" >/dev/null
nb_ folders add knowledge >/dev/null; kb_ folders add knowledge >/dev/null
nb_ add knowledge/ -c "in folder" >/dev/null
kb_ add knowledge/ -c "in folder" >/dev/null
nb_ add knowledge/noext -c "no extension" >/dev/null
kb_ add knowledge/noext -c "no extension" >/dev/null

# Timestamp names differ by construction; compare the set of non-timestamp names.
names() { find "$1" -type f -not -path '*/.git/*' -not -name '.*' \
  | sed "s|$1/||" | grep -vE '^[0-9]{14}' | sort; }
check "filenames" "$(names "$NBDIR/home")" "$(names "$KBDIR/home")"

body() { # body <dir> <relative path> — the note without frontmatter
  sed '/^---$/,/^---$/d' "$1/$2" | sed '/^$/d'
}
check "note body (titled)" \
  "$(body "$NBDIR/home" "upper_lower_-_dash_under.dot.md")" \
  "$(body "$KBDIR/home" "upper_lower_-_dash_under.dot.md")"
check "note body (extensionless)" \
  "$(body "$NBDIR/home" "knowledge/noext")" \
  "$(body "$KBDIR/home" "knowledge/noext")"

echo "── ids ──"
for ref in 1 2 3 knowledge/1; do
  check "show home:$ref --filename" \
    "$(nb_ show "home:$ref" --filename)" "$(kb_ show "home:$ref" --filename)"
done
check "count home:" "$(nb_ count home:)" "$(kb_ count home:)"

echo "── id retirement ──"
nb_ delete 2 --force >/dev/null; kb_ delete 2 --force >/dev/null
nb_ add -t "After delete" -c "x" >/dev/null
kb_ add -t "After delete" -c "x" >/dev/null
check ".index after delete+add" \
  "$(sed 's/[0-9]\{14\}\.md/TS/' "$NBDIR/home/.index")" \
  "$(sed 's/[0-9]\{14\}\.md/TS/' "$KBDIR/home/.index")"
check "retired id is gone" \
  "$(nb_ show home:2 --filename || echo notfound)" \
  "$(kb_ show home:2 --filename || echo notfound)"

echo "── bookmarks ──"
nb_ bookmark "https://example.com/x" --no-request --title "Full" \
  -c "COMMENT" -q "QUOTE" -t a,b -r "https://rel.example" >/dev/null
kb_ bookmark "https://example.com/x" --no-request --title "Full" \
  -c "COMMENT" -q "QUOTE" -t a,b -r "https://rel.example" >/dev/null
check "bookmark body" \
  "$(body "$NBDIR/home" "$(cd "$NBDIR/home" && ls *.bookmark.md)")" \
  "$(body "$KBDIR/home" "$(cd "$KBDIR/home" && ls *.bookmark.md)")"

echo "── todos ──"
nb_ todo add "買い物に行く" >/dev/null; kb_ todo add "買い物に行く" >/dev/null
nbtodo=$(cd "$NBDIR/home" && ls *.todo.md); kbtodo=$(cd "$KBDIR/home" && ls *.todo.md)
check "todo body" "$(body "$NBDIR/home" "$nbtodo")" "$(body "$KBDIR/home" "$kbtodo")"
nb_ do "$nbtodo" >/dev/null; kb_ do "$kbtodo" >/dev/null
check "todo after do" "$(body "$NBDIR/home" "$nbtodo")" "$(body "$KBDIR/home" "$kbtodo")"

echo "── state files ──"
nb_ pin note.md >/dev/null; kb_ pin note.md >/dev/null
check ".pindex" "$(cat "$NBDIR/home/.pindex" 2>/dev/null)" "$(cat "$KBDIR/home/.pindex" 2>/dev/null)"
nb_ notebooks add archived-nb >/dev/null; kb_ notebooks add archived-nb >/dev/null
nb_ notebooks archive archived-nb >/dev/null; kb_ notebooks archive archived-nb >/dev/null
check ".archived marker" \
  "$([[ -f "$NBDIR/archived-nb/.archived" ]] && echo present)" \
  "$([[ -f "$KBDIR/archived-nb/.archived" ]] && echo present)"

echo "── settings ──"
nb_ set default_extension org >/dev/null; kb_ set default_extension org >/dev/null
check "settings get" "$(nb_ settings get default_extension)" "$(kb_ settings get default_extension)"
check "rc export line" \
  "$(grep -o 'NB_DEFAULT_EXTENSION[^ ]*' "$NBDIR/.nbrc" | head -1)" \
  "$(grep -o 'KB_DEFAULT_EXTENSION[^ ]*' "$KBDIR/.kbrc" | head -1 | sed 's/KB_/NB_/g')"

echo
printf 'passed %d, failed %d\n' "$pass" "$fail"
[[ $fail -eq 0 ]]
