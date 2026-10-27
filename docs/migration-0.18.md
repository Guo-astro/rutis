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
