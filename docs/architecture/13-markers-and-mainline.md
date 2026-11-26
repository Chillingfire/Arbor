# Markers and the mainline

Markers bound each Region, and they exist only until the Work Unit passes. They must never reach the
mainline or a remote. Arbor guards this in four places. This is ADR-0002.

## The Marker scan

`scan_markers` finds every line of a tree that holds a Marker, well-formed or not, outside the paths
the Marker Allowlist names. It uses `git grep` to narrow the search to likely lines, then confirms
each candidate with the same parser the Writer uses, so the scan and the Writer agree on what a
Marker is.

The scan reads a `TreeSource`: the working tree, what is staged, or the tree of a revision.

## The Marker Allowlist

Some files hold Markers on purpose, such as tests or docs about Markers. Those paths go in a
committed `.arbor-markers-allowed` file at the repository root: gitignore-style patterns, one per
line, with `#` comments.

The check never reads the file from what it scans, so a change cannot allowlist its own Markers. It
reads the file from a trusted commit:

- When it checks the working tree or the index, it reads `HEAD`.
- When it checks a revision, as the pre-push hook does, it reads the mainline tip, which only
  reviewed changes reach.

`--allowlist-rev` names another commit, such as the base branch in CI. With no such commit, every
path is checked. A pattern that would allow every path, such as `*` or `**`, is an error, and the
scan warns each time a pattern allows more than half of the tracked files. The list only narrows
where the check looks; it can never switch the check off everywhere.

## The end-of-session check

When every Task is done, the Session checks the branch's committed diff for leftover Markers. A line
that only mentions `ARBOR:`, for example in a doc or a constant, is not a leftover. If any remain,
the Session cannot finish. The operator either removes them and Arbor checks again, approves them as
they are, or stops. This check ignores the allowlist, because a leftover Marker in a Work Unit's own
file is exactly what it is looking for.

## `arbor check-markers`

The `arbor check-markers` command prints every Marker line in a tree as `file:line: text` and exits
1 if there are any, so git hooks and CI can refuse them. It supports `--staged`, `--rev <rev>`,
`--allowlist-rev <rev>` and `--allow-session-worktree`. When it cannot run the check at all, it
exits 2, which a hook treats as a refusal too.

## The git hooks

`arbor init` offers to install three hooks that run `arbor check-markers` for git commands run
outside Arbor:

- `pre-commit` and `pre-merge-commit` run the staged check.
- `pre-push` checks each ref being pushed, so a fast-forward merge, which makes no commit, is still
  caught at the push.

A hook fails closed: if `arbor` is not on `PATH`, the hook refuses rather than let Markers by.
Arbor never overwrites a hook the operator already has; it prints the lines to add instead, as it
does when `core.hooksPath` is set.

Arbor records which hooks it installed in the shared git directory, and the startup check notices a
missing or edited one and offers to reinstall it. A hook the operator changed is kept as
`<name>.arbor-replaced` before it is replaced.

## The startup check

Every `arbor` run checks the operator's checkout and the mainline tip for Markers before the TUI
opens. If it finds any, it prints a warning and waits for Enter, so the warning is read before the
TUI covers it. This check cannot be turned off.

A checkout on a Session branch is skipped, because a Session's own commits hold Markers by design.
Lines found in both the checkout and the mainline are listed once, under both names.

## CI

For a server-side check, a CI step can run:

```sh
git fetch --depth=1 origin main
arbor check-markers --rev HEAD --allowlist-rev FETCH_HEAD
```

This lists every Marker line in the pushed commits and fails if there are any, using the mainline's
allowlist rather than the change's own.
