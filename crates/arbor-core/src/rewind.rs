//! Where a Session can be rewound to: each Checkpoint in its event log whose worktree can be
//! rebuilt from commits.

use crate::event::Event;
use crate::session::{ApplyError, Phase, Session, is_rewind_target};

/// A Checkpoint the Session can be rewound to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RewindPoint {
    /// How many events of the log the Session stood on there: the `Rewound` event's
    /// `event_count`.
    pub event_count: usize,
    /// The Checkpoint's phase.
    pub phase: Phase,
    /// The Session's last commit at the Checkpoint, or `None` before its first, where the worktree
    /// is at the branch's start.
    pub head: Option<String>,
    /// At a Scaffold or Revision Checkpoint, the commit that later took the edits it reviewed,
    /// which were still uncommitted at the Checkpoint. `None` when there were none.
    pub reviewed_edits: Option<String>,
}

/// Every Checkpoint in the log the Session can be rewound to, oldest first. Points the Session
/// was rewound away from are still listed. A Scaffold or Revision Checkpoint whose edits were
/// never committed is left out, since nothing holds them any more.
pub fn rewind_points(events: &[Event]) -> Result<Vec<RewindPoint>, ApplyError> {
    let mut session = Session::new();
    let mut points = Vec::new();
    for (index, event) in events.iter().enumerate() {
        if let Event::Rewound { event_count } = *event {
            session = session.rewound(&events[..index], event_count)?;
            continue;
        }
        let phase_before = session.phase().clone();
        session.apply(event)?;
        let phase = session.phase();
        if *phase == phase_before || !is_rewind_target(phase) {
            continue;
        }
        let event_count = index + 1;
        let reviewed_edits = match phase {
            Phase::ScaffoldCheckpoint { .. } | Phase::RevisionCheckpoint { .. } => {
                match committed_edits(&events[event_count..]) {
                    Some(edits) => edits,
                    None => continue,
                }
            }
            _ => None,
        };
        points.push(RewindPoint {
            event_count,
            phase: phase.clone(),
            head: session.commits().last().map(|commit| commit.hash.clone()),
            reviewed_edits,
        });
    }
    Ok(points)
}

/// The commit that took the edits a Scaffold or Revision Checkpoint reviewed, from the events
/// after it: `Some(None)` when approving them committed nothing, and `None` when they weren't
/// approved and committed.
fn committed_edits(later_events: &[Event]) -> Option<Option<String>> {
    let mut decisive = later_events.iter().filter(|event| {
        !matches!(
            event,
            Event::UsageRecorded { .. }
                | Event::BudgetCapSet { .. }
                | Event::RevisionCheckpointsSet { .. }
                | Event::RunEnded { .. }
        )
    });
    match (decisive.next()?, decisive.next()?) {
        (Event::ScaffoldApproved, Event::SkeletonCommitted { commit })
        | (Event::RevisionApproved, Event::RevisionCommitted { commit }) => Some(commit.clone()),
        _ => None,
    }
}
