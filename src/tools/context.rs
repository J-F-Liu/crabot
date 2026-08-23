//! Per-execution tool context: which session tab a tool call belongs to.
//!
//! The LLM loop sets it around each blocking tool execution so tools that
//! register global state (e.g. the `process` registry) can tag entries with
//! the originating session tab number. Scoped to one `spawn_blocking` call.

use std::cell::RefCell;

thread_local! {
    static CURRENT_TAB: RefCell<Option<usize>> = const { RefCell::new(None) };
}

/// Run `f` with `tab_number` as the current session tab, restoring any outer
/// value afterwards (blocking threads are pooled). The guard restores even
/// on panic, so a panicking tool cannot leak its number into later executions.
pub fn with_tab_scope<R>(tab_number: usize, f: impl FnOnce() -> R) -> R {
    struct Guard(Option<usize>);
    impl Drop for Guard {
        fn drop(&mut self) {
            CURRENT_TAB.with(|c| *c.borrow_mut() = self.0.take());
        }
    }
    let prev = CURRENT_TAB.with(|c| c.borrow_mut().replace(tab_number));
    let _guard = Guard(prev);
    f()
}

/// Session tab number of the executing tool call, or `None` outside the LLM loop.
pub fn current_tab_number() -> Option<usize> {
    CURRENT_TAB.with(|c| *c.borrow())
}
