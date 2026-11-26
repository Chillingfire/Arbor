# Configuration

`arbor-config` holds the settings. There are two files: the user config, which is per operator, and
the project's `arbor.toml`, which lives in the repository and is shared.

## Where the files live

- User config: `%APPDATA%\arbor\config.toml` on Windows, otherwise
  `$XDG_CONFIG_HOME/arbor/config.toml` or `~/.config/arbor/config.toml`.
- Project config: `arbor.toml` at the repository root.

## How they merge

`Config::from_toml` starts from the defaults, applies the user config, then applies `arbor.toml`.
The project file wins for the settings it sets. Two exceptions protect the operator:

- The API key belongs only in the user config. An `api_key` in `arbor.toml` is an error, so a key
  never lands in a repository.
- The Budget Cap is the lower of the two when both set one. A cloned repository's `arbor.toml`
  cannot loosen the limit on the operator's own money.

## The settings

- `roles`: the model and temperature for each Tier. The defaults are a large model for the Planner
  (temperature 0.5), a mid model for the Scaffolder (0.3) and a small model for the Writer (0.0).
- `clarification_mode`: `ask` or `assume`. See [The agent loop](06-agent-loop.md).
- `checks`: the `scaffold`, `unit` and `unit_filtered` commands. `unit_filtered` must contain the
  `{filter}` placeholder. See [Tools](07-tools.md).
- `mainline`: overrides the mainline branch, which is otherwise taken from `origin/HEAD`.
- `context.files`: extra Conventions files. See [Prompts and Conventions](09-prompts-and-conventions.md).
- `writer_cap_tokens`: the Writer Cap, in estimated tokens. Default 8000.
- `failure_brief_tokens`: the room a Failure Brief may use, on top of the Writer Cap.
- `retention_days`: how long an idle Session is kept before it is pruned. 0 means never.
- `revisions.tool_call_budget`: the tool calls one Revision may make before it must submit.
- `revisions.checkpoints`: pause at a checkpoint after each Revision, not only before the Planner.
  Off by default.
- `budget_cap_usd`: the Budget Cap each Session starts with. None means no cap.

Every field is optional, and an unknown key is an error, so a typo is caught rather than ignored.

## The API key

`resolve_api_key` checks `OPENROUTER_API_KEY` first, then the user config's `api_key`. A blank value
counts as unset. The `ApiKey` type never prints its value in `Debug`, so it cannot leak into a log.
`save_api_key` writes the user config with owner-only permissions where the platform supports it.

## Writing `arbor.toml`

`arbor init` detects the project type and writes its Checks. Detection looks for one manifest at the
repository root: `Cargo.toml` (Rust), `go.mod` (Go), `pyproject.toml` (Python) or `package.json`
(JavaScript). Each type carries the usual commands, for example `cargo check --all-targets` and
`cargo test` for Rust.

The detected commands are run once. If they pass, their Checks are written straight to `arbor.toml`.
If detection finds no single project type, or the commands fail, the Scaffolder reads the repository
and proposes Checks for the operator to accept or skip. Re-running `arbor init` replaces only the
`[checks]` table and leaves the rest of the file as it was. The write refuses to rewrite a file it
cannot read, so a typo never costs the operator their settings.

## Degrading gracefully

If a Check is not configured, Arbor logs a warning once and runs the core feature without it. A
missing `scaffold` check means the Skeleton is not verified to build; a missing `unit` check means
the Task's tests are not run. The Session still runs, with the warning shown. See
[Verification and escalation](12-verification-and-escalation.md).
