# Tools

The agentic Tiers, the Planner and the Scaffolder, call tools through `arbor-tools`. Every tool is
confined to the Session worktree. There is deliberately no shell tool. This is ADR-0008.

## Which Tier gets which tools

The Planner may only read. The Scaffolder reads and writes.

| Tool | Planner | Scaffolder | What it does |
| --- | --- | --- | --- |
| `read_file` | yes | yes | Reads a text file with numbered lines, at most 400 lines at a time. |
| `list_dir` | yes | yes | Lists a directory's entries, skipping files the project ignores. |
| `glob` | yes | yes | Finds files whose paths match a glob, skipping ignored files. |
| `grep` | yes | yes | Searches file contents for a regular expression. |
| `write_file` | no | yes | Creates or overwrites a file in the worktree. |
| `edit_file` | no | yes | Replaces one exact snippet in a file. |
| `run_checks` | no | yes | Runs one configured Check in the worktree. |

`Tools::call` refuses the Scaffolder-only tools for the Planner before it does anything, so the
Planner cannot write even if the model asks it to. The Planner's run type also asserts at start that
its toolset is the read-only one.

## The worktree boundary

`Tools` holds a `Worktree`, and every path the model supplies is resolved inside it. A path that
climbs out with `..` is refused. This is the boundary that keeps an autonomous model inside the
Session's own checkout.

## Checks

`run_checks` runs one of the Checks the project configured in `arbor.toml`. There are three:
`scaffold` (does the Skeleton build), `unit` (build and run all tests) and `unit_filtered` (run only
the tests matching a filter). The command comes from config; the model chooses which check and, for
`unit_filtered`, the filter.

A Check is run without a shell. Arbor splits the configured command into a program and arguments
with `shell-words`, then replaces the `{filter}` placeholder in the arguments. Splitting first means
a filter the model supplied is never parsed as shell syntax. A filter that starts with `-` is
refused, so the model cannot pass the test runner an option. The program is found on `PATH`, or
relative to the worktree.

A Check that runs longer than ten minutes is killed. Output is trimmed to its first and last 100
lines, where compilers and test runners put what matters. A Check that cannot run at all, for
example because its program is missing, is an error, not a failed Check.

## Reading the repository

`read_file`, `list_dir`, `glob` and `grep` skip files the project ignores, using the `ignore` and
`globset` crates. Results are capped so one call cannot flood the model's context. This is the same
set of files the operator's own tools would see, minus anything gitignored.

## Why no shell

The Planner and Scaffolder are autonomous models. If either could run arbitrary commands, a
misdirected run could do anything to the operator's machine. Restricting the Planner to reads and
the Scaffolder to worktree writes plus configured Checks bounds what a run can touch. A command
allowlist could be added later, but an open shell is deliberately absent.
