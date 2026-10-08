# rutis 0.7 → 0.8

[English](migration-0.7-to-0.8.en.md) · [发布说明](releases/0.8.0.md)

0.8 起内核 `rutis` 和 dylib 工具链并入发布列车，所有 rutis 包使用同一个版本。大多数项目只需要改依赖版本；自己写 resolver、对 loader 状态做穷尽匹配，或者写 dylib 插件的项目，还要看下面对应的小节。

## 改依赖版本

所有 rutis 包都改到 0.8：

```toml
# Cargo.toml
rutis = "0.8"            # 原来是 0.6
rutis-loader = "0.8"
rutis-bridge = "0.8"
```

```json
"@arcships/rutis": "^0.8.0"
```

```toml
# pyproject.toml
dependencies = ["rutis>=0.8,<0.9"]
```

内核从 0.6.1 直接升到 0.8.0，没有代码变更。内核和列车必须一起升：`rutis-loader` 0.8 依赖 `rutis` 0.8，留在 `rutis = "0.6"` 会得到两份内核，类型对不上。

宿主和语言运行时（`@arcships/rutis-runtime`、PyPI 的 `rutis`）用同一个版本。协议版本没有变，0.7 的运行时仍能运行实例之外的行；实例内的服务名需要 0.8 的运行时。

## 自己写 resolver：用 `Resolved::new` 构造

`Resolved` 新增了字段 `scoped`，并标为 `#[non_exhaustive]`，不能再用结构体字面量构造：

```rust
// 0.7
Arc::new(Resolved {
    factory,
    schema,
    meta,
    foreign_scope: false,
})

// 0.8
Arc::new(Resolved::new(factory).with_schema(schema).with_meta(meta))
```

`foreign_scope: true` 改为 `.with_foreign_scope()`；按实例构建的工厂用 `.with_scoped(scoped)`。字段仍然是公开的，读取方式不变。

## 匹配 loader 的状态

`EntryStatus` 新增变体 `Stopped`：实例里的一个副本卸载了自己，只有这个副本停止。`EntryStatus` 标为 `#[non_exhaustive]`，`match` 需要加一个通配分支：

```rust
match &entry.status {
    EntryStatus::Running(snapshot) => { /* … */ }
    EntryStatus::Stopped => { /* 实例里的副本自己停了 */ }
    _ => { /* Disabled、Inactive、Unresolved，以及以后的新状态 */ }
}
```

`EntryInfo` 新增字段 `instance`（实例里的副本所在的实例）。`EntryInfo` 和新类型 `InstanceInfo` 标为 `#[non_exhaustive]`：读取字段不受影响，解构时要写 `..`。

## 本地传输的 `Handover`

`rutis_bridge::transport::local::Handover` 新增 `Loopback`（Windows 上启动进程的方式），并标为 `#[non_exhaustive]`。对它做 `match` 的代码要加通配分支；只是给 `Spawn::handover` 赋值的代码不用改。

## dylib 插件

`rutis-sdk`、`rutis-dylib`、`rutis-dylib-meta`、`rutis-dylib-launcher` 升到 0.8.0，并首次发布到 crates.io。SDK 身份随版本变化，**0.7 时期编译的 dylib 插件不能被 0.8 宿主加载**，要用 0.8.0 的 SDK 构建包重新编译。插件仍然通过 SDK 构建包编译，不要在插件里直接依赖 crates.io 上的 `rutis-sdk`（打包器会拒绝）。

## 行为变化

- Python 运行时现在会应用行的 `isolate`。之前在 Python 行上写了 `isolate` 的配置，现在按设置隔离服务，与 Node 行一致。
- `ServiceCatalog::key(name)` 只返回全局服务名的 key；实例内的服务名用 `key_in(name, build)`。0.7 里没有实例内的服务名，已有代码不受影响。

## Windows

嵌入 Rust 时，Node / Python 运行时、远程运行时和跨语言共享服务现在可以在 Windows 上原生运行，不需要改代码。`rutis-host` 命令和 `Process::launch` / `Process::mount` 兼容接口仍只支持 Unix，在 Windows 上会返回明确的错误；使用它们的项目在 Windows 上继续用 WSL。
