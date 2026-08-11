# Checklist

A list of things you mean to do, kept between sessions.

> **You:** add buy tritium at Deciat to my checklist
>
> **Ward:** Added "buy tritium at Deciat" to the checklist.
>
> **You:** what is left?
>
> **Ward:** Checklist: 1 of 2 done. Still to do: refuel at Jameson.

Asking what is left gets you how far along you are and what remains, rather
than the whole list read out. Finished items are not read back, because you
asked what was left.

## The file

`data/checklist.md`, and it is markdown on purpose. This is the one thing Ward
keeps that you will want open in an editor beside the game:

```markdown
# Checklist

- [x] buy tritium at Deciat
- [ ] refuel at Jameson Memorial
```

Edit it by hand whenever you like. Ward reads `-`, `*` and `+` bullets, any
indentation, and a tick in either case, so you do not have to remember a house
style. It always writes `- [x]` and `- [ ]` back.

Anything that is not a task is kept exactly where you put it. A heading, a
blank line, a note in the middle of the list — Ward edits around them and
writes them back untouched:

```markdown
# Deciat run

Do these in order, the shipyard closes late:

- [x] buy tritium at Deciat
- [ ] refuel at Jameson Memorial

remember the fuel scoop
```

New items go under the last task, so a note written below the list stays below
the list.

## Finishing something

> **You:** mark the tritium done
>
> **Ward:** "buy tritium at Deciat" done. One left.

The item stays on the list, ticked. A list that shows only what remains cannot
answer how far along you are, and that is the question a spoken checklist is
best at.

You never have to repeat what you wrote down. Part of it is enough, which
matters because the transcription will not reproduce a station name exactly
either.

When two items could be meant, Ward asks instead of choosing:

> **You:** mark buy tritium done
>
> **Ward:** "buy tritium" could mean buy tritium at Deciat or buy tritium at
> Sol. Which one?

Nothing is ticked while it asks. Guessing wrong here is silent — you would hear
"done" and believe the right thing had been marked.

## Taking something off

Removing is not completing. Use it when you added the wrong thing, or changed
your mind:

> **You:** take refuelling off the list
>
> **Ward:** Took "refuel at Jameson Memorial" off the checklist.

## The panel

The checklist appears beside the conversation in Ward's window, with a tick box
per item and a Remove button. It updates as you talk — an item added by voice
appears without a reload, and an item ticked on the panel is read back
correctly by voice a second later.

The panel does not edit the list directly. Ticking a box runs the same thing
your voice runs, so the two cannot end up with different ideas about what an
edit is.

## What it will not do

Ward never decides on its own that an item is done. The journal knows when you
docked at Jameson, and one day it will offer that as a question — but it will
ask, and you will answer. A checklist that ticks itself is a checklist you have
to check.
