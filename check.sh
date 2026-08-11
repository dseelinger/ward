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

# --- every decision has a state ----------------------------------------------
#
# A decision is enforced, accepted as unenforced, or open. There is no fourth
# state, and this is what fails on one.
#
# The failure it prevents is not somebody arguing for an unenforced rule. It is
# a decision quietly ceasing to be true with nothing noticing - which is what
# happened to every decision that did not fall out as a capability, because the
# decisions lived outside the repository and nothing in here knew they existed.
#
# This check reads the record, not another check. It validates that decisions
# have owners, which is the one thing the third rule of repository discipline
# makes an exception for. It does not grow a companion proving it can fail.

echo "==> every decision has a state"

DECISIONS="docs/decisions.md"

if [ ! -f "$DECISIONS" ]; then
    echo "    FAIL: no decision record at $DECISIONS" >&2
    status=1
else
    # One pass: count the state lines under each decision heading, and report
    # any heading that does not have exactly one.
    bad=$(awk '
        /^## D[0-9]+ / {
            if (heading != "" && states != 1) print heading " has " states " states"
            heading = $2
            states = 0
            next
        }
        /^> (enforced|accepted|open) / { if (heading != "") states++ }
        END { if (heading != "" && states != 1) print heading " has " states " states" }
    ' "$DECISIONS")

    if [ -n "$bad" ]; then
        printf '%s\n' "$bad" | sed 's/^/    /' >&2
        echo "    FAIL: every decision needs exactly one state line" >&2
        echo "          '> enforced - what fails', '> accepted - why not', or" >&2
        echo "          '> open - the issue'" >&2
        status=1
    else
        echo "    ok ($(grep -c '^## D[0-9]* ' "$DECISIONS") decisions)"
    fi
fi

# --- the documentation ---------------------------------------------------------
#
# Two checks, both cheap and both static.
#
# The first is that every page under docs/ is in the summary. The builder only
# builds what the summary lists, which is the whole reason for choosing it: a
# page that is not listed is not published and cannot be stumbled into. The
# hazard that leaves is the opposite one - a page somebody wrote, meant to
# publish, and which silently is not there.
#
# The second is that every capability page quotes something real: at least one
# code block. It is a mechanical proxy for a rule a check cannot see, which is
# that a page is written from artifacts rather than from the feature name. A
# page that cannot cite a setting, a line of output or a real value is a page
# written from the idea of the feature, and those are the ones that turn out to
# describe something the code does not do.

echo "==> the documentation"

DOCS="docs"
SUMMARY="$DOCS/SUMMARY.md"

if [ ! -f "$SUMMARY" ]; then
    echo "    FAIL: no summary at $SUMMARY" >&2
    status=1
else
    for page in $(find "$DOCS" -name '*.md' ! -name 'SUMMARY.md' | sed "s|^$DOCS/||" | sort); do
        if ! grep -qF "($page)" "$SUMMARY"; then
            echo "    FAIL: $DOCS/$page is not in the summary, so nothing publishes it" >&2
            status=1
        fi
    done

    # And the other direction: a summary naming a page that is not there builds
    # a link to nothing.
    for linked in $(grep -oE '\]\([^)]+\.md\)' "$SUMMARY" | tr -d '](' | tr -d ')'); do
        if [ ! -f "$DOCS/$linked" ]; then
            echo "    FAIL: the summary names $linked, which does not exist" >&2
            status=1
        fi
    done
fi

for page in "$DOCS"/capabilities/*.md; do
    [ -e "$page" ] || continue

    if ! grep -q '^```' "$page"; then
        echo "    FAIL: $page quotes nothing real" >&2
        echo "          every capability page needs at least one code block: a" >&2
        echo "          setting, a line of output, something the code produces" >&2
        status=1
    fi
done

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

# --- the engine does not know about the window --------------------------------
#
# Ward is worn rather than watched, so the turn has to run whether or not
# anything is drawing it. It did not: the turn lived inside the window's paint
# function, and a minimized window does not paint, so Ward went deaf the moment
# it was out of the way - along with the journal, and everything that reacts to
# it.
#
# The engine now runs on its own and wakes the window through a closure it was
# handed. This keeps it that way. The failure mode being guarded against is not
# somebody arguing for the dependency - it is somebody reaching for `ctx`
# because it is what every other file has, and nothing complaining until a
# Commander is in a headset wondering why holding the key does nothing.

echo "==> the engine does not know about the window"

ENGINE="src/engine.rs"

if [ ! -f "$ENGINE" ]; then
    echo "    FAIL: no engine at $ENGINE" >&2
    echo "          if it moved, move this check with it" >&2
    status=1
elif grep -n -E '\b(egui|eframe)\b' "$ENGINE" >&2; then
    echo "    FAIL: $ENGINE names the window toolkit" >&2
    echo "          the engine must run with no window; wake one through the" >&2
    echo "          callback it was handed instead" >&2
    status=1
else
    echo "    ok"
fi

# --- a silenced test says why -------------------------------------------------
#
# A muted test is worse than one that was never written. It reads as coverage,
# it is counted in the run, and it teaches whoever finds it that a green build
# with a hole in it is normal.
#
# The rule is not that nothing may be ignored. src/outside.rs already tells a
# test that genuinely needs the wire to hold a permit and ignore itself, and a
# test needing hardware no runner has is a real thing. The rule is that the
# reason is written where the mute is, the way an exempt file in
# check/coverage.txt carries one and a decision in the record carries a state.
#
# So a bare #[ignore] fails and #[ignore = "..."] passes. The attribute has to
# start the line, because the same text appears inside the failure message in
# src/outside.rs that tells you to write it.

echo "==> a silenced test says why"

muted=$(grep -rn --include='*.rs' -E '^[[:space:]]*#\[ignore\]' src tests 2>/dev/null)

if [ -n "$muted" ]; then
    printf '%s\n' "$muted" | sed 's/^/    /' >&2
    echo "    FAIL: these tests are silenced and do not say why" >&2
    echo "          write #[ignore = \"reason\"], or delete the test" >&2
    status=1
else
    echo "    ok"
fi

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
