//! Test-only pauses in a sequencer pass, at a named point, for one lease
//! holder: `begin` (before the transaction opens) and `fenced` (after the
//! fence check, holding the transaction). One pause at a time per thread
//! (tokio tests run on one thread, spawned tasks included).

use std::cell::RefCell;
use std::sync::Arc;

use tokio::sync::Notify;

/// A paused pass: `reached` fires when it gets there; `resume` lets it go on.
#[derive(Clone)]
pub(crate) struct Pause {
    pub reached: Arc<Notify>,
    pub resume: Arc<Notify>,
}

struct Armed {
    point: &'static str,
    holder: String,
    pause: Pause,
}

thread_local! {
    static ARMED: RefCell<Option<Armed>> = const { RefCell::new(None) };
}

/// The next pass by `holder` reaching `point` waits there.
pub(crate) fn pause(point: &'static str, holder: &str) -> Pause {
    let p = Pause {
        reached: Arc::new(Notify::new()),
        resume: Arc::new(Notify::new()),
    };
    ARMED.with(|a| {
        *a.borrow_mut() = Some(Armed {
            point,
            holder: holder.to_owned(),
            pause: p.clone(),
        });
    });
    p
}

pub(crate) async fn hit(point: &'static str, holder: &str) {
    let fired = ARMED.with(|a| {
        let mut a = a.borrow_mut();
        match a.as_ref() {
            Some(x) if x.point == point && x.holder == holder => a.take(),
            _ => None,
        }
    });
    if let Some(f) = fired {
        f.pause.reached.notify_one();
        f.pause.resume.notified().await;
    }
}
