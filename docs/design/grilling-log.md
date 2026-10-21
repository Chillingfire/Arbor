# Arbor design grilling log

Working notes from the design grilling sessions. Once the grilling is finished, settled terms move into `CONTEXT.md` and settled decisions into `docs/adr/`.

**Status:** Grilling complete; shared understanding confirmed on 2026-10-20. Terms are in `CONTEXT.md` and key decisions in `docs/adr/` (0001–0010). This log keeps the full detail.

## Concept

Arbor is a harness that splits programming work across three LLM tiers:

- **Planner** (high level, Opus-class): reasons about and plans the change.
- **Scaffolder** (medium level, Sonnet-class): plans the specific code changes and writes skeletons.
- **Writer** (low level, Haiku-class): writes the actual code.

Work flows planning → scaffolding → code writing.

## Settled decisions

### Round 0: Foundations

- **Form factor**: a TUI, shipped as a single binary that users can run right away.
- **Language**: Rust.
- **Providers**: OpenRouter. Any OpenRouter model can fill any role. Defaults: Opus 5.5 (planner), Sonnet 5.5 (scaffolder), Haiku 5.5 (writer).
- **Autonomy**: the user approves at every tier, with checkpoints after planning and after scaffolding.

### Round 1

1. **How models touch the codebase**: hybrid.
   - The planner and scaffolder are agentic, with tools.
   - The writer is as deterministic and non-agentic as possible, at the lowest temperature available.
   - The writer gets only the context the scaffolder packs into its work unit.
2. **Handoffs**: structured data (serde types), not free-form prose.
   - Planner → **Plan**: a goal, constraints, and an ordered list of **Tasks**.
   - Scaffolder → for each Task, a list of **Work Units**. Each covers one file and one function or region, and carries a precise spec, the context snippets the writer needs, and acceptance criteria.
   - Writer → one Work Unit implemented.
3. **Scaffolding writes real skeletons into the repo**: types, signatures, and placeholder bodies. Because the scaffolder runs at a higher temperature, every skeleton must pass a deterministic error/type check.
4. **Target languages**: any. Arbor itself knows nothing about specific languages.
   - TODO: tree-sitter integration, with a list of supported languages.
5. **Verification and escalation**: check commands are configured per project in `arbor.toml`.
   - When a writer's work fails a check, the writer retries with the error output, up to **N=3** times.
   - After that, the work escalates to the scaffolder, which revises the spec.
   - After that, to the planner, and then to the user.
   - Every escalation appears as an event in the TUI.
6. **Git**:
   - Each new feature at the planning stage gets its own branch off mainline, unless the user specifies otherwise.
   - Pull the latest mainline before branching.
   - Arbor works in a dedicated git worktree and commits after each finished work unit.
   - If there are unstaged or staged changes, Arbor tells the user beforehand and lets them decide what to do.
7. **Checkpoint actions**:
   - Approve.
   - Edit directly: open the plan or spec in `$EDITOR`, or edit the stub code in the worktree.
   - Reject with feedback, which sends a note back to the same tier to regenerate.
   - When approving a plan, the user can also drop or reorder Tasks.
8. **Session persistence**:
   - Each session lives in `.arbor/sessions/<id>/` with the plan, specs, transcripts, and status per tier.
   - Sessions can be resumed.
   - The TUI has options for going back to a previous session.
9. **Cost**: the status bar shows cost and tokens per tier, live. An optional session budget cap pauses the session at a checkpoint when it's reached.
10. **Config**:
    - The `OPENROUTER_API_KEY` env var comes first, with a fallback to the user-level config (`~/.config/arbor/config.toml`).
    - If neither has a key, the TUI pops up a prompt to enter one.
    - Model per role is set in user config, can be overridden in project `arbor.toml`, and can be switched for a session from the TUI.

### Round 2

1. **Writer output**: the writer returns only the code for its region, plus a couple of lines of surrounding context. Temperature 0, with a fixed `seed` where the provider honors it. Output that isn't a single fenced code block is rejected, and the rejection counts as one of the 3 retries. The context lines are used as a sanity check (see 2).
2. **Region location: hybrid.**
   - The scaffolder brackets each placeholder with marker comments in the target language's comment syntax, e.g. `// ARBOR:BEGIN WU-7` … `// ARBOR:END WU-7`. Markers exist only in the session worktree.
   - The writer never touches markers. Arbor does the splice.
   - The writer's returned context lines must match the text around the marker exactly. Otherwise the output is rejected and costs one retry.
   - A missing, duplicated, or mismatched marker fails the scaffold check.
   - A marker is stripped once its Work Unit passes. At session end, Arbor greps the branch diff for `ARBOR:` and refuses to finish while any remain.
   - Intermediate commits may contain markers, so squashing is offered by default at merge/PR time.
   - Later, tree-sitter may replace markers for the languages it supports.
