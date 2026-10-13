# CI 重新设计（2026-10-13）

[English](design-ci-redesign-2026-10-13.en.md) · [CI](ci.md) · [质量规范](quality-standard.md) §12

状态：已确认（2026-10-13）。第七节的 5 个问题都按建议决定。

## 一、问题

现在的触发方式（`main` `ea487b2`）：

| 工作流 | PR | 合并到 main | 发布 tag | 定时 |
| --- | --- | --- | --- | --- |
| `ci.yml`（24 个任务） | 只跑改动相关的任务 | **全部任务** | — | — |
| `install.yml`（打包 + 5 个平台安装运行） | 改了打包相关路径时 | **每次** | 由 `release.yml` 调用 | — |
| `dylib-windows.yml`（GitHub 的 Windows，25–40 分钟） | 改了 dylib 相关路径时 | **每次** | **每次** | — |
| `release.yml` | — | — | 打包、发布门、发布、发布后验证 | — |
| `stress.yml`、`deps.yml`、`ci-stats.yml` | — | — | — | 每晚 / 每周 |

问题：

1. **PR 已经跑过的 CI，合并到 main 后又全部重跑一遍**，还要加上安装检查和 Windows dylib 检查。一次合并在 main 上要排三十多个任务，这些任务还和别的 PR 抢同一批机器。连续合并几个 PR 时，main 上的运行一直在排队（#348 只能做到"运行中的不取消、等待中的被替换"）。
2. **还有任务在排 GitHub 的机器**：`dylib-windows`（`windows-2025`）、`runtimes-go` / `runtimes-bun` 的 macOS 矩阵项（`macos-15`）、`install.yml` 的打包和大部分安装格子。用了自托管 runner 之后，`ci.yml` 里最长的任务只要 3–9 分钟（`runtimes-windows`），排队时间比运行时间长。
3. **发布流程的问题**（0.10.0 发布时暴露的）：
   - 推 `v*` tag 又触发一遍 `dylib-windows`，而 tag 指向的提交在 main 上已经跑过；
   - `go` 任务不等发布门，就打了 `go/rutis/vX.Y.Z` tag；
   - 发布后验证只等主包能下载，不等 npm 平台包；
   - 发布前要人工确认"main 上的 CI 和 install.yml 通过了"（release.md 第 2 步）。

## 二、当初为什么让 main 每次合并都跑

两个原因，都还成立：

- **PR 只跑改动相关的任务**（`changes` 按路径选择）。路径表漏了某个依赖时，PR 上该跑的任务没跑。main 上跑全部任务，可以兜住这种遗漏（Q12.3.3）。
- **PR 测的是它合并那一刻和 main 的组合**。两个 PR 各自通过，合在一起才出问题的情况，只有合并之后才能测出来。

这两个漏洞不需要每次合并都补一遍：每天补一次，再在发布前补一次，就足够了。

## 三、新的触发方式

| 场合 | 跑什么 | 在哪里跑 |
| --- | --- | --- |
| PR | 和现在一样：`ci.yml` 只跑改动相关的任务；改了打包相关路径时跑 `install.yml`；改了 dylib 相关路径时跑 `dylib-windows` | 自托管 |
| 合并到 main | **不跑** | — |
| 每晚（新增 `nightly.yml`） | main 上有新提交时：`ci.yml` 全部任务、`install.yml`（本次构建，5 个平台）、`dylib-windows`；原有的 `stress.yml` 并进来 | 自托管为主 |
| 发布 tag | `release.yml`：完整 CI + 打包 + 发布门 → 发布 → 发布后验证 | 见第五节 |
| 手动 | 任何分支可以手动跑一次全部任务（`workflow_dispatch`，`all=true`） | 自托管 |

细节：

- `ci.yml`、`install.yml`、`dylib-windows.yml` 去掉 `push: main`。`ci.yml` 增加 `workflow_call` 输入 `all`，供 `nightly.yml` 和 `release.yml` 调用；`changes` 任务在 `all=true` 时选择全部任务，与现在 push 到 main 时相同。
- **每晚只在有新提交时跑**：比较 main 当前的提交和上一次成功的每晚运行；没有新提交就跳过。
- **每晚失败时自动开问题单**，同一个工作流已经有未关闭的问题单时，就在那个问题单里追加评论（做法同 `release.yml` 的 `report`）。问题单写明失败的任务、提交范围（上一次成功的提交到这次的提交）。
- PR 的并发规则不变（新推送取消旧运行）。没有了 main 上的运行，`concurrency` 里关于 main 的特殊规则（#348）可以删掉。

## 四、机器

