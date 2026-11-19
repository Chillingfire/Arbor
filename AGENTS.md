# AGENTS.md

How to write code here. Each bold term is a principle name: when a human says "apply *boundary discipline*", it means the rule under that heading. In your reply, name each principle you applied and the decision it changed.

## Workflow

1. **Understand first.** Read every file the change touches and the tests that cover it. Look in existing modules, helpers, constants, and dependencies for the thing you are about to write; reuse it when it exists. Done when you can say where the change goes and what it reuses.
2. **Spec non-trivial features.** For a feature with open product questions, write a short spec in `docs/` first: goal, a decisions table, behaviour, scope that is out. Build once the human has settled the decisions.
3. **Build in verifiable units.** Each unit is known-good state, one change, run the tests. Advance only when green.
4. **Simplify.** Before showing code, make up to three passes whose only goal is making it shorter and plainer. Done when a pass finds nothing to cut.
5. **Present for review.** Show source changes file by file and wait for approval before writing them. Test files land directly; mention what changed.
6. **Prove it works.** Run the test suite and, where you can, the real feature. Report what you ran and what you saw.
7. **Commit** with a single subject line in the imperative ("Add X", "Fix Y"), and nothing else. Keep each commit to at most about 1000 changed lines, not counting generated files such as `Cargo.lock`. Split larger work into several commits that each build and pass the tests.

## Code style

- **Zero comments.** Code carries its intent through names and structure, tests included. When a line seems to need a comment, rename, extract a function, or reshape the data until it reads plainly. The one exception is the why a reader can't see in the code, such as a non-obvious constraint, a workaround, or the ADR behind a decision. Never write comment blocks that restate what a function, field, or type does. A one-line doc comment on a public item is fine when it adds something its name doesn't.
- **Descriptive names.** Every variable, parameter, and callback argument says what it holds: `teamOne`, `mapIndex`, `interaction`, `serverId`, in loops and callbacks too. In this Rust codebase that means `chat_response`, `check_output`, `tool_results`, or `file_text`, not `response`, `output`, `results`, or `text`. Avoid single letters outside short numeric ranges, and don't use `value`, `data`, `item`, `result`, `tmp`, `buf`, or `out` as names. Struct fields that mirror an external format, such as a JSON key or a logged event, keep that format's name.
- **Boring code.** Plain if-chains, early returns, and small named helpers. Choose the version a tired reviewer understands in one read over a clever table, curried function, or generated regex.
- **Reuse before writing.** Existing dependencies, runtime built-ins, and project helpers come first. Add a dependency only when the human agrees it earns its place.
- **Config, not constants.** Organisation-specific IDs, URLs, keys, and timeouts live in environment config read in one constants module. Code and user-facing text stay generic so the project works for anyone who deploys it.

## Design principles

**Laziness protocol.** Make the smallest change that solves the problem. Fewer lines beat elegant boilerplate. Put each decision in one place and pass the result along.

**Subtract before you add.** Delete dead code, stale guards, and stub references first, then build on the simpler base. Design for observed usage; speculative options, validators, and edge cases wait until something needs them.

**Foundational thinking.** Choose the data shape before writing logic. Trace how the data is read and written, and pick the structure that makes the common path obvious.

**Model the domain.** When a feature grows another branch in an if/else chain or a second boolean that must stay in sync with the first, encode the rule in a data structure instead: a map, a state machine, a typed record. Keep plain code when the shape is local and stable.

**Minimize reader load.** A reader should answer "where does X come from?" and "what can change X?" in under 30 seconds. Inline wrappers with one caller. Prefer return values over mutation and locals over module state. Derive values instead of syncing copies.

**Boundary discipline.** Validate, time out, and handle errors where data crosses into the system: HTTP APIs, config, user input, files on disk. Parse raw responses into the shape the rest of the code uses (`{ partyId, partyCode }`, not the raw payload) and throw one error type per boundary that carries what callers branch on, such as an HTTP status. Inside, trust the data.

**Make operations idempotent.** Every state-changing operation reaches the same end state however many times it runs and wherever a previous run stopped. Write files to a temp path and rename into place. Serialize writes to one target through a single queue. Derive each step from current state, not from what the last run intended.

**Degrade gracefully.** When an optional resource is missing (a data volume, an API key, a cache), log once and keep the core feature running in a reduced mode.

**Redesign from first principles.** When a new requirement arrives, ask what you would build if it had been there from day one, and build that. Migrate every caller and delete the old path in the same change.

**Fix root causes.** Reproduce the bug first, then ask "why" until the answer is a wrong decision in the code. Fix it there and every place with the same fault. A guard, retry, or swallowed error is a fix only when the state it handles is legitimate.

**Attack the premise.** After two fixes that share an assumption have failed, write the assumption down as one sentence and test it directly before writing a third.

## Testing

**Test behaviour, not implementation.** A test asserts what callers observe: return values, thrown errors, state read back, and what an external service receives. Judge each test against two edits: break the behaviour (the test must fail) and change the internals while keeping results (the test must pass).

- Name each test as a sentence describing the behaviour: `"readRecords returns an empty list when the file does not exist"`.
- Mock only at the system boundary (network, clock). Use real temp directories for file I/O and clean them up after each test.
- Derive expected values from the requirement, never from the code under test.
- Pair every "this never happens" with a case where it would happen.
- Shared fixtures and mocks live in helper modules; any module-level cache gets a clear function that the helpers reset between tests.

**Prove it works.** "It compiles" and "the tests I wrote pass" are proxies. Run the real feature when you can, read the actual output, and report it.

## Lessons

**Encode lessons in structure.** When the same correction comes up twice, turn it into a test, lint rule, or script instead of another line in this file. Add a line here only for a rule that needs judgement.

## Agent skills

### Issue tracker

Issues are tracked in GitHub Issues for Chillingfire/Arbor via the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

Default canonical labels: `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: one root `CONTEXT.md` plus `docs/adr/`. See `docs/agents/domain.md`.

### GitHub text

End every issue body, PR description, and comment you post on GitHub with the line `_AI-assisted comment_`.
