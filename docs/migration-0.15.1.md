# 迁移到 0.15.1

[English](migration-0.15.1.en.md)

## 事件是一条平总线

0.13–0.15.0 只把事件送给发出方作用域及其下的监听器。0.15.1 去掉这条规则，恢复 Cordis 的简单模型：一个 root 总线上同名事件的所有监听器都收到，无论在哪个插件、实例、运行时或相连的节点。这是行为变化，不是可选开关。

父级行发出、子级行监听，不再需要额外接线：

```rust
// 父级行（t1）：
ctx.events()
    .parallel(&ctx, &EventKey::named("session/created"), &created)
    .await?;

// 子级行（t1/s1），任意深度：
ctx.events().on(
    &ctx,
    &EventKey::named("session/created"),
    |_ctx, created: &Created| async move { Ok(()) },
)?;
```

兄弟实例的监听器也收到。监听器按登记顺序运行；`parallel` 返回 `CordisError::ListenersFailed`，失败按登记顺序排列；`emit` 在监听器运行处报告失败并立即返回。

### 原来依赖实例隔离

用过滤或不同的事件名：

```rust
ctx.events()
    .filter(|info| info.scope.iter().any(|scope| scope.group == "tenant"))
    .parallel(&ctx, &EventKey::named("session/created"), &created)
    .await?;
```

`Ctx::scope()` 与 `ListenerInfo::scope` 保留：作用域只是过滤用的描述，不是送达规则。跨宿主时，相连的节点仍只见到它的连接 `export` / `import` 承载的名字；连接不继续转发。

## 删除

- `Dispatch.scope`：发出方的作用域不再自报；在过滤里用 `ListenerInfo::scope`。`Dispatch::new` 不再设置它。
- 作用域辅助函数 `scope_contains`（Rust）、`inScope`（Node/Bun）、`within`（Python、Kotlin、Swift）与 `events.Within`（Go）：按名字送达后没有调用者。
- 测试工具的发出方作用域：Node 的 `h.at(scope)` 与 Python 的 `Bus.emitter(scope)` 删除；改用 `h.parallel(..)` / `bus.emitter()`。`load` 的 `scope` 参数仍设置该行监听器的描述。

其余事件模型见[事件协议](design-events-protocol-2026-10-18.md) §3–§9 与[迁移到 0.13](migration-0.13.md)。
