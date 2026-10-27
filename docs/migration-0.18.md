# 迁移到 0.18

[English](migration-0.18.en.md)

0.18 让出错的时候看得懂（[诊断设计](design-diagnostics-2026-10-26.md)）。本页随 0.18 的改动逐项补齐；下面是已经完成的部分。

## `rutis-host check`：依赖环与同名提供改为报错（#676）

0.17.2 里这两种情况只写告警，退出码不变；0.18 起写 `rutis-host: error: …`，以 1 退出（`--strict` 与否都一样）。在 CI 里跑 `check` 的配置，以前通过的可能会失败：

- **几行的 `inject` 互相等对方提供的服务（一个环）**：这几行谁也起不来。

  ```
  rutis-host: error: rows a, b wait for each other (a waits for bsvc from b, b waits for asvc from a): none of them starts; break the cycle in their inject
  ```

  改法：从其中一行的 `inject` 里去掉它其实不需要的服务；两边都需要时，把共同的部分拆成第三行，让它们都等这一行。

- **几行在同一个作用域里声明提供同一个服务**：只有先起来的那行提供，其余的失败。

  ```
  rutis-host: error: rows one, two all provide llm: only the first to start provides it, the others fail; keep one, or isolate the name in each row
  ```

  改法：只留一行；两份都要时，用 `isolate` 把这个名字隔离开：`{ "llm": "标签" }` 让提供它的行和用它的行带同一个标签，各用各的一份（`{ "llm": true }` 是这一行自己用的一份，别的行用不到）。

升级前用 0.17.2 跑一遍 `rutis-host check`：0.17.2 对这两种情况写的告警，就是 0.18 会报的错。

## 同一监听器上两个条目接受同一个 token：启动即报错（#702）

同一个监听器上，监听的节点行和连进来的运行时（`runtimes.remote` 的 `listen` + `instances`）会接受同一个 token 时（例如都没有单独的 token、都退回 `RUTIS_TOKEN`），0.17 照常启动，但连接到达时以 403 拒绝（分不出是给谁的），运行时每 30 秒重试。0.18 起 `run`、`check`、重新读配置在启动前报错。同一个监听器上任意两个条目都一样：两个连进来的运行时（0.17 同样以 403 拒绝）、两个监听的节点行（0.17 里连接都到了后一行，前一行永远连不上）。

```
rutis-host: listener public: link row main and remote runtime phone both accept RUTIS_TOKEN, so a connection presenting it cannot be told apart: set RUTIS_TOKEN_MAIN or RUTIS_TOKEN_PHONE to a token of its own
```

改法：按提示给其中一个设单独的 `RUTIS_TOKEN_<名字>`（连进来的运行时那一边也出示这个新 token）。这样的配置在 0.17 里本来就连不上，改好之后才能用。
