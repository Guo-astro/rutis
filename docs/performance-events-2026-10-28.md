# 运行时里的事件分发：删本地分发前的基准

日期：2026-10-28。代码：main `212ac7ce`（rutis 0.18 之后、0.19 之前），各语言 SDK 还在本进程分发事件（"本地分发"）。

[平台清理](design-platform-cleanup-2026-10-28.md) §7.5 要删掉各语言的本地分发，事件一律交给宿主。这份文档记下删之前与经宿主的数字，按 §7.5 的条件判断能不能接受：

- 经宿主的 `emit` 中位数按实测值接受：Node 71 微秒，Python 139 微秒（见"删后"一节）。经宿主的路径以后可优化；
- dsh 基线（`node/baseline/`）跑完一遍的时间不超过下面数字的 1.1 倍。

## 测什么

- **事件**：一个宿主，一行插件，插件里既有监听器也有发出方，所以两者在同一个运行时。Node（`tools/bench/events/bench.mjs`）和 Python（`tools/bench/events/bench_events.py`）各一份。每种方式先预热 1 000 次，再测 10 000 次，报中位数与 p99（微秒）：
  - `emit`：从调用 `ctx.emit` 到监听器执行完（监听器把等待中的 Promise / future 结束掉，发出方 `await` 它）；
  - `serial`：`await ctx.serial(...)`，监听器返回参数；
  - `bail`：同步的 `ctx.bail(...)`，同步监听器返回参数。

  插件只用公开的插件 API（`ctx.on`、`ctx.emit`、`ctx.serial`、`ctx.bail`），由 `rutis-host run` 按 `rutis.json` 跑起来，不碰 SDK 内部的分发代码，所以删本地分发后不用改就能再跑。
- **dsh 基线**：`rutis-bridge` 的测试 `dsh_baseline`（`crates/rutis-bridge/tests/dsh_baseline.rs`）跑一遍的总时间：它先在原生 Cordis 里跑一遍 `node/baseline` 的场景，再把同样的已发布 Cordis 插件经 rutis 挂载、逐个调用并对比。连跑 5 次取中位数。同时单独记原生 Cordis 那一半（`node native.mjs`）作参照。

## 怎么运行

```sh
npm --prefix node/rutis-runtime ci
npm --prefix node/baseline ci            # 只有 dsh 需要
tools/bench/events/run.sh                # node、python、dsh 三项；也可只写其中几项
```

- `RUTIS_HOST`：用已有的 `rutis-host`；不写时用 cargo 构建，`RUTIS_BENCH_PROFILE` 选 profile（默认 `release`）。
- `RUTIS_PYTHON`：Python 解释器；不写时由宿主按默认规则找。脚本把 `RUTIS_PYTHON_PATH` 设为本仓库的 `python/rutis`，解释器不需要另装 `rutis`。
- `RUTIS_BENCH_ITERATIONS`、`RUTIS_BENCH_WARMUP`：次数，默认 10 000 与 1 000。

本次用的是 `RUTIS_BENCH_PROFILE=dev tools/bench/events/run.sh`（调试构建的宿主）。删前的事件分发全在运行时进程里，不经过宿主，宿主的构建方式不影响这些数字。删后宿主在路径上，再跑时用默认的 `release`。dsh 基线的测试由 `cargo test` 构建（dev profile），删前删后相同。

## 机器

macOS 26.6.2，Apple M4 Max（16 核），128 GB 内存；rustc 1.98.1，Node v24.19.0，Python 3.12.2。没有绑核，没有固定频率，机器上同时有别的编译任务。数字是本机样本，不是 CI 门槛。

## 删前（本地分发）

连跑两次，两次接近，下表是第二次。单位：微秒。

| 运行时 | 方式 | 中位数 | p99 |
| --- | --- | ---: | ---: |
| Node | `emit` | 3.4 | 6.5 |
| Node | `serial` | 2.8 | 4.2 |
| Node | `bail` | 2.8 | 3.9 |
| Python | `emit` | 2.1 | 2.7 |
| Python | `serial` | 1.8 | 2.3 |
| Python | `bail` | 2.5 | 3.0 |

第一次：Node `emit` 3.8 / 15.0，`serial` 2.8 / 5.7，`bail` 2.8 / 5.8；Python `emit` 2.1 / 2.3，`serial` 1.8 / 2.1，`bail` 2.5 / 2.7。Node 的 p99 在两次之间差得多，受机器上别的任务影响。

dsh 基线，单位：毫秒，5 次的中位数：

| 项目 | 中位数 | 5 次 |
| --- | ---: | --- |
| `dsh_baseline` 测试跑完一遍（原生 Cordis + 经 rutis 挂载） | 1133 | 1235、1144、1133、1118、1124 |
| 其中原生 Cordis 一半（`node native.mjs`） | 176 | 341、176、227、169、118 |

第一次 `dsh_baseline` 的中位数是 1194 毫秒（第一轮 4045 毫秒，含冷启动）。删后的上限按 1133 毫秒算：1.1 倍是 1246 毫秒。

## 删后（经宿主）

代码：main `b502303d`，临时关掉本地分发，让每次发出都经过宿主（`release` 构建的 `rutis-host`），用同一条命令连跑两次，两次接近。单位：微秒。

| 运行时 | 方式 | 删前中位数 | 经宿主中位数 | 经宿主 p99 |
| --- | --- | ---: | ---: | ---: |
| Node | `emit` | 3.4 | 71 | 125 |
| Node | `serial` | 2.8 | 131 | 245 |
| Node | `bail` | 2.8 | 85 | 178 |
| Python | `emit` | 2.1 | 139 | 240 |
| Python | `serial` | 1.8 | 307 | 600 |
| Python | `bail` | 2.5 | 105 | 195 |

判断：Node 的 `emit` 在原定的 100 微秒以内；Python 的 `emit` 超出原定门槛，用户看过这组数字后决定照删，门槛改为上表的实测值。经宿主的路径以后可优化，优化不恢复本地分发。

删本地分发只去掉本地的判断，经宿主的路径不变，所以删后的数字按这组记。dsh 基线由删本地分发的 PR 用同一条命令补上。
