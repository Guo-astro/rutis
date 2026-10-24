# 迁移到 0.16

[English](migration-0.16.en.md)

0.16 保留 0.15.1 的平总线，删掉那个版本为了源码兼容留下的转接层。事件 API 回到简单形态：按名字送达，作用域只是过滤用的描述。

## Rust 删除

- `rutis::events::Dispatch::scope`：发出方的作用域不再自报。在过滤里用 `ListenerInfo::scope`，或用不同的事件名。
- `rutis::events::scope_contains`：在过滤里取 `info.scope` 自己比较。

`Dispatch` 仍用 `Dispatch::new(..)` 构造，没有放 scope 的位置。

## SDK 与测试工具删除

- Node/Bun 的 `inScope`、Python/Kotlin/Swift 的 `within`、Go 的 `events.Within`：它们只是作用域前缀谓词，不是送达规则。
- Node 的 `h.at(scope)` 与 Python 的 `Bus.emitter(scope)`：用 `h.parallel(..)` / `bus.emitter()` 发出。`load` 的 `scope` 参数仍设置该行监听器的描述。

需要旧辅助函数时，它就是一个谓词：

```rust
fn within(listener: &[Scope], emitter: &[Scope]) -> bool {
    listener.len() <= emitter.len()
        && listener.iter().zip(emitter).all(|(a, b)| a.id == b.id)
}
```

## 没有变化的

- `ctx.on` / `ctx.emit` / `parallel` / `serial` / `waterfall` / `bail` / `filter`。
- 平总线送达：一个 root 总线上同名事件的所有监听器都收到，兄弟与子级实例包括在内。
- 跨宿主：只有连接 `export` / `import` 的名字过去，事件仍只走一跳。
- 事件的线上格式。

见 [0.16.0 发布说明](releases/0.16.0.md)与[事件协议](design-events-protocol-2026-10-18.md) §3–§9。
