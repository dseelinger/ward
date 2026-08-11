# Decisions

What Ward decided, and what holds each decision in place.

Every entry carries exactly one state. There is no fourth, and `check.sh` fails
on a decision that has none:

- **enforced** — something fails when this drifts, and the entry names it.
- **accepted** — nothing enforces it, and the entry says why that is all right.
- **open** — decided and not built. The entry carries the issue.

A decision is only **enforced** when the whole of it is. A decision half held by
a check is **open**, with the entry saying which half, because the useful
question is what a green build actually checked.

This file exists because the decisions used to live somewhere else entirely.
Nothing in the repository knew they existed, so nothing could notice one
quietly stop being true, and the work that did not happen to fall out as a
capability had nothing scheduling it. The state markers are the fix: adding a
decision now costs either a check or a written admission that nothing enforces
it, which is most of what keeps this file honest.

---

## D1 — Rust, in one process

The turn, the capabilities, the game state, the interface and the headset
overlay are one binary. One concurrency model, a compiler acting as a second
reviewer, and no runtime to install.

> accepted — structural, and visible in every file. There is nothing here that
> can drift without the build stopping.

## D2 — The headset is core, and is not built first

The overlay ships, and it gets built once there is a turn worth putting in
front of the Commander. The failure to avoid is a surface with no substance
behind it.

> accepted — a sequencing decision, already honored. A check would be checking
> the past.

## D3 — No add-in channel

Ward does not host third-party extensions. Every behavior that would have
depended on one depended on another product's private and undocumented surface.

> accepted — a decision not to build something. There is nothing to inspect.

## D4 — One widget tree, drawn to the window and to the headset

No second interface, and no browser embedded to draw one. The window cannot be
more capable than the headset, because there is one implementation of both.

> accepted — nothing checks that a second surface is not added. It would be a
> large and obvious change rather than a drift, and a check that fires on
> nothing is one people learn to ignore.

## D5 — Three layers in the headset

Captions on their own layer, output only and ephemeral. A small panel that can
be glanced at. A full panel at parity with the window.

