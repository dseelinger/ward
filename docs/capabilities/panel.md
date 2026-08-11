# The panel

Everything Ward can show you, inside the headset, without taking it off.

> **You:** show me the panel
>
> **Ward:** The panel is up.

It appears about a metre and a half in front of wherever you are looking, turned
to face you, and then it stays there. It does not follow your head — captions do
that, because a caption is text you are reading right now, and a panel is
something you glance up at and then look away from.

## It is the same panel as the window

There is one Ward interface and it is drawn twice. The conversation, the
checklist, the update notice and the whole settings page are on the panel because
they are in the window, and neither one can gain something the other does not
have. That is a structural claim rather than a promise to keep them in step:
there is one widget tree, and a second surface with its own controls would have
nowhere to live.

So anything the rest of this documentation says you can do in the window, you can
do in the headset, including typing your API key and running the setup test.

## Two modes

**Big** is everything: the conversation, the checklist beside it, the tabs, and
the settings page behind them. It is a screen you turn to look at.

**Mini** is what is worth seeing without stopping flying, and it is three lines:

- what Ward is doing right now — ready, listening, answering, or looking
  something up
- the next thing left on your checklist, named rather than counted, so you can
  act on it without opening anything
- anything that needs an answer, such as a problem or a new version

It is a mode of the same panel rather than a shrunken copy of it, in the way a
media player's compact mode is, and it is drawn at its own size rather than
scaled down — text on a surface meant to be read at a glance has to stay the
size it was.

What it deliberately does not carry is the last thing Ward said. Captions
already do that, paced by the voice and placed under what you are looking at,
and a second copy of it a foot to the side is the same duplication the desktop
conversation refuses to make.

## Three ways to summon it

Because in a headset there is always one of the three that is inconvenient.

**Say so.** "Show me the panel", "put it away", or "give me the small one".

**Press a key.** Backslash unless you change it — the function row is where Elite
already keeps a great deal, and it is also the row you cannot find by touch with a
headset on.

```json
{
  "panel key": "Key_Grave"
}
```

The key cycles: away, small, big, away. Every mode is reachable from the key
alone, which matters because the other two routes each need something — the
spoken one needs Ward to be hearing you, and the grab needs a controller in
your hand.

**Use your hands.** Point at the strip along the top of the panel and hold the
**trigger**. The panel comes with your hand until you let go, keeping the
distance and bearing it had when you caught it. Let go while still moving and you
have thrown it away, and it is gone.

The strip is there because everywhere else on the panel is a control, and a press
on a control is a click. It is the one part of the surface that does nothing, so
it is the one part that can mean "pick this up".

Dragging the small panel open turns it into the big one.
It then moves with your hand, keeping the distance it had when you caught it, so
you can put it where you want rather than where Ward guessed. Grabbing the small
panel pulls it open into the big one. Let go with your hand still moving and you
have thrown it away, and it is gone.

You point at it rather than reaching for it because the panel is a metre and a
half away — far enough to read comfortably, and further than you can reach
sitting down.

## Pointing at it

The panel takes controller input the way SteamVR delivers it, so your pointer
works on it the same way it works on the SteamVR dashboard: point, and pull the
trigger to click. Scrolling works too, which matters most on the settings page.

Ward tells the toolkit that it is being pointed at rather than moused at. A ray
at a metre and a half turns an ordinary steady hand into more drift than a
desktop toolkit will accept in a click, and about half of them were being thrown
away as accidental drags before that was fixed.

## Typing

Point at any text box and SteamVR's own keyboard comes up, with your layout and
the way of typing you already know. What you type goes into the box as you type
it, and the keyboard goes away when you move on. Ward does not draw a keyboard
of its own — a worse copy that you would have to point at with the same ray it
was competing with.

The conversation is deliberately not selectable text. Dragging a selection across
a log that is growing underneath you is a poor experience with a mouse and a
worse one with a ray, so it is drawn as read-only lines with nothing to catch a
stray drag on.

## When there is no headset

Nothing here costs you anything. Ward looks for SteamVR when it starts, and if it
is not running it keeps looking — you can start Ward first, start SteamVR ten
minutes later, and the panel will be there. While there is no headset, or while
the panel is not showing, it is not drawn at all.
