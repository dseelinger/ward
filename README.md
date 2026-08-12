# Ward

A voice companion for Elite Dangerous. Hold a key, say what you want, and hear the answer —
in the cockpit, in the headset, without tabbing out of the game.

## Getting it

**0.1.0 is out.** It is early, and it is enough to fly with. The installer is on the
[releases page](https://github.com/dseelinger/ward/releases): per-user, so no administrator
and no elevation prompt, and everything Ward writes stays in one folder beside the program
that an upgrade leaves alone.

Ward ships unsigned, so Windows will show a SmartScreen dialog the first time. Each release
publishes a `SHA256SUMS.txt`; checking the installer against it is the way to know you have
the file the release meant to publish.

You will need an Anthropic API key, which Ward asks for on first run and stores encrypted for
your Windows account. The first release worth recommending will be **1.0.0**, and the
repository is still being built in the open, so the
[issue tracker](https://github.com/dseelinger/ward/milestones) says more about where this is
going than the code does.

## What it does

Answers questions about where you are and what you are flying, from what the game writes to
disk — so an answer is grounded in your actual session rather than in what a language model
remembers about the game. When Ward cannot reach something it needs, it says so plainly
instead of guessing.

Presses keys on your behalf for the things that are timing and tedium. Today that is the
discovery scanner on arrival, and it is off until you switch it on, because automation that
presses your fire key should be something you chose. Landing gear, lights, power distribution,
panels and fire groups are the same mechanism and are not built yet.

Shows all of this on a panel inside the headset, with captions for anything spoken, so it
never has to come off. The panel is the window — one interface drawn in two places, so the
desktop cannot do anything the headset cannot. Summon it by voice, by hotkey, or by reaching
out and taking hold of it.

### Where the line is

**Timing and tedium, never aiming or deciding.**

Permanently out of scope: autopilot, fire control, and anything that selects or tracks a
target. That is a boundary set by Frontier's terms, not a gap waiting to be filled.

## Milestones

| Milestone | What it means |
|---|---|
| [0.1.0](https://github.com/dseelinger/ward/milestone/1) | The first installable release, and it is out. A voice and text turn, journal awareness, VR, automatic honk, a checklist, settings, and an installer. |
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
