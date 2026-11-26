# The Autopilot

`arbor-session` holds the `Autopilot`. It is the only place that runs a Session end to end. It
applies each event to the pure Session, carries out the effect that comes back with the real Tiers,
real git and the project's real Checks, and repeats until the Session ends.

```rust
pub async fn run(&mut self, request: Request) -> Result<SessionEnd, AutopilotError>
```

## The loop

`run` starts the Session with `SessionStarted`, applies the config settings, then loops. Each
iteration takes the one effect the Session is waiting on, carries it out, and applies the events
that result.

```mermaid
flowchart LR
    apply["apply(event)<br/>-> effects"]
    carry["carry_out(effect)"]
    events["resulting events"]
    apply --> carry --> events --> apply
    carry -->|Step::End| end["SessionEnd"]
```

`carry_out` matches the effect and calls the matching method: `plan`, `scaffold`, `scaffold_checks`,
`write`, `revise`, `replan`, `checkpoint`, `escalation`, `budget_checkpoint` and the small commit
and check steps. Some effects only produce events; the checkpoints and the model runs take an
`await`.

## What the Autopilot owns

The `Autopilot` struct holds the worktree, the mainline, the model client, the Session, the event
log, the config, the Conventions and the prompts, plus a few pieces of run state:

- `writer_runs`: every Writer attempt, grouped by Work Unit, for the TUI transcript.
- `passed_commit`: the commit of a Work Unit that passed and is waiting for `WorkUnitCommitted`.
- `task_fix_checked`: set when an operator's fix already passed the Unit Check, so it is not run
  again.
- `event_log`: where each event is written before anyone is told of it.
- `observer`: a callback for live updates.
- `operator`: where checkpoints and questions go.

`Autopilot::new` loads the Conventions and the prompt templates from the Session worktree. The
builder methods `with_operator`, `log_events_to` and `on_update` wire up the operator, the event log
and the observer.

## The Operator

Every checkpoint and every Tier's clarifying questions go to an `Operator`. The default is
`ApproveEverything`, used in tests. The TUI supplies a `ScreenOperator` that opens a modal and waits
for the reply.

```rust
async fn decide(&mut self, checkpoint: &Checkpoint) -> Decision;
async fn answer(&mut self, tier: Tier, questions: &str) -> Option<String>;
```

`Decision` is what the operator chose at a checkpoint: `Approve`, `ApproveTasks(order)`,
`Reject(feedback)`, `Fixed`, `Replan`, `Stop` or `SetBudgetCap(cap)`. `checkpoint` turns a
`Decision` into the event that records it. A decision that does not fit the open checkpoint is an
error, which catches a bug in the wiring rather than silently accepting the wrong choice.

## The agentic runs

The Planner, Scaffolder, Replan and Revision runs all go through `submitted`. It takes turns on the
agent, records each turn's usage, and stops when the agent submits, asks questions, or escalates.
Between turns it checks the budget and can pause.

A submission comes back with the `UsageRecorded` of the call that made it, which the caller applies
after the submission's own event, so the call that reaches the cap still lands its result.

## The Writer run

`write` runs the Writer until the Work Unit passes or the Writer gives up. It builds a `WriterRun`,
starts it, and loops over attempts. Each attempt is one model call. A passed attempt carries the
commit the Writer already made, so `CommitWorkUnit` only records it. A failed attempt is recorded
and retried, unless the Writer has used its attempts or sent a Missing Response, in which case a
`WorkUnitFailed` goes to a Revision.

A broken Region is a special case. It ends the Writer with no attempt used and goes straight to a
Revision, because no model call can fill a Region that is not there.

## Pausing for the budget

`pause_at_budget` is called between calls. If the Session has reached its cap, it opens the Budget
Checkpoint and waits for the operator. Once the cap is raised, the run goes on where it was. If the
effect that comes back after the pause is the `Run*` effect of a run that was cut off between
calls, the runner continues that run rather than restarting it.

## How the run ends

`run` returns a `SessionEnd`:

- `Finished`: every Task is done and no Markers are left.
- `StoppedAtBudget`: the operator stopped at the Budget Checkpoint.
- `NeedsOperator(effect)`: the operator stopped at a checkpoint, and the Session waits there. A
  later resume shows the same checkpoint again.
- `TierStopped { tier, reason }`: a Tier ended without submitting, by escalating or asking
  questions.
- `ScaffoldChecksFailed { check_output }`: the Scaffold Check failed on a Skeleton the Scaffolder
  had already checked.

The TUI turns each of these into a closing message. See [The TUI](14-tui.md).
