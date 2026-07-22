use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use crate::ActorId;

/// Hybrid logical timestamp. Actor identity provides deterministic final ordering.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Hlc {
    pub physical_ms: u64,
    pub logical: u32,
    pub actor_id: ActorId,
}

impl Ord for Hlc {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.physical_ms, self.logical, &self.actor_id).cmp(&(
            other.physical_ms,
            other.logical,
            &other.actor_id,
        ))
    }
}

impl PartialOrd for Hlc {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HlcClock {
    physical_ms: u64,
    logical: u32,
    actor_id: ActorId,
}

impl HlcClock {
    #[must_use]
    pub const fn new(actor_id: ActorId) -> Self {
        Self {
            physical_ms: 0,
            logical: 0,
            actor_id,
        }
    }

    #[must_use]
    pub fn from_timestamp(timestamp: Hlc) -> Self {
        Self {
            physical_ms: timestamp.physical_ms,
            logical: timestamp.logical,
            actor_id: timestamp.actor_id,
        }
    }

    /// Advance for a local event, tolerating a wall clock that moved backwards.
    pub fn next(&mut self, wall_clock_ms: u64) -> Hlc {
        if wall_clock_ms > self.physical_ms {
            self.physical_ms = wall_clock_ms;
            self.logical = 0;
        } else {
            self.logical = self.logical.saturating_add(1);
        }
        self.timestamp()
    }

    /// Observe a remote timestamp and advance according to the HLC merge algorithm.
    pub fn observe(&mut self, remote: &Hlc, wall_clock_ms: u64) -> Hlc {
        let local_physical = self.physical_ms;
        let next_physical = wall_clock_ms.max(local_physical).max(remote.physical_ms);
        self.logical = if next_physical == local_physical && next_physical == remote.physical_ms {
            self.logical.max(remote.logical).saturating_add(1)
        } else if next_physical == local_physical {
            self.logical.saturating_add(1)
        } else if next_physical == remote.physical_ms {
            remote.logical.saturating_add(1)
        } else {
            0
        };
        self.physical_ms = next_physical;
        self.timestamp()
    }

    #[must_use]
    pub fn timestamp(&self) -> Hlc {
        Hlc {
            physical_ms: self.physical_ms,
            logical: self.logical,
            actor_id: self.actor_id.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_survives_clock_regression_and_remote_observation() {
        let mut clock = HlcClock::new(ActorId::new("a"));
        assert_eq!(clock.next(100).logical, 0);
        assert_eq!(clock.next(90).logical, 1);
        let observed = clock.observe(
            &Hlc {
                physical_ms: 200,
                logical: 4,
                actor_id: ActorId::new("b"),
            },
            150,
        );
        assert_eq!((observed.physical_ms, observed.logical), (200, 5));
    }
}
