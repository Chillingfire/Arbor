# The Tiers

This document covers what each of the three Tiers does. The shared turn loop they run on is in
[The agent loop](06-agent-loop.md).

## Planner

The Planner is the highest Tier. It turns a Request into a Plan and never writes code. It has only
read tools.

```rust
pub struct PlannerRun<'a> {
    pub prompt: &'a Prompt,
    pub model: ModelInfo,
    pub temperature: f64,
    pub clarification_mode: ClarificationMode,
    pub tools: Tools,
    pub rejected: Option<Rejection<'a>>,
}
```

Its submit tool is `submit_plan`, whose argument schema is generated from the `Plan` type. A Plan
has a goal, constraints and an ordered list of Tasks. The validator rejects an empty goal or a Plan
with no Tasks.

When the operator rejects a Plan at the Plan Checkpoint, the same Planner runs again with the
rejected Plan and the operator's feedback added to the prompt.

### Replan

A Replan is the Planner reworking a Task that failed after its Revisions. It is a separate run type,
`ReplanRun`, with the submit tool `submit_replan`. It sees the approved Plan, the failed Task, the
failure that used up the Revisions, what the Scaffolder tried, and the Task's diff. It produces a
`Replan`: a diagnosis and the Tasks that replace the failed one. See
[Verification and escalation](12-verification-and-escalation.md).

## Scaffolder

The Scaffolder is the middle Tier. It turns a Task into a compilable Skeleton, the tests for that
Task, and the Work Units the Writers fill. It has read and write tools.

The Scaffolder writes all the tests. This is ADR-0003. Tests are the executable spec the Writers
implement against, and they are reviewed at the Scaffold Checkpoint. Letting the Writer, the model
being graded, write its own tests would let it decide what passing means.

The Skeleton is the compilable structure for the Task: types, signatures, imports and the
Placeholder bodies inside each Region, all before any Writer runs.

### Submitting Work Units

The Scaffolder ends its run by calling `submit_work_units`. Before the submission is accepted,
`ScaffoldCheck` runs:

- Field problems: no Work Units, duplicate IDs, an ID below the first free one, a file path outside
  the worktree, a comment-less file, an empty spec, a missing or leading-dash test filter.
- Marker problems: each Work Unit's Region must exist in its file and be well formed, and there must
  be no stray Regions for Work Units that were not submitted.
- Writer Cap: each Work Unit's estimated Writer input must fit under the cap; see below.
- Scaffold Check: the configured build check must pass, so the Skeleton compiles.

Any problem comes back to the Scaffolder as a tool result, and it tries again. This is why a bad
Skeleton is caught before any Writer runs.

### The Writer Cap

The Writer Cap is the hard limit on the size of a Writer's input, in estimated tokens. It is set in
`arbor.toml` (default 8000). The estimate counts the spec, the acceptance criteria and the Context
Pack, and adds a 10 percent margin because the estimate is rough. A Work Unit over the cap is
rejected at submission, so the Scaffolder has to trim it or split it. The arithmetic lives in
`arbor-tiers/src/writer_cap.rs`.

### Revisions

When a Work Unit or a Task Check fails, the Scaffolder gets a Revision: a rework of the failed
Work Unit's spec, Skeleton or Context Pack, with a diagnosis of why it failed. The Revision run is
`RevisionRun`, with the submit tool `submit_revision`. It may only revise the failed Work Unit and
add new ones; it may not touch the other Work Units. A Revision that only rewords the spec is
rejected before it runs, and still counts toward the Revision limit.

## Writer

The Writer is the lowest Tier. It is a non-agentic model that fills exactly one Region using only
its Context Pack. It is a single call, not a loop.

```rust
pub struct WriterRun<'a> {
    pub prompt: &'a Prompt,
    pub work_unit: &'a WorkUnit,
    pub model: String,
    pub temperature: f64,
    pub supported_parameters: Vec<String>,
    pub worktree: &'a Path,
    pub checks: Checks,
    pub cap_tokens: u32,
    pub missing_response_allowed: bool,
}
```

The call sends temperature 0 and a fixed seed, but only when the model lists those parameters as
supported. If it does not, and Arbor sent them anyway, `require_parameters` would leave the call no
provider. This is ADR-0007; see [The LLM layer](08-llm-layer.md).

### The output contract

The Writer must return exactly one fenced code block and nothing else. The block is either code
wrapped in its context lines, or a ```` ```missing ```` block naming what the Context Pack lacked.
The parser in `arbor-tiers/src/reply.rs` rejects an empty reply, a reply cut off at the token limit,
a reply with no block, an unclosed block, several blocks, text outside the block, an empty missing
block, a block too short to hold the context lines, a context mismatch, and a Marker left in the
code. Each rejection carries a message that goes back to the Writer on the retry.

### Retries and the Missing Response

A Work Unit gets up to three Writer attempts (`MAX_WRITER_ATTEMPTS`). A retry shows the Writer its
last reply and what was wrong with it, shortened to fit under the Writer Cap. Earlier failures are
left out so the retry stays within the cap.

A Writer may instead send one Missing Response per Work Unit, naming what its Context Pack lacked.
That ends the run for that Work Unit and goes to a Revision, which is where the missing context gets
added. A second Missing Response for the same Work Unit is rejected.

## Commits

Every accepted state is committed, so `HEAD` is always the state a rejected Revision is discarded
back to. The Skeleton, each passing Work Unit, each accepted Revision, and each operator fix are
commits. These commits hold Markers and Placeholders by design and are squashed at merge time. See
[Git and Sessions](11-git-and-sessions.md) and [Markers and the mainline](13-markers-and-mainline.md).

Commit subjects come from the Work Unit's spec or the Task's title, and never mention Arbor, Work
Unit IDs or models. This is ADR-0009.