| 任务 | 现在 | 改为 |
| --- | --- | --- |
| `dylib-windows` | GitHub `windows-2025` | 自托管 Windows（`WINDOWS_RUNNER`），和 `runtimes-windows` 用同样的编译目录和并行数规则 |
| `runtimes-go` / `runtimes-bun` 的 macOS 项 | GitHub `macos-15` | 自托管 macOS（`MACOS_RUNNER`） |
| `install.yml` 的 `darwin-arm64` | 自托管 | 不变 |
| `install.yml` 的其他安装格子 | GitHub（`ubuntu-24.04`、`ubuntu-24.04-arm`、`macos-15-intel`、`windows-2025`） | **不变**：安装检查要在干净的机器上跑，自托管机器装过很多东西，测不出"缺了什么"；Intel Mac 和 Linux arm64 也没有自托管的机器 |
| `package.yml` 的打包 | GitHub（Linux 要 Docker 容器做 musl 交叉编译） | 不变：Linux 自托管 runner 在容器里，没有 Docker |

`install.yml` 和打包以后只在每晚、发布、改了打包相关路径的 PR 上跑，用 GitHub 的机器也排不了多少队。

## 五、发布流程

`release.yml` 改为：

1. **核对**：版本号与 tag 一致（不变）。
2. **完整 CI**：调用 `ci.yml`（`all=true`），在自托管机器上跑，和打包并行。这样就不用再人工确认"main 上的 CI 通过了"。release.md 第 2 步改为"合并 release PR"。
3. **打包 + 发布门**（不变）：构建五个平台的文件，在五个平台上安装运行。发布的就是检查过的这份文件，所以不复用 main 或每晚构建的文件。
4. **发布**：crates、npm、PyPI，以及 **`go` 任务**。这些都等第 2、3 步全部通过；`go` 不再提前打 tag。
5. **GitHub Release**（不变）。
6. **发布后验证**：先等所有包都能从注册表取到，**包括五个 npm 平台包**，再从注册表安装运行。
7. `dylib-windows.yml` 去掉 `tags: ["v*"]`：完整 CI 已经在第 2 步跑了 Windows 的运行时测试；dylib 的 Windows 检查由每晚覆盖。是否把 `dylib-windows` 也放进发布门，见第七节问题 2。

## 六、对质量规范的修改

| 条款 | 现在 | 改为 |
| --- | --- | --- |
| Q12.3 | PR 按改动选择任务，main 上跑全部 | PR 按改动选择任务；**每天至少一次**、以及**每次发布前**跑全部任务 |
| Q12.4 | 每次合并都有完整结果；被取代的运行取消 | 每晚的运行检查上一次以来合并的所有提交；失败时开问题单，写明提交范围；PR 上新推送取消旧运行 |
| Q12.6 | 关键路径的要求针对 PR 和 main | 只针对 PR 和发布 |

`docs/ci.md` 第 3 节（触发）、第 7 节（失败处理）、第 10–11 节（自托管 runner）随之修改。`docs/release.md` 第 2–4 步随之修改。

## 七、维护者的决定（2026-10-13，都按建议）

1. 合并到 main 不再跑任何检查，只靠每晚和发布前补漏，可以吗？
2. `dylib-windows` 要不要放进发布门？放进去发布要多等 25–40 分钟（自托管 Windows 上可能更快）；不放的话，它只靠每晚运行覆盖。建议放进去，因为 dylib 插件的 Windows 兼容只有它在测。
3. 每晚几点跑？建议北京时间凌晨 3 点（UTC 19:00），和现在的 `stress.yml` 错开。
4. 每晚失败开的问题单指派给谁？建议指派给维护者，并加 `ci` 标签。
5. `install.yml` 除 `darwin-arm64` 外仍用 GitHub 的干净机器，可以吗？

## 八、实现步骤

| PR | 内容 | 验证 |
| --- | --- | --- |
| 1 | `ci.yml` 加 `workflow_call`（`all`）；新增 `nightly.yml`（有新提交才跑、失败开问题单）；三个工作流去掉 `push: main`；`stress.yml` 并入；`ci.md`、质量规范、`quality-status.md` 同步 | 手动触发 `nightly.yml` 一次：全部任务跑完，人为让一个任务失败，确认会开问题单 |
| 2 | `dylib-windows` 和 macOS 矩阵项改用自托管 runner | 在 PR 上把 `dylib-windows` 完整跑一遍（runner 以 `NETWORK SERVICE` 运行，文件锁、Job Object、导出符号检查可能有差异） |
| 3 | `release.yml`：完整 CI 进发布门，`go` 等发布门，发布后验证等 npm 平台包；`dylib-windows` 去掉 tag 触发；`release.md` 同步 | 用 `workflow_dispatch` 在一个测试 tag 上跑到"发布"之前的步骤（发布步骤加一个 `dry-run` 输入） |
