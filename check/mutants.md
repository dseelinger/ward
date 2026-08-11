# What survives having the code changed underneath it

The coverage floors say a line ran. They cannot say whether anything would
notice that line behaving differently, and `check/coverage.sh` names that gap in
its own header while carrying no instrument that can see it. This is the
instrument: the code is changed on purpose and the suite is expected to go red.

Run by `.github/workflows/mutants.yml`, weekly and on demand. Locally:

```
cargo mutants -j 4 --file src/bindings.rs --file src/captions.rs \
  --file src/outside.rs --file src/schema.rs --file src/shown.rs \
  --file src/speech.rs --file src/sse.rs
```

## The score, on the pure tier

Recorded rather than held to. A number picked before anybody had watched it is a
number nobody has watched fail, which is the same mistake the coverage floors
were written to avoid — so this says where it landed and no build fails on it
yet.

| | mutants |
|---|---|
| Caught | 143 |
| Survived | 3, all equivalent — see below |
| Timed out | 3 |
| Unviable | 15 |
| **Total** | **164** |

It started at 129 caught and 17 survivors. The eleven tests written to kill the
other fourteen were worth having on their own: they were boundaries nobody had
tested, and two of them found real gaps rather than notional ones — a settings
range that had never been checked at either end, and a full speech queue whose
two outcomes are the difference between a line that will be spoken and one that
never will be.

A timeout is a detection rather than a survivor. All three are mutations that
turn a loop into one that never ends, which the suite notices by not finishing.

## The three that survive, and why they cannot be killed

An equivalent mutant is one where the changed code behaves identically on every
input that can reach it. Writing a test to kill one is impossible, and writing a
test that appears to is worse — it pins the shape of the workaround rather than
any behavior.

**`captions.rs:118` — the loop that eats spaces after a heading marker.**
The function ends by normalizing whitespace: `split_whitespace().join(" ")`.
Whether the marker's trailing spaces are eaten here or left for that line to
collapse, the output is the same string. The loop states the intent at the point
the intent applies, which is worth keeping, but it decides nothing that survives
to the caller.

**`captions.rs:200` — the first half of the guard that stops a word moving down,
twice.** The guard is `moved > PER_LINE || gap_after >= gap_now`, and the first
clause can never be the only reason it fires.

Write `t` and `b` for the two line lengths and `k` for the characters moved,
including the space. Then `moved = b + k` and the gap after the move is
`|t - b - 2k|`. Moving only narrows the gap when `t > b` and `k < t - b`. Since
a line is never longer than `PER_LINE`, that gives `k + b < t <= PER_LINE`, so
`moved < PER_LINE`. A move that overflows the lower line is therefore always
also a move that widens the gap, and the second clause has already fired.

The clause is a guard against something the arithmetic already prevents. It
stays because it is the condition a reader checks for and its absence would have
to be argued from the same algebra — but nothing can tell it from a mutation of
itself, and no test should pretend to.

The third mutation of that line, `||` to `&&`, is **not** equivalent and is
caught: a caption whose word fits below but whose gap would widen is left alone
under the real code and rearranged under the mutant.
