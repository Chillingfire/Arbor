# The Session event log is the source of truth

Each Session's state is an append-only `events.jsonl`, and every event is written before the TUI shows it. Resume, Rewind, the event log pane, and cost accounting are all derived by replaying it. This makes crash recovery and Rewind fall out of one mechanism instead of three. The trade-off is that every state change must be modelled as an event.
