//! 开发指南配套：配置更新 → 消费者重载 → 旧队列停止 → 跨代计数保留。
//! Run: cargo run -p rutis --example development_workflow

use std::error::Error;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rutis::host::{CtxHost, ErrorKind, FiberState, FiberView};
use rutis::{Ctx, Plugin};
use tokio::sync::{mpsc, oneshot};

type BoxError = Box<dyn Error + Send + Sync>;

// 契约：Backend 是本代实现；Completed 是独立于 worker 代际的应用状态。
struct Backend(String);
#[derive(Default)]
struct Completed(AtomicUsize);
struct Job(String, oneshot::Sender<String>);

// 一个真实的有界队列服务。旧代接收端关闭后，旧句柄明确返回错误。
struct Indexer(mpsc::Sender<Job>);
impl Indexer {
    async fn index(&self, document: &str) -> Result<String, BoxError> {
        let (reply, result) = oneshot::channel();
        self.0
            .send(Job(document.to_owned(), reply))
            .await
            .map_err(|_| "indexer generation has stopped")?;
        result.await.map_err(|_| "indexing was cancelled".into())
    }
}

struct BackendPlugin(String);
impl Plugin for BackendPlugin {
    type Deps = ();

    fn name(&self) -> &str {
        "backend"
    }

    fn validate(&self) -> rutis::Result {
        if self.0.trim().is_empty() {
            return Err(ErrorKind::Validation {
                issues: vec!["backend label must not be empty".into()],
            }
            .into());
        }
        Ok(())
    }

    async fn apply(&self, ctx: &Ctx, (): ()) -> rutis::Result {
        ctx.provide(Backend(self.0.clone()))?;
        Ok(())
    }
}

/// 纯构造：预检查和正式装载都会调用它。构造函数接 `&C`，这里 C 是 `String`。
#[allow(clippy::ptr_arg)]
fn backend(label: &String) -> rutis::Result<BackendPlugin> {
    Ok(BackendPlugin(label.clone()))
}

struct IndexerPlugin;
impl Plugin for IndexerPlugin {
    type Deps = (Arc<Backend>, Arc<Completed>);

    fn name(&self) -> &str {
        "indexer"
    }

    async fn apply(&self, ctx: &Ctx, (backend, completed): Self::Deps) -> rutis::Result {
        let (sender, mut receiver) = mpsc::channel::<Job>(8);
        let stopping = ctx.cancelled();

        // 在 effect 的闭包里启动，启动后立即交回清理责任。
        // 先登记 worker，再提供服务：清理时先撤销服务，再 join worker。
        ctx.effect(move || {
            let task = tokio::spawn(async move {
                tokio::pin!(stopping);
                loop {
                    tokio::select! {
                        biased;
                        _ = &mut stopping => break,
                        job = receiver.recv() => {
                            let Some(Job(document, reply)) = job else { break };
                            // 示例用字符串处理代替真正的异步索引 I/O。
                            let value = format!("{}: {document}", backend.0);
                            completed.0.fetch_add(1, Ordering::SeqCst);
                            let _ = reply.send(value);
                        }
                    }
                }
                // 本例选择取消未处理请求：drop receiver 及排队的 reply。
            });
            // 插件停下时 `stopping` 完成，worker 退出；清理等它结束。
            async move {
                task.await?;
                Ok(())
            }
        })?;
        ctx.provide(Indexer(sender))?;
        Ok(())
    }
}

// 等待 Active，并处理 Failed / Disposed / 观察通道关闭 / 超时。
// Pending 也能 settle，所以不能只使用 (&view).await 代替此检查。
async fn wait_active(view: &FiberView) -> Result<(), BoxError> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut state = view.watch();
        loop {
            let snapshot = state.borrow_and_update().clone();
            match snapshot.state {
                FiberState::Active => return Ok::<(), BoxError>(()),
                FiberState::Failed | FiberState::Disposed => {
                    return Err(format!("{} did not start: {snapshot:?}", view.name()).into());
                }
                _ => state.changed().await?,
            }
        }
    })
    .await??;
    Ok(())
}

async fn exercise(root: &Ctx) -> Result<(), BoxError> {
    // 这是有意选择的 root 级状态；Indexer 重载不清空它。
    root.provide(Completed::default())?;
    let indexer = root.plugin(IndexerPlugin);
    (&indexer).await?;
    assert_eq!(indexer.state().state, FiberState::Pending);

    let backend = root.plugin_from(backend, String::from("v1"));
    wait_active(&indexer).await?;
    let old = root.get::<Indexer>().ok_or("missing Indexer")?;
    assert_eq!(old.index("first.txt").await?, "v1: first.txt");
    println!("v1 indexed first.txt");

    // 预检查失败：现有实例和代数保持不变。
    let generation = indexer.state().generation;
    assert!(backend.update(String::new()).await.is_err());
    assert_eq!(indexer.state().generation, generation);

    // 正式更新只操作 provider；框架会重载声明依赖的消费者。
    backend.update(String::from("v2")).await?;
    wait_active(&indexer).await?;
    assert!(indexer.state().generation > generation);
    assert!(old.index("stale.txt").await.is_err());
    let current = root.get::<Indexer>().ok_or("missing reloaded Indexer")?;
    assert_eq!(current.index("second.txt").await?, "v2: second.txt");
    assert_eq!(
        root.get::<Completed>()
            .ok_or("missing Completed")?
            .0
            .load(Ordering::SeqCst),
        2
    );
    println!("v2 indexed second.txt; completed = 2; old handle rejected");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    let root = Ctx::root()?;
    let outcome = exercise(&root).await;
    // 即使演示中的业务步骤失败，也先尝试关闭 root。
    let closed = root.shutdown().await;
    outcome?;
    closed?;
    println!("shutdown complete");
    Ok(())
}
