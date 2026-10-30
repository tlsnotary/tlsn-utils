//! Helpers shared by the integration tests.
#![allow(dead_code)]

use std::{
    panic,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    task::Wake,
    thread,
    time::Duration,
};

/// Waker that counts how often it has been woken.
#[derive(Default)]
pub struct CountingWaker(AtomicUsize);

impl CountingWaker {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn count(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

impl Wake for CountingWaker {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// Runs `f` on its own thread and fails the test if it does not finish within
/// `timeout`, so a regression shows up as a failure instead of a hung test
/// binary.
pub fn run_with_timeout<F>(timeout: Duration, f: F)
where
    F: FnOnce() + Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        f();
        let _ = tx.send(());
    });

    match rx.recv_timeout(timeout) {
        Ok(()) => {
            if let Err(payload) = handle.join() {
                panic::resume_unwind(payload);
            }
        }
        // The sender is dropped when the thread finishes; if it never sent a
        // value, the thread panicked and the payload is still joinable.
        Err(mpsc::RecvTimeoutError::Disconnected) => match handle.join() {
            Ok(()) => panic!("test thread exited without reporting completion"),
            Err(payload) => panic::resume_unwind(payload),
        },
        Err(mpsc::RecvTimeoutError::Timeout) => {
            panic!("timed out after {timeout:?}; the operation is likely deadlocked")
        }
    }
}
