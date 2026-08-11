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

**Press a key.** `Key_F9` unless you change it:

```json
{
  "panel key": "Key_Grave"
}
```

The key toggles. Pressing it with the big panel up puts it away; pressing it any
other time brings it up.

**Use your hands.** Reach out and squeeze the grip to take hold of the panel, and
it moves with your hand — so you can put it where you want it rather than where
Ward guessed. Grabbing the mini panel pulls it open into the big one. Let go with
your hand still moving and you have thrown it away, and it is gone.

## Pointing at it

The panel takes controller input the way SteamVR delivers it, so your pointer
works on it the same way it works on the SteamVR dashboard: point, and pull the
trigger to click. Scrolling works too, which matters most on the settings page.

Ward tells the toolkit that it is being pointed at rather than moused at. A ray
at a metre and a half turns an ordinary steady hand into more drift than a
desktop toolkit will accept in a click, and about half of them were being thrown
away as accidental drags before that was fixed.

The conversation is deliberately not selectable text. Dragging a selection across
a log that is growing underneath you is a poor experience with a mouse and a
worse one with a ray, so it is drawn as read-only lines with nothing to catch a
stray drag on.

## When there is no headset

Nothing here costs you anything. Ward looks for SteamVR when it starts, and if it
is not running it keeps looking — you can start Ward first, start SteamVR ten
minutes later, and the panel will be there. While there is no headset, or while
the panel is not showing, it is not drawn at all.
