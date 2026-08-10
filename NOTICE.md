# Notices

ward is MIT licensed. See [LICENSE](LICENSE).

It is built on open source, and everything it ships is permissively licensed by
construction rather than by inspection: `deny.toml` holds an allowlist, and a
dependency carrying anything not on that list fails the build. Nothing here is
copyleft. Where a crate offers a choice that includes a copyleft licence, the
permissive side is taken and the choice is recorded in `deny.toml`.

Most of the tree is MIT or Apache-2.0 and is satisfied by that licence text
alone. The entries below carry obligations that are not, and so are named here.

## Bundled typefaces

The user interface toolkit ships default fonts whose licences are combined with
AND rather than OR, so they apply in addition to the toolkit's own terms and
cannot be traded for a permissive alternative.

- **Ubuntu font family** — Ubuntu Font Licence 1.0.
- **Additional bundled faces** — SIL Open Font Licence 1.1.

Both permit use, modification and redistribution, including as part of a
larger work. Reserved font names must not be reused for a modified version.

## Unicode data

- **unicode-ident** and the ICU crates — Unicode Licence v3, alongside MIT or
  Apache-2.0. Covers the Unicode character data tables these crates embed.

## Root certificates

- **webpki-root-certs** — CDLA-Permissive-2.0. The bundled certificate
  authority list, used to verify the connection to the model.

## Direct dependencies

| Crate | Licence |
|---|---|
| anyhow | MIT OR Apache-2.0 |
| eframe | MIT OR Apache-2.0 |
| futures-util | MIT OR Apache-2.0 |
| reqwest | MIT OR Apache-2.0 |
| serde | MIT OR Apache-2.0 |
| serde_json | MIT OR Apache-2.0 |
| tokio | MIT |
| windows-sys | MIT OR Apache-2.0 |

The full set, including everything reached indirectly, is in `Cargo.lock`.
