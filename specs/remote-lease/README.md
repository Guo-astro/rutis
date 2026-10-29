# 远端租约：接管与重连（TLA+）

[English](README.en.md)

用 TLC 穷举远程运行时的租约、新连接接管、link 重连与停止的交错。对应 issue #210，质量规范 Q11.2、Q6.13.3，现状 §7.2（风险 E4、E5）。两个模型：

- `RemoteLease.tla`：监听的远端运行时，和拨号连它的两个控制方 link（issue 的原始范围）；
- `AccessPoint.tla`：远端拨入 `rutis-host/listen` 的一行：握手并行、每个 id 一个 session、同 id 的新连接顶替旧的，以及拨出方的重连规则。

## 对应的设计和代码

| 模型部分 | 设计 | 实现 |
| --- | --- | --- |
| 控制方 link：拨号、退避重连、按失败类型等待、停止 | [远程插件](../../docs/design-remote-plugins-2026-10-03.md) §3.2；[拨出稿](../../docs/design-remote-runtime-dial-2026-10-14.md) §六 | `crates/rutis-bridge/src/link.rs`：`dialing`、`start` |
| 运行时的租约与接管顺序（鉴权 → 关闭旧会话 → 清理旧租约 → 回应新 hello） | 远程插件 §4.4 | `python/rutis/rutis/__main__.py` `serve`；`go/rutis/internal/runtime/listen.go` `listen`；`node/rutis-runtime/src/serve.mjs` `serve`（Bun 的 `src/remote.ts` 同） |
| listen：握手并行，同 id 顶替（旧 session 卸载完 → 旧 Channel 以 4002 关闭 → 装新 session） | [远程会话](../../docs/design-remote-session-2026-10-28.md) §4.2 | `crates/rutis-host/src/connect/listen.rs`：`Listening::run`、`Replaceable` |
| 拨出方的重连规则 | 拨出稿 §六 | `python/rutis/rutis/dialing.py` `keep_dialing`（Go、Node、Bun 各有一份同样的规则） |

## 模型

两个模型共同的约定：每条连接每个方向按顺序送达，延迟任意长（比 issue 写的"最多延迟 2 步"更一般）；一方关闭连接后不再读它，发给它的帧丢弃（只有需要读它们的变异保留）。环境的动作（停止、超时、断线）任何时刻都可能发生，配置决定打开哪些。

### `RemoteLease.tla`

- **参与者**：控制方 `c1`、`c2`（rutis 的拨号 link），一个监听的远端运行时。连接 `<<c, n>>` 是 `c` 第 n 次拨号。
- **控制方**（`link.rs` `dialing`）：拨号成功后升级完成，运行时的监听器把连接排进队列；等运行时的 hello（`start`），收到后提供 Peer、状态 `Ready`；之后 loader 经 Peer 装载行（`load` 帧）。会话结束（被接管 4002，或运行时结束会话）按可重试处理，退避后重连。停止（dispose）中止任务：关闭会话、取消重试。环境：拨号失败、握手超时或断线（`Lose`）、停止。
- **运行时**（Python `serve` 等）：一次只服务一个控制方。取下一个连接时若有当前会话，按 §4.4 分三步：① 关闭旧会话的通道（4002），不再读它的帧；② 清理旧租约（行全部卸载）；③ 新会话开始，回应 hello。会话的对端关闭时清理它的租约。
- **重连次数**：每个控制方最多拨 4 次（3 次重连）。

### `AccessPoint.tla`

- **参与者**：拨出的远端 `d1`、`d2`，自报同一个 id（`SameId`）或各自的 id；rutis 侧是一行 `rutis-host/listen`。
- **拨出方**（`dialing.py`）：拨号即连接送到 listen、开始握手。收到 rutis 的 hello 即"connected"；连接被关闭（4002、断线、握手失败）后等待再拨。停止后不再拨号。
- **listen**（`Listening::run`）：
  - 握手在 `JoinSet` 里并行：`HandshakeDone`（回 hello，远端的插件这时一个都还没注册）或 `HandshakeFails`（关闭这条连接，什么别的都不动）。
  - `Join`：循环取一个握手完成的连接。这个 id 没有 session 就装上；有就把旧连接的 closer 标为 replaced、从 `live` 拿掉、开始卸载旧 session，然后循环停在 `unload().await` 上，别的什么都不做。
  - `Replace`：旧 session 卸载完（`Unloaded`：远端插件全部卸载，Channel 最后关闭，因为标了 replaced 所以是 4002），装新 session。
  - `Register`：装上的 session 注册远端的插件。
  - `EndSelf`、`Unloaded`、`ReadEnding`：session 自己结束（远端关闭或断线），卸载；循环读到它的结束时，只有 `live` 里还是它（同一个 serial）才删掉。
