# Version

Ask Ward which build it is.

> **You:** what version are you running?
>
> **Ward:** Ward 0.1.0.

That is the whole capability. It exists because the answer to "which build is
this" has to come from the build itself rather than from anything that could be
out of date — a note in a file, a page on a site, or the model's own idea of
what it is.

## Why it cannot be wrong

The version is compiled in. Ward reads it from the same string the binary was
stamped with when it was built, so there is no configuration to disagree with,
no file to go stale, and nothing to keep in step:

```rust
format!("Ward {}", env!("CARGO_PKG_VERSION"))
```

If Ward tells you 0.1.0, you are running 0.1.0.

## What it does not do

It does not check whether a newer release exists. That needs the network, and
it is a separate thing Ward will learn to do later. This answers only what you
are running now.

## Saying it

Any of these reach it:

- "what version are you"
- "which build is this"
- "are you up to date"

The last one gets an honest answer about what you have, not a comparison
against what is available.
