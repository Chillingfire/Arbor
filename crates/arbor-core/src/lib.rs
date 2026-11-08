//! Domain types and the pure Session state machine. No I/O: events in, effects out (ADR-0005).

mod cost;
mod event;
mod log;
mod revision;
mod session;
mod types;

pub use cost::{Usage, UsageTotals, Usd};
pub use event::Event;
pub use log::{EventLog, ReadError, read_events};
pub use revision::{MAX_REVISIONS, Revision, RevisionCause, RevisionRound};
pub use session::{ApplyError, Effect, Phase, Session};
pub use types::{
    ASSUMPTION_PREFIX, LeftoverMarker, Plan, Priority, Request, Snippet, Task, Tier, WorkUnit,
    WorkUnitId,
};