- **重连次数**：每个拨出方最多拨 4 次（3 次重连）；打开断线（`lose`）的配置为 3 次，全部故障一起的为 2 次，见"结果"。

## 简化

- 时间不建模：退避、30 秒、握手 10 秒、"连接稳定 60 秒后重置退避"都只体现为"之后某个时刻"。超时是任意时刻都可能发生的 `Lose`，比真实的计时更一般。
- 有限的重连次数是模型的边界，不是实现的行为。活性只在没有任何故障的运行里检查；在那里每个行为都是有限的（每次拨号用掉一次额度），所以 `WF_vars(Next)` 足以让每个能走的步骤走到。两个控制方配置了同一个运行时（或两个拨出方用同一个 id）时会互相顶掉（拨出稿 §六 已写明）；模型里额度用完时最后一个连上的持有租约，实现里靠退避把节奏放慢到每方约 30 秒一次。
- `RemoteLease`：控制方的 hello 不建模（运行时开始会话时读它，没有别的作用）。
- `AccessPoint`：token 路由（`Router`）不建模：拨号即送到这一行。远端插件只建模"全部注册"与"全部卸载"两个状态，逆序卸载合成一步；远端经会话的调用不建模。`old.closer.close(Replaced)` 在 `Unloaded` 之后，Channel 已经关闭，模型里不单独成步。
- 每个动作是原子的。

## 不变式与活性

| 名字 | 模型 | 内容 |
| --- | --- | --- |
| `OneLease` | RemoteLease | 运行时最多一个活动租约：正在服务的会话和还没清理的行加起来只属于一个连接 |
| `OldCommandsStayOld` | RemoteLease | 一个会话的指令（`load`）只作用于这个会话的租约，旧会话迟到的指令不作用于新租约 |
| `StoppedStaysDown` | 两者 | 停止的 link / 拨出方不再拨号，也不留打开的连接 |
| `ReadyIsCurrent` | RemoteLease | 控制方只在当前、仍打开的连接上 `Ready`：放弃了的握手迟到的 hello 不会让它复活 |
| `OnePluginSetPerId` | AccessPoint | 同一个 id 永远不会有两个 session 的远端插件同时注册着：新 session 只在旧的完全卸载之后才注册 |
| `EventuallyHeld` | 两者，无故障 | 最终有一个控制方持有租约（`RemoteLease`：某个控制方 `Ready`，运行时正在服务它的连接；`AccessPoint`：每个 id 有装上的 session、远端插件已注册，且它的拨出方已连上） |
| `EndsHeld` | 两者，无故障 | `[]<>Held`：在有限的运行里即"结束时有人持有租约"，排除"互相顶掉后谁都没拿到" |

## 结果

TLC 2.19（tla2tools v1.7.4），Java 17，16 核（`-workers auto`）。"通过"指穷举完成、所列性质全部成立；这些行的状态数是确定的。

| 配置 | 内容 | 结果 | 状态数 | 时间 |
| --- | --- | --- | --- | --- |
| `lease-healthy` | 2 个控制方，各 4 次拨号，无故障；含活性 | 通过 | 791 | 1 s |
| `lease-faults` | 同上，停止、拨号失败、超时 / 断线任意时刻发生 | 通过 | 1,050,999 | 12 s |
| `access-healthy-same` | 2 个拨出方同一 id，各 4 次拨号，无故障；含活性 | 通过 | 16,020 | 4 s |
| `access-healthy-distinct` | 各自的 id；含活性 | 通过 | 64 | 2 s |
| `access-lose` | 同一 id，各 3 次；拨号失败、握手失败、断线、session 结束 | 通过 | 6,350,873 | 48 s |
| `access-all` | 同一 id，各 2 次；全部故障（断线、停止） | 通过 | 129,131 | 3 s |