3. **Scaffold checks**: `arbor.toml` has `checks.scaffold` (build/typecheck only) and `checks.unit` (build and tests). After scaffolding, Arbor runs the scaffold checks and validates region locations. On failure, the scaffolder auto-retries up to 3 times before the checkpoint. If no checks are configured, Arbor warns once and skips them.
4. **Scaffolder revisions**:
   - A revision is triggered when a writer fails a Work Unit 3 times.
   - Input: the spec, the skeleton, all failed outputs, and their check errors.
   - Output: a diagnosis of why the writer failed, plus a material change: more context snippets, a tighter spec or acceptance criteria, fixed types or signatures, or a split into smaller Work Units. A revision that only rewords is rejected and counts toward the limit.
   - Scope: only that Work Unit's file and region. A fix that needs a signature or type other Work Units depend on goes straight to the planner.
   - Effort: one agentic run with a configurable tool-call budget (default 15).
   - Revision 2 sees what revision 1 tried and must take a different approach.
   - Limit: 2 revisions, then escalate to the planner.
   - Revisions show only in the event log, with no per-revision checkpoint. There is a user checkpoint before anything escalates to the planner. A config flag can turn on per-revision checkpoints.
5. **Tools**:
   - Planner: read-only (`read_file`, `list_dir`, `glob`, `grep`).
   - Scaffolder: the same, plus `write_file`/`edit_file` (restricted to the worktree) and `run_checks` (only the configured commands).
   - No arbitrary shell in v1.
6. **Git**:
   - Mainline comes from `origin/HEAD`, can be overridden in `arbor.toml`, and can be picked at session start.
   - `git fetch`, then branch from `origin/<mainline>`. Arbor never modifies the local mainline. With no remote, it uses local mainline and tells the user.
   - Dirty tree: Arbor warns and offers stash, commit, proceed without them, or abort.
   - Session end: keep the branch, merge locally, or push and open a PR.
   - Branch naming: `arbor/<short-feature-slug>`.
   - **Hard rule: any action that leaves the local machine (push, PR creation, anything touching a remote beyond a read-only fetch) blocks on explicit human approval, every time.** OpenRouter model calls are exempt.
7. **Going back**: both a session browser (list, inspect, resume) and an in-session rewind to a checkpoint's commit. A rewind keeps the abandoned transcript.
8. **API key**: if the env var or the user config already has a key, never prompt. Otherwise the popup offers "save to user config" (restrictive permissions) or "this session only". The key is validated with a cheap OpenRouter call. TODO: OS keychain.
9. **Writers**: single-threaded in v1. TODO: parallel writers, each in its own worktree.
10. **TUI layout**: provisional. Tree on the left, detail pane on the right, event log at the bottom, status bar, checkpoints as modals. The user may change it later.

### Round 3

1. **Planner escalation**:
   - The planner gets the full failure chain (writer errors, scaffolder diagnoses, and revisions).
   - It outputs a revised Task, or several, through the normal plan checkpoint.
   - On approval, Arbor rewinds the worktree to that Task's start commit and scaffolds again.
   - Limit: 1 replan per Task, then the user.
