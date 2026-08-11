# Ward

A voice companion for Elite Dangerous. Hold a key, ask it something, hear the
answer — in the cockpit, in a headset, without taking your hands off anything.

Ward reads the game's own journal to know where you are and what you are
flying, so what it tells you comes from what the game wrote rather than from
what a language model remembers. When it cannot find something out, it says so.

```
Commander   How much fuel have I got?
Ward        Nineteen point two tons, which is most of the way full.
```

## What it needs

- Windows, and Elite Dangerous
- A microphone, or not — every answer is reachable by typing
- An Anthropic API key for the model. The voice is free.

A headset is optional. Ward draws captions into SteamVR when one is running,
and works the same without.

## Where things are

Every capability has a page describing what it does, what it needs, and what it
does when it cannot do it. [The decisions](decisions.md) records what Ward
decided and what holds each decision in place — worth reading before changing
anything, and worth reading first if you are wondering why something is the way
it is.
