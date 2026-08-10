# Notices

Ward is MIT licensed. See [LICENSE](LICENSE).

It is built on open source, and everything it ships is permissively licensed by
construction rather than by inspection: `deny.toml` holds an allowlist, and a
dependency carrying anything not on that list fails the build. Nothing here is
copyleft. Where a crate offers a choice that includes a copyleft license, the
permissive side is taken and the choice is recorded in `deny.toml`.

Most of the tree is MIT or Apache-2.0 and is satisfied by that license text
alone. The entries below carry obligations that are not, and so are named here.

## Bundled typefaces

The user interface toolkit ships default fonts whose licenses are combined with
AND rather than OR, so they apply in addition to the toolkit's own terms and
cannot be traded for a permissive alternative.

- **Ubuntu font family** — Ubuntu Font License 1.0.
- **Additional bundled faces** — SIL Open Font License 1.1.

Both permit use, modification and redistribution, including as part of a
larger work. Reserved font names must not be reused for a modified version.

## Bundled native code

The speech model runs on this machine, and the C++ that runs it is compiled
from source and linked into Ward's binary during the build.

- **whisper.cpp**, including ggml — MIT, Copyright (c) 2023-2024 The ggml
  authors.

Named here because the license gate cannot see it. `cargo deny` reads what a
crate declares about itself, and `whisper-rs-sys` declares Unlicense while
carrying an MIT-licensed C++ library inside it. An allowlist over crate
metadata is exactly the mechanism that misses a vendored native library, which
is why anything shipping one is read by hand as well.

## Unicode data

- **unicode-ident** and the ICU crates — Unicode License v3, alongside MIT or
  Apache-2.0. Covers the Unicode character data tables these crates embed.

## Root certificates

- **webpki-root-certs** — CDLA-Permissive-2.0. The bundled certificate
  authority list, used to verify the connection to the model.

## Direct dependencies

| Crate | License |
|---|---|
| anyhow | MIT OR Apache-2.0 |
| cpal | Apache-2.0 |
| eframe | MIT OR Apache-2.0 |
| futures-util | MIT OR Apache-2.0 |
| reqwest | MIT OR Apache-2.0 |
| rodio | MIT OR Apache-2.0 |
| roxmltree | MIT OR Apache-2.0 |
| serde | MIT OR Apache-2.0 |
| serde_json | MIT OR Apache-2.0 |
| sha2 | MIT OR Apache-2.0 |
| time | MIT OR Apache-2.0 |
| tokio | MIT |
| tokio-tungstenite | MIT |
| tracing | MIT |
| tracing-appender | MIT |
| tracing-subscriber | MIT |
| whisper-rs | Unlicense |
| windows-sys | MIT OR Apache-2.0 |

The full set, including everything reached indirectly, is in `Cargo.lock`.
