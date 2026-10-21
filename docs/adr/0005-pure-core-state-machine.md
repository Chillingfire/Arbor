# Pure core state machine, I/O at the edges

`arbor-core` holds the domain types and the Session state machine and does no I/O: events in, effects out. The LLM client, tools, git, and TUI are separate crates that carry out effects. This lets the Escalation and Checkpoint logic, which is the hardest part, be tested with plain unit tests, and lets the LLM be replaced by a scripted fake replaying recorded fixtures.
