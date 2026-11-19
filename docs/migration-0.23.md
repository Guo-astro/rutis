# 迁移到 0.23

[English](migration-0.23.en.md)

0.23 尚未发布；本指南随行为改动一起写，一节对应一处改动，并按 [#694](https://github.com/arcships/rutis/issues/694) 的要求在发布前写完。它只写与 0.22 的差异；被去掉的形式直接删除，不保留弃用期。

0.23 **不删除、也不改名任何插件看得到的 API**：插件 API 仍是 3，`rutis.json` 的 `format` 仍是 1。会话协议是 **5**，所有端必须一起升级：协议 4 的宿主、运行时、节点或 SDK 在握手时被拒，不存在协议 4 与协议 5 混跑的会话。

## 要改什么

### 所有端一起升级：会话协议 4 → 5（#1099）

设计：[会话协议](protocol-session.md) §2.1、§7、§8、§13；模型：[`specs/call-lifecycle`](../specs/call-lifecycle/README.md)（`call-gap-ack`、`relay-healthy`）。

#### 改了什么

协议 5 新增 `ack` 帧和一条规则：取消由被调方恰好结算一次。调用方的取消表（`calls.cancelled`）以前只由迟到回复清理，而排队中被取消的调用没有合规实现会回复，于是长会话里反复取消排队调用会把每一条都留到会话结束。现在：

| 情形 | 0.22（协议 4） | 0.23（协议 5） |
| --- | --- | --- |
| 还在排队的调用被取消 | 丢掉，不回复；调用方的表项留到会话结束 | 丢掉，方法不运行，参数引用归还，被调方发 `ack`；调用方清掉表项 |
| 已经在运行的调用被取消 | 跑到结束；被调方按各实现决定照常回复或丢掉；调用方丢弃迟到回复 | 跑到结束；结果被丢弃，被调方发 `ack`；不再有回复 |
| 正在等待的 `await` 被取消 | 丢掉，不回复 | 丢掉并确认 |
| 回复在读到取消之前已经写出 | 该回复是迟到回复，被丢弃 | 该回复就是结算；不再发 `ack`，调用方丢弃该回复 |
| 表项已被交叉回复清掉后收到 `ack` | （没有 `ack`） | 忽略 |
| `ack` 之后收到回复 | （没有 `ack`） | `response for unknown call`：会话结束 |
| 对端说协议 4 | 可以通信 | `incompatible session: the far end speaks protocol 4, this side 5: upgrade the far end` |

改动只在线上格式。插件 API、调用结果、错误值、引用授予和 `rutis.json` 格式都不变。一次没有被取消的普通插件调用与 0.22 完全相同。

#### 要改什么

- 宿主和**每一个**运行时、节点、SDK 一起升级：`rutis-host` / Rust crate、Node 与 Bun 运行时包、Python 包、Go 模块、Swift 包、Kotlin 构件。部署里只要有一个 0.22 端就起不来：握手被拒，链路停下，并按协议不兼容处理（不再重试；身份不匹配仍按原来的慢重试处理）。
- 插件代码不用改；依赖“运行中被取消的调用仍会把结果作为值送回”的插件在 0.22 就已经不可移植（SESS-61 允许回或不回），0.23 里结果被丢弃并确认。
- 本仓库七种实现之外的会话协议实现，必须补 `ack` 帧：对它仍持有的取消回 `ack`，`ack` 之后抑制回复，表项已不在时忽略 `ack`；同时要在问候里接受协议 5，WebSocket 子协议用 `rutis.5`。
- 固定 WebSocket 子协议的宿主或工具要改成 `rutis.5`；`rutis.4` 会按旧拨号方被拒。

#### 影响谁

- 插件作者：源码不用改。取消自己发起的调用（丢弃 future / `Promise` / task）看到的调用结果与以前相同：自己的取消错误，绝不会是对端的值。
- 宿主维护者：会话协议、`hello.version` 和 WebSocket 子协议变了；`rutis-host` 的命令、状态行、退出码和关闭码不变。同一部署里的运行时镜像和 SDK 一起升级。
- 部署者：0.22 与 0.23 混跑的部署在握手时被拒，报错会指出该升级哪一端。上线前把两端都升级。

#### 开发者看到什么

**能跑时：** 普通调用、对象、事件和引用授予不变；唯一区别是被取消调用的表项在被调方确认时消失，而不是留到会话结束。

**出错时（Rust 侧，逐字）：**

| 情形 | 输出 |
| --- | --- |
| 对端是协议 4 | `incompatible session: the far end speaks protocol 4, this side 5: upgrade the far end` |
| 对端是协议 99 | `incompatible session: the far end speaks protocol 99, this side 5: upgrade this side (Rust crates rutis-host / rutis-bridge)` |
| 不合规的被调方在 `ack` 之后仍回复 | 会话以传输错误结束，`response for unknown call`；在途调用以 `peer-gone` 失败 |
| 对已取消调用的迟到回复（`ack` 之前） | 不报错；回复被丢弃并计数（`orphans()`），会话继续 |

**要学的新概念：** 只有运行时和会话作者需要知道一个新帧 `ack`；插件看不到它。

#### 验证

```sh
cargo test -p rutis-bridge --all-features
cargo test -p rutis-bridge --all-features --test runtime_conformance
RUTIS_TEST_RUNTIMES=node,bun,python,go,swift,kotlin \
  cargo test -p rutis-bridge --all-features --test runtime_conformance
TLA2TOOLS=/tmp/tla2tools.jar specs/call-lifecycle/run.sh
node tools/check-guarantees.mjs
```

会话测试覆盖排队取消、运行中取消、交叉回复、`ack` 清表、无表项的 `ack`、`ack` 之后的回复；TLA 配置 `call-gap-ack`、`call-mut-ack-then-reply`、`relay-healthy`、`relay-mut-ack-then-reply` 是模型部分。
