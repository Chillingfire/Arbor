# Three Tiers with a non-agentic Writer

Arbor splits every change across a Planner, a Scaffolder, and a Writer. Only the Planner and Scaffolder are agentic (they have tools). The Writer is a single, deterministic call (temperature 0, a fixed seed where supported) that sees nothing but its Context Pack and returns only the code for one Region. The goal is to minimise expensive-model tokens: the strong models do the reasoning and decide exactly what the cheap model needs to see, so the cheap model can't wander, read the repo, or burn tokens exploring. The cost is that the Scaffolder must pack context precisely. The Missing Response and the Escalation path exist to recover when it doesn't.

## Considered Options

- **Agentic Writer with read tools**: rejected. Less predictable, harder to verify, and it spends the tokens this design exists to save.
- **Writer returns full files or diffs**: rejected. Region-only output is smaller and can be spliced mechanically.
