# What survives having the code changed underneath it

The coverage floors say a line ran. They cannot say whether anything would
notice that line behaving differently, and `check/coverage.sh` names that gap in
its own header while carrying no instrument that can see it. This is the
instrument: the code is changed on purpose and the suite is expected to go red.

Run by `.github/workflows/mutants.yml`, weekly and on demand, or `sh
check/mutants.sh` on any machine. Slow by construction — one build and one test
run per mutant — so it is not on the path a push takes.

## Where it stands, on the pure tier

| | mutants |
|---|---|
| Caught | 145 |
| Survived | 0 |
| Timed out | 3 |
| Unviable | 15 |
| **Score** | **97%**, floor 92% |

A timeout counts against the score rather than for it. All three are mutations
that turn a loop into one that never ends, which the suite does detect — by
never finishing — but counting a hang as a pass is an argument this file would
rather not make, and three of them cost two points out of a five point gap.

Unviable mutants did not compile and are counted neither way.

## How it got here, including the part that was wrong

It began at 129 caught and 17 survivors. Thirteen tests later there are none.

The tests were worth writing for their own sake rather than for the number. Two
found real gaps: a settings range that had never been checked at either end, so
a Commander could have been refused the exact value the help text told them was
allowed; and a full speech queue, whose two outcomes are the difference between
a line about to be spoken and one that never will be. The rest are the edges of
caption layout, where which character falls on which line is the whole feature.

**Three survivors were written up here as equivalent mutants — unkillable by any
honest test — and two of them were not.** The claim rested on a proof that a
guard in `bottom_heavy` could never be the only reason a word stayed put, and
the proof assumed no line is longer than the allowance. That is true of every
line the caller produces. It is not true of the function, which is handed lines
by a caller today and has its own promise to keep: that the lower line fits
whatever it is given. The guard exists for exactly the input the current caller
cannot produce, and testing a defense against the case it defends against is
ordinary rather than contrived.

The lesson is not about mutation testing. It is that a proof of unreachability
is only as good as its assumptions, and the assumption doing the work was never
written down — which is precisely how a guard becomes untested and then, later,
wrong.

**The third was genuinely equivalent, and the answer was to delete the code.** A
loop ate the spaces after a heading marker, and the last line of that function
collapses runs of whitespace, so eating them there and leaving them produced the
same string. Nothing could tell the loop from its own absence. That is a finding
rather than a line to keep, and the same rule this project already applies to an
assertion that is true by construction.

## When a survivor appears

It is the work queue rather than a number. Killing one is a reason to write a
test; raising a percentage is not.

The floor sits five points under a clean sweep so that one survivor is something
read in the output rather than a build somebody has to stop and fix. Where one
genuinely cannot be killed, write down why here — and, before doing that, check
what the argument is assuming. The last two entries on this page were wrong.
