//! 工具集——`ToolRegistry` 服务 + `ToolsPlugin`(设计 §三.5)。
//!
//! schema 直接用 aimux [`FunctionTool`](进 `CallOptions.tools`);
//! runner 失败转 `error: ...` 文本回喂模型,panic 任务边界兜底,不崩循环。
//!
//! 内置工具(minimal mode,`docs/design-minimal-mode-2026-08-18.md`):
//! [`bash`](bash) 与 [`replace_text`](replace_text)——`ToolDef` 数据
//! 装进本插件,不做 dsh 的中央工具服务 + 能力 seam 两层(设计 §三)。

pub mod bash;
pub mod replace_text;

use std::collections::HashMap;
use std::future::Future;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aimux_core::options::Tool;
use aimux_core::tool::{FunctionTool, ToolCall};
use futures::future::BoxFuture;
use rutis::{Ctx, Plugin};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

/// 一次工具执行的输出(失败已转为模型可见的 `error: ...` 文本)。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolOutput {
    pub ok: bool,
    pub output: String,
}

/// 声明式工具:aimux `FunctionTool`(schema,直接进 `CallOptions.tools`)
/// 加异步 runner。runner 收参数对象,返回值字符串化进 tool 结果消息
/// (`Value::String` 原样,其余 JSON 序列化)。
#[derive(Clone)]
pub struct ToolDef {
    pub tool: FunctionTool,
    pub run: Arc<dyn Fn(Value) -> BoxFuture<'static, Result<Value, String>> + Send + Sync>,
}

impl ToolDef {
    pub fn new<F, Fut>(name: &str, description: &str, parameters: Value, run: F) -> Self
    where
        F: Fn(Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Value, String>> + Send + 'static,
    {
        let tool = FunctionTool::new(name, parameters).with_description(description);
        Self::from_function_tool(tool, run)
    }

    pub fn from_function_tool<F, Fut>(tool: FunctionTool, run: F) -> Self
    where
        F: Fn(Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Value, String>> + Send + 'static,
    {
        Self {
            tool,
            run: Arc::new(move |args: Value| Box::pin(run(args))),
        }
    }

    pub fn name(&self) -> &str {
        &self.tool.name
    }
}

/// 工具注册表服务:统一注册、schema 汇入 prompt、按名执行。
/// 真装配单元——由 [`ToolsPlugin`] 提供,fiber 管生命周期,可热替换。
pub struct ToolRegistry {
    tools: Mutex<HashMap<String, ToolDef>>,
    handle: tokio::runtime::Handle,
}

/// Dropping `execute` must also cancel its spawned runner. Tokio dropping a bare
/// JoinHandle detaches the task; abort requests cooperative cancellation at
/// the next await and cannot interrupt synchronous code that never yields.
struct AbortOnDrop(tokio::task::JoinHandle<Result<Value, String>>);

const CANCEL_JOIN_GRACE: Duration = Duration::from_secs(2);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl ToolRegistry {
    pub(crate) fn new(handle: tokio::runtime::Handle, defs: Vec<ToolDef>) -> Self {
        Self {
            tools: Mutex::new(defs.into_iter().map(|d| (d.tool.name.clone(), d)).collect()),
            handle,
        }
    }

    /// 每步交给模型的工具 schema(aimux `Tool`,直接进 `CallOptions.tools`)。
    pub fn schemas(&self) -> Vec<Tool> {
        self.tools
            .lock()
            .unwrap()
            .values()
            .map(|d| Tool::Function(d.tool.clone()))
            .collect()
    }

    /// 运行时注册/替换工具(热加载):运行中的 agent 可现场加入新能力,
    /// 后续 turn 的 schema 立即包含它。同名覆盖。
    pub fn register(&self, def: ToolDef) {
        self.tools
            .lock()
            .unwrap()
            .insert(def.tool.name.clone(), def);
    }

    pub fn get(&self, name: &str) -> Option<ToolDef> {
        self.tools.lock().unwrap().get(name).cloned()
    }

    pub fn len(&self) -> usize {
        self.tools.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.lock().unwrap().is_empty()
    }

    /// 执行一次工具调用;失败转为模型可见的 `error: ...` 结果,不崩循环。
    /// 工具执行在任务边界:runner 创建或 poll 中的 panic 同样转为模型
    /// 可见错误(评审 #13,对齐 python `except Exception` 兜底)。
    /// `cancel` 在工具执行期间生效(取消 → `error: cancelled` 回喂,
    /// 循环在下一边界以 `Stopped` 收尾)。
    pub async fn execute(&self, call: &ToolCall, cancel: &CancellationToken) -> ToolOutput {
        let Some(def) = self.tools.lock().unwrap().get(&call.tool_name).cloned() else {
            return ToolOutput::err(format!("error: unknown tool '{}'", call.tool_name));
        };
        let run = def.run.clone();
        let input = call.input.clone();
        let fut = match catch_unwind(AssertUnwindSafe(move || run(input))) {
            Ok(fut) => fut,
            Err(p) => return ToolOutput::err(format!("error: {}", panic_message(&p))),
        };
        let mut join = AbortOnDrop(self.handle.spawn(fut));
        let outcome = tokio::select! {
            biased;
            out = &mut join.0 => out,
            _ = cancel.cancelled() => {
                join.0.abort();
                // A third-party runner may block synchronously in one poll.
                // Abort remains requested, but must not stall this turn forever.
                if tokio::time::timeout(CANCEL_JOIN_GRACE, &mut join.0).await.is_err() {
                    eprintln!("[tools] runner did not stop within cancellation grace period");
                }
                return ToolOutput::err("error: tool execution cancelled".to_string())
            }
        };
        match outcome {
            Ok(Ok(value)) => ToolOutput {
                ok: true,
                output: match value {
                    Value::String(s) => s,
                    other => other.to_string(),
                },
            },
            Ok(Err(e)) => ToolOutput::err(format!("error: {e}")),
            // 取消不是 panic:into_panic 在取消场景会二次 panic(评审 P2)
            Err(join_err) => ToolOutput::err(if join_err.is_panic() {
                format!("error: {}", panic_message(&join_err.into_panic()))
            } else {
                "error: tool task cancelled".to_string()
            }),
        }
    }
}

impl ToolOutput {
    pub(crate) fn err(output: String) -> Self {
        Self { ok: false, output }
    }
}

/// 任务边界捕获的 panic 转消息(与核心 panic_error 同构,crate 私有)。
fn panic_message(p: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "tool panicked".to_string()
    }
}

/// 工具集插件:装配 `ToolRegistry` 服务(统一注册 / 门控 / 可热替换)。
pub struct ToolsPlugin {
    defs: Vec<ToolDef>,
}

impl ToolsPlugin {
    pub fn new(defs: Vec<ToolDef>) -> Self {
        Self { defs }
    }
}

impl Plugin for ToolsPlugin {
    type Deps = ();

