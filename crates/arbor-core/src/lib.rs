//! Domain types and the pure Session state machine. No I/O: events in, effects out (ADR-0005).

mod event;
mod types;

pub use event::Event;
pub use types::{Plan, Priority, Request, Snippet, Task, WorkUnit, WorkUnitId};
