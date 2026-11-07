"""Draft GitHub issues for Arbor v1. Run with: python docs/design/create-issues.py <path-to-gh.exe> [--dry-run]

Creates labels if missing, creates issues in order, then rewrites "Blocked by" placeholders
to real issue numbers and adds native dependencies where available.
"""
import json
import subprocess
import sys

DOCS = "Design references: `docs/design/grilling-log.md`, `CONTEXT.md`, `docs/adr/`."

# key, title, labels, blocked_by keys, body
ISSUES = [
    ("ws", "Set up Cargo workspace and CI", ["ready-for-agent"], [], """
Create the Cargo workspace from ADR-0005: `arbor-core`, `arbor-llm`, `arbor-tools`, `arbor-git`, `arbor-tui`, and the `arbor` binary, plus an `xtask` crate for dev tooling.

## Acceptance criteria
- `cargo build` and `cargo test` pass on an empty workspace.
- `arbor-core` has no I/O dependencies (no tokio, reqwest, or fs-heavy crates).
- CI runs build, test, clippy (`-D warnings`), and fmt on Windows, macOS, and Linux.
"""),
    ("core", "Domain types and Session event log in arbor-core", ["ready-for-agent"], ["ws"], """
Define the serde domain types (Request, Plan, Task, Work Unit with spec / Context Pack snippets with `required`/`helpful` priority / test filter) and the Session event model. The core is a pure state machine: events in, effects out (ADR-0005).

The Session's source of truth is an append-only `.arbor/sessions/<id>/events.jsonl` (ADR-0004). Each event is persisted before it is shown.

## Acceptance criteria
- Types derive `Serialize`, `Deserialize`, and `JsonSchema` (`schemars`).
- An event log writer appends and flushes. Replay rebuilds identical state.
- The state machine has unit tests for the happy path (plan → scaffold → write → done) with no I/O.
"""),
    ("llm", "OpenRouter client behind a trait, with a scripted fake", ["ready-for-agent"], ["ws"], """
`arbor-llm`: a chat-completions client for OpenRouter (ADR-0007) behind a trait, plus a scripted fake that replays recorded fixtures for tests.

## Acceptance criteria
- Supports tool calling, temperature, seed, `provider.require_parameters`, and `cache_control` pass-through.
- Fetches model metadata (context length, `supported_parameters`, pricing) and caches it per run.
- Returns usage (input, output, and cache-read tokens) and cost per call.
- The fake can record a real session to fixtures and replay it deterministically.
- Live tests run only behind an opt-in flag.
"""),
    ("cfg", "Config loading and API key resolution", ["ready-for-agent"], ["ws"], """
User config at `~/.config/arbor/config.toml` (platform-appropriate equivalent on Windows), plus project `arbor.toml`.

## Acceptance criteria
- API key order: `OPENROUTER_API_KEY` env var, then user config. If either exists, never prompt.
- Model + temperature per role in user config, overridable in `arbor.toml`. Defaults: planner Opus 5.5 @ 0.5, scaffolder Sonnet 5.5 @ 0.3, writer Haiku 5.5 @ 0.
- `arbor.toml` schema: `checks.scaffold`, `checks.unit`, `checks.unit_filtered` (template with a test-filter placeholder), mainline override, `context.files`, writer cap (default 8k), failure brief allowance (default 2k), retention days (default 30, 0 = never), clarification mode (default Ask).
- Saving a key to user config uses restrictive file permissions where supported.
"""),
    ("git", "Git adapter: mainline, worktree, commits", ["ready-for-agent"], ["ws"], """
`arbor-git`, per the Round 1 and Round 2 git decisions.

## Acceptance criteria
- Mainline from `origin/HEAD`, overridable in `arbor.toml`, selectable at session start.
- `git fetch`, then branch `arbor/<short-feature-slug>` from `origin/<mainline>` into a dedicated worktree. Never modifies local mainline. With no remote, uses local mainline and reports it.
- Detects a dirty tree and exposes stash / commit / proceed without / abort as options.
- Commits use the operator's git identity. The message is one short sentence plus an optional brief description, with no mention of Arbor or models (ADR-0009).
- Can reset the worktree to a recorded commit (for Rewind and Replan).
- No push or other remote-writing operation exists in this crate without an explicit approval token argument (ADR-0006).
"""),
    ("tools", "Agent tools confined to the worktree", ["ready-for-agent"], ["ws"], """
`arbor-tools` for the Planner and Scaffolder (ADR-0008).

## Acceptance criteria
- Planner: `read_file` (max 400 lines, offset/limit paging), `list_dir`, `glob`, `grep` (capped, with a "N more" note).
- Scaffolder: the same, plus `write_file` / `edit_file` restricted to the Session worktree (paths escaping it are rejected, including via symlinks), plus `run_checks`, which runs only the commands configured in `arbor.toml`.
- No arbitrary shell tool.
"""),
    ("prompts", "Prompt templates and Conventions loading", ["ready-for-agent"], ["ws", "cfg"], """
Round 5 decisions 1, 2, and 6.

## Acceptance criteria
- A template per Tier, compiled in with `include_str!` and rendered with `minijinja`. Sections: Role, Inputs, Output contract, Rules.
- Overrides in `.arbor/prompts/<tier>.md`. An unknown variable fails at startup.
- The raw rendered prompt text per Tier is copied into `.arbor/sessions/<id>/prompts/`.
- Conventions: `AGENTS.md`, `CLAUDE.md`, `CONTEXT.md`, and the README if present, plus `context.files`, capped at 4k tokens total, given to the Planner and Scaffolder only.
- Prompt order is static first (template → conventions → session inputs → transcript) so it can be cached.
"""),
    ("planner", "Planner Tier: Request to Plan via submit_plan", ["ready-for-agent"], ["core", "llm", "tools", "prompts"], """
An agentic Planner with read-only tools that ends by calling `submit_plan`, whose schema is generated from the Plan type.

## Acceptance criteria
- A schema or serde error goes back as the tool result, up to 3 times, then escalates.
- Models without tool calling are rejected for this role at config time.
- Prompt rules: each Task leaves the build green and its tests passing, 1–6 Tasks (suggest splitting otherwise), no code.
- Clarification Mode: in Ask, questions are batched into one request to the operator per run. In Assume, assumptions are recorded in the Plan's constraints and flagged for the checkpoint.
- At 80% of the model's context window, Arbor tells the model to submit now.
"""),
    ("scaffolder", "Scaffolder Tier: Skeleton, Markers, tests, Work Units", ["ready-for-agent"], ["planner"], """
Round 1 decisions 2–3, Round 2 decisions 2–3, Round 3 decision 4, Round 4 decisions 2–3, Round 5 decision 4. ADR-0002, ADR-0003.

## Acceptance criteria
- Writes compilable Skeletons with language-idiomatic Placeholders bracketed by `ARBOR:BEGIN/END WU-n` Markers in the target language's comment syntax.
- Writes complete, compact tests. Each Work Unit gets a test filter.
- `submit_work_units` returns the estimated size per Work Unit against the Writer Cap (10% margin) and rejects over-cap ones with exact numbers.
- Scaffold validation: run `checks.scaffold` and check every Marker pair (missing, duplicated, or mismatched fails). Auto-retry up to 3 times with the errors before the checkpoint. If no checks are configured, warn once and skip.
- Follows Clarification Mode like the Planner.
"""),
    ("writer", "Writer Tier: fill one Region and splice it", ["ready-for-agent"], ["scaffolder"], """
ADR-0001, Round 2 decision 1, Round 3 decision 4, Round 5 decision 5.

## Acceptance criteria
- A single non-agentic call: temperature from config (0 by default), fixed seed, `provider.require_parameters: true`. Input is only the Context Pack.
- Output is exactly one fenced block: code with exactly 2 unchanged context lines before and after (which must match the file around the Markers), or a ```` ```missing ```` block. Anything else is rejected and counts as a retry.
- Arbor splices the code between the Markers, runs `checks.scaffold` plus `checks.unit_filtered` with the Work Unit's filter (build/typecheck only when no template), strips the Markers on success, and commits.
- Writers run sequentially (ADR-0010).
"""),
    ("task-done", "Task completion and end-of-session Marker check", ["ready-for-agent"], ["writer"], """
## Acceptance criteria
- After the last Work Unit of a Task, run the full `checks.unit`.
- On failure, the Scaffolder gets the output and the Task diff and revises the affected Work Units (same limits as a Work Unit Revision).
- At session end, grep the branch diff for `ARBOR:`. The Session can't finish while any Marker remains.
"""),
    ("escalation", "Escalation chain: retries, Revisions, Replan", ["ready-for-agent"], ["task-done"], """
Round 1 decision 5, Round 2 decision 4, Round 3 decision 1.

## Acceptance criteria
- Writer: up to 3 retries with error output. A Missing Response (allowed once per Work Unit) skips the remaining retries and goes straight to a Revision.
- Revision: diagnosis plus a real change (rewording only is rejected and counts). Scope is one Work Unit. A shared-signature change goes straight to the Planner. Tool-call budget of 15, configurable. Revision 2 must take a different approach. Limit 2. Logged as events, with no per-revision checkpoint unless the config flag is set.
- Checkpoint before anything escalates to the Planner.
- Replan: the Planner gets the full failure chain and outputs revised Task(s) through the plan checkpoint. The worktree resets to the Task's start commit. Limit 1 per Task, then the operator.
"""),
    ("tui", "TUI shell: tree, detail pane, event log, status bar", ["ready-for-human"], ["core"], """
ratatui. The layout is provisional (Round 2 decision 10), and the operator may change it.

## Acceptance criteria
- Left: a collapsible Plan → Tasks → Work Units tree with status icons. Right: detail pane (spec / transcript / diff). Bottom: event log, driven by the Session event log.
- Status bar: cost and tokens per Tier (cache-hit tokens shown separately), Budget Cap, active model per Tier, Clarification Mode.
- Free-text Request entry to start a Session.
"""),
    ("checkpoints", "Checkpoint and operator modals", ["ready-for-agent"], ["tui", "planner"], """
Round 1 decision 7, Round 3 decision 2, Round 5 decision 3.

## Acceptance criteria
- Checkpoint modal: approve, edit (Plan/spec in `$EDITOR`, or stubs in the worktree), reject with feedback (the same Tier regenerates). Plan approval also allows dropping or reordering Tasks.
- Assume-mode assumptions are highlighted at the checkpoint.
- Clarifying-question modal for Ask mode, plus an Ask/Assume toggle switchable mid-session.
- Escalation-to-operator modal: fix it yourself (then `checks.unit` must pass), hint and retry from a chosen Tier, retry with a stronger model, skip (warn about dependents), abort (keep the branch).
- Stronger-model retry: writer role, same Context Pack, plus a mechanically assembled Failure Brief (2k allowance). The model and estimated cost are shown before running, and the result is tagged with its model in the event log.
"""),
    ("sessions", "Session browser, resume, Rewind, and pruning", ["ready-for-agent"], ["core", "git", "tui"], """
Round 2 decision 7, Round 3 decisions 7–8.

## Acceptance criteria
- A browser listing past Sessions with status and cost. Inspect or resume one. Resume replays `events.jsonl`, and an in-flight model call is redone.
- Rewind to any Checkpoint resets the worktree to that Checkpoint's commit. The abandoned transcript is kept.
- On startup, prune Sessions whose last event is older than the configured retention: the session folder and worktree are removed, the branch is always kept, dirty worktrees are skipped with a warning, and a one-line notice lists what was pruned.
"""),
    ("budget", "Cost accounting and Budget Cap", ["ready-for-agent"], ["llm", "core"], """
## Acceptance criteria
- Live cost and tokens per Tier from reported usage, including cache reads.
- Optional Budget Cap per Session. When reached, the current call finishes, then a budget checkpoint opens (raise the cap or stop). A call is never aborted partway.
"""),
    ("probe", "Model and temperature validation on first run", ["ready-for-agent"], ["llm", "cfg", "tui"], """
Round 3 decision 6.

## Acceptance criteria
- Static range check: 0–2 (0–1 for Anthropic models). Warn if a model lacks temperature/seed in `supported_parameters`.
- Probe call (`max_tokens: 1`) per (model, temperature), cached in the user config dir. A probe error blocks with a clear message, and the event log notes that providers may clamp silently.
- With no cached probe for the current config: a dialog to (a) set model and temperature per role manually or (b) use the recommended defaults. Then probe, and show the dialog again on failure.
- API key popup when none is configured: save to user config or use for this session only. Validate with a cheap call first.
"""),
    ("init", "Automatic arbor.toml init", ["ready-for-agent"], ["cfg", "tools", "scaffolder"], """
Round 4 decision 5.

## Acceptance criteria
- With no `arbor.toml`, detect the project type from manifests (`Cargo.toml`, `package.json`, `pyproject.toml`, `go.mod`, …) and validate the detected commands by running them once in the worktree.
- All pass: write `arbor.toml` silently with a one-line notice.
- Otherwise: a read-only Scaffolder run proposes commands from the repo, README, and CI config. The operator confirms once, or skips.
- `arbor init` command for re-running manually.
"""),
    ("end", "Session end: keep, merge, or push and open a PR", ["ready-for-agent"], ["git", "checkpoints"], """
Round 2 decision 6, ADR-0006.

## Acceptance criteria
- Options: keep the branch, merge into mainline locally, or push and open a PR (via `gh` if installed). Squash is offered by default.
- Push and PR creation always block on explicit operator approval at that moment. Approval is never remembered.
"""),
    ("issue-input", "Start a Session from a GitHub issue", ["ready-for-agent"], ["planner"], """
## Acceptance criteria
- `arbor --issue <n>` pulls the title, body, and comments via `gh` (read-only, no approval gate) as the Request.
- A clear error when `gh` is missing or unauthenticated.
"""),
    ("caching", "Prompt caching for agentic Tiers", ["ready-for-agent"], ["planner", "prompts"], """
## Acceptance criteria
- For models that support it, add `cache_control` breakpoints after the Conventions block and on the growing transcript.
- Cache-read tokens are recorded and shown separately in the status bar.
"""),
    ("e2e", "End-to-end tests against a sample repo", ["ready-for-agent"], ["task-done"], """
## Acceptance criteria
- A tiny sample repo fixture. Tests run the full pipeline with the scripted fake LLM, real git, and real checks.
- Covers the happy path, a writer retry, a Missing Response, and a Revision.
"""),
    ("eval", "xtask eval for prompt quality", ["ready-for-agent"], ["e2e"], """
Dev-only, not shipped.

## Acceptance criteria
- Runs fixed Requests against small sample repos with real models.
- Reports pass rate, escalation counts, and cost per Tier. Results are written to a file for comparison across runs.
"""),
    ("release", "Release binaries via GitHub Releases", ["ready-for-agent"], ["ws"], """
## Acceptance criteria
- CI builds and attaches binaries for Windows x64, macOS arm64/x64, and Linux x64/arm64 on tag.
- `cargo install arbor` works.
"""),
    # TODOs from the design, left for triage
    ("todo-jev", "TODO: decision model between Tiers (Jev)", ["needs-triage"], [], """
Add a dedicated decision model (TypeSafe's Jev, `typesafe/jev-1.13` on OpenRouter) at planning time or at the hand-offs between Tiers. It returns typed choices or probabilities with a confidence value instead of text.

Candidate decision points: picking a Writer model by Work Unit difficulty; the next step after a failure (retry / Missing Response / Revision / escalate); whether an ambiguity warrants an Ask; scoring Writer output before the Checks.

Constraints: never replaces a Checkpoint; needs a fallback for out-of-set labels and low confidence; a fourth configured role, not a Tier. Confirm `jev-1.13` is callable with a normal key (the public model list showed only `typesafe/jev-router` on 2026-10-20). Grill this before implementing.
"""),
    ("todo-ts", "TODO: tree-sitter for supported languages", ["needs-triage"], [], """
Use tree-sitter to locate Regions (possibly replacing Markers, ADR-0002) for the languages it supports.
"""),
    ("todo-par", "TODO: parallel Writers, each in its own worktree", ["needs-triage"], [], """
Lift ADR-0010: run Writers in parallel, each in its own worktree, and merge the results.
"""),
    ("todo-keychain", "TODO: store the API key in the OS keychain", ["needs-triage"], [], """
Offer the OS keychain as a third option in the API key popup.
"""),
    ("todo-tests", "TODO: hybrid test authorship", ["needs-triage"], [], """
If real sessions show test cost is significant: the Scaffolder writes test signatures plus a case table, and Writers expand them (see ADR-0003).
"""),
    ("todo-shell", "TODO: allowlisted shell for agentic Tiers", ["needs-triage"], [], """
Possibly add a command allowlist (ADR-0008 forbids an open shell).
"""),
]

