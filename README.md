# Ward

A voice companion for Elite Dangerous. Hold a key, say what you want, and hear the answer —
in the cockpit, in the headset, without tabbing out of the game.

## Ward does not run yet

There is nothing installable here. The repository is at its beginning and is being built in
the open, so the issue tracker is currently more informative than the code.

If you arrived looking for something to use, the first release to try will be **0.1.0**, and
the first release worth recommending will be **1.0.0**. Both are milestones on the tracker.

## What it is meant to do

Answer questions about where you are and what you are flying, from what the game writes to
disk — so an answer is grounded in your actual session rather than in what a language model
remembers about the game. When Ward cannot reach something it needs, it says so plainly
instead of guessing.

Press keys on your behalf for the things that are timing and tedium: the discovery scanner on
arrival, landing gear, lights, power distribution, panels and fire groups.

Show all of this on a panel you can read inside VR, with captions for anything spoken, so the
headset never has to come off.

### Where the line is

**Timing and tedium, never aiming or deciding.**

Permanently out of scope: autopilot, fire control, and anything that selects or tracks a
target. That is a boundary set by Frontier's terms, not a gap waiting to be filled.

## Milestones

| Milestone | What it means |
|---|---|
| [0.1.0](https://github.com/dseelinger/ward/milestone/1) | The first installable release. A voice and text turn, journal awareness, VR, automatic honk, a checklist, settings, and an installer. |
| [1.0.0](https://github.com/dseelinger/ward/milestone/2) | The first release that ships without a caveat attached. |
| [Future](https://github.com/dseelinger/ward/milestone/3) | Wanted, not scheduled. |

## Working on Ward

Enable the commit hook after cloning. Git will not do this for you, and without it the
commit-message check does not run:

```sh
git config core.hooksPath .githooks
```

Run the checks:

```sh
sh check.sh
```

`check.sh` reads a list from `.local/banned-words.txt`, which is not in this repository. It
fails without one rather than passing quietly, because a check that does nothing when its
input is missing is a green build that checked nothing. Contributors do not need this file —
the same check runs in CI on merge.

## License

MIT. See [LICENSE](LICENSE).