2. **User escalation modal**:
   - Fix it yourself, then mark the Work Unit done (Arbor runs `checks.unit` first).
   - Hint and retry from a chosen tier.
   - **Retry with a stronger model** (default: the scaffolder's model). The model and the estimated cost are shown clearly before running. It runs in the writer role: same packed context, same region-only output contract and checks. Only the model changes. It also gets a short failure brief that Arbor assembles mechanically, with no extra LLM call: the latest check errors (trimmed), the scaffolder diagnoses, and any "insufficient context" note. The brief has its own 2k-token allowance on top of the 8k cap. The result is tagged with its model in the event log.
   - Skip, with a warning about dependents.
   - Abort, keeping the branch.
3. **Structured output**: the planner and scaffolder finish by calling a submit tool (`submit_plan`, `submit_work_units`) whose parameter schema is generated from the serde types (`schemars`). A parse error goes back to the model as the tool result, up to 3 times. Models without tool calling are rejected for these roles.
4. **Writer context cap**: hard cap on writer input tokens, default 8k, configurable. Over-cap Work Units fail the scaffold check. No silent truncation.
   - The scaffolder's prompt states the cap.
   - `submit_work_units` returns the estimated size per Work Unit, e.g. "WU-7: 9,340 / 8,000 — over, trim or split", so the scaffolder decides what to cut.
   - Context snippets carry a priority (`required` / `helpful`).
   - Arbor estimates tokens locally with a 10% margin. Actual counts from OpenRouter are logged for tuning.
   - **Insufficient context**: instead of code, the writer may return a structured "insufficient context: <what's missing>" response. This skips its remaining retries and goes straight to a scaffolder revision, with the request as input. Allowed once per Work Unit.
5. **Agentic transcript growth**:
   - `read_file` caps at 400 lines with offset/limit paging. `grep` and `glob` results are capped with a "N more" note.
   - At 80% of the model's context window (taken from OpenRouter metadata), Arbor tells the model to submit now.
   - No automatic summarization in v1.
6. **Temperature and seed**: Arbor reads `supported_parameters` from OpenRouter, and writer calls send `provider.require_parameters: true`. If unsupported, Arbor warns and runs anyway.
   - Static range check: 0–2 (0–1 for Anthropic models).
   - Probe: one `max_tokens: 1` call per (model, temperature), cached in the user config dir. An error blocks the session with a clear message. The event log notes that providers may clamp silently.
   - **No cached probe for the current config** (first run or changed config): Arbor asks the user to either (a) set the model and temperature per role manually, or (b) use the recommended default config per role. Then it probes. If the probe fails, the dialog comes back.
7. **Prompts**: defaults are compiled into the binary, with project overrides in `.arbor/prompts/<tier>.md`. The raw prompt text for each tier is copied into `.arbor/sessions/<id>/prompts/`. No hashing.
   - **Retention**: on startup, sessions whose last event is older than 30 days (configurable; `0` = never) are removed: the session folder and its worktree. The git branch is always kept. Sessions whose worktree has uncommitted changes are skipped with a warning. A one-line notice lists what was pruned.
8. **Persistence**: an append-only `events.jsonl` is the source of truth, and each event is written before it is shown. Resume replays the log. An in-flight call is redone and its cost counted. Rewind and the event log pane derive from it.
9. **Budget cap**: Arbor lets the current call finish, then opens a budget checkpoint (raise the cap or stop). It never cuts off a call partway.
10. **Crate layout**: a Cargo workspace with `arbor-core` (pure domain types and the state machine: events in, effects out), `arbor-llm`, `arbor-tools`, `arbor-git`, `arbor-tui` (ratatui), and `arbor` (the binary).

### Round 4

1. **Session input**: a free-text request typed in the TUI, or `arbor --issue <n>`, which pulls the issue's title, body, and comments via `gh` (read-only, so no approval gate). The planner may ask clarifying questions, shown as a checkpoint-style modal, before submitting the plan.
2. **Tests**: the scaffolder writes real, complete tests during scaffolding, reviewed at the scaffold checkpoint. Its prompt pushes for compact, table-driven tests where the language allows. Writers never write or edit tests. This is the only mode in v1.
   - Rationale: Sonnet output costs 20× Haiku's ($10 vs $0.50 per 1M on OpenRouter), but that's only cents per Task, while one bad test can trigger a whole retry/revision chain. A writer that writes tests would also get to define "passing".
   - TODO: a hybrid where the scaffolder writes signatures plus a case table and writers expand them, if real sessions show test cost is significant.
3. **Per-Work-Unit verification**: `checks.scaffold` plus only that Work Unit's tests. The scaffolder gives each Work Unit a test filter, substituted into a `checks.unit_filtered` template in `arbor.toml`. With no template, Arbor runs build/typecheck only.
4. **Task completion**: the full `checks.unit` runs after each Task. On failure, the scaffolder gets the output and the Task diff and revises the affected Work Units (2 revisions), then the user checkpoint, then the planner.
5. **Automatic init**: when a session starts with no `arbor.toml`:
   1. Arbor detects the project type from manifest files (`Cargo.toml`, `package.json`, `pyproject.toml`, `go.mod`, …).
   2. It runs each detected command once in the worktree to validate it.
   3. If all pass, it writes `arbor.toml` silently, with a one-line notice.
   4. If detection or validation fails, a read-only scaffolder run inspects the repo, README, and CI config and proposes commands. The user confirms them once in a modal, or skips, and then checks are skipped with the usual warning.
   - `arbor init` stays available for re-running manually.
6. **Testing Arbor**: `arbor-core` gets plain unit tests. `arbor-llm` sits behind a trait, with a scripted fake replaying recorded fixtures. A few end-to-end tests run against a sample repo with the fake LLM, real git, and real checks. Live OpenRouter tests are opt-in behind a flag.
7. **Distribution**: prebuilt binaries for Windows, macOS (arm64/x64), and Linux (x64/arm64) on GitHub Releases via CI, plus `cargo install arbor`.
8. **Commit messages**: ordinary commits with no mention of Arbor or models and no trailers. The subject is one short sentence, optionally followed by a brief description. Authored with the user's git identity. Which model wrote what lives only in the session event log.
9. **Recommended defaults**: planner Opus 5.5 at temperature 0.5, scaffolder Sonnet 5.5 at 0.3, writer Haiku 5.5 at 0. The failure brief allowance is 2k tokens.

### Round 5: Prompts

1. **Assembly**: prompt templates per tier are compiled in (`include_str!`) and rendered with `minijinja`. Variables include `request`, `plan`, `work_unit`, `cap_tokens`, and `conventions`. Every template has four sections: Role, Inputs, Output contract, Rules. Overrides in `.arbor/prompts/<tier>.md` use the same variables, and an unknown variable fails at startup.
2. **Conventions**: the planner and scaffolder automatically get `AGENTS.md`, `CLAUDE.md`, `CONTEXT.md`, and the README if present, capped at 4k tokens total. More files can be added with `context.files` in `arbor.toml`. The writer gets only what the scaffolder packed.
3. **Planner rules**:
   - Each Task leaves the build green and its tests passing.
   - Aim for 1–6 Tasks. More than that means the planner suggests splitting the request.
   - It names files and modules but never writes code.
   - **Clarification mode**: a setting, toggled in the TUI, between **Ask** (the default: lean towards asking the human whenever something is unclear) and **Assume** (make reasonable assumptions and record them in the Plan's constraints). 
     - Ask is the default. It's set in user config, overridable in `arbor.toml`, and switchable mid-session in the TUI. The status bar shows the current mode.
     - It applies to the planner and the scaffolder. The writer is never affected: its only channel is the `missing` response.
     - Ask mode batches questions into one modal per tier run.
     - Assume mode records every assumption in the Plan's or Work Unit's constraints and highlights them at the checkpoint.
4. **Scaffolder rules**:
   - Work Units are one function or region, aiming for ≤ ~60 lines of writer output. Bigger ones get split.
   - Placeholders must compile, using the language idiom, with markers in the language's comment syntax.
   - The scaffolder writes all types, signatures, and imports, so writers only fill bodies.
   - Tests are compact and table-driven, with one filter per Work Unit.
   - `required` snippets are packed first, and the writer is assumed to know nothing else.
5. **Writer output**: exactly one fenced block. Either code (```` ```<lang> ````) with exactly 2 unchanged lines of context before and after the region, or ```` ```missing ```` listing what's needed, one item per line. Anything else is rejected and costs one retry.
6. **Prompt caching**: static parts come first (template → conventions → session inputs → transcript). Cache breakpoints (`cache_control` on Anthropic via OpenRouter) go after the conventions and on the growing transcript. The status bar shows cache-hit tokens separately.
7. **Prompt evals**: a dev-only `xtask eval` runs fixed requests on sample repos with real models and reports pass rate, escalations, and cost per tier. It is run manually before changing default prompts, and results are saved for comparison.

### Post-grilling TODO: decision model between Tiers

TODO: add a dedicated **decision model** at planning time or at the hand-offs between Tiers, e.g. [Jev from TypeSafe AI](https://letsdatascience.com/news/typesafe-ai-launches-jev-decision-model-889a38c0). Jev returns typed choices, scores, or probabilities with a confidence value, not text. It launched in early access in September 2026, and its speed and cost claims are only the vendor's own.

Candidate decision points (not yet decided):
- Routing a Work Unit to a Writer model by estimated difficulty.
- Choosing the next step after a failure: retry, send a Missing Response, revise, or escalate.
- Whether an ambiguity is worth an Ask in Clarification Mode.
- Guardrail scoring of Writer output before running the Checks.

Constraints to respect:
- A decision model must never replace a Checkpoint. The Human Operator still approves at every Tier.
- The target is the Jev model itself, `typesafe/jev-1.13` on OpenRouter (https://openrouter.ai/typesafe/jev-1.13), not `typesafe/jev-router`. Being on OpenRouter means it fits ADR-0007 (OpenRouter only). On 2026-10-20 the public `/api/v1/models` listing returned only `typesafe/jev-router`, probably because of early-access gating, so confirm `jev-1.13` can be called with a normal key when this is picked up. It would be a fourth configured role, not a Tier.
- It needs a fallback when the model returns a label outside the valid set, or confidence is low.

## Still to come

- **Next step (paused 2026-10-20):** create the GitHub issues. 30 drafts (24 implementation + 6 TODOs, with blockers) are in `docs/design/create-issues.py`.
  1. Log in: `& "C:\Program Files\GitHub CLI\gh.exe" auth login` (gh 2.102.0 was installed via winget, and a new shell may have it on PATH as `gh`).
  2. Preview: `python docs/design/create-issues.py gh --dry-run`.
  3. Create: `python docs/design/create-issues.py "C:\Program Files\GitHub CLI\gh.exe"` (also creates missing triage labels).
  - The design branch `design/initial-design` stays unpushed until the operator says otherwise.
