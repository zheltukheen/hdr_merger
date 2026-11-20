# AGENTS.md — working agreement

This file is the contract for anyone (human or agent) changing this repository.
Read it before the first edit.

## Ground rules

1. **First make it work, then make it right.** One small working change, proven,
   then the cleanup. No half-migrated states.
2. **TDD where it is honest.** Write the failing test, see it fail for the right
   reason, make the smallest change that passes. If a test cannot be honest,
   write none and say so in the PR description.
3. **Domain first.** The data shape lives in `hdr-core`; if a rule can be
   expressed in a type, express it there instead of in a branch.
4. **Boundary discipline.** Parse outside data exactly once (in `io`), then trust
   it. Business logic stays in pure functions (`stack`, `tonemap`, `align`).
5. **No shared mutable state.** The pipeline owns its cache directory, the UI
   owns its state, threads communicate through channels.
6. **One decision, one place.** Stack semantics live in `stack.rs` only; the
   UI and the CLI both call it.

## What goes where

| Rule | Lives in |
| --- | --- |
| File formats, encoding inference | `crates/hdr-core/src/io.rs` |
| Pick-one-value-from-N semantics | `crates/hdr-core/src/stack.rs` |
| Translation estimation | `crates/hdr-core/src/align.rs` |
| Exclusion masks | `crates/hdr-core/src/mask.rs` |
| Exposure / curves / sRGB | `crates/hdr-core/src/tonemap.rs` |
| Orchestration, caching, streaming | `crates/hdr-core/src/pipeline.rs` |
| Presentation only | `crates/hdr-cli`, `crates/hdr-gui` |

Adding a feature to the GUI that the CLI cannot do means the feature is in the
wrong crate.

## Commands

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release
```

All four must pass before a commit lands. CI runs the same list.

## Quality gates

The project's stated gates are `crapper` (CRAP < 10), `dryer` (duplication
< 0.8) and `mutator`. **None of them are installed for this workspace** — they
are Clojure/Python tools. Do not claim a gate passed. Instead:

* keep functions short and test them at the behaviour level;
* report explicitly in the PR that the gates were unavailable;
* a hotspot that `crapper` would flag is anything over ~30 lines with low
  coverage; split it by hand.

## Tests

* Behaviour, not implementation: call the public API and assert on the result a
  user would see.
* A test that still passes when the function returns nothing is not a test.
* Fixtures are generated in `std::env::temp_dir()` and cleaned by the test body;
  no binary fixtures in the repo.
* One integration test per user-visible promise (median removes a mover, a mask
  falls through, alignment recovers a shift, EXR keeps floats).

## Commits

* Small and ordered. Each commit message says **why**, **what changed** and
  **how it was proven**.
* Conventional prefix (`feat:`, `fix:`, `docs:`, `ci:`, `refactor:`), imperative
  subject, no trailing period.
* Never commit `target/`, caches, or generated images.

## Definition of done

* `cargo test --workspace` green.
* `cargo clippy --workspace --all-targets -- -D warnings` green.
* The behaviour is observable from the CLI (or the GUI), not only from tests.
* Docs touched if a contract changed: `ARCHITECTURE.md` for structure,
  `ROADMAP.md` for scope, `README.md` for user-facing commands.

## Log

Keep a short log of what you did and why while working. Report it when the task
ends.
