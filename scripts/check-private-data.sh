#!/bin/sh
# Refuses to let real drill data into Git.
#
#   scripts/check-private-data.sh             check the staged index (pre-commit)
#   scripts/check-private-data.sh --tracked   check every tracked file
#   scripts/check-private-data.sh --paths F…  check the given working-tree files
#
# A file is rejected when ANY of these hold, regardless of its name:
#   * a private extension (.3du, .3dudb, .dots, .fieldwork, .om, .omz, audio, archives…)
#   * a binary signature: 3DUD, SQLite, ZIP, gzip, Ogg, RIFF, ID3, FLAC
#   * an extracted drill dataset (JSON/text containing per-performer coordinates)
#   * a string from the optional, untracked .private-denylist (one per line)
#   * larger than 1 MiB
# Exit status is 1 when something is rejected. Nothing is ever modified.

set -u

mode=staged
if [ "${1:-}" = "--tracked" ]; then mode=tracked; shift
elif [ "${1:-}" = "--paths" ]; then mode=paths; shift
fi

root=$(git rev-parse --show-toplevel 2>/dev/null) || { echo "check-private-data: not a git repository" >&2; exit 2; }
cd "$root" || exit 2

max_bytes=1048576
failures=0
tmp=$(mktemp 2>/dev/null || echo "${TMPDIR:-/tmp}/cpd.$$")
trap 'rm -f "$tmp"' EXIT INT TERM

reject() {
    echo "BLOCKED: $1 — $2" >&2
    failures=$((failures + 1))
}

# Write the content being checked to $tmp: the staged blob, or the working-tree file.
load() {
    case $mode in
        staged) git cat-file blob ":$1" > "$tmp" 2>/dev/null ;;
        *) cat "$1" > "$tmp" 2>/dev/null ;;
    esac
}

check_file() {
    f=$1
    lower=$(printf '%s' "$f" | tr '[:upper:]' '[:lower:]')

    case $lower in
        *.3du|*.3du.*|*.3dudb|*.3dudb.*|*.3dz|*.dots|*.dots-*|*.fieldwork|*.fieldworks|*.fieldwork.*|*.fieldworks.*|\
        *.om|*.omz|*.omt|*.musicxml|*.mxl|*.ogg|*.mp3|*.wav|*.m4a|*.aif|*.aiff|*.flac|*.zip|*.7z|*.rar|*.tar|*.tgz|*.gz|\
        *.pkl|*.pickle|*.sqlite|*.sqlite3|*.db)
            reject "$f" "private or binary drill-related extension"; return ;;
    esac
    case $lower in
        *_extracted*|*extracted.json|*intermediate*.json|*.drill.json)
            reject "$f" "name matches an extracted drill dataset"; return ;;
    esac

    load "$f" || return 0
    size=$(wc -c < "$tmp" | tr -d ' ')
    if [ "$size" -gt "$max_bytes" ]; then
        reject "$f" "larger than 1 MiB ($size bytes); real drills and media are large"
        return
    fi

    magic=$(head -c 16 "$tmp" | od -An -tx1 | tr -d ' \n')
    case $magic in
        33445544*) reject "$f" "3DUD (.3du drill) signature"; return ;;
        53514c69746520666f726d6174203300*) reject "$f" "SQLite database (.dots / .fieldwork show data)"; return ;;
        504b0304*|504b0506*) reject "$f" "ZIP archive (.fieldwork, extraction archive)"; return ;;
        1f8b*) reject "$f" "gzip data (.omz)"; return ;;
        4f676753*|52494646*|494433*|664c6143*) reject "$f" "audio file"; return ;;
    esac

    # Extracted datasets. Source code and docs legitimately mention these keys, so only
    # data-like files are inspected, and synthetic fixtures must say so explicitly.
    case $lower in
        *.rs|*.md|*.sh|*.ps1|*.toml|*.yml|*.yaml|*.h|*.swift|*.txt.md) ;;
        *)
            if grep -a -q -E '"(positions_xy|count_times_seconds|source_magic|marcherId|xSteps|ySteps)"' "$tmp" \
                || grep -a -q -E 'org\.fieldwork\.show|<score-partwise' "$tmp"; then
                case $f in
                    tests/fixtures/synthetic/*)
                        grep -a -q '"x-synthetic-fixture": *true' "$tmp" \
                            || reject "$f" "drill dataset in fixtures without \"x-synthetic-fixture\": true"
                        ;;
                    *) reject "$f" "looks like extracted or exported drill data" ;;
                esac
                return
            fi
            ;;
    esac

    if [ -f .private-denylist ]; then
        while IFS= read -r word || [ -n "$word" ]; do
            case $word in ''|'#'*) continue ;; esac
            if grep -a -q -i -F -- "$word" "$tmp"; then
                reject "$f" "contains a term from .private-denylist"
                return
            fi
        done < .private-denylist
    fi
}

case $mode in
    staged) list=$(git diff --cached --name-only --diff-filter=ACMR) ;;
    tracked) list=$(git ls-files) ;;
    paths) list=$(printf '%s\n' "$@") ;;
esac

old_ifs=$IFS
IFS='
'
for f in $list; do
    IFS=$old_ifs
    [ -n "$f" ] && check_file "$f"
    IFS='
'
done
IFS=$old_ifs

if [ "$failures" -gt 0 ]; then
    echo "check-private-data: $failures file(s) blocked. Keep real drills outside the repository (see docs/PRIVACY.md)." >&2
    exit 1
fi
echo "check-private-data: OK ($mode)"
