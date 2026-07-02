use std::sync::atomic::{AtomicI64, Ordering};

/// Injected time source. `Send + Sync` so the async daemon (Stage 3c) can share it.
pub trait Clock: Send + Sync {
    /// Current time in whole Unix seconds.
    fn now_unix(&self) -> i64;
}

/// A test/manual clock whose time only changes when explicitly told.
pub struct ManualClock {
    now: AtomicI64,
}

impl ManualClock {
    pub fn new(start: i64) -> ManualClock {
        ManualClock { now: AtomicI64::new(start) }
    }

    pub fn set(&self, t: i64) {
        self.now.store(t, Ordering::Relaxed);
    }

    pub fn advance(&self, secs: i64) {
        self.now.fetch_add(secs, Ordering::Relaxed);
    }
}

impl Clock for ManualClock {
    fn now_unix(&self) -> i64 {
        self.now.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_clock_starts_at_and_advances() {
        let clock = ManualClock::new(1000);
        assert_eq!(clock.now_unix(), 1000);
        clock.advance(60);
        assert_eq!(clock.now_unix(), 1060);
        clock.set(5);
        assert_eq!(clock.now_unix(), 5);
    }
}
