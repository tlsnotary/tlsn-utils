use async_trait::async_trait;
use futures::channel::oneshot;

pub type Backend = RayonBackend;

/// Allows to spawn a closure on a thread outside of the async runtime
///
/// This allows to perform CPU-intensive tasks without blocking the runtime.
#[async_trait]
pub trait NonBlockingBackend {
    /// Spawn the closure in a separate thread and await the result
    async fn spawn<F: FnOnce() -> T + Send + 'static, T: Send + 'static>(closure: F) -> T;
}

/// A CPU backend that uses Rayon
pub struct RayonBackend;

#[async_trait]
impl NonBlockingBackend for RayonBackend {
    async fn spawn<F: FnOnce() -> T + Send + 'static, T: Send + 'static>(closure: F) -> T {
        let (sender, receiver) = oneshot::channel();
        rayon::spawn(move || {
            _ = sender.send(closure());
        });

        receiver.await.expect("channel should not be canceled")
    }
}

/// A macro for asynchronously evaluating an expression on a non-blocking backend.
///
/// The expression must be `Send + 'static`, including it's returned type.
///
/// All variables referenced in the expression are moved to the backend scope.
///
/// # Example
///
/// ```rust
/// # use utils_aio::blocking;
/// # futures::executor::block_on(async {
/// let a = 1u8;
/// let b = 2u8;
///
/// let sum = blocking!(a + b);
///
/// assert_eq!(sum, 3);
/// # });
/// ```
///
/// # Example: Returned arguments
///
/// When variables used in the expression they are moved into the backend scope. If you still need
/// to use them after evaluating the expression you can have them returned using the following syntax:
///
/// ```rust
/// # use utils_aio::blocking;
/// struct NotCopy(u32);
///
/// # futures::executor::block_on(async {
/// let a = NotCopy(1);
/// let b = NotCopy(2);
///
/// let (a, b, sum) = blocking! {
///    (a, b) => a.0 + b.0
/// };
///
/// assert_eq!(a.0, 1);
/// assert_eq!(b.0, 2);
/// assert_eq!(sum, 3);
/// # });
/// ```
#[macro_export]
macro_rules! blocking {
    (($($arg:ident),+) => $expr:expr) => {
        {
            use $crate::non_blocking_backend::NonBlockingBackend;
            $crate::non_blocking_backend::Backend::spawn(move || {
                let result = $expr;
                ($($arg),+, result)
            }).await
        }
    };
    (() => $expr:expr) => {
        {
            use $crate::non_blocking_backend::NonBlockingBackend;
            $crate::non_blocking_backend::Backend::spawn(move || $expr).await
        }
    };
    ($expr:expr) => {
        {
            use $crate::non_blocking_backend::NonBlockingBackend;
            $crate::non_blocking_backend::Backend::spawn(move || $expr).await
        }
    };
}

#[cfg(test)]
mod tests {
    use super::{Backend, NonBlockingBackend};

    #[tokio::test]
    async fn test_spawn() {
        let sum = Backend::spawn(compute_sum).await;
        assert_eq!(sum, 4950);
    }

    fn compute_sum() -> u32 {
        (0..100).sum()
    }
}
