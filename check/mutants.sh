#!/usr/bin/env sh
#
# Ward — the mutation score, and the floor under it.
#
# The coverage floors say a line ran. They cannot say whether anything would
# object to that line behaving differently, and check/coverage.sh names that gap
# in its own header. This is the instrument that can see it: the code is changed
# on purpose and the suite is expected to go red.
#
# Slow by construction - one build and one test run per mutant - so this is not
# on the path a push takes. It runs on a schedule and on demand.

set -u

repo_root=$(git rev-parse --show-toplevel) || exit 1
cd "$repo_root" || exit 1

# The pure tier, which is where this is least ambiguous: those files need
# nothing but the process they run in, so a survivor is a real hole rather than
# something the runner could not reach.
FILES="src/bindings.rs src/captions.rs src/outside.rs src/schema.rs \
       src/shown.rs src/speech.rs src/sse.rs"

# Five points under where it stood when the floor went in, which was every
# killable mutant caught. The gap is there so that one survivor is a thing you
# read about in the output rather than a build somebody has to stop and fix, and
# so that a file arriving with a few genuinely unkillable mutants in it does not
# turn the build red on its first day.
#
# A timeout counts against the score here, deliberately. It is a real detection
# - the suite noticed by never finishing - but counting a hang as a pass is an
# argument this file would rather not be making, and the three that exist are
# comfortably inside the gap.
FLOOR=92

echo "==> changing the code to see what nobody would notice"

report=$(mktemp)
trap 'rm -f "$report"' EXIT

# shellcheck disable=SC2086
args=""
for file in $FILES; do
    args="$args --file $file"
done

# The exit status is ignored on purpose. It is nonzero when anything survived,
# and whether that fails the build is this file's decision rather than the
# tool's - a survivor inside the gap is work to do, not a stop.
# shellcheck disable=SC2086
cargo mutants -j 4 $args >"$report" 2>&1

tail -n 40 "$report"

summary=$(grep -E "mutants tested" "$report" | tail -n 1)

if [ -z "$summary" ]; then
    echo "FAIL: the run produced no summary, so nothing was measured" >&2
    exit 1
fi

# Each count is absent from the line when it is zero, so every one of these
# defaults rather than assuming the shape of a run that found everything.
count_of() {
    echo "$summary" | grep -oE "[0-9]+ $1" | grep -oE "^[0-9]+" || echo 0
}

caught=$(count_of caught)
missed=$(count_of missed)
timeouts=$(count_of timeouts)

judged=$((caught + missed + timeouts))

if [ "$judged" -eq 0 ]; then
    echo "FAIL: no mutant was judged either way" >&2
    exit 1
fi

# Integer arithmetic, rounding down. A floor met only by rounding up is a floor
# that moved.
score=$((caught * 100 / judged))

echo
echo "    caught     $caught"
echo "    survived   $missed"
echo "    timed out  $timeouts"
echo "    score      $score% (floor $FLOOR%)"

if [ "$missed" -gt 0 ]; then
    echo
    echo "    Survivors, which are the work queue rather than a number:"
    grep -E "^MISSED" "$report" | sed 's/ in [0-9]*s build.*//' | sed 's/^/    /'
    echo
    echo "    Killing one is a reason to write a test. Where one genuinely"
    echo "    cannot be killed, say why in check/mutants.md."
fi

if [ "$score" -lt "$FLOOR" ]; then
    echo
    echo "FAIL: $score% is below the floor of $FLOOR%" >&2
    exit 1
fi

echo "    ok"
