#!/usr/bin/env sh
#
# Ward — the machine-evaluable definition of done.
#
# Every step runs even after one fails, because a run that stops at the first
# problem hides the other three. Exit status is nonzero if any step failed.

set -u

repo_root=$(git rev-parse --show-toplevel) || exit 1
cd "$repo_root" || exit 1

status=0

SHARED="check/banned-words.txt"
LOCAL="${WARD_BANNED_WORDS:-.local/banned-words.txt}"

# --- banned words ------------------------------------------------------------
#
# Two lists of words Ward does not write.
#
#   check/banned-words.txt   Style. Tracked, because publishing it documents
#                            the writing standard.
#
#   $LOCAL                   Project-specific terms. Deliberately not tracked:
#                            a published list of words you refuse to print is
#                            itself a kind of publication, and says more than
#                            the silence it is meant to keep. It lives in
#                            .local/, and CI supplies the same content through
#                            WARD_BANNED_WORDS, written outside the checkout.
#
# Neither list is ever echoed - not on a match, not on an error, not on
# success. Matches are reported in full, because a match is already sitting in
# a tracked file and you need it to fix the line.
#
# A missing list is a hard failure. A check that quietly does nothing when its
# input is absent is a green build that checked nothing.

echo "==> banned words"

scan_files() {
    # Tracked files and new ones that are not ignored. Both, because a file's
    # first commit is the one this check was least able to see: `git ls-files`
    # alone skips anything not yet added, so a new module passes locally and
    # fails in CI the moment it is committed. That happened once, over a
    # British spelling in a comment.
    #
    # Two exclusions, for different reasons.
    #
    # The shared list, because it contains every pattern and would match itself.
    #
    # Vendored files, because they are somebody else's published source and the
    # only way to satisfy this check on one would be to edit it — which would
    # defeat the point of vendoring, which is to hold the file they published.
    # A check that can fail with no legitimate fix available is a check people
    # learn to bypass.
    git ls-files -z --cached --others --exclude-standard |
        grep -zv '^check/banned-words\.txt$' |
        grep -zv '^crates/openvr-sys/vendor/'
}

# The list files allow '#' comments and blank lines, so they cannot be handed
# to `grep -f` directly: a blank line there is a pattern that matches every
# line. Join them into one alternation instead and pass it as an argument -
# which also means neither list is ever written to disk by this script.
load_patterns() {
    sed -e '/^[[:space:]]*#/d' -e '/^[[:space:]]*$/d' "$1" | paste -sd '|' -
}

check_list() {
    list="$1"

    if [ ! -f "$list" ]; then
        echo "    FAIL: no word list at $list" >&2
        echo "          set WARD_BANNED_WORDS, or create .local/banned-words.txt" >&2
        return 1
    fi

    pattern=$(load_patterns "$list")

    if [ -z "$pattern" ]; then
        echo "    FAIL: the word list at $list has no patterns" >&2
        return 1
    fi

    hits=$(scan_files | xargs -0 -r grep -I -n -i -E -e "$pattern" -- 2>/dev/null)

    if [ -n "$hits" ]; then
        printf '%s\n' "$hits" | sed 's/^/    /' >&2
        echo "    FAIL: banned word found" >&2
        return 1
    fi

    return 0
}

check_list "$SHARED" || status=1
check_list "$LOCAL" || status=1

[ "$status" -eq 0 ] && echo "    ok"

# --- the installer keeps the data folder --------------------------------------
#
# Everything the Commander owns lives in one folder beside the executable:
# their settings, their checklist, their key, their logs. An upgrade replaces
# the program and must not touch it, and an uninstall takes the program and
# leaves it behind.
#
# This is free and static, so it runs on every change rather than only on the
# path that ships. The failure it prevents is not subtle - it is somebody's
# settings and stored key disappearing under a version bump - but it is
# invisible until a second release exists, which is exactly too late.

echo "==> the installer keeps the data folder"

SETUP="installer/ward.iss"

if [ ! -f "$SETUP" ]; then
    echo "    FAIL: no installer script at $SETUP" >&2
    exit 1
fi

# The declaration has to name the folder and mark it as surviving an uninstall,
# on one line, because that is how Inno Setup reads it.
if ! grep -qE '^Name: *"\{app\}\\data".*uninsneveruninstall' "$SETUP"; then
    echo "    FAIL: $SETUP does not declare {app}\\data as surviving an uninstall" >&2
    echo "          expected a [Dirs] entry with the uninsneveruninstall flag" >&2
    status=1
fi

# And nothing may delete it on the way out. A [UninstallDelete] entry covering
# the folder would undo the flag above without contradicting it in a way
# anybody would notice while reading.
if grep -E '^Type: *(files|filesandordirs|dirifempty)' "$SETUP" | grep -q 'data'; then
    echo "    FAIL: $SETUP deletes something under the data folder on uninstall" >&2
    status=1
fi

[ "$status" -eq 0 ] && echo "    ok"

exit "$status"
