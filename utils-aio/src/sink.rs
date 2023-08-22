use futures::Sink;

/// A sink with `std::io::Error` as the error type.
pub trait IoSink<T>: Sink<T, Error = std::io::Error> {}

impl<T, U> IoSink<T> for U where U: Sink<T, Error = std::io::Error> {}
