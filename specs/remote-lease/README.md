# 远端租约：接管、重连与吊销（TLA+）

[English](README.en.md)

用 TLC 穷举远程运行时的租约、新连接接管、link 重连与停止、注册撤销和身份吊销的交错。对应 issue #210，质量规范 Q11.2、Q6.13.3，现状 §7.2（风险 E4、E5）。两个模型：

- `RemoteLease.tla`：监听的远端运行时，和拨号连它的两个控制方 link（issue 的原始范围）；
- `AccessPoint.tla`：运行时拨出到 rutis 的接入点（issue 之后落地的部分）：监听器上的注册与代次、移交、自报 id 的准入、同 id 接管、接入点停止再启动、吊销一个 id 或共享 token。

## 对应的设计和代码

| 模型部分 | 设计 | 实现 |
| --- | --- | --- |
| 控制方 link：拨号、退避重连、按失败类型等待、停止、被吊销的远端 | [远程插件](../../docs/design-remote-plugins-2026-10-03.md) §3.2；[拨出稿](../../docs/design-remote-runtime-dial-2026-10-14.md) §五（吊销钩子，#273）、§六 | `crates/rutis-bridge/src/link.rs`：`dialing`、`start`、`cut` |
| 运行时的租约与接管顺序（鉴权 → 关闭旧会话 → 清理旧租约 → 回应新 hello） | 远程插件 §4.4 | `python/rutis/rutis/__main__.py` `serve`；`go/rutis/listen.go` `listen`；`node/rutis-runtime/src/serve.mjs` `serve`（Bun 的 `src/remote.ts` 同） |
| 注册、代次、移交时复核 | 远程插件 §3.2 | `crates/rutis-bridge/src/registration.rs`：`route`、`hand_over` |
| 接入点：连接即实例、同 id 接管、旧连接释放后再接受新的 | 拨出稿 §四、§六 | `crates/rutis-bridge/src/access_point.rs`：`Shared::connection`、按 id 的 slot |
| 身份层与吊销 | 拨出稿 §五 | `crates/rutis-bridge/src/identity.rs`：`Revocable`、`Revocations`、`SharedToken` |
| 拨出方的重连规则 | 拨出稿 §六 | `python/rutis/rutis/dialing.py` `keep_dialing`（Go、Node、Bun 各有一份同样的规则） |

## 模型

两个模型共同的约定：每条连接每个方向按顺序送达，延迟任意长（比 issue 写的"最多延迟 2 步"更一般）；一方关闭连接后不再读它，发给它的帧丢弃（只有需要读它们的变异保留）。环境的动作（吊销、停止、超时、断线、接入点停止）任何时刻都可能发生，配置决定打开哪些。

### `RemoteLease.tla`

- **参与者**：控制方 `c1`、`c2`（rutis 的拨号 link，各自的身份可吊销远端），一个监听的远端运行时。连接 `<<c, n>>` 是 `c` 第 n 次拨号。
- **控制方**（`link.rs` `dialing`）：循环顶部，远端已被吊销就不拨，按认证失败等待；拨号成功后升级完成，运行时的监听器把连接排进队列；等运行时的 hello（`start`），收到后提供 Peer、状态 `Ready`；之后 loader 经 Peer 装载行（`load` 帧）。会话结束（被接管 4002，或运行时结束会话）按可重试处理，退避后重连；吊销时 `cut` 关闭会话，按认证失败等待。停止（dispose）中止任务：关闭会话、取消重试。环境：拨号失败、握手超时或断线（`Lose`）、吊销、停止。
- **运行时**（Python `serve` 等）：一次只服务一个控制方。取下一个连接时若有当前会话，按 §4.4 分三步：① 关闭旧会话的通道（4002），不再读它的帧；② 清理旧租约（行全部卸载）；③ 新会话开始，回应 hello。会话的对端关闭时清理它的租约。
- **重连次数**：每个控制方最多拨 4 次（3 次重连）。

