# hop-top-inv-e2e

End-to-end tests, one file per user story in [`docs/stories/`](../../docs/stories/).

Each test reproduces a story's **Given / When / Then** acceptance criteria
against real implemented surfaces — no synthetic abstractions, no mocks of
inv-internal seams. Where a test crosses an external boundary (outbound
webhook delivery, inbound fin bus events, MCP stdio subprocess) it goes
through an [`xrr`](https://crates.io/crates/hop-top-xrr) cassette
recorded once + replayed on every run.

## Run

```sh
cargo test -p hop-top-inv-e2e                          # all stories, replay mode
cargo test -p hop-top-inv-e2e --test freelancer_qc_01_draft_link_fin_paid
```

## xrr cassettes

Tests default to `Mode::Replay` reading from `cassettes/<test-name>/`.
To re-record against the real world:

```sh
XRR_MODE=record cargo test -p hop-top-inv-e2e --test <test-name>
```

Inspect the resulting `cassettes/<test-name>/*.yaml` for any leaked
secrets (synthetic fixtures shouldn't carry any — verify anyway), then
commit.

## When to re-record

- The story's surface contract changed (request shape, response shape,
  payload fields). The story doc is the source of truth — update it
  first, then re-record.
- xrr itself bumped (alpha → next alpha) and changed the cassette
  envelope. The pin in [`Cargo.toml`](Cargo.toml) is intentionally
  narrow; bump it deliberately, re-record everything, commit.

## Layout

```
crates/e2e/
├─ Cargo.toml
├─ README.md
├─ src/lib.rs                    ← empty; workspace registration only
├─ tests/
│  ├─ common/mod.rs              ← shared fixtures (fresh_pool, seed_customer, ...)
│  └─ <persona>_<NN>_<slug>.rs   ← one file per story
└─ cassettes/<test-name>/        ← committed; one subdir per xrr-using test
```
