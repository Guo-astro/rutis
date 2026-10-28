/// A boxed, `Send`, dyn-compatible future; the same definition as
/// `futures::future::BoxFuture`.
pub type BoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;