### `AccessPoint.tla`

- **参与者**：拨出的运行时 `d1`、`d2`，自报同一个 id（`SameId`）或各自的 id；rutis 侧的 transport 注册表、接入点、准入（共享 token + 自报 id，可吊销）。
- **拨出方**（`dialing.py`）：拨号时 transport 按凭据路由（`route`）：监听器上没有注册为可重试；共享 token 被吊销为 403；否则拿到当前注册代次的票据。收到 rutis 的 hello 即"connected"；连接被关闭（4002、4003、断线）后等待再拨。停止后不再拨号。
- **rutis**：
  - `HandOver`：在注册表锁内，票据的代次仍是当前注册的才移交，否则关闭；接入点读到 hello 时准入检查 id（被吊销的以 4003 拒绝）。
  - `TakeSlot`：锁住这个 id 的 slot（同一 id 的接管一个接一个）；到达得比当前连接早的被取代（4002）；接入点已停止的关闭；否则关闭（4002）并释放当前连接，旧实例先走。
  - `Accept`：创建实例（`Accept::accept`），释放 slot。
  - `EndLive`：实例的会话关闭、接入点停止、或它的 id 被吊销时，在 slot 锁内结束它（如果它仍是当前连接）。
  - 环境：接入点停止（注册撤销）并以新代次重新注册；吊销一个 id；吊销全部（收回共享 token）。
- **重连次数**：每个拨出方最多拨 4 次（3 次重连）；打开断线（`lose`）的配置为 3 次（2 次重连），见"结果"。

## 简化

- 时间不建模：退避、30 秒、握手 10 秒、"连接稳定 60 秒后重置退避"都只体现为"之后某个时刻"。超时是任意时刻都可能发生的 `Lose`，比真实的计时更一般。
- 有限的重连次数是模型的边界，不是实现的行为。活性只在没有任何故障的运行里检查；在那里每个行为都是有限的（每次拨号用掉一次额度），所以 `WF_vars(Next)` 足以让每个能走的步骤走到。两个控制方配置了同一个运行时（或两个拨出方用同一个 id）时会互相顶掉（拨出稿 §六 已写明，`leases.rs` 的注释同）；模型里额度用完时最后一个连上的持有租约，实现里靠退避把节奏放慢到每方约 30 秒一次。
- `RemoteLease`：控制方的 hello 不建模（运行时开始会话时读它，没有别的作用）；拨号时"检查吊销"与"拨号"合成一步（两者之间被吊销时 `Revocable::credential` 不给凭据，运行时回 401，同样连不上）。
- `AccessPoint`：移交与读 hello、准入合成一步（两者之间接入点停止，与在之后停止一样由 `TakeSlot` 处理）；rutis 的 hello 在准入之后才发（实现里会话打开就发，被拒的拨出方会先看到"connected"再收到 4003，这不影响不变式）；关闭旧连接与释放它合成一步。行的装载不建模（运行时一侧的租约由 `RemoteLease` 覆盖），只建模拨出方经会话发给 rutis 的调用。
- 静态 link 的监听端（rutis 监听、对端拨入，一个对端一个注册）没有单独建模：它是接入点去掉"自报 id"和并发 slot 后的顺序特例，修复和测试见下文"反例"。
- 每个动作是原子的。吊销恰好落在"检查吊销"与"提供 Peer / 创建实例"之间（没有 await 的几行代码，或 `Accept::accept` 运行期间）时，按"之后吊销"处理：已建立的会话随即被吊销关闭，与运行中吊销相同。

## 不变式与活性

