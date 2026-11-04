# 迁移到 0.21

[English](migration-0.21.en.md)

0.21 还没发布，这份指南随不兼容的改动一起写：每合并一件加一节，发布前按 [#694](https://github.com/arcships/rutis/issues/694) 补齐。下面只写与 0.20 相比的变化，删掉的写法直接删，不标弃用。

不兼容的改动按类列在这里，每类的改法见对应一节：

- **交来的函数默认异步**：不在同步调用里时，调用另一个进程交来的函数现在是异步的（Node、Bun 返回 Promise，Python 可 `await`），旧的写法要加 `await`（[交来的函数默认异步](#交来的函数默认异步932)）。
- **Rust 的 `rutis::Error::Value` 变成带 `reason` 的结构体变体**：`Value(String)` → `Value { message, reason }`，按变体匹配的宿主与以前构造它的代码要改（[Rust 的值错误带上 reason](#rust-的值错误带上-reason936)）。

其余都是行为不变，或者只放宽。

## Rust 的值错误带上 reason（#936）

设计：[值传不过去时的报错统一](design-value-errors-2026-11-03.md)。只有 Rust 的宿主代码受影响：插件的写法不变。

### 改了什么

`rutis::Error::Value` 从元组变体变成结构体变体，多带一个 `reason`：它是「值为什么传不过去」的名字，会出现在错误的 `data.reason` 里（`non-finite`、`cycle`、`too-deep`、`type`、`tag`、`limit` 之一）。同时新增两个构造器 `Error::value(message)` 与 `Error::why(reason, message)`。

| 0.20 | 0.21 |
| --- | --- |
| `Error::Value(String::from("…"))` | `Error::value("…")`（不带 reason），或 `Error::why("too-deep", "…")` |
| `match error { Error::Value(text) => … }` | `match error { Error::Value { message, .. } => … }` |
| `error.data()` 只有 `{part, argument?, limit?}` | 值传不过去时多一个 `reason` |

### 要改什么

- **构造**：把 `Error::Value(text)` 换成 `Error::value(text)`；原样保留 `text` 的那个变量，类型仍是 `String`。要给出 reason 的地方用 `Error::why("non-finite", text)`，名字只能取上面那六个。
- **匹配**：`Error::Value(text) =>` 换成 `Error::Value { message, .. } =>`；需要 reason 时写 `Error::Value { message, reason } =>`，`reason` 是 `Option<&'static str>`，不传的时候是 `None`。
- **读 `data`**：程序按名读 `data.reason` 的新值；不读它的代码不受影响（多一个键）。跨进程的码与帧都没变，旧宿主收到新运行时的错误照旧只看它认识的键。

```rust
// 0.20
let error = Error::Value(format!("{call}: …"));
match error {
    Error::Value(text) => eprintln!("{text}"),
    _ => {}
}

// 0.21
let error = Error::value(format!("{call}: …"));
let error = Error::why("too-deep", format!("{call}: …"));
match error {
    Error::Value { message, .. } => eprintln!("{message}"),
    _ => {}
}
```

### 影响

- 插件作者：看不到变化，插件的写法与插件 API（3）都不变。
- 宿主持有者：Rust 宿主里构造或匹配 `rutis::Error::Value` 的地方要改（上面的两行）；别的语言没有这个变体。
- 部署的人：看不到变化。会话协议（4）、`rutis.json` 格式（1）、清单格式（1）都不变。

## 交来的函数默认异步（#932）

设计：[交来的函数怎样调用](design-function-refs-2026-11-03.md)。Node、Bun、Python 一致；Go、Swift、Kotlin、Rust 的写法不变。

### 改了什么

按引用传来的函数（另一个插件交给你的回调、宿主给的回调）现在与服务方法一样调用：不在同步调用里时是异步的，在同步调用里（`sync` 方法、同步的 `waterfall`、同步调用的监听器）同步返回结果。

| 情况 | 0.20 | 0.21 |
| --- | --- | --- |
| 在 `apply`、`async` 方法、监听器、定时器里调用交来的函数 | 同步等结果、返回结果本身；写成 `async` 的假服务会撞上 `SyncWaitCycle` | 返回 Promise（Node、Bun）或可 await 的对象（Python）；`await` 它得到结果 |
| 在 `sync` 方法、同步 `waterfall` 里调用交来的函数 | 同步返回结果 | 不变：那里没有别的东西能跑，同步返回结果 |
| 测试里调用插件交给测试的函数（TypeScript、Bun） | 同步等结果 | 异步：`await` 它。假服务可以写 `async` |
| 测试里调用插件交给测试的函数（Python） | 测试拿到的是插件自己的函数 | 不变：`async def` 时 `await` 它，普通 `def` 时直接用它的值 |

### 要改什么

- **TypeScript、Bun**：`const text = f()` → `const text = await f()`。改过的写法在 0.20 的 Node、Bun 上也能运行（`await` 一个不是 Promise 的值原样给出它）。
- **Python**：`text = f()` → `text = await f()`（`f.call_async(...)` 是同一件事；函数自己返回的 Promise 一并等完，和 JavaScript 的 `await` 一样，不用再 `await` 一次）；同一条 `sync` 方法里保持 `f()`，它在那里同步返回结果。0.20 的运行时上 `await` 一个普通值会失败（`TypeError: object str can't be used in 'await' expression`），所以同一份 Python 插件不要在两种运行时上混用。
- **测试里的假服务**：过去必须直接返回结果的地方，现在两种都行。写成 `async` 而调用的地方又 `await` 的测试，在 0.21 上通过、在 0.20 上失败 —— 按 0.21 的写法改，不要交叉使用。

### 影响

- 插件作者：只在调用别的进程交来的函数时受影响（大多数插件没有）；服务方法的调用、事件的发出与监听都不变。
- 宿主持有者：宿主调用插件交来的函数不变（Rust 的 `call` / `call_async`，Go、Swift、Kotlin 各自的写法）。宿主把回调交给 Node、Bun、Python 的插件时，那些插件的代码 `await` 它。
- 部署的人：看不到变化。
- 线上格式、会话协议（4）、`rutis.json` 格式（1）、插件 API（3）都不变；`rutis-host types` 写出的类型不用重新生成（回调的返回类型本来就写 `string | Promise<string>`，`await` 在两种上下文都通过类型检查）。
