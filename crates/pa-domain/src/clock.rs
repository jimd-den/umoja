use chrono::{DateTime, Utc};

/// Time, as a dependency.
///
/// Schedules, budgets and heartbeats are all "is it time yet" questions. Taking
/// the clock as a port is what lets those be tested by advancing a number
/// instead of by sleeping.
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

/// A clock frozen at a chosen instant, for tests.
#[derive(Debug, Clone)]
pub struct FixedClock(pub DateTime<Utc>);

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}