| 名字 | 模型 | 内容 |
| --- | --- | --- |
| `OneLease` | RemoteLease | 运行时最多一个活动租约：正在服务的会话和还没清理的行加起来只属于一个连接 |
| `OldCommandsStayOld` | RemoteLease | 一个会话的指令（`load`）只作用于这个会话的租约，旧会话迟到的指令不作用于新租约 |
| `StoppedStaysDown` | 两者 | 停止的 link / 拨出方不再拨号，也不留打开的连接 |
| `ReadyIsCurrent` | RemoteLease | 控制方只在当前、仍打开的连接上 `Ready`：放弃了的握手迟到的 hello 不会让它复活 |
| `RevokedStaysOut` | 两者 | 身份被吊销后不再为它建立会话：控制方不再 `Ready`（不提供 Peer），接入点不再为这个 id 创建实例 |
| `OneInstancePerId` | AccessPoint | 每个连接 id 最多一个实例（接入点里的"租约"） |
| `OldCallsStayOld` | AccessPoint | 一个会话发来的调用只作用于这个会话的实例 |
| `NoStaleHandOver` | AccessPoint | 握手路由到的注册已被撤销（接入点停止，或停止后又以新代次注册）时，不完成移交 |
| `RevocationScoped` | AccessPoint | 因吊销而结束的连接，它自己的 id 确实被吊销了（吊销一个 id 只关闭这个 id 的连接） |
| `EventuallyHeld` | 两者，无故障 | 最终有一个控制方持有租约（`RemoteLease`：某个控制方 `Ready`，运行时正在服务它的连接；`AccessPoint`：每个 id 有实例，且它的运行时已连上） |
| `EndsHeld` | 两者，无故障 | `[]<>Held`：在有限的运行里即"结束时有人持有租约"，排除"互相顶掉后谁都没拿到" |

## 结果

TLC 2.19（tla2tools v1.7.4），Java 17，16 核（`-workers auto`）。"通过"指穷举完成、所列性质全部成立；这些行的状态数是确定的。

| 配置 | 内容 | 结果 | 状态数 | 时间 |
| --- | --- | --- | --- | --- |
| `lease-healthy` | 2 个控制方，各 4 次拨号，无故障；含活性 | 通过 | 791 | 1 s |
| `lease-faults` | 同上，吊销、停止、拨号失败、超时 / 断线任意时刻发生 | 通过 | 4,206,661 | 25 s |
| `access-healthy-same` | 2 个拨出方同一 id，各 4 次拨号，无故障；含活性 | 通过 | 24,982 | 1 s |
| `access-healthy-distinct` | 各自的 id；含活性 | 通过 | 244 | 1 s |
| `access-revoke-same` | 同一 id，各 4 次；吊销 id 或 token、拨出方停止 | 通过 | 324,889 | 3 s |
| `access-revoke-distinct` | 各自的 id；同上 | 通过 | 27,578 | 1 s |
| `access-registration` | 同一 id，各 4 次；接入点停止再启动（2 个注册代次）、吊销 | 通过 | 3,109,267 | 18 s |
| `access-lose` | 同一 id，各 3 次；超时 / 断线、吊销 | 通过 | 4,951,529 | 40 s |
| `access-all` | 同一 id，各 2 次；全部故障 | 通过 | 1,171,884 | 9 s |

全部 21 个配置（含下文的反例与变异）一共约 2 分钟；`TLC_WORKERS=4` 约 3 分钟。

打开断线的接入点配置限制为每方 3 次拨号：4 次时（只开断线）10 分钟内状态数超过 8,100 万仍在增长。所以按故障分开跑（吊销、注册、断线各一份配置），全部故障一起的配置用更小的规模。

## 实现里的反例（已修复）

`lease-before-fix`、`access-before-fix` 按修复前的实现建模（`RevokeCheck = FALSE`），TLC 找到违反 `RevokedStaysOut` 的反例：

