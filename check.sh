#!/usr/bin/env sh
#
# ward — the machine-evaluable definition of done.
#
# Every step runs even after one fails, because a run that stops at the first
# problem hides the other three. Exit status is nonzero if any step failed.

set -u

repo_root=$(git rev-parse --show-toplevel) || exit 1
cd "$repo_root" || exit 1

status=0

WORDS="check/banned-words.txt"
NAMES="${WARD_BANNED_NAMES:-.local/banned-names.txt}"

# --- banned terms ------------------------------------------------------------
#
# Two lists, stored differently on purpose.
#
#   check/banned-words.txt   Prose we don't write. Tracked, because publishing
#                            it documents the writing standard.
#
#   $NAMES                   Names of other tools. NEVER tracked. A curated
#                            enumeration of tools we police reads as a hit
#                            list, which is the problem the rule exists to
#                            prevent. Lives in .local/ (gitignored); CI
#                            supplies it via WARD_BANNED_NAMES, written to a
#                            path outside the checkout.
#
# This check never echoes either list — not on a match, not on an error, not
# on success. It reports matches only. A match is different from the list: it
# is already sitting in a tracked file, so naming it leaks nothing that the
# diff doesn't already show, and you need it to fix the line.
#
# A missing list is a hard failure. A check that silently does nothing when its
# input is absent is a green build that checked nothing.

echo "==> banned terms"

scan_files() {
    # Exclude the words list from its own scan; it contains every pattern.
    git ls-files -z | grep -zv '^check/banned-words\.txt$'
}

# The list files allow '#' comments and blank lines, so they cannot be handed
# to `grep -f` directly: a blank line there is a pattern that matches every
# line. Join them into one alternation instead and pass it as an argument —
# which also means the name list is never written to disk.
load_patterns() {
    sed -e '/^[[:space:]]*#/d' -e '/^[[:space:]]*$/d' "$1" | paste -sd '|' -
}

check_list() {
    list="$1"
    label="$2"

    if [ ! -f "$list" ]; then
        echo "    FAIL: $label list not found" >&2
        if [ "$label" = "name" ]; then
            echo "          set WARD_BANNED_NAMES, or create .local/banned-names.txt" >&2
        fi
        return 1
    fi

    pattern=$(load_patterns "$list")

    if [ -z "$pattern" ]; then
        echo "    FAIL: $label list has no patterns" >&2
        return 1
    fi

    hits=$(scan_files | xargs -0 -r grep -I -n -i -E -e "$pattern" -- 2>/dev/null)

    if [ -n "$hits" ]; then
        printf '%s\n' "$hits" | sed 's/^/    /' >&2
        echo "    FAIL: banned $label found" >&2
        return 1
    fi

    return 0
}

check_list "$WORDS" "word" || status=1
check_list "$NAMES" "name" || status=1

[ "$status" -eq 0 ] && echo "    ok"

exit "$status"
