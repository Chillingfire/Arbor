# No arbitrary shell for agentic Tiers in v1

The Planner has only read tools. The Scaffolder adds file writes restricted to the Session worktree and `run_checks`, which runs only the Checks configured in `arbor.toml`. Neither Tier can run arbitrary commands. This bounds what an autonomous model can do to the operator's machine. A command allowlist may be added later, but an open shell is deliberately absent.