1. **控制方 link：握手期间被吊销，仍然提供了 Peer**（`lease-before-fix`，5 步）。控制方拨号；运行时开始会话、发出 hello；控制方的身份吊销运行时；hello 到达，`start` 提供 Peer、状态 `Ready`，之后 `select!` 上的 `revoked()` 才把会话切断。在这中间依赖 Peer 的插件（`RuntimePlugin`、loader）可能已经开始挂载、装载行，把求值后的配置（可能含密钥）发给一个已被吊销的远端。监听的 link 同样：`start` 先提供 Peer，再检查 `is_revoked()` 并切断。
2. **接入点：接管期间被吊销的 id 仍被接受**（`access-before-fix`）。同一 id 的新连接 hello 已通过准入，正在等 slot 或等旧连接释放；这时 id 被吊销；释放完成后 `Accept::accept` 照常运行（创建实例、挂载运行时），然后才被吊销监听切断。

修复（单独的提交）：`link.rs` 的 `start` 在 hello 之后、提供 Peer 之前检查吊销，被吊销就关闭会话（监听时以 4003 告诉拨号方），按认证失败等待；`access_point.rs` 在接管之后、`Accept::accept` 之前检查，被吊销就以 4003 拒绝，发出 `AccessEvent::Rejected`。修复后的模型（其余配置，`RevokeCheck = TRUE`）全部通过。测试：`crates/rutis-bridge/tests/revocation.rs` 的 `a_far_end_revoked_while_it_greets_never_becomes_a_peer`、`an_id_revoked_while_its_connection_takes_over_is_not_accepted`，修复前分别失败（前者 8/8 次，后者确定失败）。

TLC 没有找到其他与实现不符的反例：§4.4 的接管顺序、注册代次复核、按连接（而不是按 id）读帧、停止时取消重试，都与模型一致。

## 变异：确认模型不是空的

每个不变式和活性性质都配一个故意引入的错误，TLC 必须找到反例（`expected.txt` 里的 `violated:<名字>`；活性为 `violated:liveness`）：

| 配置 | 变异 | 被违反的性质 |
| --- | --- | --- |
| `lease-mut-start-before-clean` | 运行时在清理旧租约之前回应新 hello（§4.4 的 ② ③ 对调） | `OneLease` |
| `lease-mut-read-old` | 运行时继续读被接管的会话，把它的指令作用到当前租约（共用一张行表） | `OldCommandsStayOld` |
| `lease-mut-stop-keeps-retry` | 停止一个正在等待重连的 link 时没有取消重试 | `StoppedStaysDown` |
| `lease-mut-late-hello` | 已放弃的握手迟到的 hello 让 link 进入 `Ready` | `ReadyIsCurrent` |
| `lease-before-fix` | 修复前的 link（见上） | `RevokedStaysOut` |
| `lease-mut-wait-for-old` | 接管时运行时停止读旧会话，但不关闭它，等旧控制方自己断开（无故障运行） | 活性（`EndsHeld`） |
| `access-mut-accept-before-release` | 接管时先接受新连接，再释放旧连接 | `OneInstancePerId` |
| `access-mut-route-by-id` | 继续读被接管的会话，把它的调用按 id 交给当前实例 | `OldCallsStayOld` |
| `access-mut-no-gen-check` | 移交时不比较注册代次 | `NoStaleHandOver` |
| `access-before-fix` | 修复前的接入点（见上） | `RevokedStaysOut` |
| `access-mut-revoke-cuts-all` | 任何吊销都唤醒并结束所有连接（不再复核自己的 id） | `RevocationScoped` |
| `access-mut-stop-keeps-retry` | 停止一个正在等待再拨的拨出方时没有取消再拨 | `StoppedStaysDown` |

每个变异都在 1 秒左右找到反例（几十到一万多个状态）。

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

远程插件 §3.2、§4.4 或拨出稿 §五、§六 修改时，或 `link.rs`（`dialing`、`listening`、`start`）、`registration.rs`（`hand_over`）、`access_point.rs`（`Shared::connection`）、各语言监听端的接管顺序、拨出方的重连规则改变时，同步修改这个模型和 `expected.txt`（规范 Q11.2.3）。
