# 迁移到 0.18

[English](migration-0.18.en.md)

0.18 让出错的时候看得懂（[诊断设计](design-diagnostics-2026-10-26.md)）。本页随 0.18 的改动逐项补齐；下面是已经完成的部分。

## 同一监听器上两个条目接受同一个 token：启动即报错（#702）

同一个监听器上，监听的节点行和连进来的运行时（`runtimes.remote` 的 `listen` + `instances`）会接受同一个 token 时（例如都没有单独的 token、都退回 `RUTIS_TOKEN`），0.17 照常启动，但连接到达时以 403 拒绝（分不出是给谁的），运行时每 30 秒重试。0.18 起 `run`、`check`、重新读配置在启动前报错：

```
rutis-host: listener public: link row main and remote runtime phone both accept RUTIS_TOKEN, so a connection presenting it is refused as ambiguous: set RUTIS_TOKEN_MAIN or RUTIS_TOKEN_PHONE to a token of its own
```

改法：按提示给其中一个设单独的 `RUTIS_TOKEN_<名字>`（连进来的运行时那一边也出示这个新 token）。这样的配置在 0.17 里本来就连不上，改好之后才能用。