    fn name(&self) -> &str {
        "tools"
    }

    async fn apply(&self, ctx: &Ctx, _: ()) -> rutis::Result {
        let handle = tokio::runtime::Handle::current();
        ctx.provide(ToolRegistry::new(handle, self.defs.clone()))?;
        Ok(())
    }
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;
    use crate::scripted::tool_call;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    #[tokio::test]
    async fn cancelled_or_dropped_execute_aborts_async_runner() {
        for drop_caller in [false, true] {
            let started = Arc::new(tokio::sync::Notify::new());
            let release = Arc::new(tokio::sync::Notify::new());
            let wrote = Arc::new(AtomicBool::new(false));
            let def = ToolDef::new("waiting", "wait", serde_json::json!({}), {
                let started = started.clone();
                let release = release.clone();
                let wrote = wrote.clone();
                move |_| {
                    let started = started.clone();
                    let release = release.clone();
                    let wrote = wrote.clone();
                    async move {
                        started.notify_one();
                        release.notified().await;
                        wrote.store(true, Ordering::SeqCst);
                        Ok(Value::Null)
                    }
                }
            });
            let registry = Arc::new(ToolRegistry::new(
                tokio::runtime::Handle::current(),
                vec![def],
            ));
            let cancel = CancellationToken::new();
            let task = tokio::spawn({
                let registry = registry.clone();
                let cancel = cancel.clone();
                async move {
                    registry
                        .execute(&tool_call("id", "waiting", Value::Null), &cancel)
                        .await
                }
            });
            tokio::time::timeout(Duration::from_secs(2), started.notified())
                .await
                .unwrap();
            if drop_caller {
                task.abort();
                assert!(task.await.is_err());
            } else {
                cancel.cancel();
                let result = task.await.unwrap();
                assert!(!result.ok);
                assert!(result.output.contains("cancelled"));
            }
            release.notify_one();
            tokio::task::yield_now().await;
            assert!(!wrote.load(Ordering::SeqCst));
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancellation_wait_is_bounded_for_a_blocking_runner() {
        let started = Arc::new(tokio::sync::Notify::new());
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let release_rx = Arc::new(Mutex::new(Some(release_rx)));
        let def = ToolDef::new("blocking", "block", serde_json::json!({}), {
            let started = started.clone();
            let release_rx = release_rx.clone();
            move |_| {
                let started = started.clone();
                let release_rx = release_rx.clone();
                async move {
                    let release_rx = release_rx.lock().unwrap().take().unwrap();
                    started.notify_one();
                    let _ = release_rx.recv();
                    Ok(Value::Null)
                }
            }
        });
        let registry = Arc::new(ToolRegistry::new(
            tokio::runtime::Handle::current(),
            vec![def],
        ));
        let cancel = CancellationToken::new();
        let execute = tokio::spawn({
            let registry = registry.clone();
            let cancel = cancel.clone();
            async move {
                registry
                    .execute(&tool_call("id", "blocking", Value::Null), &cancel)
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(1), started.notified())
            .await
            .unwrap();
        cancel.cancel();
        let output = tokio::time::timeout(Duration::from_secs(3), execute)
            .await
            .expect("execute waited indefinitely for a synchronous poll")
            .unwrap();
        assert!(!output.ok);
        assert!(output.output.contains("cancelled"));
        release_tx.send(()).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancelling_bash_stops_background_side_effect() {
        let dir = std::env::temp_dir().join(format!(
            "rutis-bash-cancel-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let started = dir.join("started");
        let late = dir.join("late");
        let command = format!(
            "echo started > '{}'; (sleep 0.3; echo late > '{}') & wait",
            started.display(),
            late.display()
        );
        let registry = Arc::new(ToolRegistry::new(
            tokio::runtime::Handle::current(),
            vec![bash::bash_tool()],
        ));
        let cancel = CancellationToken::new();
        let task = tokio::spawn({
            let registry = registry.clone();
            let cancel = cancel.clone();
            async move {
                registry
                    .execute(
                        &tool_call(
                            "id",
                            "bash",
                            serde_json::json!({"command": command, "description": "test cancellation"}),
                        ),
                        &cancel,
                    )
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while !started.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        cancel.cancel();
        assert!(!task.await.unwrap().ok);
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(
            !late.exists(),
            "background descendant survived cancellation"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
