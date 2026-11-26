# The agent loop

The Planner and the Scaffolder both run on one shared loop, in `arbor-tiers/src/agent.rs`. The loop
drives a model through a conversation until it calls its submit tool or gives up.

## One call per turn

`Agent::turn` makes one model call. That is deliberate: the caller records each call's usage and can
pause at the Budget Checkpoint between calls. A turn returns the call's `Usage` and an optional
`Outcome`.

```rust
pub enum Outcome<T> {
    Submitted(T),
    Questions(String),
    Escalated { failures: Vec<String> },
}
```

`None` as the outcome means the turn did not end the run; the caller keeps calling `turn`.

## Tools and the submit tool

`Agent::new` builds the tool list from the Tier's tools plus one submit tool. The submit tool's
parameter schema is generated from the submission type, so `submit_plan` takes a `Plan` and
`submit_work_units` takes a `WorkUnits` wrapper.

A turn with tool calls runs each tool and feeds the results back to the model as tool messages. The
submit tool is different: its arguments are parsed into the submission type and checked by the
submission's validator. The validator can be sync or async, which lets `submit_work_units` run the
Scaffold Check before accepting.

If a submission fails validation, the failure goes back to the model as a tool error and the run
continues. After `MAX_SUBMIT_FAILURES` failed submissions the agent escalates.

## Clarification Mode

The Planner and the Scaffolder can ask the operator questions, depending on Clarification Mode.

- In Ask mode, if the model replies with text instead of calling a tool, and it has not already
  asked, that text becomes `Outcome::Questions`. The operator's answers go back as a user message
  and the run continues.
- In Assume mode, or after the model has already asked, a text reply is not allowed. The model is
  told to make assumptions, record each one, and submit. Assumptions are recorded in the Plan's
  constraints or the Work Unit's spec, each starting with `Assumption:`, so they show up in the
  checkpoint for the operator to check.

A model may ask at most once per run. After that it must submit.

## Keeping the run on track

Two mechanisms push the model to submit before it wanders:

- Context guard. After a turn that did not end the run, if the model has used 80 percent of its
  context window, Arbor adds a message telling it to stop exploring and submit.
- Tool-call budget. A run may cap how many tool calls it makes before it must submit. The submit
  tool does not count. The Revision run uses this so a Revision cannot explore without end.

Both set `force_submit`, which changes the request's `tool_choice` to name the submit tool, so the
next call has to submit.

## Escalation

After too many failed submissions the agent returns `Outcome::Escalated` with the list of failures.
The caller turns that into a `TierStopped` session end. This is the top of the Tier's own patience,
before the work goes up to the next Tier or the operator.

## The scripted fake

`arbor-llm` ships a scripted client (`ScriptedClient`) that replays recorded exchanges. The agent
loop and the whole pipeline are tested against it, so the hardest logic runs with no network and no
model. See [Testing and CI](16-testing-and-ci.md).
