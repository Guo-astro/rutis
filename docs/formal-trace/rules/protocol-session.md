# 规则：协议清单 session（#634 评论）

[中文索引](../README.md) · [English](../README.en.md) · 来源：[评论 6384933233](https://github.com/arcships/rutis/issues/634#issuecomment-6384933233) · [评论 6384933561](https://github.com/arcships/rutis/issues/634#issuecomment-6384933561) · [保证登记表](../../guarantees.md)

本文件把 #634 的 `session` 分区的上下两条评论（[评论 6384933233](https://github.com/arcships/rutis/issues/634#issuecomment-6384933233) · [评论 6384933561](https://github.com/arcships/rutis/issues/634#issuecomment-6384933561)）里的 Markdown 表格导出到仓库。表格内容原样保留；单元格里的 `|` 按 Markdown 表格规则转义为 `\|`，渲染后文字不变。列的含义见 #634 正文：编号、来源、规则、参与方、手段、对应、现有形式化、状态、归属 issue、备注。

抽取基线：`main` `3496669b`（issue 正文的统计口径）。表格保持 issue 原文；这 350 行已按 0.22 现行来源复核，并在追踪表的语义重抽里逐行重抽来源、锚点、证据与状态（见 `README.md` §1 与[复核表](../recheck-0.22.md) §5）。状态列仍是 issue 的词汇，`node tools/formal-trace.mjs` 在统计时按 `README.md` §3 的映射换成追踪表状态；映射与复核都只降不升，不改写本文件。

分区合计：现行 246 条，已取代 104 条，共 350 行；两条评论合起来与 issue 正文的统计表一致。

## 分区 `session`（上）

> 分区 `session`：会话协议、值、调用链与通道（SES、VAL、TAG、M1、CHN）
>
> 现行 246 条，已取代 104 条（列出、不计入）。"归属 issue"一列带 ※ 的是这次新分配或调整的。列的含义见 #634 正文。

| 编号 | 来源 | 规则 | 参与方 | 手段 | 对应 | 现有形式化 | 状态 | 归属 issue | 备注 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| SES-1-1 | SES §一 | 真实对象留在原进程，对端只得到它的引用（代理） | 跨进程 | K | — | `session::testing::session`（hold / fire / drop：对端保存的函数回调到原进程） | 部分 | #639 ※ | 只有函数引用；对象引用（`Kind::Object`，Rust 只作中继）没有一致性用例 |
| SES-1-2 | SES §一 | 参数、返回值、事件载荷里都可以有引用 | 跨进程 | K + PT | — | `session::testing::session`（apply：参数里的回调）；`*_releases_balance`（make：返回值里的函数）；`events.expected.json` `values/function-argument` | 部分 | #639 ※ | Future、对象引用在返回值与事件里无形式化用例 |
| SES-1-3 | SES §一 | Rust runner 运行 rutis 插件；两种 runner 都多插件共进程、每插件独立上下文与装载代 | 跨进程 | — | — | — | 已取代 | — | Rust 插件在宿主进程里，没有 Rust runner（多语言运行时设计 #121）；Node 的行共用一个 Cordis Context（`loader/runtime.rs:13-18`） |
| SES-1-4 | SES §一、§二 | 不承诺现有插件零修改跨进程：同步远程访问、反射、未声明的接口须显式适配 | 跨进程 | — | — | — | 已取代 | — | 被 VAL V0（开发者无感）、M1 §十四（未声明的服务自动投到 rutis）取代；同步调用已支持（`sync-wait`） |
| SES-1-5 | SES §一 | 基础版事件只带发布者自有或接收者拥有的对象，不支持借事件转交第三方对象 | 跨进程 | — | — | — | 已取代 | — | 事件由统一事件协议规定（SES §十一 自述已取代） |
| SES-2-1 | SES §二 表 | 跨进程方法调用一律异步（Promise / Future 明示等待） | 跨进程 | — | — | — | 已取代 | — | 调用帧带 `sync`，调用方可同步等待（`protocol.rs:148-160`；`rpc.rs:1646-1665`）；boundary/5.6 |
| SES-2-2 | SES §二 表 | 返回活对象时传保留身份的对象引用，不序列化整个对象 | 跨进程 | K | — | — | 未覆盖 | #639 ※ | 按语言的值表：Node 带方法的对象、类实例成为对象引用（`session.mjs:8-20`）；Python dataclass 按 dict 传（weather `python/dataclass`） |
| SES-2-3 | SES §二 表、§7.3 | 属性读取只用不变快照或显式异步 getter，不悄悄同步阻塞 | 跨进程 | — | — | — | 已取代 | — | `get` 帧是活读取，可带 `sync`（`protocol.rs:170-178`） |
| SES-2-4 | SES §二 表、§7.1 | 对象相等：同一导入范围与权限视图内稳定，跨视图用协议身份比较 | 跨进程 | — | — | — | 已取代 | — | 没有权限视图；身份按（会话，引用号）与中继复用，现行见 M1-7.1-3 |
| SES-2-5 | SES §二 表 | 函数参数按引用传，不发送源码或闭包捕获的内容 | 跨进程 | K | — | `session::testing::session`（apply：回调在原进程执行，7 种实现） | 已覆盖 | — |  |
| SES-2-6 | SES §二 表 | 卸载时由框架撤销远程对象、回调、订阅与调用 | 跨进程 | — | — | — | 已取代 | — | 会话层只在会话结束时整表清空（`rpc.rs:1347-1385`）；行卸载由多语言运行时设计与 loader/row-unload-leaves-nothing 规定 |
| SES-2-7 | SES §二 末段、§7.3 | 代理不远程执行字段赋值、枚举 getter、`instanceof` 或原型链访问 | 跨进程 | K | boundary/5.7 | `session::testing::malformed`（未知 op 结束会话） | 部分 | #639 ※ | 帧只有 call / get / await，没有写入；`instanceof` 不成立只由 `boundary_rules.rs` 普通测试 |
| SES-2-8 | SES §二 | 不可变对象关系声明且物化后才能直接导航（`actor.agent.session`） | 跨进程 | — | — | — | 已取代 | — | 没有接口描述符与物化；`get` 是活读取 |
| SES-4-1 | SES §四 | 一个托管插件 = 主进程代理 fiber + runner 顶层 fiber；内部子插件不建全局代理 | 跨进程 | — | — | — | 已取代 | — | 现为 loader 的行（JsRow）+ 运行时里的行（M1 §五） |
| SES-4-2 | SES §四 | 内部服务失效撤销导出；必要导出失效则托管根退出全局可用 | 跨进程 | — | — | — | 已取代 | — | 槽位换值与撤下按 M1-5.3-2、M1-5.3-3 |
| SES-4-3 | SES §四 | runner 注入代绑定执行门控，只有宿主发起 start 才能打开 | 跨进程 | — | — | — | 已取代 | — | `rows.load` / `rows.unload`，无 activation |
| SES-4-4 | SES §四 | 本地刷新不在旧 activation 中再次 apply；新装载须宿主签发新 activation | 跨进程 | — | — | — | 已取代 | — | 按代次装载与迟到的 `rows.ended`，见 #625 |
| SES-4-5 | SES §四 | 本地框架发现依赖失效可立即停止并上报，不等远端许可 | 跨进程 | — | — | — | 已取代 | — | `rows.ended`，见 #625 |
| SES-5-1 | SES §五 | 语言、版本、依赖与信任兼容的插件可共享 runner；每插件独立上下文与代 | 跨进程 | — | — | — | 已取代 | — | 一种语言一个运行时，隔离按需多开（多语言运行时设计） |
| SES-5-2 | SES §五 | TS runner 代码 / 依赖变更重启整组，不删模块缓存假装卸载 | 外部 | — | — | — | 已取代 | — | 头注；reload 语义见 loader/reload-all-or-nothing |
| SES-5-3 | SES §五 | 缺失 factory 在准备期拒绝 | 纯逻辑 | — | — | — | 已取代 | — | 没有 Rust runner 与工厂表 |
| SES-5-4 | SES §五、§9.1 | prepare 只读包与配置，不执行插件来探测依赖；build 不执行插件代码 | 外部 | — | — | — | 已取代 | — | `rows.schema` 由运行时加载模块读 `inject` 导出（M1 §5.1） |
| SES-5-5 | SES §五 | 路径限包内相对路径，拒绝越界符号链接；执行用 argv；包版本目录不可变 | 外部 | — | — | — | 已取代 | — | 行可指 npm 包、子路径或 `file://` 绝对路径（`loader/runtime.rs:14-18`） |
| SES-5-6 | SES §五 | requires 静态化，缺依赖 Pending，完整图查环，不许动态引用增加必需注入 | 进程内并发 | — | — | — | 已取代 | — | 门控现行见 M1-5.2-1、loader/rows-gate-on-services |
| SES-6-1 | SES §六 | 接口描述符是统一语义来源，各语言消费同一份打包字节；对象引用不是 `$ref`，回调不是可序列化函数 | 纯逻辑 | — | — | — | 已取代 | — | 无接口描述符；形状来自 `rows.schema` 的 `provides` 或推断（M1 §十四） |
| SES-6-2 | SES §六 | 合法对象图可以有环；数据不得递归展开成无限 JSON | 纯逻辑 | PT | — | — | 未覆盖 | #631 ※ | Node 对带环的数据报错 `cyclic data requires an object binding`（`session.mjs:101`）；Rust `Value` 不能成环。各语言无随机检查 |
| SES-6-3 | SES §六 | 值 schema 只用受限子集；整数超出 JS 安全范围用声明的十进制字符串 | 纯逻辑 | — | — | — | 已取代 | — | 被 SES §6.1 `bigint`（V6）取代 |
| SES-6-4 | SES §六 | 接口身份 = 精确版本 + bundle SHA-256，准入时校验可达接口集合 | 纯逻辑 | — | — | — | 已取代 | — | 无接口 bundle |
| SES-6.1-1 | SES §6.1 值；VAL V3 | 会话里的值是 `{"type": 标记, "value": 内容}`；已有 `undefined`（无 value）、`data`、`list`、`record`、`signal`、`reference` | 纯逻辑 | FZ + PT | bridge/value-roundtrip | `session_frame`；`*_values_round_trip`（7 种） | 部分 | #631 | fuzz 只查 Rust 的解析与重写稳定；其他实现接受的语法只经合法值往返 |
| SES-6.1-2 | SES §6.1 值；VAL §三 | 十种新标记与 `reference` 同级；数组、对象里含它们时按 `list` / `record` 编码 | 纯逻辑 | PT | bridge/value-tags | `*_tagged_values_round_trip`（tagged_value 把新值混进 list / record） | 部分 | #631 ※ | 没有 Python 列 |
| SES-6.1-3 | SES §6.1 值、§十二；VAL V3、§十一 | 普通 JSON 字段永远不被解释为类型 | 纯逻辑 | PT | bridge/value-tags | `*_values_round_trip`（随机对象的键含 `type`、`value`） | 部分 | #631 | 很少生成恰好形如标记的对象；`objects.rs::data_shaped_like_a_marker_stays_data` 是单元测试；#631 的"serde 标记"项覆盖 Rust serde 一层 |
| SES-6.1-4 | SES §6.1 表；VAL §三 | `instant`：RFC 3339，`YYYY-MM-DDTHH:MM:SS`，可选 1–9 位小数，大写 `T` 与 `Z`，只用 UTC，年 0001–9999，秒 00–59 | 纯逻辑 | PT + FZ | bridge/value-tags | `session_frame`（解码后写回再解码一致）；`*_tagged_values_round_trip`（全年份范围的随机时间点） | 部分 | #631 ※ | 没有检查"接受的文本恰好是规格的语法"（如与正则做差分）；拒绝一侧只有单元测试 |
| SES-6.1-5 | SES §6.1 表；VAL V5、§三、§七.2 | `datetime`：同 `instant` 去掉 `Z`；接收方原样保留，不按时区解释，不与 `instant` 互转 | 纯逻辑 | PT + FZ | bridge/value-tags | `session_frame`；`*_tagged_values_round_trip` | 部分 | #631 ※ | 同 SES-6.1-4；Python 收发只有 weather `python/value-naive-datetime` 一个样本 |
| SES-6.1-6 | SES §6.1 表；VAL §三 | `date`：`YYYY-MM-DD`，年 0001–9999，必须是存在的日子 | 纯逻辑 | PT + FZ | bridge/value-tags | `session_frame`；`*_tagged_values_round_trip`（每一天） | 部分 | #631 ※ | 同 SES-6.1-4 |
| SES-6.1-7 | SES §6.1 表；VAL §三 | `time`：`HH:MM:SS`，24 小时制 00–23，可选 1–9 位小数，无时区 | 纯逻辑 | PT + FZ | bridge/value-tags | `session_frame`；`*_tagged_values_round_trip` | 部分 | #631 ※ | 同 SES-6.1-4 |
| SES-6.1-8 | SES §6.1 表；VAL §三 | `bytes`：标准字母表 base64，带 `=` 填充，无空白，未用的低位必须为 0 | 纯逻辑 | FZ + PT | bridge/value-tags | `session_frame`（接受的文本必须原样写回，即只收规范形式）；`*_tagged_values_round_trip` | 部分 | #631 ※ | Rust 一侧完整；其他实现的接收只经合法值往返，非规范文本是否拒绝没有检查 |
| SES-6.1-9 | SES §6.1 表；VAL §三 | `map`：`[[键, 值], …]`，每对两元素数组，键值都带标记；保持发送方的迭代顺序 | 纯逻辑 | PT + K | bridge/value-tags | `*_tagged_values_round_trip`（按顺序比较）；`weather.expected.json`（`node/value-map`、`python/value-int-keys` 等） | 部分 | #631 ※ | 没有 Python 的 PT 列 |
| SES-6.1-10 | SES §6.1 表；VAL §三 | `set`：`[元素, …]`，保持发送方的迭代顺序 | 纯逻辑 | PT + K | bridge/value-tags | `*_tagged_values_round_trip`；`events.expected.json` `values/set` | 部分 | #631 ※ | 同上 |
| SES-6.1-11 | SES §6.1 表；VAL §三 | `bigint`：十进制，可带 `-`，无前导零，不写 `-0`，至多 4300 位 | 纯逻辑 | PT + FZ | bridge/value-tags | `session_frame`（只查往返稳定）；`*_tagged_values_round_trip`（含 4300 位） | 部分 | #631 ※ | fuzz 抓不到"接受了 `007`"一类错误（BigInt 存原文，写回仍稳定） |
| SES-6.1-12 | SES §6.1 表；VAL §三 | `decimal`：`-?(0\|[1-9][0-9]*)(\.[0-9]+)?`，无指数，保留末尾零，整数与小数合计至多 4300 位，无 NaN / 无穷 | 纯逻辑 | PT + FZ | bridge/value-tags | `session_frame`；`*_tagged_values_round_trip` | 部分 | #631 ※ | 同 SES-6.1-11 |
| SES-6.1-13 | SES §6.1 表；VAL §三 | `uuid`：小写十六进制 8-4-4-4-12 | 纯逻辑 | PT + FZ | bridge/value-tags | `session_frame`；`*_tagged_values_round_trip` | 部分 | #631 ※ | 同 SES-6.1-11（大写的被拒只有单元测试） |
| SES-6.1-14 | SES §6.1 键；VAL V8、§五 | Map 键、Set 元素只能是字符串、有限数字、布尔、`bigint`、`datetime`、`date`、`time`、`decimal`、`uuid`；不能是 `instant`、`bytes`、`null`、`undefined`、数组、对象、`map`、`set`、引用、`signal` | 纯逻辑 | PT + K | bridge/value-tags | `*_tagged_values_round_trip`（只生成允许的种类）；`weather.expected.json`（`node/value-date-key`、`python/value-tuple-key`、`swift/value-date-key` 被拒） | 部分 | #631 ※ | 不允许的种类没有随机生成，只有个别样本 |
| SES-6.1-15 | SES §6.1 键、精度与边界 | 超出 ±(2^53−1) 的整数作键是 `bigint` 键，与同值 `bigint` 相等；超出范围的整数值浮点数不能作键 | 纯逻辑 | PT + K | bridge/value-tags | `*_tagged_values_round_trip`（键含超界整数）；weather `node/value-unsafe-key`、`go/big-int-keys`、`kotlin/big-long-keys` | 部分 | #631 ※ | 整数值浮点数作键被拒只有单元测试 |
| SES-6.1-16 | SES §6.1 键；VAL V8、§五 | 同一 map 的两键、同一 set 的两元素不得相等：数字按数值（`1` 与 `1.0`、`0` 与 `-0` 相等），其余按种类与文本 | 纯逻辑 | PT + K | bridge/value-tags | weather `node/value-duplicate-elements`（被拒） | 部分 | #631 ※ | PT 生成器用 `distinct` 滤掉重复，从不生成相等的键 |
| SES-6.1-17 | SES §6.1 键；VAL §五 | 发送方在发出前检查键（种类与相等） | 纯逻辑 | PT + K | bridge/value-tags | weather 的拒绝样本（Node、Python、Swift 作发送方） | 部分 | #631 ※ | Rust 发送方只有单元测试（`map_keys_and_set_elements_are_checked_before_sending`） |
| SES-6.1-18 | SES §6.1 键；VAL V8、§五、§七.5 | 接收方转成本语言类型后键相等（如 Python 的 `1` 与 `True`）时这次调用失败 | 纯逻辑 | PT | bridge/value-tags | `node_tagged_values_round_trip`、`bun_tagged_values_round_trip`（期望 JS 分不开的键被拒） | 部分 | #631 ※ | Python 没有 PT 列 |
| SES-6.1-19 | SES §6.1 键 | `map` 里值为 `undefined` 的条目，发送方写成 `null` | 纯逻辑 | PT + K | bridge/value-tags | weather `node/value-map`（undefined 为 null） | 部分 | #631 ※ | PT 生成器不产生 `Undefined` 值 |
| SES-6.1-20 | SES §6.1 深度 | `map`、`set` 各算一层，计入 64 层上限 | 纯逻辑 | PT | bridge/large-and-deep-values | — | 未覆盖 | #631 ※ | `*_depth_limit` 是固定用例，且只用 list 与 data |
| SES-6.1-21 | SES §6.1 深度 | Rust 读取的帧深度上限 3 × 64 + 16 = 208 层（`MAX_FRAME_DEPTH`） | 纯逻辑 | FZ | bridge/large-and-deep-values | `session_frame`（先 `deeper_than` 再解析，208 层内解析不溢出栈） | 部分 | #631 ※ | 没有检查"64 层内的值写成帧不超过 208 层" |
| SES-6.1-22 | SES §6.1 能力；VAL V4、§四 | 每种标记一个能力 `value-<tag>`；声明表示接收这种标记；实现可以只声明一部分 | 纯逻辑 | PT | bridge/value-tags | `rust_tagged_values_refused_without_capabilities` | 部分 | #631 ※ | 只 Rust 作发送方、对端什么都不声明；部分声明的组合未查 |
| SES-6.1-23 | SES §6.1 能力；VAL §四 | endpoint 格式（协议 3）与 compat 格式（协议 2）都在 `hello.capabilities` 声明；compat 下 Rust 只认 `COMPAT_DECLARED`；不升协议版本 | 纯逻辑 | PT | bridge/cross-version-meetings | — | 未覆盖 | #631 | #631 的"握手判定表"含能力；代码 `rpc.rs:746-780`、`2055-2065` |
| SES-6.1-24 | SES §6.1 发送；VAL V4、§四、§七.6 | 只向声明了能力的对端发该标记；未声明时这次调用失败（参数：不发出；返回值：对端收到 `throw`），错误说明哪种值、缺哪个能力，会话继续 | 纯逻辑 | PT | bridge/value-tags、bridge/grant-rollback | `rust_tagged_values_refused_without_capabilities`（参数情形，查错误文字与会话未关） | 部分 | #631 ※ | 返回值情形、其他实现作发送方没有形式化检查 |
| SES-6.1-25 | SES §6.1 发送；VAL §四 | 宿主转发（直接或经 relay）按下游对端的能力再检查：下游不能收时被转发的这次调用失败 | 跨进程 | PT + K | bridge/value-tags | — | 未覆盖 | #631 ※ | `tests/tagged_values.rs` 是普通测试；转发在目标会话上编码，所以检查的是下游（`rpc/relay.rs:126-160`） |
| SES-6.1-26 | SES §6.1 接收 | 收到本侧未声明的标记、`value` 不合格式、键不合规则或相等时，只让所在调用失败：对端的调用以 `throw` 回复，对本侧调用的回复让本侧调用失败；会话继续 | 跨进程 | PT + FZ | bridge/value-tags | — | 未覆盖 | #631 | #631 的帧接收有状态 fuzz 只计划"不 panic、不挂"，不检查"只失败这一次调用"；代码 `rpc.rs:2186-2210`、`2233-2244` |
| SES-6.1-27 | SES §6.1 接收 | `value` 的 JSON 形状不对（`instant` 的 value 不是字符串、`map` 条目不是两元素数组）是格式错误的帧，结束会话 | 纯逻辑 | FZ + K | — | `session_frame`（只查不 panic）；`session::testing::malformed`（只有 `{"type":"bogus"}`） | 部分 | #631 ※ | 形状错误的用例不在 MALFORMED |
| SES-6.1-28 | SES §6.1 精度；VAL V7、§六末、§七.3 | 接收方时钟比 `instant` 粗时（JS 毫秒、Python 微秒、Swift 约微秒）向负无穷截断，不报错；Rust、Go、Kotlin 保留纳秒 | 纯逻辑 | PT | bridge/value-tags | `node_tagged_values_round_trip`、`bun_tagged_values_round_trip`（期望截到毫秒，含 1970 年前）；`go_`、`kotlin_`、`rust_tagged_values_round_trip`（纳秒不变） | 部分 | #631 ※ | Python、Swift 的截断没有 PT |
| SES-6.1-29 | SES §6.1 精度；VAL V6、§七.4 | 超出 ±(2^53−1) 的整数发送方自动按 `bigint` 发出，含它的数组、对象按 `list` / `record`；对端未声明 `value-bigint` 时调用失败 | 纯逻辑 | PT + K | bridge/value-tags、bridge/value-roundtrip | `*_values_round_trip`（Rust 发出；对未声明的对端发出前失败）；weather `python/big-int`、`go/big-int-in-struct`、`kotlin/big-long`、`swift/big-int` | 部分 | #631 ※ | 其他语言作发送方只有固定样本 |
| SES-6.1-30 | SES §6.1 精度 | 浮点数不受影响，照旧是 `data` | 纯逻辑 | PT | bridge/value-roundtrip | `*_values_round_trip`（浮点逐位相同，含 `9007199254740993.0`） | 已覆盖 | — |  |
| SES-6.1-31 | SES §6.1 精度 | 接收方把 `bigint` 转成本语言整数（放得下时）或大整数类型；旧实现发来的 `data` 里超范围整数照旧按数字读 | 纯逻辑 | PT | bridge/value-tags | `*_tagged_values_round_trip` | 部分 | #631 ※ | "旧实现的 data 照旧按数字读"无形式化检查 |
| SES-6.1-32 | SES §6.1 精度末；VAL 范围外、§七.7 | 节点之间转发的事件（`NodeEvent`）不承载这些值，含它们的事件在转发时拒绝（0.12） | 跨进程 | — | — | — | 已取代 | — | 同一句话写明 0.13 起节点之间的事件承载全部值（事件协议第十节）；VAL §七.7 与范围外一段同样过时 |
| SES-7.1-1 | SES §7.1 | 对象身份 = owner runtime epoch + owner activation + object id | 跨进程 | — | — | — | 已取代 | — | 引用是会话导出表的号加 `home`、`kind`、`origin`（`protocol.rs:41-46`） |
| SES-7.1-2 | SES §7.1 | 仍存活的同一对象重复导出时身份一致（导出表的稳定身份映射） | 进程内并发 | PT | bridge/release-count | `*_releases_balance`（Bounce 再次授予，只数表的大小） | 部分 | #631 | 代码 `rpc.rs:1854-1868`（`identities`）；PT 不断言号相同 |
| SES-7.1-3 | SES §7.1 | 对象 id 在本会话内不复用 | 纯逻辑 | PT + TLA | bridge/reference-release-count | — | 未覆盖 | #626 | 代码单调分配（`rpc.rs:2654-2662`）；#626 的 `PinnedTarget` 含"旧的 Release 不拿走新的授予" |
| SES-7.1-4 | SES §7.1 | 跨视图提供显式 `sameObject`；同一导入 scope 与权限视图复用代理 | 跨进程 | — | — | — | 已取代 | — | 无权限视图 |
| SES-7.1-5 | SES §7.1、§10.1 | 回传 owner 时定位原对象，但供业务调用的引用仍是受门控的 facade | 跨进程 | — | — | — | 已取代 | — | `home: true` 解码为原对象本身（`rpc.rs:1975-1993`）；现行见 M1-7.1-4 |
| SES-7.1-6 | SES §7.1 | 业务 `close()` 与代理 release 分开，释放代理不调用任何业务方法 | 跨进程 | K | — | — | 未覆盖 | #639 ※ | `release` 只减导出表的计数（`rpc.rs:2268-2289`） |
| SES-7.2-1 | SES §7.2 | broker 为每次交付登记 grant，绑定接收者、对象、接口视图与唯一 delivery id | 跨进程 | — | — | — | 已取代 | — | 无 broker；按次数计的授予（`rpc.rs:2664-2677`） |
| SES-7.2-2 | SES §7.2 | Object id 不是凭证；每次调用校验调用者、grant、owner 代、接口、方法、作用域 | 跨进程 | — | — | — | 已取代 | — | 只按号查导出表（`rpc.rs:2129-2138`）；"能力不授予权限"（`rpc.rs:987`） |
| SES-7.2-3 | SES §7.2 | 同组跨托管插件调用也经 broker | 跨进程 | — | — | — | 已取代 | — | 同一运行时的 Cordis 行互用原生对象（M1 §十四） |
| SES-7.2-4 | SES §7.2 | 基础版不支持第三方转交，delegate 返回 UnsupportedCapability | 跨进程 | — | — | — | 已取代 | — | 经中继的第三方转交已实现（M1 §7.1） |
| SES-7.2-5 | SES §7.2 | 引用失效时对它的操作返回错误，不把对象关系变成依赖门控 | 跨进程 | K | — | `session::testing::session`（drop 之后 fire 失败） | 部分 | #639 ※ | 会话结束后的旧引用（`Import::connection` 报 `session closed`）无一致性用例 |
| SES-7.2-6 | SES §7.2 | 命名服务 provider 失效走依赖驱逐；回调对象失效不驱逐服务图 | 进程内并发 | — | — | — | 已取代 | — | 依赖门控现行见 loader/rows-gate-on-services |
| SES-7.3-1 | SES §7.3 | 接口声明不可变快照、对象关系与异步 get/set；不默认遍历 getter 或递归序列化任意对象 | 纯逻辑 | — | — | — | 已取代 | — | 无接口声明；Python dataclass 按 dict 序列化（weather `python/dataclass`） |
| SES-7.3-2 | SES §7.3 | 直接属性写入、未声明成员、`__proto__` 等反射入口拒绝 | 跨进程 | — | — | — | 已取代 | — | 方法取对象的公开方法（M1 §十四）；`get` 可读任意属性名，中继不检查（`rpc/relay.rs:118-122`） |
| SES-7.3-3 | SES §7.3 | 同步属性不靠阻塞 Node 事件循环或 Rust executor 模拟远程同步 getter | 跨进程 | — | — | — | 已取代 | — | 同步等待即阻塞调用方线程并泵帧（`rpc.rs:1646-1665`） |
| SES-8-1 | SES §八 | 不实现分布式垃圾收集：跨会话的引用环不被回收，直到会话结束（明确不保证） | 跨进程 | 测试 | — | — | 不适用 | — | 明确不保证，没有可检查的性质；兜底是 SES-8.2-4 |
| SES-8-2 | SES §八 | 每个引用、pin、回调、订阅、调用必须属于 activation / 子资源 / 一次调用的 scope | 跨进程 | — | — | — | 已取代 | — | 引用属于会话，会话结束整表清空 |
| SES-8-3 | SES §八 表、§十 | borrow 参数 / 回调只在调用期间有效，保存后越界调用明确失败 | 跨进程 | — | — | — | 已取代 | — | 引用按次数计，释放前一直有效（`session::testing::session` 的 hold / fire 证明了这一点） |
| SES-8-4 | SES §八 表 | scope 返回对象由接收 scope 拥有，显式 release 或 scope 卸载回收；独占 disposer | 跨进程 | — | — | — | 已取代 | — | `release { reference, count }` |
| SES-8-5 | SES §八 表 | owner 失活立即拒绝新操作，撤销派生 grant，在飞任务取消并被跟踪 | 跨进程 | — | — | — | 已取代 | — | 现行见 CHN-2-9（会话结束时在途调用以传输错误失败） |
| SES-8-6 | SES §八 表 | 最后一次授予被释放后，仍在执行的调用用到的对象保持存活，执行完才释放 | 跨进程 | TLA | bridge/reference-release-count | — | 未覆盖 | #626 | 已收下的调用持有 `Arc<Object>`（`rpc.rs:2356-2372`）；#626 `PinnedTarget` |
| SES-8.1-1 | SES §8.1 | 每次交付独立编号（delivery id），一个消息里重复的同一项只算一次，同编号重传幂等 | 跨进程 | — | — | — | 已取代 | — | 每出现一次授予一次、按次数释放（`rpc.rs:2664-2677`、`797-814`）；`*_releases_balance` 的 Grant 一次传 1–3 份 |
| SES-8.1-2 | SES §8.1 | 接收时附加 token、复用 Active 包装，物化后 `accept(d)`，失败则 `release(d)` | 跨进程 | — | — | — | 已取代 | — | 没有 accept 帧 |
| SES-8.1-3 | SES §8.1 | 代理 release 释放截至此刻已收到的 token，不通配未来交付；release 后新交付进新包装代 | 跨进程 | — | — | — | 已取代 | — | 性质保留为"旧的 release 不扣新授予"，用计数实现（Import 记收到的次数，释放时写 count）；登记为 bridge/reference-release-count |
| SES-8.1-4 | SES §8.1 | 不加每变量 / 每 Arc 持有者计数；Rust Drop / TS finalizer 只辅助，scope 卸载才是兜底 | 跨进程 | — | — | — | 已取代 | — | Rust 以 `Import` 的 Drop 写 release 为主路径（`rpc.rs:797-814`） |
| SES-8.2-1 | SES §8.2 | 交付状态 Offered / Accepted / Released 及重复 accept / release 幂等 | 跨进程 | — | — | — | 已取代 | — | 无交付状态；计数为 0 或超额的 release 结束会话（CHN-4-6） |
| SES-8.2-2 | SES §8.2 | 全 scope 卸载先关交付准入，再回收全部 token；在线不响应则显示未确认记录 | 跨进程 | — | — | — | 已取代 | — | 会话关闭见 SES-8.2-4 |
| SES-8.2-3 | SES §8.2 | 解码失败、迟到的回复被丢弃时，其中的引用照样登记并随之释放 | 跨进程 | TLA + PT | bridge/value-tags | — | 未覆盖 | #626 | 代码 `rpc.rs:1900-1906`、`2233-2240`；#626 `BoundedTables`（静止时表为空） |
| SES-8.2-4 | SES §8.2、MNT 引用计数 | 会话结束时两端的导出表与导入表整表清空，不等 GC | 进程内并发 | SH | bridge/no-call-on-a-dead-session | — | 未覆盖 | #628 | `rpc.rs:1366-1367`；#628"close() × …静止时表为空"；#628 疑点：`close()` 之后 `respond()` 仍会加入导出 |
| SES-8.2-5 | SES §8.2 | 终态记录回收基于确认序号 / epoch 关闭，不按时间猜；未知旧 token 不当作新交付 | 跨进程 | — | — | — | 已取代 | — | 无 token；未知引用结束会话（CHN-4-6） |
| SES-8.2-6 | SES §8.2 | 取消用户等待不立即释放仍在执行的任务要用的引用；先撤入口，确认完成后才释放 | 跨进程 | TLA | bridge/reference-release-count | — | 未覆盖 | #626 | 正在执行的同步调用持着参数到结束；"确认完成"（`call/finished`）已不存在 |
| SES-8.2-7 | SES §8.2 | owner 卸载不等外部 release 才关闭闸门；未确认则 StopUnconfirmed | 跨进程 | — | — | — | 已取代 | — | 无此状态 |
| SES-9.1-1 | SES §9.1 | supervisor 独立启动 runtime 并完成 hello，不等成员 apply | 跨进程 | — | — | — | 已取代 | — | 现行见 M1-5.4-1 |
| SES-9.1-2 | SES §9.1 | activation 发布屏障：Active 前不开放对外调用与事件，未就绪返回可辨识错误 | 跨进程 | — | — | — | 已取代 | — | 行的服务在装载后投出，见 M1-14-9、#625 `ServicesBeforeRunning` |
| SES-9.1-3 | SES §9.1 | 不要求整组同时 Ready，组内依赖不造成全组屏障死锁 | 跨进程 | — | — | — | 已取代 | — | 现行见 M1-5.4-1、loader/runtimes-do-not-wait-for-each-other |
| SES-9.1-4 | SES §9.1 | 同名导出、缺导出、接口不匹配或部分启动失败回滚 activation | 跨进程 | — | — | — | 已取代 | — | 同名现行见 M1-14-4 |
| SES-9.1-5 | SES §9.1 | 旧 Arc / JS 引用绑定旧代，不因同名 provider 更换而偷偷重绑 | 跨进程 | — | — | — | 已取代 | — | 宿主服务的新租约换掉表里的 dispatch，运行时里的同一代理随之调到新提供者（`runtime/process.rs:1281-1289`） |
| SES-9.2-1 | SES §9.2 | 失活先关闭闸门再发 stop；stop 完成意味着清理收敛；Closing 仍收完成控制消息 | 跨进程 | — | — | — | 已取代 | — | `rows.unload` |
| SES-9.2-2 | SES §9.2 | 本地框架主动失败先撤销再上报，不在 runner 里悄悄换代并复用旧对象 id | 跨进程 | — | — | — | 已取代 | — | `rows.ended`，#625 |
| SES-9.2-3 | SES §9.2 | 超过截止未清理完默认 StopUnconfirmed、禁止自动重试 | 外部 | — | — | — | 已取代 | — | 运行时到截止就放弃清理并退出（`host_gone.rs`，`RUTIS_SHUTDOWN_TIMEOUT`） |
| SES-9.2-4 | SES §9.2 | 普通 RPC 超时不触发整组强杀 | 跨进程 | — | — | — | 已取代 | — | 协议不设调用超时（boundary/5.8；MNT"取消与超时"） |
| SES-9.3-1 | SES §9.3 | 组崩溃 / 断线先撤销 RuntimeReady 与全组 activation，取消调用，驱逐消费者 | 跨进程 | — | — | — | 已取代 | — | loader/runtime-down-stops-its-rows |
| SES-9.3-2 | SES §9.3 | 旧进程确认回收后才启动新 epoch；不恢复旧对象或请求；RecoveryBlocked / Quarantined | 跨进程 | — | — | — | 已取代 | — | 不自动重启（CHN-7-8、CHN-11-10） |
| SES-10-1 | SES §十 | 调用标识含发起端 epoch、activation、call id；不以另一连接恰好相同的 id 匹配 | 跨进程 | — | — | — | 已取代 | — | 调用号 = 前缀 + 会话内计数（`rust:` / `node:` / `<端点>:`，`rpc.rs:1536-1540`）；跨会话靠标签（TAG） |
| SES-10-2 | SES §十、§10.1 | 不保证执行完成顺序；需要先后时调用方显式 await（明确不保证） | 跨进程 | 测试 | — | — | 不适用 | — | 明确不保证 |
| SES-10-3 | SES §十、§10.1 | 同一对象的调用按 broker 分配的序号进入 handler（入站交付顺序） | 跨进程 | — | — | — | 已取代 | — | 只有 `events.*` 与 `rows.ended` 按帧序（`rpc.rs:1060-1065`、`2380-2396`）；其余调用各自 spawn，开始顺序不保证（`rpc.rs:2444-2451`） |
| SES-10-4 | SES §十 | 读帧（接收泵）与业务 handler 分开：读线程不运行插件代码，A→B→A 回调不被单线程分发锁死 | 进程内并发 | TLA + SH | boundary/5.6、bridge/sync-wait-cycle-refused | `cross-runtime-sync` 不死锁、`RoutingSound`；`session::testing::session`（reenter） | 部分 | #626、#628 | 模型不含读线程的写回（Throw、Release）与通道背压（#626"不卡死"）；读线程不跑插件代码见 `rpc.rs:2385-2386` |
| SES-10-5 | SES §十 | 同步 API 要么本地适配为异步，要么明确 unsupported，不偷偷阻塞 executor | 跨进程 | — | — | — | 已取代 | — | 同步调用阻塞调用方并泵帧；成环时 `SyncWaitCycle` |
| SES-10-6 | SES §十 | 基础版业务回调只支持 borrow；回调不传原始 Ctx，执行保持创建它的 activation；`this` 由适配绑定 | 跨进程 | — | — | — | 已取代 | — | 引用计数；调用方的行见 bridge/caller-row |
| SES-10-7 | SES §十 | deadline 覆盖排队、写入、执行与流读取，过线传剩余时长；派生调用传播剩余 deadline | 跨进程 | — | — | — | 已取代 | — | 协议没有截止时间（boundary/5.8；MNT"协议不另设默认超时"） |
| SES-10-8 | SES §十 | 调用已发出后放弃等待则发 `cancel` | 跨进程 | TLA + K | bridge/cancel-reaches-callee | `session::testing::session`（丢弃异步调用使对端的 signal 中止，7 种实现） | 部分 | #626 | 只一跳、异步；"未发送时移出队列"一半不适用（发送在调用里同步完成）；#626 `CancelReaches` |
| SES-10-9 | SES §十 | 本地等待只结算一次 | 进程内并发 | TLA + SH | bridge/in-flight-call-fails-once、bridge/replies-match-calls | — | 未覆盖 | #626、#628 | #626 `SettlesOnce`；#628"进来调用的认领恰好一次" |
| SES-10-10 | SES §十 | 取消后仍跟踪执行，handler 结束后 `call/finished` 才确认完成 | 跨进程 | — | — | — | 已取代 | — | 没有 finished 帧；取消之后被调方是否回复没有成文规则（#626） |
| SES-10-11 | SES §十 | 迟到的完成与取消幂等，不恢复业务等待 | 跨进程 | — | — | — | 已取代 | — | 现行见 CHN-4-9、CHN-4-10 |
| SES-10-12 | SES §十 | 转发的调用随来路调用的取消逐跳取消 | 跨进程 | TLA | bridge/multihop-cancel-and-release | — | 未覆盖 | #626 | 中继的异步转发在进来调用被取消时丢弃（`rpc/relay.rs:140-167`）；#626 `CancelReaches`（两跳） |
| SES-10-13 | SES §十 | 不自动重试有副作用的调用；断线 / 超时的结果可为执行情况未知 | 跨进程 | TLA | bridge/in-flight-call-fails-once | — | 未覆盖 | #626 | `RemoteLease` 的 `OldCommandsStayOld` 只说旧会话的指令不作用于新租约，不是"不重发" |
| SES-10-14 | SES §十 | 服务端流基础版在接口准入时拒绝 | 跨进程 | — | — | — | 已取代 | — | 协议没有流 |
| SES-10.1-1 | SES §10.1、§十二 | 无 promise pipelining、直连或路径缩短；跨 runner 调用经宿主中转 | 跨进程 | — | — | — | 已取代 | — | 现行见 M1-7.1-1（中继） |
| SES-11-1 | SES §十一 | 跨界事件声明参数、作用域、分发模式、结果与错误规则 | 跨进程 | — | — | — | 已取代 | — | §十一 自述被统一事件协议取代 |
| SES-11-2 | SES §十一 | broker 维护已导出事件的唯一监听列表与顺序，不两端各广播 | 跨进程 | — | — | — | 已取代 | — | 同上 |
| SES-11-3 | SES §十一 表 | 通知 emit 不独立跨界，迁为 parallel | 跨进程 | — | — | — | 已取代 | — | 同上 |
| SES-11-4 | SES §十一 表 | parallel 取一次快照并发调用、等待全部、汇总错误；serial 按权威顺序、显式 Continue / Return | 跨进程 | — | — | — | 已取代 | — | 同上 |
| SES-11-5 | SES §十一 表 | waterfall 基础版拒绝；同步 bail / waterfall 跨界拒绝 | 跨进程 | — | — | — | 已取代 | — | 同上 |
| SES-11-6 | SES §十一 | 监听属于创建它的 activation；注销先关闭回调准入；once 执行前原子摘除 | 跨进程 | — | — | — | 已取代 | — | 同上 |
| SES-11-7 | SES §十一 | 派发用快照，执行前再查闸门 | 跨进程 | — | — | — | 已取代 | — | 同上 |
| SES-11-8 | SES §十一 | 事件 scope 由宿主授权，不接受远端自报 scope；不支持的过滤器导出期拒绝 | 跨进程 | — | — | — | 已取代 | — | 同上 |
| SES-11-9 | SES §十一 | 订阅有 ready 确认，需要不漏首事件时等它 | 跨进程 | — | — | — | 已取代 | — | 同上 |
| SES-11-10 | SES §十一 | 事件携带同一 session 对象时接收 scope 得到稳定代理 | 跨进程 | — | — | — | 已取代 | — | 同上 |
| SES-12-1 | SES §十二 | 每组一条继承的 Unix stream socket，u32 网络字节序长度 + UTF-8 JSON | 外部 | — | — | — | 已取代 | — | 按换行分帧的紧凑 JSON（CHN-3-2）与 WebSocket 文本消息（CHN-10-8） |
| SES-12-2 | SES §十二 | stdout / stderr 不承载协议 | 外部 | — | — | — | 已取代 | — | 同一规则的现行表述见 CHN-11-7 |
| SES-12-3 | SES §十二 | 解析前检查长度、深度与结构，不按未验证的长度分配 | 纯逻辑 | FZ | bridge/large-and-deep-values | `local_lines`（超限即结束）；`session_frame`（先查深度再解析） | 部分 | #631 | "不越读"（读到上限加一个缓冲就停）只有单元测试 `endless_bytes_without_a_newline_stop_at_the_limit` |
| SES-12-4 | SES §十二 表 | 消息族 runtime/hello…、object/export、grant、accept、release、revoke、object/call…finished、event/…、stream/… | 跨进程 | — | — | — | 已取代 | — | 帧为 hello、invoke、call、get、await、return、throw、release、cancel（`protocol.rs:136-205`）；事件为 `events.*` 控制调用 |
| SES-12-5 | SES §十二 | 载荷用显式 tagged value，不把形似 id 的普通 JSON 字段解释为能力 | 纯逻辑 | — | — | — | 已取代 | — | 同一规则的现行表述见 SES-6.1-3 |
| SES-12-6 | SES §十二 | 正式协议必须冻结编码、引用表、版本、双向路由与错误 schema | 外部 | — | — | — | 已取代 | — | 流程要求，头注 |
| SES-12-7 | SES §十二 | 原始对象 id / 令牌不进业务日志 | 外部 | — | — | — | 已取代 | — | 现行见 CHN-12-1（trace 不记内容）、CHN-10-5（token 不进日志） |
| SES-12-8 | SES §十二 | 不接受任意成员路径作为调用表达式（`method` / `property` 是单个名字） | 纯逻辑 | K | — | — | 未覆盖 | #639 ※ | 需要各实现对含 `.` 的方法名、属性名的一致性用例 |
| SES-12-9 | SES §十二 | 握手只表示可以接收装载，不表示插件就绪；不二次 hello | 跨进程 | — | — | — | 已取代 | — | 现行见 CHN-6-7（就绪）与 CHN-4-8（重复 hello） |
| SES-12-10 | SES §十二 | 生命周期消息以宿主授权为准，普通成员不能发组管理命令 | 跨进程 | — | — | — | 已取代 | — | 无组管理命令 |
| SES-12-11 | SES §十二 | 正常业务错误只影响它所属的调用，会话继续 | 跨进程 | K | bridge/error-code-and-data | `session::testing::session`（`fail` 抛出带名字与消息的错误，之后的检查在同一会话上继续，7 种实现） | 已覆盖 | — |  |
| SES-12-12 | SES §十二 | 损坏帧、半帧写失败、无法归属的控制消息终止连接；正常迟到的释放 / 完成不杀整组 | 跨进程 | — | — | — | 已取代 | — | 现行见 CHN §畸形帧（CHN-4-*） |
| SES-12-13 | SES §十二 | 错误区分 InvalidParams、InterfaceMismatch、CapabilityDenied、StaleObject、ScopeClosed、Cancelled、DeadlineExceeded、Unavailable 与业务错误；带阶段与 `execution` | 纯逻辑 | — | — | — | 已取代 | — | `throw` 只带 `name`、`message`、可选 `graph`（`protocol.rs:87-93`）；Rust 的 `Error` 为 Transport / Remote / SyncWaitCycle / Value / Handshake（`mod.rs:39-55`） |
| SES-12-14 | SES §十二 | 业务错误不回显任意堆栈或秘密 | 外部 | — | — | — | 已取代 | — | 作为值返回的 Error 带 `stack` 传（weather `node/error-value`；`session.mjs:262`） |
| SES-13-1 | SES §十三 | 协议授权不是 OS 沙箱；同组代码共享地址空间，必须互信；默认同用户，不保证秘密隔离（明确不保证） | 外部 | 测试 | — | — | 不适用 | — | 明确不保证；代码注明能力不授予权限（`rpc.rs:987`） |
| SES-13-2 | SES §十三 | 线上不暴露内存指针、Rust TypeId、任意 Ctx、JS 原型或方法反射 | 纯逻辑 | 测试 | — | — | 不适用 | — | 代码审查项：引用号是计数器（`rpc.rs:2654-2662`），没有交错性质 |
| SES-13-3 | SES §十三 | 单插件停止只处理它的 scope；supervisor 回收旧进程及后代才开新 epoch；诊断展示 StopUnconfirmed 等 | 外部 | — | — | — | 已取代 | — | loader/runtime-down-stops-its-rows；不自动重启 |
| SES-14-1 | SES §十四 | 旧 rutis-cordis、dsh host 保留原握手；新对象协议用独立 family / 实验版本，不把旧 liveRef 当可调用对象 | 跨进程 | — | — | — | 已取代 | — | crate 已合并进 rutis-bridge；compat 格式即协议 2 |
| SES-14-2 | SES §十四 | 能力子集的语言缺对象 / 回调 / 事件能力时拒绝相应接口，不默默转成 JSON | 跨进程 | — | — | — | 已取代 | — | 现行为按能力声明（远程插件设计 §6.2，他区）；本区相关的缺口见 M1-13-3 |
| VAL-1-V0 | VAL §一 V0、§二 末段 | 凭空创建本语言没有的类型的值（JS 只有日期等）用 SDK 构造函数；普通字符串照旧是字符串 | 纯逻辑 | K | bridge/value-tags | `weather.expected.json`（`node/value-constructed`、`go/value-constructed`、`swift/value-constructed`、`kotlin/value-constructed`） | 已覆盖 | — |  |
| VAL-1-V2 | VAL §一 V2、§七.1 | 默认转换不丢信息；只有确实会丢信息时这次调用失败，错误说明原因 | 纯逻辑 | K + PT | bridge/value-tags | weather `node/value-typed-array`（元素类型会丢，被拒）、`*/nan` | 部分 | #639 ※ | "会丢信息"的判定逐语言写在 §二、§六；没有随机检查 |
| VAL-1-V9 | VAL §一 V9、§六 Go、§八 | Go 的结构体、带类型的切片与 map、顶层 `time.Time` 经同一编码器逐个检查，不走 `encoding/json` | 纯逻辑 | K + PT | bridge/value-tags | weather `go/value-struct-bytes`、`go/value-struct-time`、`go/big-int-in-struct` | 部分 | #639 ※ | 只有固定样本；#630 的 Go 原生 fuzz 没有指向编码器 |
| VAL-1-V10 | VAL §一 V10、§七.8、§八 | 数据里的 `undefined` 在所有语言统一为 `null`（Python dict 不再删键） | 纯逻辑 | K | bridge/value-tags | weather `node/undefined-in-data`、`python/undefined-in-data` | 已覆盖 | — | 只有 JS 与 Python 有 undefined |
| VAL-1-V11 | VAL §一 V11、§八 | Rust `Value` 新变体标 `#[non_exhaustive]` | 纯逻辑 | 测试 | — | — | 不适用 | — | 编译期（`rpc.rs:41`） |
| VAL-1-V12 | VAL §一 V12、§九 | 测试工具与真实运行时对同一份期望文件比对，新值全部纳入 | 跨进程 | K | sdk/testing-matches-runtime | `weather.expected.json`（66 个样本，0 个 pending） | 部分 | #624 | 与运行时共用代码的工具可能同错；#624 更新 sdk/testing-matches-runtime |
| VAL-2-1 | VAL §二 表、§二 JS | JS（Node、Bun）收到：`instant`→`Date`、`bytes`→`Uint8Array`、`map`→`Map`、`set`→`Set`、`bigint`→`bigint`；`datetime`、`date`、`time`、`decimal`、`uuid`（小写）→普通字符串 | 纯逻辑 | PT + K | bridge/value-tags | `node_tagged_values_round_trip`、`bun_tagged_values_round_trip`（按 JS 的对应计算期望）；`events.expected.json` `values/through-javascript` | 已覆盖 | — |  |
| VAL-2-2 | VAL §二 表 | Python 收到：`instant`→带 UTC 的 `datetime`、`datetime`→无时区 `datetime`、`date`、`time`、`bytes`、`map`→`dict`（子类）、`set`、`bigint`→`int` 子类、`Decimal`、`UUID` | 纯逻辑 | PT + K | bridge/value-tags | `events.expected.json` `values/every-tag`、`values/set`（Python 作监听器回显） | 部分 | #631 ※ | 没有 Python 的带标记值 PT 列 |
| VAL-2-3 | VAL §二 表、§二 Go | Go 收到：`time.Time`（UTC）、`rutis.DateTime`/`Date`/`TimeOfDay`、`[]byte`、`map[K]V`、set 按目标类型（`any` 时为去重后的切片）、`*big.Int`、Decimal/UUID 为命名字符串 | 纯逻辑 | PT + K | bridge/value-tags | `go_tagged_values_round_trip`；`events.expected.json` `values/set` 的 except（Go 回显为列表） | 部分 | #631 ※ | PT 只看往返结果，不看 Go 一侧的类型 |
| VAL-2-4 | VAL §二 表 | Rust 收到 `Instant`、`DateTime`、`Date`、`TimeOfDay`、`Vec<u8>`、`Vec<(Value, Value)>`、`Vec<Value>`、`BigInt`、`Decimal`、`Uuid` | 纯逻辑 | PT | bridge/value-tags | `rust_tagged_values_round_trip` | 已覆盖 | — |  |
| VAL-2-5 | VAL §二 表、§六 | Kotlin 收到 `Instant`、`LocalDateTime`、`LocalDate`、`LocalTime`、`ByteArray`、`Map`、`Set`、`BigInteger`、`BigDecimal`、`UUID` | 纯逻辑 | PT + K | bridge/value-tags | `kotlin_tagged_values_round_trip` | 部分 | #631 ※ | 只看往返不变 |
| VAL-2-6 | VAL §二 表、§二 Swift | Swift 收到 `Date`、`Rutis.DateTime`/`CalendarDate`/`TimeOfDay`、`Data`、`Dictionary`、`Set`、`Rutis.BigInt`、`Decimal`、`UUID`；Decimal 超出 38 位有效数字时这次调用失败 | 纯逻辑 | PT + K | bridge/value-tags | `swift_tagged_values_round_trip` | 部分 | #631 ※ | 38 位的失败没有用例 |
| VAL-2-7 | VAL §二 JS | 经 JS 原样转发时 `datetime`、`date`、`time`、`decimal`、`uuid` 的类型不保留（明确不保证） | 跨进程 | 测试 | — | — | 不适用 | — | 明确不保证；`events.expected.json` `values/every-tag` 的 except 记录了这一差异 |
| VAL-2-8 | VAL §二 Python | Python 收到的 `bigint` 是 `int` 子类，原样发回仍是 `bigint`（JS 的 `5n` 经 Python 回 JS 仍是 `5n`）；Go 到 `any` 为 `*big.Int` 同理 | 跨进程 | K | bridge/value-tags | `events.expected.json` `values/every-tag`（Python、Go 回显超界 bigint） | 部分 | #639 ※ | ±(2^53−1) 以内的 bigint（如 `5n`）经 Python / Go 转回没有用例 |
| VAL-2-9 | VAL §二 Map | JS 字符串键的 `Map` 到 Python 是 `dict` 子类，原样转发仍是 Map；Python 普通字符串键 `dict` 照旧按对象传 | 跨进程 | K | bridge/value-tags | — | 未覆盖 | #639 ※ |  |
| VAL-6-1 | VAL §六 JS | JS 发送：`Date`→`instant`（无效日期报错）；`Uint8Array`、`ArrayBuffer`、`DataView`、`Buffer`→`bytes`；`Map`→`map`；`Set`→`set`；`bigint`→`bigint`；构造函数→`datetime` / `date` / `time` / `decimal` / `uuid` | 纯逻辑 | K + PT | bridge/value-tags | weather `node/value-instant`、`value-bytes`、`value-map`、`value-set`、`value-bigint`、`value-constructed`、`value-invalid-date` | 部分 | #639 ※ | `ArrayBuffer`、`DataView`、`Buffer` 无样本；Bun 无单独样本 |
| VAL-6-2 | VAL §六 Python | Python 发送：带时区 `datetime`→`instant`；无时区→`datetime`；`date`；`time`（带时区的拒绝）；`bytes`/`bytearray`/`memoryview`；`set`/`frozenset`；非字符串键 dict 与收到的 Map→`map`；超界 int 与收到的大整数→`bigint`；`Decimal`；`UUID`；三者不再按对象引用传 | 纯逻辑 | K | bridge/value-tags | weather `python/value-*`、`python/big-int*`、`python/set`、`python/bytes` | 部分 | #639 ※ | "收到的 Map / 大整数原样发回"无样本 |
| VAL-6-3 | VAL §六 Go | Go 发送：`time.Time`→`instant`；`rutis.DateTime`/`Date`/`TimeOfDay`；`[]byte`→`bytes`；非字符串键 map→`map`；`*big.Int`/`big.Int`→`bigint`；`rutis.Decimal`/`rutis.UUID` | 纯逻辑 | K | bridge/value-tags | weather `go/value-*`、`go/big-int-*` | 部分 | #639 ※ | `big.Int`（非指针）无样本 |
| VAL-6-4 | VAL §六 Rust | Rust：`Value::json()` 遇到新值报错（同引用） | 纯逻辑 | — | — | — | 已取代 | — | V6 修订后 64 位内的 `BigInt` 由 `json()` 还原成数字（`rpc.rs:127-131`），其余新值才报错 |
| VAL-6-5 | VAL §六 Rust | Cordis 生成器把 `Uint8Array`、`Date`、`bigint`、`Map`、`Set` 映射到 Rust 类型（按 `rutis_bridge::session` 的路径识别，插件同名类型不算）；Map 键值、Set 元素里的 NaN、无穷报错 | 纯逻辑 | PT | — | — | 未覆盖 | #631 ※ | 代码生成只有单元测试 |
| VAL-6-6 | VAL §六 Kotlin、Swift | Kotlin、Swift 的值层用 SDK 自己的编码器逐个检查，不依赖 `JSONEncoder` / kotlinx 的默认编码 | 纯逻辑 | K | bridge/value-tags | weather `kotlin/value-data-class`、`swift/value-struct` | 部分 | #639 ※ | 只有固定样本 |
| VAL-7-4 | VAL §七.4 | JS 自己的 `number` 超出 ±(2^53−1) 时照旧不精确（JS 本身的限制，明确不保证） | 纯逻辑 | 测试 | bridge/value-limits | — | 不适用 | — | 明确不保证；`value_roundtrip.rs` 的比较按 Doubles 放宽 |
| TAG-2-1 | TAG §二（接收方的校验） | 调用帧的 `path` 含本帧 `id` 时判为非法帧并结束整个会话（Rust、Node、Python、Bun、Go 同） | 跨进程 | K + FZ | — | — | 未覆盖 | #639 ※ | `session::testing::malformed` 没有这一用例；代码 `rpc.rs:2352-2354` |
| TAG-2-2 | TAG §二（origin 的格式） | origin 条目：compat 为 `(node\|rust\|s[0-9]+/[a-z0-9-]+):[1-9][0-9]*`，端点格式为 `(s[0-9]+/)?[a-z0-9-]+:[1-9][0-9]*`；嵌套标签 `s1/s2/node:1` 被拒 | 纯逻辑 | PT + K | — | — | 未覆盖 | #631 ※ | 只有各运行时的单元测试；Rust 收到引用时不查 origin 的格式（`rpc.rs:1994-2027`），规则只在运行时一侧 |
| TAG-3-R1 | TAG §一 C2、§三 R1 | 调用链的一个条目在链经过的所有进程里唯一地指出（会话，调用号）：跨任意多个进程、往返任意次都不歧义 | 跨进程 | PT + TLA | bridge/call-chains-across-processes | `cross-runtime-sync` `RoutingSound`（单进程，标签视为互不相同） | 部分 | #631 | #631"经 k 次改写后能还原、单射"；多进程模型没有 issue 计划 |
| TAG-3-R2 | TAG §三 R2；M1 §7.2 | 条目回到所属会话时还原为原生调用号，且只有标签所属的进程还原它 | 纯逻辑 | PT | bridge/call-chains-across-processes | `cross-runtime-sync`（`RebasePath` 抽象同样的规则） | 部分 | #631 | 模型检查的是设计，不检查 `rebase` 的实现 |
| TAG-3-R3 | TAG §三 R3 | 等待环检测每个宿主只为本进程的会话建图；别的进程的条目不能被解成本进程的会话 | 跨进程 | PT + TLA | bridge/call-chains-across-processes、bridge/sync-wait-cycle-refused | — | 未覆盖 | #631 | `waits.rs:135` `decode`；#631"外来的不会变成本地的" |
| TAG-4-1 | TAG §四、§五 | 同一进程的标签共用一个随机数、以计数器区分，同进程内确定地唯一 | 纯逻辑 | PT | bridge/call-chains-across-processes | — | 未覆盖 | #631 | `rpc.rs:703-724`；`rpc/tests.rs::sessions_have_distinct_tags…` 是单元测试 |

## 分区 `session`（下）

| 编号 | 来源 | 规则 | 参与方 | 手段 | 对应 | 现有形式化 | 状态 | 归属 issue | 备注 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| TAG-4-2 | TAG §四、§五 | 不同进程的标签只在 15 位随机数相等时（约 10⁻¹⁵）可能相同（明确不保证：唯一性是概率性的） | 外部 | 测试 | bridge/call-chains-across-processes | — | 不适用 | — | 随机性；`session_tags.rs` 两进程测试 |
| TAG-4-3 | TAG §四 | 取不到系统随机数时退回 `RandomState` 随机键、进程号与时间的哈希，仍随进程不同 | 外部 | 测试 | — | — | 不适用 | #641 ※ | 操作系统随机源；没有测试也没有 issue |
| TAG-4-4 | TAG §一 C4、§四 | 旧版本的计数器标签与新标签（16 位以上数字）不会相等；只有两个旧进程之间仍有 #325 | 纯逻辑 | PT | bridge/call-chains-across-processes | — | 未覆盖 | #631 | "旧标签最多十几位"是对计数器大小的假设，见问题 7 |
| TAG-5-1 | TAG §一 C3、C4、§五 | 不需要协议版本变化：线格式、帧字段、握手、能力不变，标签只锁定满足 `s[0-9]+`；四种运行时的 origin 校验照常接受新标签 | 跨进程 | K + PT | bridge/call-chains-across-processes | — | 未覆盖 | #639 ※ | 一致性套件里没有带标签 origin 的用例；只有 `session_tags.rs` 与 e2e `migration.rs` |
| TAG-5-2a | TAG §五 行为表 | 另一个进程的调用不因被当成本进程的调用而结束会话 | 跨进程 | PT + TLA | bridge/call-chains-across-processes | — | 未覆盖 | #631 |  |
| TAG-5-2b | TAG §五 行为表、§二 #325 | 另一个进程的调用不被投到本进程正在同步等待的错误线程上 | 跨进程 | PT + TLA | bridge/call-chains-across-processes | `cross-runtime-sync` `RoutingSound`（单进程） | 部分 | #631 |  |
| TAG-5-2c | TAG §五 行为表 | 另一个进程的调用不进入本进程的等待图 | 跨进程 | PT + TLA | bridge/call-chains-across-processes | — | 未覆盖 | #631 |  |
| TAG-5-3 | TAG §五 | 跨进程的等待环不在检测范围内，每个宿主只看自己的表（明确不保证） | 跨进程 | 测试 | bridge/sync-wait-cycle-refused | — | 不适用 | #614 | 明确不保证；#614 计划写进多语言设计 §九 与登记表，现在登记表没写 |
| M1-4-1 | M1 §二、§四 | 按名字跨语言共享的服务只有一种表示 `host_key(name)`（`dyn HostDispatch`）；提供者是 Rust 还是某个运行时的行都一样 | 进程内并发 | K | loader/services-by-name | — | 未覆盖 | #639 ※ |  |
| M1-4-2 | M1 §四、§5.4 | 运行时的行调用 `host:<name>` 时按名字查当前提供者，就地调用或转发到另一个会话；查不到报"没有这个宿主服务" | 跨进程 | K | loader/services-by-name | — | 未覆盖 | #639 ※ |  |
| M1-4.1-1 | M1 §4.1、§十三 | `HostDispatch::methods` 给出 `{method: sync\|async}`；返回 None 时由运行时的 `host` 声明补上 | 纯逻辑 | K | — | — | 未覆盖 | #639 ※ | 代码的优先级相反：有 `host` 声明时用它，`methods()` 才是后备（`runtime/process.rs:1300-1303`、`loader/runtime.rs:838`），见问题 8 |
| M1-4.1-2 | M1 §4.1、§八 | `CordisRuntimePlugin::host(name, methods)` 只登记形状，不再让运行时依赖这个服务 | 进程内并发 | K | loader/runtimes-do-not-wait-for-each-other | — | 未覆盖 | #639 ※ |  |
| M1-4.1-3 | M1 §4.1 | 行服务的 `HostDispatch` 总是带形状 | 纯逻辑 | 测试 | — | — | 不适用 | — | 构造即保证（`runtime/rows.rs:79-81`） |
| M1-4.2-1 | M1 §二、§4.2、§八 | `register_shared` 登记过的名字才是 rutis 认识的共享服务名，只有它们由 rutis 门控；JS 行要用 rutis 服务须登记并写进插件的 `inject` | 进程内并发 | — | — | — | 已取代 | — | catalog 默认按名字共享一切，`share_registered_only` 时才只认登记的（`loader/runtime.rs:43-48`）；M1 §十四 |
| M1-5.1-1 | M1 §5.1、§十四 | `rows.schema(entry)` 返回 `{config, inject, provides}`，另加 `announces`（无 `rutis.provides` 时为 true）与 `cordis: true`（Cordis 插件） | 跨进程 | K | loader/schema | — | 未覆盖 | #639 ※ |  |
| M1-5.1-2 | M1 §5.1、§5.2 | `inject` 规范成对象并区分 `required`；可选依赖不进 rutis 的依赖 | 纯逻辑 | — | — | — | 已取代 | — | M1 §十三：`inject` 是名字列表，全部按必需（Cordis 4 没有可选依赖） |
| M1-5.1-3 | M1 §5.1、§十四 | 写了 `rutis.provides` 的名字按声明的形状投到 rutis | 跨进程 | K | loader/announced-services | `weather.expected.json`（`methods` 与 `undeclared`） | 部分 | #639 ※ |  |
| M1-5.1-4 | M1 §5.1 | 没有 `rutis.provides` 的插件，它提供的服务只在 Cordis 里可见 | 跨进程 | — | — | — | 已取代 | — | M1 §十四：全部投到 rutis |
| M1-5.1-5 | M1 §5.1、§八 | 旧 runner 返回裸 Schema；Rust 凭 `features` 区分，不猜格式 | 纯逻辑 | PT | bridge/cross-version-meetings | — | 未覆盖 | #631 ※ | `runtime/process.rs:915-918`、`1157`；#631 的握手判定表只含会话 hello，不含 mount 的 features |
| M1-5.2-1 | M1 §5.2、§十三、§十四 | 行的依赖 = 本运行时的 `RuntimeRows` + 由 rutis 门控的 `inject` 名字的 `host_key(n)`；其余 `inject` 名字与行配置的 `inject` 由 Cordis 门控 | 跨进程 | TLA + K | loader/rows-gate-on-services | — | 未覆盖 | #625 | 门控的名字现为 catalog 共享的名字（默认全部；叶子运行时每个 inject 都门控，`loader/runtime.rs:38-48`）；#625 的"依赖出现与消失"与活性 |
| M1-5.2-2 | M1 §5.2 | 依赖在解析时固定，依赖声明变了要重新解析这一行 | 跨进程 | TLA | loader/rows-gate-on-services | — | 未覆盖 | #625 | #625 `NoProvisional` |
| M1-5.3-1 | M1 §5.3、§十四 | `rows.load` 带这一行声明的服务名与形状；runner 为每个名字建导出槽位，exporter fiber 随行卸载 | 跨进程 | TLA | loader/announced-services | — | 未覆盖 | #625 | #625 `WithdrawBeforeUnload` |
| M1-5.3-2 | M1 §5.3、§十四 | 槽位变化经 `service(name, handle, version[, {row, methods}])` 通知报给 Rust | 跨进程 | TLA | boundary/5.1 | — | 未覆盖 | #625 | #625 `NewerWins` |
| M1-5.3-3 | M1 §5.3 | 行收到有 handle 的通知时在自己的 Ctx 上 `provide_as` `host_key(name)` 的 `RowService`；收到 None 时撤销 | 跨进程 | TLA | loader/services-by-name | — | 未覆盖 | #625 |  |
| M1-5.3-4 | M1 §5.3、§十三 | `RowService::invoke` 在提供方会话上调用这一行的服务；调用方是另一个运行时的会话时经 `forward` 改写调用链 | 跨进程 | TLA + K | bridge/sync-wait-cycle-refused | `cross-runtime-sync`（`RustForward` 建模 `RowService::invoke` 的转发与改写）；`sync_cycles.rs` | 部分 | #614 | M1 §十三"留到 M2"的一项已做（`runtime/rows.rs:45-67`）；未知方法报错 |
| M1-5.3-5 | M1 §5.3、§11.2 | 服务注册在行（或叶子插件）的 fiber 上，卸载时自动撤销，依赖它的行与 Rust 插件按原生规则停下 | 进程内并发 | TLA + K | loader/rows-gate-on-services | — | 未覆盖 | #625 |  |
| M1-5.3-6 | M1 §5.3 | 两行声明同一个名字时只报诊断 | 跨进程 | — | — | — | 已取代 | — | M1 §十三、§十四：后提供的一方失败，见 M1-14-4 |
| M1-5.4-1 | M1 §二、§5.4、§八 | 运行时插件的 `injects` 与 `Mount::hosts` 为空：运行时只依赖启动本身需要的东西，不等任何宿主服务，所有等待落在行上 | 进程内并发 | K | loader/runtimes-do-not-wait-for-each-other | — | 未覆盖 | #639 ※ |  |
| M1-5.4-2 | M1 §5.4 | `hosts.provide(name, methods[, label])`：runner 以 `ctx.provide(name, hostProxy(...))` 注册代理并保存撤销函数；`hosts.withdraw(name)` 调用它 | 跨进程 | TLA + K | boundary/5.7 | — | 未覆盖 | #625 | 标签参数是后加的（`runtime/process.rs:1304-1307`） |
| M1-5.4-3 | M1 §5.4 | 宿主服务表按（作用域）名字计数：第一个租用写表并发 `hosts.provide`，最后一个释放时发 `hosts.withdraw` 并移出 | 跨进程 | TLA + SH | loader/row-unload-leaves-nothing、bridge/caller-row | — | 未覆盖 | #625 | `runtime/process.rs:1264-1347`；#625 `LeaseWithinRow` 与疑点"`lease()` 在等 `hosts.provide` 时被丢弃" |
| M1-5.4-4 | M1 §5.4、§十四 | `JsRow::apply` 在 `rows.load` 前为每个门控的名字取 `host_key(n)`；提供方就在本运行时（`origin` 相同）时跳过、Cordis 里用原生对象；否则租用一次并把释放登记为行的清理 | 跨进程 | TLA + K | loader/announced-services | — | 未覆盖 | #625 |  |
| M1-5.4-5 | M1 §5.4 | 清理顺序：先 `rows.unload`，再释放宿主服务租约，插件不会在卸载前看到服务消失；装载失败的行归还租约 | 跨进程 | TLA | loader/row-unload-leaves-nothing | — | 未覆盖 | #625 | #625 `LeaseWithinRow` |
| M1-5.4-6 | M1 §5.4 | 提供者被替换时依赖它的行按原生规则重启：先释放（可能 withdraw），再登记（provide 新 dispatch），运行时里不就地替换代理 | 跨进程 | TLA | loader/rows-gate-on-services | — | 未覆盖 | #625 | 仍有别的行持租约时，表里的 dispatch 被新租约换掉（`runtime/process.rs:1281-1289`） |
| M1-6.1-1 | M1 §6.1 | 运行时启动前解析的行返回"没有 schema、只依赖运行时"的结果，不缓存 | 跨进程 | TLA | loader/rows-gate-on-services | — | 未覆盖 | #625 | #625 `NoProvisional` |
| M1-6.2-1 | M1 §6.2 | `RuntimeRowsPlugin` 依赖运行时与 Loader，刷新行的解析结果后才提供 `RuntimeRows`；行在它出现前一直等待，`reload` 不会让它们启动 | 跨进程 | TLA | loader/rows-gate-on-services | — | 未覆盖 | #625 |  |
| M1-6.2-2 | M1 §6.2、§十三 | 要刷新的行：运行时不在时解析的，加上 `package.json` 的 `version` 变了的（`take_stale`）；先让 resolver 丢掉它们的缓存，再逐个 `Loader::reload` | 跨进程 | TLA | loader/rows-gate-on-services | — | 未覆盖 | #625 |  |
| M1-6.2-3 | M1 §6.2 | 刷新在一个任务里做，任务登记为插件的清理：卸载或重启时取消，未提供的 `RuntimeRows` 不再提供；`apply` 立即返回，不与进行中的 reconcile 互相等 | 进程内并发 | TLA + SH | loader/rows-gate-on-services | — | 未覆盖 | #625 | #625 活性"被重启打断的刷新最终重做" |
| M1-6.2-4 | M1 §6.2 | 某一行刷新失败时它按 loader 规则报失败，其余行照常放行，`RuntimeRows` 照常提供 | 跨进程 | TLA + K | loader/unresolved-rows | — | 未覆盖 | #625 | #625 没有单列这一情形 |
| M1-7.1-1 | M1 §7.1 | 编码另一个会话的远程引用时在本会话导出同种类的本地中继：函数转调、Future 转等、对象的方法与属性读取转给原引用 | 跨进程 | TLA + PT + K | bridge/multihop-cancel-and-release | `sync_cycles.rs` `function_references_calling_each_other`（函数经中继） | 部分 | #626、#631 | Future 与对象的中继没有一致性用例；对象中继发给 Node / Python 的问题见 M1-13-3 |
| M1-7.1-2 | M1 §7.1 | 中继持有原引用；对端释放中继（计数归零）时中继与原引用一起丢掉，向原会话发 `release`；计数沿中继链传递，不需要新帧 | 跨进程 | TLA + SH | bridge/multihop-cancel-and-release、bridge/release-count | — | 未覆盖 | #626、#628 | `*_releases_balance` 只有两方；#628"Import 析构 × 重新授予 × 编码回滚 × Release" |
| M1-7.1-3 | M1 §7.1 | 同一引用多次转交到同一会话复用同一中继（按（会话，引用）），对端看到同一个引用，身份比较成立 | 跨进程 | PT | bridge/release-count | — | 未覆盖 | #631 | `rpc/relay.rs:36-71`；#631"引用计数 PT 扩到中继三方" |
| M1-7.1-4 | M1 §7.1 | 转回原会话时不经中继，以 `home: true` 发回，接收方得到原对象 | 跨进程 | PT | bridge/release-count | `*_releases_balance`（Bounce：把对端的函数传回对端） | 部分 | #631 | 只两方；中继回家时还原为原引用（`rpc.rs:1985-1990`）没有三方用例 |
| M1-7.1-5 | M1 §7.1 | 中继的中继解开到最内层再判断（中继回到 Rust 就是原引用本身） | 跨进程 | PT | bridge/release-count | — | 未覆盖 | #631 | `rpc.rs:1805-1810` |
| M1-7.2-1 | M1 §二、§7.2 | 每个会话有一个标签；调用链改写只在中继与 `RowService` 这类跨会话转发处做 | 纯逻辑 | PT | bridge/call-chains-across-processes | `cross-runtime-sync`（`current-norebase` 说明改写不可少） | 部分 | #631 |  |
| M1-7.2-2 | M1 §7.2；TAG §二 | `rebase(path, from, to)`：不带标签的条目加 `from` 的标签；带 `to` 标签的去掉标签还原；带其他标签的原样保留 | 纯逻辑 | PT | bridge/call-chains-across-processes | `cross-runtime-sync`（`RebasePath` 与此相同） | 部分 | #631 | 模型不检查实现（`rpc.rs:684-693`） |
| M1-7.2-3 | M1 §7.2 | 转发前把当前调用链从源会话改写到目标会话，作为发出调用的 `path` | 跨进程 | TLA | bridge/sync-wait-cycle-refused | `cross-runtime-sync` `RoutingSound` | 部分 | #614 | 模型只建同步的服务与函数调用；await、get、中继转发的 origin 不建 |
| M1-7.2-4 | M1 §7.2 | 带标签的条目永远对不上任何会话里的原生调用号，不会被误认成本会话的调用 | 纯逻辑 | PT | bridge/call-chains-across-processes | `cross-runtime-sync` `RoutingSound`（单进程） | 部分 | #631 |  |
| M1-7.2-5 | M1 §7.2 | 回到原会话时它认出"这个反向调用属于我正在同步等待的调用"，在等待它的线程上嵌套执行，不死锁 | 跨进程 | TLA + K | boundary/5.6、bridge/sync-wait-cycle-refused | `cross-runtime-sync` `RoutingSound`、`ChainRecognised`、不死锁；`session::testing::session`（reenter）；`sync_cycles.rs` | 部分 | #614 | 每个调用一个线程的运行时（Go、Swift、Kotlin）不在模型里 |
| M1-7.2-6 | M1 §7.2（实现 `rpc.rs` `receive`） | Rust 收到调用后：按 `path` 从后往前找本会话正在同步等待的调用交给它的线程；找不到再按等待者的 origin；都没有则 spawn（对象调用在对象的执行器上） | 进程内并发 | TLA + SH | bridge/sync-wait-cycle-refused | `cross-runtime-sync` `RoutingSound`（只建 path 一支） | 部分 | #614、#628 | `rpc.rs:2405-2451`；#628"进来调用的认领交给正确的等待者" |
| M1-7.2-7 | M1 §7.2（实现 `rpc.rs` `send`） | 同步等待开始时，已在排队、属于这条链的进来调用按调用号顺序交给等待线程（与登记等待者原子地进行） | 进程内并发 | SH | — | — | 未覆盖 | #628 | `rpc.rs:1614-1631` |
| M1-8-1 | M1 §二、§八；CHN §阶段与版本 | 本机运行时会话帧格式不变，`PROTOCOL` 仍是 2 | 纯逻辑 | PT + K | bridge/version-refusal | — | 未覆盖 | #631 | `mod.rs:12`；一致性套件只跑端点格式（协议 3） |
| M1-8-2 | M1 §二、§八 | runner 在 `mount` 回复里报 `features`（`rows.v2`、`hosts`…）；行模式缺 `rows.v2` 时报运行时版本过旧、需要 0.3.0 以上 | 纯逻辑 | PT | bridge/cross-version-meetings | — | 未覆盖 | #631 ※ | 包名现为 `@arcships/rutis-runtime`（`runtime/process.rs:1185-1190`） |
| M1-8-3 | M1 §八 | 构建期生成的静态挂载不用 `rows.*`，仍可在 `Mount::hosts` 一次注册宿主服务 | 跨进程 | K | — | — | 未覆盖 | #639 ※ |  |
| M1-8-4 | M1 §八 | 没挂 `RuntimeRowsPlugin` 时行一直等 `RuntimeRows`，诊断的等待原因指向它 | 外部 | 测试 | — | — | 不适用 | #641 ※ | 诊断文字；没找到测试 |
| M1-11.1-1 | M1 §11.1 | 叶子插件必须用 `definePlugin` 显式标记（一个 Symbol），只看形状分不出 Cordis 函数插件 | 纯逻辑 | K | — | — | 未覆盖 | #639 ※ |  |
| M1-11.1-2 | M1 §11.1、§十四 | `apply` 可以 async，清理函数可以 async 或不返回；`provides` 写在代码里（可选）；`config` 直接是 JSON Schema | 跨进程 | K | — | — | 未覆盖 | #639 ※ |  |
| M1-11.2-1 | M1 §11.2 | runner 把叶子插件包成 Cordis 插件装进同一个 Node 运行时，不另开进程；与 Cordis 插件互用服务不走进程间通信 | 跨进程 | K | loader/services-by-name | — | 未覆盖 | #639 ※ |  |
| M1-11.2-2 | M1 §11.2 | 叶子插件的 ctx 只有 `use` 和 `provide` | 纯逻辑 | — | — | — | 已取代 | — | 插件 API 3 加了 `ctx.on` 与五种发出方式（sdk/leaf-events）；拿不到 Cordis Context 这一点仍成立 |
| M1-11.2-3 | M1 §11.2 | `use` 拿到同进程的原生对象或宿主服务代理，插件分不出 | 跨进程 | K | loader/services-by-name | — | 未覆盖 | #639 ※ |  |
| M1-11.2-4 | M1 §11.2 | 叶子插件配置变化一律重启（没有 volatile 字段） | 跨进程 | K | loader/volatile-in-place | — | 未覆盖 | #639 ※ |  |
| M1-11.2-5 | M1 §11.2、§十四 | `rows.schema` 遇到叶子插件直接从 `definePlugin` 对象读 `inject`、`provides`、`config`；没声明 `provides` 的报 `announces: true` | 跨进程 | K | loader/schema | — | 未覆盖 | #639 ※ |  |
| M1-13-1 | M1 §十三 | 两行声明同名服务时后装载的一行失败，错误写明已被哪一行导出 | 跨进程 | — | — | — | 已取代 | — | 被 M1 §十四 推广到任何已有提供者，见 M1-14-4 |
| M1-13-2 | M1 §十三 | （留到 M2）`RowService::invoke` 不经 `forward` | 跨进程 | — | — | — | 已取代 | — | 已改为经 `forward`（`runtime/rows.rs:45-67`），见 M1-5.3-4 |
| M1-13-3 | M1 §十三 | 对象引用经中继转给 Node 侧时 Node 不接受（函数与 Future 可以） | 跨进程 | K | — | — | 未覆盖 | #639 ※ | 后果比"不接受"重：Node 解码时抛错，`receive` 的 catch 结束整个会话（`session.mjs:369-371`、`523`）；Python 同样不声明 `objects`；Rust 的中继编码不查对端是否声明 `objects`（`rpc/relay.rs:23-73`）。见问题 1 |
| M1-14-1 | M1 §十四 | Cordis 行提供的服务全部投到 rutis：方法取服务对象自身与原型链上的函数（到 `Object.prototype` 与 Cordis `Service` 基类为止，不含 `constructor`），都是 async；写了 `rutis.provides` 的按声明 | 跨进程 | K | loader/announced-services | `weather.expected.json` `undeclared` | 部分 | #639 ※ | weather 的插件是叶子插件，Cordis 行推断方法的规则没有一致性用例 |
| M1-14-2 | M1 §十四 | runner 为行的 fiber（含子 fiber）提供、在行作用域可见、尚无槽位的服务建槽位，每次换对象发带 `{row, methods}` 的 `service` | 跨进程 | TLA | loader/announced-services | — | 未覆盖 | #625 |  |
| M1-14-3 | M1 §十四 | 门控仍按 `host_key(n)`；同一运行时别的 Cordis 行提供的服务也出现在那里，依赖它的行在服务出现时放行 | 跨进程 | TLA + K | loader/announced-services | — | 未覆盖 | #625 |  |
| M1-14-4 | M1 §十三、§十四 | 同名时后提供的一方失败（已有的是别的运行时的行、Rust 插件或节点导入都一样），错误写明双方 | 跨进程 | TLA + K | loader/announced-services | — | 未覆盖 | #625 | #625 的"拒绝撞名" |
| M1-14-5 | M1 §十四 | 行配置 `isolate` 的名字留在行自己的作用域，不投到 rutis；实例的作用域照旧（`host_key_in`） | 跨进程 | K | loader/isolate-and-inject、loader/instance-services | — | 未覆盖 | #639 ※ |  |
| M1-14-6 | M1 §十四 | `check` 对只可能由这类行提供的服务报 `may be provided at runtime by …`，不算失败；相关行里有 Cordis 行时才补"在 Cordis 里解析" | 外部 | 测试 | loader/check | — | 不适用 | — | 诊断文字（`Supply::Announced`）；`rutis-loader` 的 check 测试 |
| M1-14-7 | M1 §十四 | runner 在 `features` 加 `rows.announce`；宿主在 `mount` 参数加 `announce: true`，runner 只对这样的宿主发 `service` 的第四个参数；新旧宿主与 runner 混用时各自如前 | 跨进程 | PT + K | bridge/cross-version-meetings | — | 未覆盖 | #631 ※ | e2e `cross_version.rs` 是普通测试 |
| M1-14-8 | M1 §十四 | 带推断服务的行在回复 `rows.load` 前等宿主处理完这些通知，所以重名在装载时失败 | 跨进程 | TLA | loader/announced-services | — | 未覆盖 | #625 | #625 `ServicesBeforeRunning` |
| M1-14-9 | M1 §十四（#442） | 四种 SDK 的叶子插件 `ctx.provide` 都自动对外：方法取公开方法（JS 自身与原型链函数、Python 不以 `_` 开头的可调用属性、Go 导出方法），都是 async；`provides` 可选，写了仍按声明 | 跨进程 | K | loader/announced-services | `weather.expected.json` `undeclared`（Node、Python、Go 等） | 部分 | #639 ※ |  |
| CHN-0-1 | CHN 范围；§逻辑通道 | 一个 Session 用一个逻辑 Channel；连接池、复用与多路复用不进入会话协议 | 进程内并发 | K | — | `channel::testing::contract`（`memory_mux.rs` 的复用 Adapter 也跑） | 部分 | #639 ※ |  |
| CHN-0-2 | CHN 范围；§本机子进程端点；测试表"端点与职责隔离""操作契约接入" | 两种远程端点共用承载、身份、link 与 Session；叶子运行时接入不要求实现节点桥功能 | 跨进程 | K | — | `session::testing::session`（叶子运行时不装节点功能即通过） | 部分 | #639 ※ |  |
| CHN-1-1 | CHN §分层；§Node 与其他语言 | Channel 只搬运不透明字节、不认识帧与编码；codec 不依赖通道种类；Session 不依赖通道种类或身份验证机制 | 纯逻辑 | 测试 | — | — | 不适用 | — | 编译期的依赖结构（D1 验收：`rpc.rs`、`protocol.rs` 不引用 `std::os::unix`） |
| CHN-1-2 | CHN §分层 | `rutis-channel` 不依赖协议 crate；每个承载一个独立 crate | 纯逻辑 | — | — | — | 已取代 | — | 文件头注：2026-10-06 合并为 `rutis-bridge` 的模块 |
| CHN-1-3 | CHN §分层 | link 只负责连接、身份核对与握手、重连退避、会话替换、就绪与清理；不认识 loader、schema 或行 | 纯逻辑 | 测试 | — | — | 不适用 | — | 编译期：rutis-bridge 不依赖 rutis-loader |
| CHN-1-4 | CHN §分层 | 身份撤销触发依赖它的 link 的清理 | 跨进程 | TLA + TV | bridge/revocation-closes-only-its-connections | `remote-lease`：`RevokedStaysOut`（两模型）、`RevocationScoped`（AccessPoint） | 部分 | #615、#624 ※ | 静态 link 的监听端没有单独建模；实现未经 TV |
| CHN-1-5 | CHN §分层；§WebSocket 绑定末段 | 节点桥功能的权限由两端为对方安装的功能插件决定，与拨号方向无关 | 跨进程 | K | — | — | 未覆盖 | #639 ※ | `node_conformance.rs`、`websocket_conformance.rs` 查节点契约，未见方向无关的断言 |
| CHN-1-6 | CHN §分层 | 承载插件卸载时撤销 Transport 服务并清理监听器、连接、子进程；用它的 link 按原生门控停止 | 进程内并发 | K | — | — | 未覆盖 | #639 ※ |  |
| CHN-1-7 | CHN §分层 | 不用网络时不引入 WebSocket 依赖；新增承载不修改 link 或协议层、不放宽 WebSocket 的安全规则 | 纯逻辑 | 测试 | — | — | 不适用 | — | 编译期 feature（`websocket`） |
| CHN-2-1 | CHN §Channel 契约 | 按发送顺序交付，不丢、不重；做不到的介质在通道内部补序号、确认与重传 | 进程内并发 | K + SH | bridge/replies-match-calls | `channel::testing::contract`（`order_and_boundaries`、`nothing_lost_while_the_receiver_lags`）；`local_lines` | 部分 | #628 | 契约是固定时序；#628 的 WebSocket `Pipe`、内存 `Direction` 不丢唤醒 |
| CHN-2-2 | CHN §Channel 契约；§分帧 | 一帧一条消息，不拆分、合并、解析、修改或注入 | 纯逻辑 | K + FZ | — | `channel::testing::contract`（`order_and_boundaries`、`large_messages`，各承载）；`local_lines` | 已覆盖 | — |  |
| CHN-2-3 | CHN §Channel 契约 | 全双工：同步调用等待回复期间仍能接收反向调用 | 进程内并发 | K | — | `session::testing::session`（apply、reenter） | 部分 | #639 ※ | 通道层的契约测试没有这一项 |
| CHN-2-4 | CHN §Channel 契约；§Rust 接口 | 有界缓冲；发送方在背压下阻塞等待，不无限积压 | 进程内并发 | K + SH | — | `channel::testing::contract`（`nothing_lost_while_the_receiver_lags`：发送方等待而不丢） | 部分 | #628 | 有界没有断言 |
| CHN-2-5 | CHN §Channel 契约；§Node 与其他语言；测试表 | 通道独立于调用方执行器推进：Rust 阻塞式接口、异步承载自带线程；Node 在 I/O worker、主线程同步等待时 worker 继续收发与心跳 | 进程内并发 | K + SH | — | — | 未覆盖 | #639 ※ | 测试表要求的"调用方 `current_thread` 被阻塞时通道仍推进"不在 `channel::testing::contract` 里 |
| CHN-2-6 | CHN §Channel 契约 | 静默失效的网络介质在有限时间内发现失联；本机 socket 用 EOF | 外部 | 测试 | — | — | 不适用 | — | 计时；`websocket.rs::heartbeats_find_a_half_open_connection` |
| CHN-2-7 | CHN §Channel 契约；§Rust 接口 | 身份只经 `ChannelInfo` 交付，不注入消息流 | 纯逻辑 | 测试 | — | — | 不适用 | — | 结构性（`channel/mod.rs:66-78`） |
| CHN-2-8 | CHN §Channel 契约；§共享监听器"会话接收"；§WebSocket"对端身份" | 握手 `hello` 的端点 id 必须与连接器确认的对端身份（以及 link 期望的）一致，否则不就绪 | 纯逻辑 | PT | — | — | 未覆盖 | #631 | `rpc.rs:2081-2097`；#631 的握手判定表含"期望、已验证" |
| CHN-2-9 | CHN §Channel 契约 | 通道结束可观察并带诊断原因；在途调用以 Transport 错误失败 | 跨进程 | TLA + K | bridge/in-flight-call-fails-once | — | 未覆盖 | #626 | #626 `SettlesOnce`（含 Close） |
| CHN-2-10 | CHN §Channel 契约；§建立阶段错误；测试表 | 关闭原因只供诊断，不用于程序分支 | 纯逻辑 | 测试 | — | — | 不适用 | — | 代码性质：分支只看结构化类别（`link.rs:392-393`、`506-507`） |
| CHN-2-11 | CHN §Rust 接口 | `recv` 阻塞到下一条；`Ok(None)` 表示对端正常结束，且在它之前发出的消息之后才到 | 进程内并发 | K | — | `channel::testing::contract`（`the_far_end_closing_ends_the_channel`） | 已覆盖 | — |  |
| CHN-2-12 | CHN §Rust 接口 | `close` 幂等，唤醒阻塞在 `send` 与 `recv` 上的线程 | 进程内并发 | K + SH | — | `channel::testing::contract`（`close_is_idempotent_and_wakes_a_blocked_receive`、`close_wakes_a_blocked_send`） | 部分 | #623、#628 | 固定时序，不是可控调度 |
| CHN-2-13 | CHN §Rust 接口 | `ChannelError` 只表达通道运行期的结束，只有 `Closed { reason }` | 纯逻辑 | — | — | — | 已取代 | — | 代码加了 `Rejected { reason }`（建立后身份被拒，4003）与 `Closer::replaced` / `rejected`（`channel/mod.rs:39-63`、`117-127`；`rpc.rs:2795`），文档没跟上 |
| CHN-2-14 | CHN §Rust 接口 | 会话在发送锁内完成编码和发送，引用表的变化与发送顺序一致 | 进程内并发 | SH | bridge/release-count | — | 未覆盖 | #628 | `rpc.rs:1526-1645`、`2630-2652` |
| CHN-2-15 | CHN §Rust 接口 | `close` 先在发送锁外打断阻塞的发送，再取锁清理表 | 进程内并发 | SH | — | — | 未覆盖 | #623、#628 | `rpc.rs:1347-1352`；#623"本端关闭 × 被阻塞的发送" |
| CHN-2-16 | CHN §Rust 接口 | 读线程把 `Closed { reason }` 映射为 `Error::Transport("<label>: <reason>")` | 纯逻辑 | 测试 | — | — | 不适用 | — | 错误映射与文字；代码另把 `Rejected` 映射为 `Handshake::IdentityMismatch`（`rpc.rs:2788-2797`） |
| CHN-2-17 | CHN §Rust 接口 | link 不把缺少已确认对端身份的通道标为会话就绪 | 跨进程 | PT | — | — | 未覆盖 | #631 | 本机 compat 会话按远程插件设计 §6.1 豁免（身份由启动方指定） |
| CHN-2.2-1 | CHN §Node 与其他语言；§建立阶段错误 | Node 等实现的建立失败返回结构化类别（等价的稳定类别字段），不从 `closed(reason)` 推断 | 跨进程 | K | bridge/websocket-refusal-categories | `dial_conformance.rs`（六种拨出方按类别重连） | 部分 | #639 ※ | 查的是拨出方的行为，不是 JS 通道接口本身 |
| CHN-2.2-2 | CHN §Node 与其他语言 | 通道规格：`unix:<path>`、`fd:<n>`、`ws://`（仅回环）、`wss://`、`listen:ws(s)://`；裸路径等同 `unix:` | 纯逻辑 | PT | — | — | 未覆盖 | #631 ※ |  |
| CHN-2.2-3 | CHN §Node 与其他语言 | `listen:` 接受第一条通过鉴权的连接 | 跨进程 | — | — | — | 已取代 | — | 一次服务一个控制方、新连接接管（远程插件设计 §4.4，loader/remote-runtime-lease） |
| CHN-2.2-4 | CHN §Node 与其他语言 | 凭据、CA、证书与私钥经环境变量传入，不进通道规格、URL 或命令行 | 外部 | 测试 | — | — | 不适用 | — | 外部输入方式（环境变量）；变量名已改为 `RUTIS_TOKEN`、`RUTIS_CA`、`RUTIS_CERT`、`RUTIS_KEY`（另有 `RUTIS_CHANNEL_TOKEN`），文中 `RUTIS_INTEROP_*` 过时 |
| CHN-3-1 | CHN §分帧 | 协议层用紧凑 JSON，字符串里的换行转义；带原始换行的消息被通道拒收 | 纯逻辑 | PT + FZ | — | `*_values_round_trip`（字符串含 `\n`、`\r\n`，7 种） | 部分 | #631 ※ | 带原始换行被拒（`transport/local/lines.rs:180-182`）只有单元测试 |
| CHN-3-2 | CHN §分帧；§阶段与版本 | Unix socket 与继承 fd 按换行分帧：发送时通道加换行，接收时去掉 | 纯逻辑 | FZ | — | `local_lines` | 部分 | #631 | Rust 完整；Node、Python 等的行分帧没有 fuzz（#631"各语言的解码器"） |
| CHN-3-3 | CHN §分帧 | 每条消息（不含换行）至多 16 MiB，Rust、Node、Python 一致 | 纯逻辑 | FZ + K | bridge/large-and-deep-values | `local_lines`（任意上限：恰好上限送达，多出即关闭）；`channel::testing::size_limit`（`unix.rs`、`websocket.rs`） | 部分 | #631 | Node、Python 只有各自的单元测试 |
| CHN-3-4 | CHN §分帧 | 发送超限：拒绝并关闭通道 | 纯逻辑 | K | bridge/large-and-deep-values | `channel::testing::size_limit`（多一字节被拒并结束通道） | 部分 | #639 ※ | Rust 承载完整；关闭通道即整个会话结束（`rpc.rs:1634-1639`），登记表只写"这次调用失败"，见问题 9 |
| CHN-3-5 | CHN §分帧 | 接收超限：读到上限仍无换行即关闭通道，不再读下去，原因"消息超过上限" | 纯逻辑 | FZ | bridge/large-and-deep-values | `local_lines`（TooBig，原因含 `over the limit`） | 部分 | #631 | "不再读下去"只有单元测试 |
| CHN-3-6 | CHN §分帧 | 行分帧没有关闭码，对端看到的是连接结束 | 纯逻辑 | K | — | `channel::testing::size_limit`（对端看到结束而不是消息） | 已覆盖 | — |  |
| CHN-3-7 | CHN §分帧 | 流在一条消息中间结束算失败，不算正常结束 | 纯逻辑 | FZ | — | `local_lines`（Truncated，原因含 `inside a message`） | 部分 | #631 | 只 Rust |
| CHN-3-8 | CHN §分帧 | 只有 `\n` 分隔消息；它前面的 `\r` 是消息的一个字节并计入上限，上限加 `\r\n` 算超限，三边一致 | 纯逻辑 | FZ | — | `local_lines`（只按 `\n` 切分） | 部分 | #631 | "三边一致"没有检查 |
| CHN-4-1 | CHN §畸形帧 | 收到的每条消息必须是一帧，所指的调用与引用在本端存在；否则结束会话并关闭通道，在途调用全部失败，不回复 | 跨进程 | K + FZ | — | `session::testing::malformed`（13 种消息，7 种实现，只回 hello 后结束） | 部分 | #631 | 缺的情形见 CHN-4-4、4-6、4-7、4-8、TAG-2-1 |
| CHN-4-2 | CHN §畸形帧 | 不是 JSON：含空帧和不合法 UTF-8（Node 不得替换成 U+FFFD） | 纯逻辑 | K | — | `session::testing::malformed`（malformed JSON、empty、BOM、invalid UTF-8 inside a string） | 已覆盖 | — |  |
| CHN-4-3 | CHN §畸形帧 | 不是对象，或 `op` 未知 | 纯逻辑 | K | — | `session::testing::malformed`（not an object、unknown op） | 已覆盖 | — |  |
| CHN-4-4 | CHN §畸形帧 | 字段缺失或类型不对（`invoke` 的 `target` 不是字符串、缺 `method`；`cancel` 的 `id` 不是字符串；`throw` 的 `error.name` / `message` 不是字符串） | 纯逻辑 | K + FZ | — | `session::testing::malformed`（wrong field type、missing field，都是 invoke） | 部分 | #639 ※ | cancel、throw 的字段错误不在用例 |
| CHN-4-5 | CHN §畸形帧 | 值的标签未知 | 纯逻辑 | K | — | `session::testing::malformed`（unknown wire value） | 已覆盖 | — |  |
| CHN-4-6 | CHN §畸形帧 | 引用不存在或已释放（对它的 `call`、`get`、`await`、`release`）；`release` 计数为 0 或超过已授予的次数 | 跨进程 | K | — | `session::testing::malformed`（未知引用的 call、await、release） | 部分 | #639 ※ | `get`、已释放的引用、计数为 0 或超额都不在用例；代码 `rpc.rs:2268-2289` |
| CHN-4-7 | CHN §畸形帧 | 回复不属于任何调用（未知调用 id 的 `return` / `throw`） | 跨进程 | K + TLA | — | `session::testing::malformed`（只有 return） | 部分 | #626 | #626 `NoUnknownCall` 只证正常两端之间不出现；throw 的用例缺 |
| CHN-4-8 | CHN §畸形帧；SES §十二 | 握手顺序错：握手前的请求、重复的 `hello` | 跨进程 | K | — | — | 未覆盖 | #639 ※ | MALFORMED 都在 hello 之后发；代码 `rpc.rs:2222-2231` |
| CHN-4-9 | CHN §畸形帧 例外 | 对本端已取消的调用的迟到回复被丢弃（计数），不结束会话 | 跨进程 | TLA + K | bridge/cancel-reaches-callee | — | 未覆盖 | #626 | `rpc.rs:2115-2128`；被调方不回复时 `cancelled` 只增不减（#626） |
| CHN-4-10 | CHN §畸形帧 例外 | 对本端没有的调用的 `cancel` 被忽略 | 跨进程 | TLA + K | bridge/cancel-reaches-callee | — | 未覆盖 | #626 | `rpc.rs:2252-2267` |
| CHN-4-11 | CHN §畸形帧 末段 | Rust 解析时拒绝未知字段，Node 与 Python 忽略未知字段（尚未统一） | 纯逻辑 | K | — | — | 未覆盖 | #639 ※ | 规格自认实现不一致，按 Q5.1.1 应定下一种或写成明确不保证，见问题 4 |
| CHN-5-1 | CHN §逻辑通道与物理连接 | 一个运行时实例的多个插件共用它的控制 Session，一个节点 link 的桥功能与代装插件共用 link 的 Session；装卸单个插件不创建或关闭 Session | 跨进程 | TLA + K | loader/remote-runtime-rows、loader/peer-rows | — | 未覆盖 | #625 |  |
| CHN-5-2 | CHN §逻辑通道与物理连接 | Adapter 决定逻辑 Channel 独占还是共享物理连接；Session 与 link 不碰物理连接句柄 | 纯逻辑 | 测试 | — | — | 不适用 | — | 结构性 |
| CHN-5-3 | CHN §逻辑通道与物理连接 | send / recv、顺序、背压与 close 以逻辑 Channel 为单位；复用时隔离边界与流控，一个通道阻塞不致其他通道无界积压 | 进程内并发 | K + SH | — | `channel::testing::contract`（`memory_mux.rs`） | 部分 | #639 ※ | 隔离与流控只有 `memory_mux.rs` 的普通测试；#628 不含复用 |
| CHN-5-4 | CHN §逻辑通道与物理连接 | 关闭或替换会话只释放它的逻辑 Channel，共享的物理连接与其他 Channel 照常 | 进程内并发 | K | — | — | 未覆盖 | #639 ※ |  |
| CHN-5-5 | CHN §逻辑通道与物理连接 | 物理连接故障通知全部受影响的 Channel；不能保持有序可靠时终止 Channel；终止后不换物理连接复活，不自动重放 Session 请求 | 跨进程 | TLA + K | bridge/no-call-on-a-dead-session、bridge/in-flight-call-fails-once | `remote-lease`：`OldCommandsStayOld`、`OldCallsStayOld` | 部分 | #626 | 模型说的是旧会话的指令不作用于新会话；Adapter 一层不在模型里 |
| CHN-5-6 | CHN §逻辑通道与物理连接 | Channel 失败后由 link 重新申请逻辑 Channel 并建新 Session；Adapter 与 link 不各自重试同一次接入；不做会话恢复或进程自动重启 | 跨进程 | TLA | bridge/no-call-on-a-dead-session | `remote-lease` `RemoteLease`（link 退避重连，每次新会话） | 部分 | #615 | Adapter 一层的重试不在模型里 |
| CHN-5-7 | CHN §逻辑通道与物理连接 | `ChannelInfo.peer`、验证结果与接收注册属于逻辑 Channel；身份与安全上下文兼容才共享物理连接；每个通道独立完成接收授权，不因复用串用权限 | 跨进程 | K | — | — | 未覆盖 | #639 ※ | 没有生产用的复用 Adapter |
| CHN-5-8 | CHN §逻辑通道与物理连接 | 撤销注册、Identity 或 link 只清理它的逻辑通道与待交付结果；凭据撤销使共享连接失效时 Adapter 关闭它并通知所有受影响通道 | 跨进程 | TLA | bridge/revocation-closes-only-its-connections | `remote-lease` AccessPoint `RevocationScoped` | 部分 | #624 | 复用连接不在模型里 |
| CHN-6-1 | CHN §连接器 | `spawn`（继承 fd）：fd 3 为 socketpair 的一端，支持时为默认；`spawn`（路径）：临时目录 socket、子进程回拨 | 外部 | 测试 | bridge/spawned-processes-own-channel | — | 不适用 | — | 操作系统；`concurrent_launch.rs`、`local_spawn.rs` |
| CHN-6-2 | CHN §连接器 | WebSocket `dial` / `listen` 每次连接产出 Channel，身份放进 `ChannelInfo.peer`；`dial` 只尝试一次；`protocol` 由上层传入，承载只核对、不认识会话版本 | 跨进程 | FZ + TLA | — | `websocket_upgrade`（接受时的协议是上层给的 `rutis.3`）；`RemoteLease`（拨号是单次动作，重试归 link） | 部分 | #631 ※ |  |
| CHN-6-3 | CHN §连接器；§WebSocket 重连参数 | 连接器每次只报告一次建立结果，不重试、不替换会话、不决定会话就绪；承载 `dial` 不含重试循环 | 跨进程 | TLA | — | `RemoteLease`（link 是唯一的重试方） | 部分 | #615 |  |
| CHN-6-4 | CHN §连接器 | Adapter 可内部管理连接池；单次建立必须可取消，内部维护不演变为无限等待或另一个重连循环 | 进程内并发 | SH | — | — | 未覆盖 | #628 ※ |  |
| CHN-6-5 | CHN §连接器；SES §十二 | link 完成身份核对与协议握手后才会话就绪；就绪不表示 loader 的 schema 或行已就绪，两阶段行就绪不是 link 的就绪条件 | 跨进程 | TLA | — | `RemoteLease` `ReadyIsCurrent` | 部分 | #615、#625 |  |
| CHN-6-6 | CHN §连接器 | 卸载承载或身份依赖时 link 按原生门控停止、撤销注册并清理连接；配套插件清理它管理的行 | 跨进程 | TLA | — | `remote-lease` `StoppedStaysDown`（两模型） | 部分 | #615 | 配套插件清理行不在模型里 |
| CHN-7-1 | CHN §建立阶段错误 | 建立阶段返回跨语言一致的结构化类别 `Retryable` / `AuthRejected` / `Incompatible`，与已建立通道的 `ChannelError` 分开 | 纯逻辑 | PT + K | bridge/websocket-refusal-categories、bridge/dial-out-rules | `dial_conformance.rs`（Python、Node、Bun、Go、Kotlin、Swift 拨出方按类别） | 部分 | #631 | Rust 拨号的 link 一侧只有 `websocket_cross.rs` 普通测试 |
| CHN-7-2 | CHN §建立阶段错误；§共享监听器"路由" | `Retryable` 含"凭据有效但没有 link 监听它"（无注册，或对端 link 尚未注册）；拨号侧指数退避后再单次 `dial` | 跨进程 | TLA + PT | bridge/websocket-refusal-categories | `remote-lease` AccessPoint（路由：监听器上无注册为可重试） | 部分 | #631 |  |
| CHN-7-3 | CHN §建立阶段错误；§WebSocket 重连参数 | `AuthRejected`：按 30 秒间隔慢重试并持续报错，不绕过验证 | 跨进程 | TLA + 测试 | bridge/dial-out-rules | `remote-lease`（被拒后按认证失败等待，时间不建模） | 部分 | #633 | 30 秒是计时 |
| CHN-7-4 | CHN §建立阶段错误；§WebSocket 重连参数 | `Incompatible`：停止自动重试，等配置修正或显式重启 link | 跨进程 | TLA + K | bridge/dial-out-rules | `dial_conformance.rs`（`an_incompatible_access_point_stops_the_dialer`） | 部分 | #615 ※ | 控制方 link 一侧不在模型里 |
| CHN-7-5 | CHN §建立阶段错误 | 类别从类型、协议状态或验证结果生成，不解析诊断文字；`reason` 不含凭据 | 纯逻辑 | 测试 | bridge/tls-server-verified | — | 不适用 | — | 代码性质与文字；`websocket.rs` 的类别测试 |
| CHN-7-6 | CHN §建立阶段错误 | 会话握手的身份拒绝、协议不兼容以结构化结果交给 link，按对应的慢重试或停止处理，不从 `Closed.reason` 反推 | 纯逻辑 | PT | bridge/version-refusal | — | 未覆盖 | #631 | `mod.rs:144-154`（`Handshake`）、`link.rs:392-393` |
| CHN-7-7 | CHN §建立阶段错误 | 运行期断线按 link 策略恢复；本地子进程不自动重启 | 跨进程 | TLA | loader/runtime-down-stops-its-rows | `RemoteLease`（断线后退避重连） | 部分 | #615、#627 | 本机运行时监督不在模型里（#627 的 Launcher 接缝） |
| CHN-7-8 | CHN §建立阶段错误 | link 停止或依赖撤销后不再安排重试；已发起的连接结果不得再交付为新会话 | 跨进程 | TLA | — | `remote-lease` `StoppedStaysDown`、`ReadyIsCurrent` | 已覆盖 | — | 模型层面；实现未经 TV |
| CHN-8-1 | CHN §共享监听器 | 注册至少绑定监听器、承载实例、本地端点 id、预期对端 id、Identity 规则、会话协议、所属 link 与注册代次；每个监听器一个本地端点；注册按（监听器，对端）唯一 | 进程内并发 | PT | — | — | 未覆盖 | #631 | #631"接入点的注册路由：有状态 PT 对照参考模型" |
| CHN-8-2 | CHN §共享监听器"注册验证" | link、承载与 Identity 都须有效，端点 id 与身份绑定一致；同一对端只允许一个有效 link 路由，重复或歧义注册拒绝 | 进程内并发 | PT | — | — | 未覆盖 | #631 |  |
| CHN-8-3 | CHN §共享监听器"入站认证"；测试表"安全与身份" | 以验证得到的对端 id 查注册；不得以未验证的自报 id 或请求参数选定授权目标 | 跨进程 | TLA + PT | — | `remote-lease` AccessPoint（按凭据路由） | 部分 | #631 | 接入点按共享 token 加自报 id 准入（拨出稿 §五），与本条矛盾，见问题 5 |
| CHN-8-4 | CHN §共享监听器"路由"；§WebSocket"接收授权" | 只交给身份绑定匹配的有效注册；无注册、已撤销或身份不符直接拒绝，不建隐式 link；可验证无注册为 `Retryable`（503），不能验证为 `AuthRejected`（403）；凭据验证不等于接收授权 | 跨进程 | TLA + PT | bridge/websocket-refusal-categories | `remote-lease` AccessPoint（无注册可重试、共享 token 被吊销 403） | 部分 | #631 |  |
| CHN-8-5 | CHN §共享监听器"移交复核" | 移交 link 前重新核验注册代次、所属 link 与 Identity；撤销与移交有确定先后，旧握手不能越过撤销完成移交 | 跨进程 | TLA + TV | bridge/revocation-refuses-reconnects | `remote-lease` AccessPoint `NoStaleHandOver`（变异 `access-mut-no-gen-check`）、`RevokedStaysOut` | 部分 | #615、#624 ※ | 静态 link 的注册是"顺序特例"，没有单独建模；实现未经 TV |
| CHN-8-6 | CHN §共享监听器"会话接收" | link 再核对 `hello` 的端点 id 与 `ChannelInfo.peer`；握手完成前不就绪；旧会话只由 link 决定是否替换 | 跨进程 | TLA + PT | — | `RemoteLease` `ReadyIsCurrent` | 部分 | #631 |  |
| CHN-8-7 | CHN §共享监听器"撤销" | 注册句柄随 link 生命周期释放；link 停止、重建或 Identity 撤销时撤销旧代次，清理握手中的连接与已移交的通道 / 会话，迟到结果关闭，旧注册不再接受连接 | 跨进程 | TLA + TV | bridge/revocation-refuses-reconnects | `remote-lease` AccessPoint `NoStaleHandOver`、`RevokedStaysOut`、`StoppedStaysDown` | 部分 | #624 |  |
| CHN-8-8 | CHN §共享监听器 | 撤销一个 link 不关闭共享监听器或其他 link | 跨进程 | TLA | bridge/revocation-closes-only-its-connections | `remote-lease` AccessPoint `RevocationScoped` | 部分 | #615、#624 ※ | 静态 link |
| CHN-8-9 | CHN §共享监听器 | 新 link 必须重新注册，不能复用已撤销的句柄；承载卸载时撤销它的全部注册 | 进程内并发 | PT + TLA | — | `remote-lease` AccessPoint（停止后以新代次注册） | 部分 | #631 |  |
| CHN-9-1 | CHN §WebSocket 重连参数 | 拨号侧 link 唯一负责重连：指数退避初始 0.5 秒、上限 30 秒、±20% 抖动；连接稳定 60 秒后重置 | 外部 | PT + 测试 | bridge/dial-out-rules | — | 未覆盖 | #631、#633 | 退避序列与抖动是纯逻辑（#631 的 `Backoff` PT），60 秒重置是计时（#633）；`link.rs:56-67` |
| CHN-10-1 | CHN §WebSocket 绑定 | 一条 WebSocket 物理连接对应一个逻辑 Channel，关闭码、心跳与接管作用于它；多路复用要另行协商承载分帧，不改 Session 操作格式 | 纯逻辑 | 测试 | — | — | 不适用 | — | 结构性 |
| CHN-10-2 | CHN §WebSocket 绑定"子协议"；§阶段与版本 | 子协议 `rutis.<会话协议主版本>`，不符时在升级握手阶段拒绝并返回 `Incompatible`；新增不兼容格式不复用现有版本号；通道内部变化不升会话协议 | 纯逻辑 | FZ + K | bridge/version-refusal、bridge/websocket-refusal-categories | `websocket_upgrade`（接受时协议为 `rutis.3`） | 部分 | #631 ※ | 拒绝与类别不在 fuzz 里 |
| CHN-10-3 | CHN §WebSocket 绑定"TLS" | 非回环地址必须用 `wss`；反向代理终止 TLS 时后端只听回环；拨号方用系统根证书或配置的 CA 校验 | 外部 | 测试 | bridge/tls-server-verified | — | 不适用 | — | `transport/websocket/config.rs:52-55`；`websocket.rs` 的 wss 测试 |
| CHN-10-4 | CHN §WebSocket 绑定"鉴权" | 升级请求带 `Authorization: Bearer <token>` 或用客户端证书；Identity 给凭据与规则，承载验证并映射对端 id | 外部 | FZ + 测试 | bridge/tls-client-certificate-verified | `websocket_upgrade`（读 Bearer 不 panic） | 部分 | #631 ※ | 证书验证只能靠测试（`websocket.rs`） |
| CHN-10-5 | CHN §WebSocket 绑定"鉴权" | 不把 token 放进 URL、诊断或日志 | 外部 | 测试 | bridge/tls-server-verified | — | 不适用 | — | 文字输出 |
| CHN-10-6 | CHN §分帧；§WebSocket 绑定"消息" | 一帧一条文本消息，UTF-8 JSON，不带换行；二进制消息留给后续二进制编码（收到即关闭） | 纯逻辑 | K | — | `channel::testing::contract`（`websocket.rs`） | 部分 | #639 ※ | 二进制消息的处理（`transport/websocket/connection.rs:240-241`）只有普通测试 |
| CHN-10-7 | CHN §WebSocket 绑定"大小上限" | 默认 16 MiB，可配置；任一方向超限以 1009 关闭该通道，会话看到一次 `Closed` | 纯逻辑 | K + 测试 | bridge/large-and-deep-values | `channel::testing::size_limit`（`websocket.rs::meets_the_size_limit_check`） | 部分 | #639 ※ | 关闭码 1009 只有普通测试 |
| CHN-10-8 | CHN §WebSocket 绑定"心跳" | 双方每 10 秒 ping，30 秒内没收到任何消息即判失联关闭；启用 TCP keepalive；心跳独立于调用方执行器，用 ping / pong，不注入会话帧 | 外部 | 测试 | — | — | 不适用 | — | 计时；`websocket.rs::heartbeats_find_a_half_open_connection` |
| CHN-10-9 | CHN §WebSocket 绑定"关闭原因""有序关闭" | close frame 的 reason 为 UTF-8、至多 123 字节、只供诊断；有序关闭用 1001 | 外部 | 测试 | — | — | 不适用 | #641 ※ | 关闭帧的协议细节；没找到断言 123 字节截断或 1001 的测试 |
| CHN-10-10 | CHN §WebSocket 绑定"接管" | link 决定同一端点 id 的新连接接管旧会话后调用旧通道的 `Closer::replaced()`；WebSocket 以 4002、reason `replaced by a new connection` 关闭，其他承载普通关闭 | 跨进程 | TLA + 测试 | bridge/dial-out-rules | `remote-lease`：`OneLease`、`OneInstancePerId` | 部分 | #615 | 关闭码与文字靠测试 |
| CHN-10-11 | CHN §WebSocket 绑定 末段 | 远程叶子运行时只监听，由控制方 link 拨号 | 跨进程 | — | — | — | 已取代 | — | 拨出稿：运行时可以 `dial:` 拨出到接入点（bridge/dial-out-rules） |
| CHN-11-1 | CHN §本机子进程端点 | 启动参数 `<程序> <通道> <插件或 project>`，通道为 `fd:3`、`unix:/path` 或裸路径；新会话格式追加 `--id <id>`（启动方指定），兼容会话不传 | 外部 | 测试 | — | — | 不适用 | — | 命令行 |
| CHN-11-2 | CHN §本机子进程端点 | 是否用继承 fd 由启动方决定（Node 包的 `rutisChannels` 含 `fd`、`RuntimePlugin::python`、自定义 `Launcher` 的 `inherit_fd()`），未声明的用路径方式 | 外部 | 测试 | — | — | 不适用 | — | 打包与配置 |
| CHN-11-3 | CHN §本机子进程端点 | 继承 fd：只允许该 fd 作为通道被继承，其余 fd 保持 `CLOEXEC`，标准流按原用途 | 外部 | 测试 | bridge/spawned-processes-own-channel | — | 不适用 | — | 操作系统；`concurrent_launch.rs` |
| CHN-11-4 | CHN §本机子进程端点；§关联规范；SES §十二 | stdout 不承载协议帧，可供插件输出 | 外部 | 测试 | — | — | 不适用 | — | 进程的标准流；`local_stdio.rs` |
| CHN-11-5 | CHN §本机子进程端点 | `spawn` 的 Receiver 在 EOF 后最多等 1 秒取子进程退出状态再返回 `Closed` | 进程内并发 | — | — | — | 已取代 | — | 现为 2 秒 GRACE 与 10 秒 HANG_GUARD，注释写明"不是等退出状态"（`transport/local/spawn.rs:981-989`，#243、#246） |
| CHN-11-6 | CHN §本机子进程端点 | 通道的结束说明进程怎样结束（退出状态或信号） | 进程内并发 | SH | — | — | 未覆盖 | #623 |  |
| CHN-11-7 | CHN §本机子进程端点 | 保留已有退出诊断的逐字兼容（如 `Cordis process exited with signal: 9 (SIGKILL)`） | 外部 | — | — | — | 已取代 | — | 现为 `the process exited with …`（`transport/local/spawn.rs:610`、`1150`） |
| CHN-11-8 | CHN §本机子进程端点；§建立阶段错误；§WebSocket 重连参数 | 子进程不自动重启 | 跨进程 | TLA + SH | loader/runtime-down-stops-its-rows | — | 未覆盖 | #627 |  |
| CHN-11-9 | CHN §本机子进程端点 | 冻结的反方向 `server.rs` 与 `client.mjs` 的 `launch` 用 Unix 连接器，保留监听后拉起模式，行为不变 | 外部 | 测试 | — | — | 不适用 | — | 兼容路径（`crates/rutis-bridge/src/cordis/server.rs`、`node/rutis-runtime/src/client.mjs`） |
| CHN-12-1 | CHN §装饰器 | `trace` 默认关闭，只记方向、长度与关闭原因，不记内容 | 外部 | 测试 | — | — | 不适用 | — | 输出；开关现为 `RUTIS_TRACE`（`runtime/local.rs:37`），文中 `RUTIS_INTEROP_TRACE` 过时 |
