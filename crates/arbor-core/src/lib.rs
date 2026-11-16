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
pub use revision::{
    BrokenRegion, FailedAttempt, MAX_REVISIONS, Revision, RevisionCause, RevisionRound,
    WorkUnitFailure,
};
pub use session::{ApplyError, Effect, Phase, Session, StrongerWriter};
pub use types::{
    ASSUMPTION_PREFIX, CommitOrigin, LeftoverMarker, Plan, Priority, Replan, Request,
    SessionCommit, Snippet, Task, Tier, WorkUnit, WorkUnitId, assumptions_in, is_assumption,
};
