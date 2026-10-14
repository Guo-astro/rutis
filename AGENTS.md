# AGENTS

给 coding agent 的任务指引，放在 `.agents/skills/<名字>/`。每个 skill 自包含（`SKILL.md` + `references/`），可以整个目录拷走单独使用。

| 任务 | 指引 |
| --- | --- |
| 让插件跑在别的机器、设备或应用上（远程运行时、节点 link、应用托管） | [.agents/skills/rutis-remote-plugin/SKILL.md](.agents/skills/rutis-remote-plugin/SKILL.md) |
| 为 rutis 添加一种插件语言 / 运行时，或让已有运行时支持远程 | [.agents/skills/rutis-new-runtime/SKILL.md](.agents/skills/rutis-new-runtime/SKILL.md) |

设计原则见 [docs/design-philosophy.md](docs/design-philosophy.md) §七；文档中英同步（`.md` 与 `.en.md`）。
