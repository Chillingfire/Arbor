# Arbor

Arbor is a TUI harness that splits a programming change across three LLM tiers (planning, scaffolding, writing) so that expensive models do only the reasoning and cheap models write most of the code.

## Language

### Sessions and work

**Session**:
One end-to-end attempt at a single Request, from planning to merge, with its own branch and worktree.
_Avoid_: Run, job

**Request**:
The change the human operator asks for, typed free-text or pulled from a GitHub issue.
_Avoid_: Prompt, ticket, feature

**Plan**:
The Planner's output for a Request: a goal, constraints, and an ordered list of Tasks.

**Task**:
One step of a Plan that leaves the build green and its tests passing when finished.
_Avoid_: Step, story

**Work Unit**:
The smallest unit of work: one Region in one file, with a spec, a Context Pack, acceptance tests, and a test filter. Written by exactly one Writer.
_Avoid_: Job, subtask, chunk

### Tiers

**Tier**:
One of the three roles a model can fill: Planner, Scaffolder, or Writer.
_Avoid_: Agent, level, layer

**Planner**:
The highest Tier. It turns a Request into a Plan and never writes code.

**Scaffolder**:
The middle Tier. It turns a Task into a Skeleton, tests, and Work Units.

**Writer**:
The lowest Tier. A non-agentic model that fills exactly one Region using only its Context Pack.
_Avoid_: Coder, implementer

### Code structure

**Skeleton**:
The compilable structure the Scaffolder writes for a Task (types, signatures, imports, and Placeholders) before any Writer runs.
_Avoid_: Stub file, scaffold (as a noun for the code)

**Region**:
The span of code a single Work Unit fills, bounded by Markers.

**Placeholder**:
The compilable stand-in body inside a Region, e.g. `todo!()`, that a Writer replaces.

**Marker**:
A comment carrying a Work Unit's ID that bounds a Region in the file. It exists only until that Work Unit passes.
_Avoid_: Tag, anchor

**Context Pack**:
The snippets the Scaffolder selects for a Work Unit, which are everything a Writer is allowed to know. Limited by the Writer Cap.
_Avoid_: Context window, prompt context

**Writer Cap**:
The hard limit on the size of a Writer's input.

**Conventions**:
The project's own guidance documents (e.g. `AGENTS.md`, `CONTEXT.md`, README) given to the Planner and Scaffolder.

### Verification and escalation

**Check**:
A project-configured command Arbor runs to verify work. A **Scaffold Check** verifies that the Skeleton builds. A **Unit Check** builds and runs tests.

**Escalation**:
Handing a failed piece of work to the next Tier up: Writer → Scaffolder → Planner → human operator.

**Revision**:
A Scaffolder's rework of a failed Work Unit's spec, Skeleton, or Context Pack, with a diagnosis of why it failed.

**Replan**:
A Planner's rework of a failed Task.

**Missing Response**:
A Writer's reply that names what its Context Pack lacked instead of returning code.
_Avoid_: Insufficient context error

**Failure Brief**:
A short summary of a Work Unit's failures that Arbor assembles mechanically when the operator retries it with a stronger model.

### Human control

**Human Operator**:
The developer running Arbor. The final level of Escalation and the only party who can approve anything that leaves the local machine.
_Avoid_: User (in prompts), driver

**Checkpoint**:
A pause where the Human Operator approves, edits, or rejects a Tier's output before work continues.
_Avoid_: Gate, review step

**Clarification Mode**:
Whether the Planner and Scaffolder ask the Human Operator about ambiguities (**Ask**) or make recorded assumptions (**Assume**).

**Rewind**:
Returning a Session to an earlier Checkpoint, discarding the work done since.
_Avoid_: Undo, rollback

**Budget Cap**:
An optional spending limit per Session that pauses at a Checkpoint when it's reached.
