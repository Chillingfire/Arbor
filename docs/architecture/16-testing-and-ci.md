# Testing and CI

Arbor's testing follows two rules from the repo's `AGENTS.md`: test behaviour, not implementation,
and mock only at the system boundary.

## The pure core

`arbor-core` does no I/O, so its state machine is tested with plain unit tests. The escalation and
checkpoint logic, which is the hardest part, runs with no network, no git and no model. A test feeds
events in and checks the effects and the resulting phase.

## The scripted fake

The model is a system boundary, so it is faked there. `arbor-llm` ships a `ScriptedClient` that
replays recorded exchanges, and a `RecordingClient` that records new ones. The whole pipeline is
tested against the fake, replaying recorded fixtures.

The fixtures live in `crates/arbor/tests/fixtures/pipeline`: `happy_path.json`, `escalation.json`,
`escalation_replan.json`, `revision.json`, `writer_retry.json`, `missing_response.json`,
`broken_region_duplicated.json` and `broken_region_deleted_begin.json`. Each one drives a full
Session through the Autopilot with the fake client.

## Real git and files

Git and file I/O are tested against real repositories and real temp directories, cleaned up per
test. `arbor-git` and the tools use `tempfile` for this. The `gh` calls are tested with a fake
`GhRunner`, so no network is needed.

## Test names are sentences

Every test is named as a sentence describing the behaviour, in snake_case, for example
`no_newline_marker_inside_a_hunk_is_skipped`. The name says what the code does, not which function
it calls.

## What CI runs

`.github/workflows/ci.yml` runs on a push to `main` and on every pull request, on Linux, macOS and
Windows:

```sh
cargo fmt --all --check
cargo build --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo xtask check-core-deps
```

The last one keeps `arbor-core` free of I/O crates, which is what makes the pure core testable. It
is the rule from ADR-0005 enforced by a check rather than a convention.

## The release build

`.github/workflows/release.yml` builds a `v*` tag for Windows x64, macOS arm64 and x64, and Linux
x64 and arm64 (static musl). It publishes a GitHub Release with the archives and a `SHA256SUMS`
file. The tag has to match the workspace version, or the release job fails.

## Lessons become structure

The repo's `AGENTS.md` says that when the same correction comes up twice, it should become a test, a
lint rule or a script rather than another line in the doc. The `check-core-deps` xtask is one
example; the `compile_fail` tests on `RemoteApproval` are another, since they make the
remote-approval rule a build error rather than a review note.