全部 13 个配置（含下文的变异）一共不到 2 分钟。

打开断线的配置限制拨号次数：只开断线时每方 3 次，断线和停止一起时 2 次（3 次时 1,044 万个状态、76 秒）。

## 实现里的反例（已修复）

TLC 找到过两处握手期间吊销身份仍建立会话（link 提供了 Peer、接入点在接管期间接受了被吊销的 id），已修复（#210）。身份吊销（`Revocations` / `Revocable`，#273）在 0.19 删除，配置 `lease-before-fix`、`access-before-fix`、`access-revoke-same`、`access-revoke-distinct`、`access-mut-revoke-cuts-all` 与性质 `RevokedStaysOut`、`RevocationScoped` 随之删除。

0.19 删掉接入点（`access_point.rs`）和注册代次（`registration.rs`），`AccessPoint.tla` 改为按 `rutis-host/listen` 的代码重建；配置 `access-registration`、`access-mut-route-by-id`、`access-mut-no-gen-check` 与性质 `OneInstancePerId`、`OldCallsStayOld`、`NoStaleHandOver` 随之删除。

除此之外 TLC 没有找到与实现不符的反例：§4.4 的接管顺序、listen 的顶替顺序、停止时取消重试，都与模型一致。

## 变异：确认模型不是空的

每个不变式和活性性质都配一个故意引入的错误，TLC 必须找到反例（`expected.txt` 里的 `violated:<名字>`；活性为 `violated:liveness`）：

| 配置 | 变异 | 被违反的性质 |
| --- | --- | --- |
| `lease-mut-start-before-clean` | 运行时在清理旧租约之前回应新 hello（§4.4 的 ② ③ 对调） | `OneLease` |
| `lease-mut-read-old` | 运行时继续读被接管的会话，把它的指令作用到当前租约（共用一张行表） | `OldCommandsStayOld` |
| `lease-mut-stop-keeps-retry` | 停止一个正在等待重连的 link 时没有取消重试 | `StoppedStaysDown` |
| `lease-mut-late-hello` | 已放弃的握手迟到的 hello 让 link 进入 `Ready` | `ReadyIsCurrent` |
| `lease-mut-wait-for-old` | 接管时运行时停止读旧会话，但不关闭它，等旧控制方自己断开（无故障运行） | 活性（`EndsHeld`） |
| `access-mut-accept-before-release` | 顶替时先装新 session，再卸载旧的（§4.2 的 3 挪到 1 之前） | `OnePluginSetPerId` |
| `access-mut-stop-keeps-retry` | 停止一个正在等待再拨的拨出方时没有取消再拨 | `StoppedStaysDown` |

每个变异都在 2 秒内找到反例（几十到几千个状态）。

## 怎么运行

需要 Java 11 以上和 `tla2tools.jar`（[v1.7.4](https://github.com/tlaplus/tlaplus/releases/tag/v1.7.4)，sha256 `936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88`）。jar 不要提交进仓库。

```sh
curl -sSLo /tmp/tla2tools.jar https://github.com/tlaplus/tlaplus/releases/download/v1.7.4/tla2tools.jar
TLA2TOOLS=/tmp/tla2tools.jar specs/remote-lease/run.sh                     # 全部配置
TLA2TOOLS=/tmp/tla2tools.jar specs/remote-lease/run.sh lease-faults        # 单个配置
TLC_WORKERS=4 TLA2TOOLS=/tmp/tla2tools.jar specs/remote-lease/run.sh       # 限定 TLC 的线程数
```

`run.sh` 按名字选模型（`lease-*` → `RemoteLease.tla`，`access-*` → `AccessPoint.tla`），把每个配置的结果和 `expected.txt` 比较，全部符合时退出码为 0；日志和反例写在 `TLC_OUT`（默认一个临时目录）。CI 的 `specs` 任务在改了 `specs/**` 的 PR 和每晚的完整运行里跑它（[CI](../../docs/ci.md) §4）。

## 维护

远程插件 §3.2、§4.4、拨出稿 §六 或远程会话 §4.2 修改时，或 `link.rs`（`dialing`、`listening`、`start`）、`listen.rs`（`Listening::run`）、各语言监听端的接管顺序、拨出方的重连规则改变时，同步修改这个模型和 `expected.txt`（规范 Q11.2.3）。
