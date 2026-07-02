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
        ManualClock {
            now: AtomicI64::new(start),
        }
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

use std::time::{SystemTime, UNIX_EPOCH};

/// The production clock: real wall-clock time. This is the ONLY sanctioned place
/// the daemon reads `SystemTime::now()`; everything else takes time via `Clock`.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_unix(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod system_clock_tests {
    use super::*;

    #[test]
    fn system_clock_is_after_2020() {
        // 2020-01-01 UTC = 1_577_836_800. A real clock must be well past this.
        assert!(SystemClock.now_unix() > 1_577_836_800);
    }
}
