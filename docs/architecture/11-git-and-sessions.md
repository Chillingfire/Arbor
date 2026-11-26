# Git and Sessions

`arbor-git` is the adapter to git and to `gh`, the GitHub CLI. Every operation runs the `git` or
`gh` executable directly with an argument list, never through a shell, so the operator's own
identity and configuration apply.

## The mainline and the Session branch

A Session works on its own branch, named `arbor/<slug>` from the first words of the Request, for
example `arbor/add-oauth-login`. The branch is created from the mainline, which is `origin/<default>`
by default, or the branch set in `arbor.toml`, or the local `main` or `master` when there is no
remote. The branch tracks nothing, so it cannot be pushed onto the mainline by accident.

`Repo::mainline` resolves the name and refuses a mainline that does not exist. When there is no
remote, the `Mainline` type records that, and the operator is told the local branch is used.

## The worktree

Each Session gets a dedicated worktree under `.arbor/sessions/<id>/worktree`, checked out on its
branch. Sessions live inside the project so they are easy to find, and `.arbor/sessions` is added to
the shared `info/exclude` so the worktrees do not show up as untracked files.

The worktree is where the whole Session runs. The operator's own checkout is left alone, and
uncommitted changes in it are left out of the Session unless the operator chose to bring them in.

`Worktree::reset_to` puts the branch back to a recorded commit and discards uncommitted changes and
untracked files, keeping ignored files such as build output. This is what Rewind and Replan use.

## Commits

Every accepted state is committed, so `HEAD` is always the state a rejected Revision is discarded
back to. The Skeleton, each passing Work Unit, each accepted Revision, and each operator fix are
commits. A commit uses the operator's git identity and a one-sentence subject. It never mentions
Arbor, Work Unit IDs or models. This is ADR-0009. Which model wrote which code is recorded only in
the Session event log.

`Worktree::commit` stages every change including new and deleted files, and returns `None` without
committing when nothing changed. Commit messages are passed with `--cleanup=whitespace`, so a
description line that starts with `#` is kept.

## Diffs

`Worktree::diff_from` and `committed_diff_from` produce unified diffs. Every option the operator's
diff config could change is pinned, so the same text comes out on every machine. The Task diff, from
the parent of the Task's first commit to the worktree, is what a Revision and a Replan see. The
end-of-session Marker check reads the committed diff from the branch's fork point.

## Remote approval

Arbor never pushes or performs any other remote-writing action without the operator's approval at
that moment, every time. This is ADR-0006. The mechanism is the `RemoteApproval` type: it has a
private field, so it can only be built by `RemoteApproval::granted_by_operator`, and every function
that writes to a remote takes one by value, so it is used up by that one action. A push cannot
happen without one, and one approval cannot cover two pushes. The type's own tests are
`compile_fail` examples that prove this.

Read-only remote calls need no approval. `git fetch` and reading a GitHub issue are the exceptions,
along with the model calls to OpenRouter.

## Reading a GitHub issue

`arbor --issue <n>` reads an issue as a Request through `gh issue view`. The read runs with
`GH_PROMPT_DISABLED` and `NO_COLOR` set, so `gh` never stops to ask the terminal anything under the
TUI. The issue's title, body and comments become the Request text.

Failures are classified into one error per case: `gh` not installed, not authenticated, no GitHub
remote, issue not found, or some other failure. When `gh` is missing, the error says how to install
it and how to start without GitHub, by writing the Request to a file and using `--handover-file`.
The classification is tested with a fake `gh`, so no network is needed.

## What the operator keeps

The mainline and the operator's checkout are never touched. The Session branch is kept even when a
run stops early, so the work can be resumed or inspected. When a Session ends without finishing,
Arbor warns about the Markers left in the branch; see [Markers and the mainline](13-markers-and-mainline.md).