> open — captions ship; the panels do not.
> [#13](https://github.com/dseelinger/ward/issues/13)

## D6 — The big panel answers to voice, hotkey and controller

Summoned and dismissed three ways, because a Commander in a headset should
never have to reach for the one that is inconvenient.

> open — [#13](https://github.com/dseelinger/ward/issues/13)

## D7 — Reachable without a microphone or a speaker

Typing a question and reading the answer stays possible. Accessibility is
tracked debt rather than an assumption, because the toolkit's support for it is
well behind what a browser offers.

> accepted — a test holds the typed path open: Ward completes a turn with no
> microphone at all, which is what a Commander without one has rather than one
> that fails. Screen reader support is deliberately out of scope, decided by
> the maintainer, and recorded here so it is not reopened by accident.

## D8 — Input is a gate, output is one stream of speech acts

A held key, continuous listening and a wake word are three policies over one
audio gate, not three architectures. Everything Ward says goes onto one stream
carrying speaker, register, priority and whether it may be cut off, and
captions render from that stream rather than beside it.

> accepted — the stream and its arbiter are built and tested, and the reply is
> the only producer into it so far. What is unproven is the claim that one
> arbiter serves every voice, and that cannot be tested until a second voice
> exists.

## D9 — Hold a key to talk

One input policy to start with.

> accepted — built. Continuous listening and a wake word are their own issues
> and do not replace this.

## D10 — Duck and gate, and cancel the echo later

Pressing the key stops Ward mid-word and the microphone ignores what the room
gives back. Cancelling Ward's own voice out of the input is the unlock for
listening without a key held, and is separate work.

> open — ducking and gating are built. The cancellation is
> [#105](https://github.com/dseelinger/ward/issues/105).

## D11 — Take the free latency wins, and make speed a setting

In order of what they are worth: how hard the model thinks per turn, warming
the cache at startup, a right-sized transcription model, and speaking the
answer in pieces as it is written.

> open — speaking in pieces is built and measured. Thinking depth is
> [#88](https://github.com/dseelinger/ward/issues/88), the transcription model
> is [#89](https://github.com/dseelinger/ward/issues/89) with the card as
> [#127](https://github.com/dseelinger/ward/issues/127), and the cache warm is
> not yet anywhere.

## D12 — Every event carries a monotonic offset

Wall clock lines an event up against something outside the process. The offset
survives the clock being adjusted underneath a running session. Both are
printed, from the first commit, because timing is not something that can be
retrofitted onto an ordered list.

> enforced — `cargo test`, the clock tests in `src/diag.rs`, and every log line
> carries it by construction.

## D13 — Never buy latency by turning thinking off

With reasoning disabled the model can write a tool call into its visible text
instead of making one. The turn succeeds, nothing runs, nothing errors, and
Ward says it did something it did not do.

> accepted — nothing offers the setting today, so there is nothing to check.
> The check belongs with [#88](https://github.com/dseelinger/ward/issues/88),
> which is where the option would be added.

## D14 — Live game state rides in a trailing operator message

Not folded into what the Commander said. It preserves the cached prefix and it
is the channel that untrusted text cannot reach.

> accepted — built, and covered by the client's tests. What is not checked is
> that a future capability keeps using it rather than appending to the question.

## D15 — Two model providers, and one of them free to try

One first-party path and one implementation of the common protocol, which
reaches every service and every local server speaking it. The free path is the
reason, not reach.

> open — the first-party path ships. The second implementation does not exist,
> so the seam it is supposed to prove is unproven.

## D16 — Other Commanders are meant to use this

Public from the first commit, for use rather than revenue. It is why free
paths, first-run setup and documentation are real requirements rather than
polish.

> accepted — a goal that shapes other decisions. There is nothing for a check
> to read.

## D17 — Never positioned against anything else

Ward says what it is for and never what something else is not. No comparison,
no naming, in any file or any commit message.

> enforced — `check.sh` scans every tracked and untracked file against two word
> lists, and `.githooks/commit-msg` covers the surface a file scan cannot see.

## D18 — A free voice and a premium one

The free voice is half of what lets somebody try Ward without paying for
anything, and it stays.

> open — the free voice ships. The premium one is
> [#119](https://github.com/dseelinger/ward/issues/119).

## D19 — Silent about where the reasoning came from

The record explains why a decision was taken without naming what taught it.
Silence is the reversible direction: an acknowledgment can be added later and
cannot be taken out of a history.

> enforced — the same two lists and the same commit hook as D17. The list that
> matters here lives outside the repository, because a published list of words
> you refuse to print says more than the silence it keeps.

## D20 — Capabilities register once, in one list

One composition point, in declaration order, because tool-schema order is part
of the cached prefix. A capability that describes nothing does not compile, and
a capability that is not wired up fails a test rather than going unnoticed.

> enforced — `cargo test`, `every_capability_describes_itself` and the
> duplicate tool-name test in `src/capabilities.rs`, plus the trait's required
> description methods, which make the compiler the first reviewer.

## D21 — Documentation is a site, with a page per capability

Discoverable before install, which matters because other people are meant to
use this.

> open — the site is built and checked on every change, and nothing publishes
> it yet. Publishing is a repository setting the maintainer turns on, plus one
> job. The checker that kept internal pages out of a published site is not
> needed: the builder only builds what `docs/SUMMARY.md` lists, so a page that
> is not listed cannot be served, and `check.sh` holds the opposite direction —
> a page nothing lists, and a summary naming a page that is not there.
> [#122](https://github.com/dseelinger/ward/issues/122)

## D22 — Pages are written, then edited, and never re-emitted

A program that regenerates a page throws away every improvement made to it.
Four checks hold the prose to what the code actually does: every capability has
a page, documented defaults match the schema, every page quotes something real,
and the word lists apply.

> enforced — all four. `every_capability_has_a_page` and
> `documented_defaults_match_the_code` in `cargo test`, the word lists in
> `check.sh`, and every capability page must carry at least one code block —
> which is a mechanical proxy for the rule a check cannot see, that a page is
> written from artifacts rather than from the feature name. A page that cannot
> cite a setting, a line of output or a real value is one written from the idea
> of a feature, and those are the pages that describe something the code does
> not do.

## D23 — What a capability declares, and what falls out of it

Identity, the tools it offers, the settings it contributes, what it has to
show, and the page that documents it. One string is the tool argument, the name
help advertises and the field the call receives, so the three cannot drift.
Spoken help is projected from this and never asked of the model, which invents
capabilities that do not exist.

> enforced — `cargo test`, the descriptor tests in `src/capability.rs` and
> `src/capabilities.rs`. An undescribed capability fails to compile.

## D24 — Game data is generated at the build and compiled in

A rebuild is the refresh, so nothing can go stale without a signal. Being
confidently wrong about a jump range is worse than saying nothing.

> open — no dataset is generated or embedded yet.
> [#114](https://github.com/dseelinger/ward/issues/114)

## D25 — Replay the journal instead of flying somewhere

A recorded session, replayed at speed, so every journal-driven feature is
testable with no game and no headset.

> accepted — `src/replay.rs` and its fixtures exist and are used by the journal
> tests. Nothing requires a new journal feature to come with a fixture, and
> that is the part which will rot first.

## D26 — Outside data at runtime, game data at the build

Galaxy search, route planning, real distances between systems and community
goals come from community services. Anything the game itself publishes is read
locally.

> open — no service client exists.
> [#111](https://github.com/dseelinger/ward/issues/111),
> [#112](https://github.com/dseelinger/ward/issues/112),
> [#113](https://github.com/dseelinger/ward/issues/113),
> [#115](https://github.com/dseelinger/ward/issues/115)

## D27 — A grounded answer, or none

When a source cannot be reached Ward says so and offers nothing else. It never
falls back on what the model happens to remember. The rules that hold this are
static prompt material and must never be strippable by a budget or a speed
setting.

> enforced — `cargo test`, the guardrail tests in `src/anthropic.rs`. They
> assert on the serialized request rather than on the constant it is supposed
> to be made from, across every tool list and every effort, because the failure
> to guard against is not the rules being deleted but a request assembled some
> other way that quietly does not carry them. There is also no argument for
> leaving them out: a budget, a persona or a second provider can change the
> model, the tools and how hard it thinks, and none of those is a way to
> compose a request without the rules.

## D28 — The naming allowlist covers integrations only

A service Ward talks to may be named in the code that talks to it, in
configuration keys, in endpoint constants and in that integration's own setup
page. Nowhere else, and never comparatively.

> open — the allowlist currently names two services and the runtime work needs
> four. It must be widened before any code names the other two, and one dataset
> path will contain a name that was deliberately not added.
> [#111](https://github.com/dseelinger/ward/issues/111)

## D29 — Attribution lands with the dependency that needs it

The obligation is never left outstanding. Licenses are an allowlist rather than
a blocklist, because a blocklist only catches what somebody thought to name.

> enforced — the `licenses` job runs `cargo-deny` against the allowlist in
> `deny.toml`, and `NOTICE.md` exists. What is not enforced is that `NOTICE.md`
> stays current, which is why the rule is to write it with the dependency
> rather than after it.

## D30 — Timing and tedium, never aiming or deciding

Ward presses keys for the things that are tedious and never for the things that
are the game. Nothing that targets, aims or flies. Injection uses scancodes,
never a low-level keyboard hook, because that is the interface a keylogger uses
and this ships unsigned.

> accepted — no key is pressed yet beyond the discovery scanner, so there is
> nothing to check. The line has to be held by the issues that build the action
> groups, and each of them names it.

## D31 — Consent binds to the armed action, not to the passage of a turn

Confirming something must match what was armed. A gate that proves only that a
new turn happened will fire whatever is waiting.

> open — nothing arms an action yet.
> [#100](https://github.com/dseelinger/ward/issues/100)

## D32 — Memory is layered, and the journal owns what the journal knows

A stable block for how to address the Commander and how they like to be spoken
to. Standing facts the Commander can read and edit. A bounded conversation.
Live state attached per turn. Ranks, ships and locations stay out of the cached
part, where they go stale and make Ward confidently wrong about the Commander.

> open — [#106](https://github.com/dseelinger/ward/issues/106)

## D33 — Only the Commander and the journal write to memory

The model never decides on its own to remember something it read, and recalled
memory is quoted as data with its source, never spliced in as though Ward
reasoned it. One hostile sentence reaching memory otherwise replays into every
future session.

> open — [#106](https://github.com/dseelinger/ward/issues/106)

## D34 — Defaults live in code, and only changes reach the disk

No settings file ships. A default is shown as placeholder text behind an empty
box rather than as a value in it, so tabbing past a field cannot commit today's
default as a permanent override. Clearing a setting returns it to default.

> enforced — `cargo test`, the settings tests in `src/config.rs`.

## D35 — Two stores: running state shrugs, settings fault

An unknown key in the window's remembered size is ignored. An unknown key in
the Commander's settings rejects the whole file, because a partly applied
settings file is worse than a refused one. Both write to a sibling file and
move it into place.

> enforced — `cargo test`, the loader tests in `src/config.rs`.

## D36 — Secrets live in a file, encrypted to this account, never in the environment

A stored key is unreadable to anything but this Commander on this machine. No
environment variable is ever read for one: it is a plaintext bypass and it
hides first-install problems. Keys are write-only through the interface, and
rotating one rebuilds the provider rather than needing a restart.

> enforced — `cargo test`, the tests in `src/secrets.rs`, and the client's
> `Debug` is written by hand so a key cannot reach a log by accident.

## D37 — Five test tiers

Pure, integration without hardware, hardware, headset, and game. The line is
what a test touches rather than how slow it is. A tier that cannot run says so,
rather than passing silently.

> enforced — `check/coverage.txt` gives every file under `src/` a tier, and
> `check/coverage.sh` fails on a file that has none, so a new module cannot
> arrive without one. The tier is carried by the code rather than by any one
> test: it says how far out you have to reach to exercise the file at all,
> which is the only version of this that can be measured. The last three tiers
> collapse into one exemption, because CI can run none of them — a device, a
> service, a headset and a window are all equally out of reach from a runner.

## D38 — Three guarantees, held mechanically

No test in the first two tiers may reach the network, because an unattended
loop that reaches a live provider spends money while nobody is watching.
Coverage floors per tier, measured rather than specified. A release gate that
installs the built program, launches it and asserts it survives.

> enforced — all three. Every call that leaves the process says so first and a
> test without a permit fails on it, with the wiring at each call site tested
> rather than only the mechanism (`src/outside.rs`). The floors are measured
> per tier and a breach fails the build (`check/coverage.sh`). The release gate
> installs, launches and watches Ward survive (`.github/workflows/gate.yml`).
> Two of the four things the first bullet asks for do not apply here and are
> recorded rather than invented: no test redirects the data folder, and there
> is no mock-mode variable to clear.

## D39 — Everything writable in one folder beside the program

One folder to delete for a genuine first run and one to copy. It removes the
difference between how a development build and an installed build find their
own state, which is where first-run problems hide.

> enforced — `check.sh` asserts the installer declares the folder as surviving
> an uninstall and deletes nothing under it.

## D40 — Quick on every change, thorough on the path that ships

Lint, compile and the cheap tiers on a pull request. The install and upgrade
gates on a release, because a pull request cannot ship a broken upgrade.

> enforced — `.github/workflows/check.yml` on every change, and
> `.github/workflows/gate.yml` on a release. The gate lives in a workflow of its
> own so it can be run against any branch without publishing anything, which is
> what let it be exercised before it ever guarded a release — and its first run
> found that the release build had not linked for seven hours.

## D41 — The upgrade is tested synthetically

A folder with a marker file in it, the installer run over it, the marker
asserted to have survived. Seconds, and it tests the mechanism that actually
breaks.

> enforced — `.github/workflows/gate.yml`, which installs, writes a settings
> file, installs over the top and asserts the file is there with the same
> contents. Contents rather than existence, because a preserved folder with a
> replaced file is the likelier failure and an existence check would call that a
> pass. Watched failing, on the real gate, with everything before it passing.

## D42 — Updates come from releases, with nothing to run

No server, no service. A single network gate checked before the request, so
switching it off means no call is made. An unreadable setting reads as enabled,
because a safety net is never disabled by surprise. Availability waits for the
installer to be attached rather than for the release to exist.

> enforced — `cargo test`, the tests in `src/update.rs`. The gate is checked
> before the request, so off means nothing is sent and the network guard would
> fail the test if it were. A listing Ward cannot read is an unanswered
> question rather than a negative answer, and a release whose installer is not
> attached yet is not offered.

## D43 — Ship unsigned, and verify by checksum

No certificate. A program that fetches and runs a new binary is a different
risk from an install somebody chose, so the checksum is published with the
release and verified before anything executes, and the download host is fenced
to an allowlist.

> enforced — `cargo test`, the tests in `src/update.rs`. The fence refuses
> plain HTTP, a host that merely contains an allowed one, and a URL whose real
> host hides after a userinfo section. The checksum is matched against the name
> the release published it under, and a mismatch refuses rather than warns.
> Nothing is executed by the code that downloads it.

## D44 — What an update does

Visible in the headset as well as the window. Distinguishes "could not check"
from "up to date", because collapsing an unanswered question into a negative
answer once hid four installable releases behind a confident green. Installs on
the way out, so an installer window never appears over the game. Askable and
actionable by voice.

> open — two of the four. The three-way answer is built and tested, and it
> installs on the way out rather than over the game. It is visible in the
> window and not yet in the headset, which waits on the panel that will carry
> it; and it cannot be asked for by voice, which wants the update state
> reachable from a capability and is the piece that is genuinely unbuilt rather
> than merely blocked.
> [#126](https://github.com/dseelinger/ward/issues/126)

## D45 — Three rules of repository discipline, and no more

`CLAUDE.md` stays under one screen, which forces the right question about every
rule: can a check do this instead. Every rule is enforced, accepted as
unenforced, or deleted, and there is no fourth state. No check may have another
check as its subject.

> enforced — `check.sh` fails on any decision in this file without exactly one
> state, which is the second rule holding the whole file up. The first and third
> rules are accepted as unenforced deliberately: a check counting lines in an
> instruction file, and a check reading other checks, are both the third rule's
> own failure mode.

## D46 — Mutation score sits behind the coverage floors

A floor says a line ran. It does not say anything would notice that line
changing, and the cheapest way to meet a floor is a test that executes code and
asserts nothing. Mutation testing is the honest form of what coverage
approximates: change the code, expect the suite to go red, and treat what
survives as the work queue. It runs on a schedule rather than on a pull request,
because it is slow and the answer does not change between two commits. Where a
survivor can be killed by an invariant rather than an example, the invariant
wins — it does not have to be derived from the code it is testing, which is the
one thing a test written by reading the implementation can never claim.

Until something does measure it, the writing rule carries it: a test written to
reach a floor says which bug it would catch, and behavior that has an entry in
this record is tested against the entry rather than against the implementation.
A test derived by reading the code pins what the code does rather than what it
should do, and preserves a bug as faithfully as it preserves anything else.

> open — nothing runs mutants today, so the floors are the only thing measuring
> the tests and they cannot see this.
> [#130](https://github.com/dseelinger/ward/issues/130)

## D47 — A silenced test says why, in the attribute

A muted test is worse than one that was never written: it reads as coverage, it
is counted in the run, and it teaches whoever finds it that a green build with a
hole in it is normal. Ignoring a test is allowed, because a test that genuinely
needs the wire or hardware no runner has is a real thing. The reason goes where
the mute is, the way an exempt file carries one and a decision carries a state.

> enforced — `check.sh` fails on a bare `#[ignore]` under `src/` and `tests/`,
> and passes `#[ignore = "..."]`. There are none of either today, which is why
> it went in now rather than once there was one to argue about. Watched failing,
> and watched passing on the reasoned form.
