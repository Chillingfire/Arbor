# Arbor
Multi-LLM coding harness for minimizing token usage and wasted compute

Goal of project: To implement an efficient LLM harness for coding, as well as to practice/refine skills in spec-driven AI development

## Install

Download the archive for your platform from the
[latest release](https://github.com/Chillingfire/Arbor/releases/latest), check it
against `SHA256SUMS`, and put `arbor` on your `PATH`. Builds are published for
Windows x64, macOS arm64 and x64, and Linux x64 and arm64 (static musl).

Or build it with Cargo:

```sh
cargo install --git https://github.com/Chillingfire/Arbor --locked arbor
```

TODO: decide how `cargo install` should work. The names `arbor` and `arbor-core` are taken on
crates.io, so publishing would need different package names. Revisit once the repo is public.

## Set up a project

Arbor verifies work with the Checks in the project's `arbor.toml`. Run `arbor init` in the
repository to write them: Arbor detects the project type from `Cargo.toml`, `go.mod`,
`pyproject.toml` or `package.json` and runs the usual commands once. If they pass, it writes
`arbor.toml` straight away. Otherwise the Scaffolder reads the repository, README and CI
configuration and proposes commands for you to accept or skip. Running `arbor init` again replaces
only the `[checks]` table.

## Sessions

Each Session keeps its event log and worktree in `.arbor/sessions/<id>/`. Press `b` in the TUI to
browse them: each shows its status, cost, age, whether its branch still holds Markers, and how long
until it is pruned. From there you can inspect, resume, rewind to a Checkpoint, or delete one.
Delete is the only way Arbor deletes a Session branch, and it asks first, naming the branch.

On startup Arbor prunes Sessions whose last event is older than `retention_days`: their folder and
worktree are removed, their `arbor/*` branch is always kept, and a Session whose worktree has
uncommitted changes is skipped with a warning. The default is **30 days**. Set `retention_days` in
`arbor.toml` or in the user config (`arbor.toml` wins); `0` keeps Sessions forever.

## Keep Markers out of the mainline

Arbor brackets the code each Writer fills with Marker comments (`ARBOR:BEGIN WU-7` …
`ARBOR:END WU-7`) that must never reach the mainline or a remote. Arbor checks for them itself,
and `arbor init` also offers `pre-commit`, `pre-merge-commit` and `pre-push` hooks that run
`arbor check-markers` for git commands run outside Arbor. It never overwrites a hook you already
have; it prints the lines to add instead, as it does when `core.hooksPath` is set.

For a server-side check too, add this step to CI. It lists every Marker line and fails if there
are any:

```sh
arbor check-markers --rev HEAD
```

If some files hold Markers on purpose, such as tests or docs about them, list their paths in a
committed `.arbor-markers-allowed` file at the repository root: gitignore-style patterns, one per
line, with `#` comments. The check reads the file from what it scans: the working tree, the index
with `--staged`, or the tree at `--rev`. The list only narrows where the check looks; a pattern
that would allow every path, such as `*` or `**`, is an error. Arbor's own checks that a Writer's
Region was stripped, and the Marker check at the end of a Session, ignore the list.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in
Arbor by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any
additional terms or conditions.
