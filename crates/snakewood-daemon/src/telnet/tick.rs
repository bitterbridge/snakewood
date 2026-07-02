use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use crate::Engine;

/// Advance the world once per `period_secs` and take interval snapshots.
pub async fn run_tick_loop(engine: Rc<RefCell<Engine>>, period_secs: u64) {
    let mut interval = tokio::time::interval(Duration::from_secs(period_secs));
    loop {
        interval.tick().await;
        let mut e = engine.borrow_mut();
        e.tick();
        if let Err(err) = e.maybe_snapshot() {
            eprintln!("snapshot failed: {err:?}");
        }
    }
}
