# CLI and entry points

The `arbor` binary is the front door. Its argument parser is hand-rolled, because Arbor takes a few
subcommands and options; the code says to switch to `clap` if more arrive.

## The commands

| Command | What it does |
| --- | --- |
| `arbor` | Opens the TUI to type a Request. |
| `arbor --version` / `-V` | Prints the version. |
| `arbor init` | Writes the project's Checks to `arbor.toml`. |
| `arbor --issue <number>` | Starts a Session from a GitHub issue. |
| `arbor --handover-file <path>` | Starts a Session from a file, or stdin with `-`. |
| `arbor check-markers [options]` | Lists the Markers in a tree for hooks, CI and manual use. |

Bad arguments print a usage line and exit with code 2.

## Starting a Session

A Request comes from one of three places: typed in the TUI, read from a GitHub issue, or read from a
handover file. The issue and file routes both produce a `Request` and then call the same `start`.

When there is no terminal, `arbor --issue` and `--handover-file` print the Request text and exit,
instead of trying to open the TUI. This is what lets them be scripted.

## The live run

`arbor/src/live.rs` runs the TUI and a live Session behind it. On start it:

1. Loads the project config and the API key.
2. Runs the startup Marker and hook check.
3. Builds the `App` and enters the event loop.

When the operator sends a Request, it creates the Session branch and worktree under
`.arbor/sessions`, creates the `events.jsonl`, and spawns the Session thread. The thread builds the
`Autopilot`, wires the `ScreenOperator` and the event log, and runs it on a small async runtime.

The Session thread sends `DriverMessage`s to the screen: `Started`, `Update`, `Checkpoint`,
`Questions` and `Ended`. The screen turns a `Checkpoint` or `Questions` into a modal and sends the
reply back over a oneshot channel.

## `arbor init`

`arbor init` runs the detection and proposal flow described in
[Configuration](10-configuration.md), then offers to install the Marker git hooks. It can run
without an API key when the detected Checks pass, and only reaches for a model when detection cannot
find working Checks.

## `arbor check-markers`

This is the command the git hooks and CI run. It resolves the tree to scan, works out which commit
supplies the Marker Allowlist, scans, prints the Marker lines and exits 1 if there are any. See
[Markers and the mainline](13-markers-and-mainline.md).

## Exit codes

- 0: success.
- 1: a failure such as a Marker found, an unreadable issue, or a Session that could not start.
- 2: a usage error, or `arbor check-markers` could not run the check at all.