LABELS = {
    "needs-triage": "fbca04", "needs-info": "d4c5f9", "ready-for-agent": "0e8a16",
    "ready-for-human": "1d76db", "wontfix": "ffffff",
}


def run(gh, *args):
    return subprocess.run([gh, *args], check=True, capture_output=True, text=True).stdout.strip()


def main():
    gh = sys.argv[1]
    dry = "--dry-run" in sys.argv
    if dry:
        for key, title, labels, blocked, _ in ISSUES:
            print(f"[{key}] {title}  labels={labels}  blocked_by={blocked}")
        print(len(ISSUES), "issues")
        return
    repo = run(gh, "repo", "view", "--json", "nameWithOwner", "--jq", ".nameWithOwner")
    existing = {l["name"] for l in json.loads(run(gh, "label", "list", "--json", "name", "--limit", "200"))}
    for name, color in LABELS.items():
        if name not in existing:
            run(gh, "label", "create", name, "--color", color)
    numbers, ids = {}, {}
    for key, title, labels, blocked, body in ISSUES:
        blocked_line = ""
        if blocked:
            blocked_line = "Blocked by: " + ", ".join(f"#{numbers[b]}" for b in blocked) + "\n\n"
        full = blocked_line + body.strip() + "\n\n" + DOCS
        url = run(gh, "issue", "create", "--title", title, "--body", full,
                  *[a for l in labels for a in ("--label", l)])
        n = int(url.rsplit("/", 1)[1])
        numbers[key] = n
        ids[key] = int(run(gh, "api", f"repos/{repo}/issues/{n}", "--jq", ".id"))
        for b in blocked:
            try:
                run(gh, "api", "--method", "POST", f"repos/{repo}/issues/{n}/dependencies/blocked_by",
                    "-F", f"issue_id={ids[b]}")
            except subprocess.CalledProcessError:
                pass  # "Blocked by" line in the body is the fallback
        print(f"#{n} {title}")


if __name__ == "__main__":
    main()
