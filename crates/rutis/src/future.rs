/// dyn 兼容的 future 别名(与 `futures::future::BoxFuture` 同一定义,D1)。
pub type BoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;
