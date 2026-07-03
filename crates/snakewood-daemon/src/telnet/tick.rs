use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use crate::Engine;

/// Advance the world once per `period` and take interval snapshots. The tick is
/// both the game quantum and the command-processing beat: each tick drains the
/// intent queue. Snapshot cadence is independent (clock-gated in `maybe_snapshot`).
pub async fn run_tick_loop(engine: Rc<RefCell<Engine>>, period: Duration) {
    let mut interval = tokio::time::interval(period);
    loop {
        interval.tick().await;
        let mut e = engine.borrow_mut();
        e.tick();
        if let Err(err) = e.maybe_snapshot() {
            eprintln!("snapshot failed: {err:?}");
        }
    }
}
