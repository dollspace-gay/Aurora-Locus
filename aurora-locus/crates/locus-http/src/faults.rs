//! Named points where tests inject faults: a test registers a hook for one
//! node and one point, and the code awaits it there. Outside tests every
//! point is a no-op.

use locus_core::CoreResult;

use crate::state::AppState;

#[cfg(test)]
pub type Hook = std::sync::Arc<
    dyn Fn() -> futures::future::BoxFuture<'static, CoreResult<()>> + Send + Sync,
>;

/// By (node address, point).
#[cfg(test)]
#[allow(clippy::type_complexity)]
static HOOKS: std::sync::Mutex<
    Option<std::collections::HashMap<(usize, &'static str), Hook>>,
> = std::sync::Mutex::new(None);

/// Runs the hook a test set for `point` on this node, if any; its error
/// is the fault.
#[cfg(test)]
pub async fn at(s: &AppState, point: &'static str) -> CoreResult<()> {
    let hook = HOOKS
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|m| m.get(&(std::ptr::from_ref(s) as usize, point)).cloned());
    match hook {
        Some(h) => h().await,
        None => Ok(()),
    }
}

#[cfg(not(test))]
#[inline]
#[allow(clippy::unused_async)]
pub async fn at(_: &AppState, _: &'static str) -> CoreResult<()> {
    Ok(())
}

/// Sets (or with None, clears) the hook for `point` on this node.
#[cfg(test)]
pub fn set(s: &AppState, point: &'static str, hook: Option<Hook>) {
    let mut g = HOOKS.lock().unwrap();
    let m = g.get_or_insert_with(Default::default);
    let k = (std::ptr::from_ref(s) as usize, point);
    match hook {
        Some(h) => {
            m.insert(k, h);
        }
        None => {
            m.remove(&k);
        }
    }
}
