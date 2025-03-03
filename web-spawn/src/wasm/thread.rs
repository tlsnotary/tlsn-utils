use std::panic::{AssertUnwindSafe, catch_unwind};

use crossbeam_channel::bounded;

use crate::wasm::{JoinHandle, SENDER};

/// Builder for a thread.
pub struct Builder {
    pub(crate) name: Option<String>,
}

impl Builder {
    /// Creates a new thread builder.
    pub fn new() -> Self {
        Self { name: None }
    }

    /// Names the thread-to-be.
    pub fn name(mut self, name: String) -> Self {
        self.name = Some(name);
        self
    }

    /// Spawns a new thread, running the provided function.
    pub fn spawn<F, T>(self, f: F) -> std::io::Result<JoinHandle<T>>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let (sender, receiver) = bounded(1);

        let f = move || {
            let result = catch_unwind(AssertUnwindSafe(f));
            // Ignore if the join handle is dropped.
            let _ = sender.send(result);
        };

        SENDER
            .get()
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "spawner has not been initialized",
                )
            })?
            .unbounded_send((self, Box::new(f)))
            .map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::Other, "spawner has been stopped")
            })?;

        Ok(JoinHandle(receiver))
    }
}
