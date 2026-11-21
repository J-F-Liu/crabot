//! Spawn blocking tool work on a named worker thread.

use std::sync::mpsc;

/// Spawn `task` on a thread labeled `label` (also used in errors); the task sends
/// its results through the returned channel. Callers poll the receiver so a
/// blocked task can be abandoned on cancel or timeout.
pub(crate) fn spawn_worker<T: Send + 'static>(
    label: &str,
    task: impl FnOnce(mpsc::Sender<T>) + Send + 'static,
) -> Result<mpsc::Receiver<T>, String> {
    let (sender, receiver) = mpsc::channel();
    std::thread::Builder::new()
        .name(label.into())
        .spawn(move || task(sender))
        .map_err(|error| format!("{label} failed: could not start the worker: {error}"))?;
    Ok(receiver)
}
