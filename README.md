# Arbor

Multi-LLM coding harness for minimizing token usage and wasted compute

Goal of project: To implement an efficient LLM harness for coding, as well as to practice/refine
skills in spec-driven AI development

## How Arbor works

Arbor splits every change across three model Tiers so that expensive models do only the reasoning
and cheap models write most of the code. The Planner turns a Request into a Plan. The Scaffolder
turns each Task into a compilable Skeleton, the tests, and a set of Work Units. The Writer is a
single non-agentic call that fills one Region using only its Context Pack.

```mermaid
flowchart LR
    request["Request"]
    planner["Planner<br/>(read-only)"]
    scaffolder["Scaffolder<br/>(writes skeleton + tests)"]
    writer["Writer<br/>(one Region, one call)"]
    checks["Checks"]
    request --> planner --> scaffolder --> writer --> checks
```

A run is a Session, which works on its own branch and worktree. Each Session writes an append-only
event log, and every state change is an event, so resume, rewind and cost accounting all come from
replaying it. Work stops at a Checkpoint whenever the operator needs to approve, edit or reject a
Tier's output.

Work Units are located in code by Marker comments, so Arbor works in any language:

```
// ARBOR:BEGIN WU-7
todo!()
// ARBOR:END WU-7
```

The Writer replaces the Placeholder between the Markers and Arbor strips them when the Work Unit
passes. When a piece of work keeps failing, Arbor escalates it up the Tiers: Writer to Scaffolder to
Planner to the operator.

The full technical breakdown of each part is in [`docs/architecture/`](docs/architecture/README.md).
The decisions behind the design are in [`docs/adr/`](docs/adr/).

## Install

Download the archive for your platform from the
[latest release](https://github.com/Chillingfire/Arbor/releases/latest), check it
against `SHA256SUMS`, and put `arbor` on your `PATH`. Builds are published for
Windows x64, macOS arm64 and x64, and Linux x64 and arm64 (static musl).

Or build it with Cargo:

```sh
cargo install --git https://github.com/Chillingfire/Arbor --locked arbor
```

Arbor is not published to crates.io: the names `arbor` and `arbor-core` are taken there, so
installing from this repository is the Cargo route.

## Set up a project

Arbor verifies work with the Checks in the project's `arbor.toml`. Run `arbor init` in the
repository to write them: Arbor detects the project type from `Cargo.toml`, `go.mod`,
`pyproject.toml` or `package.json` and runs the usual commands once. If they pass, it writes
`arbor.toml` straight away. Otherwise the Scaffolder reads the repository, README and CI
configuration and proposes commands for you to accept or skip. Running `arbor init` again replaces
only the `[checks]` table.

## Configuration

Arbor reads a per-operator user config (`~/.config/arbor/config.toml`, or
`%APPDATA%\arbor\config.toml` on Windows) and the project's `arbor.toml`, then merges them. The
models for each Tier, the Checks, the Writer Cap, the Budget Cap and Clarification Mode all live
there. The OpenRouter API key comes from `OPENROUTER_API_KEY` or the user config, never from
`arbor.toml`. Every setting is listed in [Configuration](docs/architecture/10-configuration.md).

## Keep Markers out of the mainline

Arbor brackets the code each Writer fills with Marker comments (`ARBOR:BEGIN WU-7` …
`ARBOR:END WU-7`) that must never reach the mainline or a remote. Arbor checks for them itself, and
`arbor init` also offers `pre-commit`, `pre-merge-commit` and `pre-push` hooks that run
`arbor check-markers` for git commands run outside Arbor. It never overwrites a hook you already
have; it prints the lines to add instead, as it does when `core.hooksPath` is set.

For a server-side check too, add this step to CI. It lists every Marker line and fails if there are
any:

```sh
git fetch --depth=1 origin main
arbor check-markers --rev HEAD --allowlist-rev FETCH_HEAD
```

If some files hold Markers on purpose, such as tests or docs about them, list their paths in a
committed `.arbor-markers-allowed` file at the repository root: gitignore-style patterns, one per
line, with `#` comments. The check never reads the file from what it scans, so a change can't
allowlist its own Markers. It reads it from `HEAD` when it checks the working tree or the index
(`--staged`), and from the mainline tip when it checks a revision (`--rev`), as the pre-push hook
does; `--allowlist-rev` names another commit, such as the base branch in CI. So a change to the
list takes effect for commits once it is committed, and for pushes and CI once it is on the
mainline. With no such commit, every path is checked.

The list only narrows where the check looks; a pattern that would allow every path, such as `*` or
`**`, is an error. Every file a pattern matches goes unchecked, so `arbor check-markers` warns each
time a pattern allows more than half of the tracked files, such as `*.rs` or `src/`. Arbor's own
checks that a Writer's Region was stripped, and the Marker check at
the end of a Session, ignore the list.

## Repository layout

Arbor is a Cargo workspace. `arbor` is the binary; the other crates each own one part of the system.
The domain types and the Session state machine live in `arbor-core`, which does no I/O. The crate
map and what each crate owns is in [`docs/architecture/`](docs/architecture/README.md).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in
Arbor by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any
additional terms or conditions.
