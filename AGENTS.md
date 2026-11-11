# Agents

## Agent skills

### Issue tracker

Issues are tracked in GitHub Issues for Chillingfire/Arbor via the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

Default canonical labels: `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: one root `CONTEXT.md` plus `docs/adr/`. See `docs/agents/domain.md`.

### GitHub text

End every issue body, PR description, and comment you post on GitHub with the line `_AI-assisted comment_`.

### Commits

Keep each commit to at most about 1000 changed lines, not counting generated files such as `Cargo.lock`. Split larger work into several commits that each build and pass the tests.

### Comments

Write code that reads without comments: clear names, small functions, and types that say what they hold. Comment only the why a reader can't see in the code, such as a non-obvious constraint, a workaround, or the ADR behind a decision. Don't write comment blocks that restate what a function, field, or type does. A one-line doc comment on a public item is fine when it adds something its name doesn't.
