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

## Two ways to summon it

Because in a headset one of them is always inconvenient: hands on the stick, say
it; mid-sentence with the model, press it.

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

## Putting it where you want it

Point at the strip along the top of the panel and hold the **trigger**. The panel
comes with your hand until you let go, keeping the distance, bearing and angle it
had when you caught it — so you can tilt one to read from below, or turn one to
sit off to your side.

The strip is there because everywhere else on the panel is a control, and a press
on a control is a click. It is the one part of the surface that does nothing, so
it is the one part that can mean "pick this up".

**Moving it only moves it.** It will not change size, change mode, or disappear
because of how you happened to move your hand. What is showing is something you
ask for, by key or out loud.

## Curved

The big panel is bent slightly around you, so its far edges sit at about the
distance its middle does rather than further away. Straighten it or bend it more:

```json
{
  "panel curve": 0
}
```

Captions are never curved. They are two short lines in the middle of your view,
where there are no far edges to bring closer.
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

## Typing, and where it is not

**The panel cannot be typed into.** Point at a text box and it takes focus, and no
letters go in — there is no keyboard in the headset.

Three ways were built and none survived. Reading your keyboard directly worked and
typed into the game at the same time, so a folder path also went to the cockpit
where those letters do things. Taking the keyboard away from the game worked too,
and was removed: too many ways to be left unable to type to anything at all, and
the consequence of being wrong lands mid-flight. SteamVR draws its own keyboard,
and it was not worth having.

So settings are a thing you do at the desk, in Ward's own window, before you fly.
Everything on the panel that is not typing — the conversation, the checklist,
picking a voice from the list, every checkbox and every Reset — works from the
headset.

This is tracked as [#133](https://github.com/dseelinger/ward/issues/133), and it
is genuinely undecided whether the answer is to make typing work or to stop
drawing the settings page in the headset at all.

## When there is no headset

Nothing here costs you anything. Ward looks for SteamVR when it starts, and if it
is not running it keeps looking — you can start Ward first, start SteamVR ten
minutes later, and the panel will be there. While there is no headset, or while
the panel is not showing, it is not drawn at all.
