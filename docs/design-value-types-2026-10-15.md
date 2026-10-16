# 跨语言值类型：日期时间、二进制、Map、Set、大整数、Decimal、UUID

[English](design-value-types-2026-10-15.en.md)

状态：定稿；阶段 1（规范，见[协议设计](design-protocol-plugins-2026-09-25.md) §6.1）与阶段 2（Rust）已实现，阶段 3、4 未实现。日期：2026-10-15。基准：`main`（0.10 之后，#400–#402 已合入）。
依据：[#308](https://github.com/arcships/rutis/issues/308) 的方案草稿与核对、[#355](https://github.com/arcships/rutis/issues/355)、[#360](https://github.com/arcships/rutis/issues/360)、维护者决定（2026-10-15，记在 #308）；[插件 API](guide/plugin-api.md)「值怎样传递」；[协议设计](design-protocol-plugins-2026-09-25.md) §六（显式带标记的值）；[设计哲学](design-philosophy.md) §七。

范围：会话协议的值层，以及 Rust、Node、Bun、Python、Go 五种实现和它们的测试工具；Swift、Kotlin SDK 按本文实现（[Swift SDK](design-swift-sdk-2026-10-12.md)、[Android 应用托管](design-android-runtime-2026-10-12.md)）。

范围外：节点之间的事件转发（`NodeEvent` 的参数是 JSON，新类型在那里拒绝，见 §七）；服务之间数据是否等价、能否互换（服务作者的事）。

## 一、决定

| # | 决定 |
| --- | --- |
| V0 | **最高原则：开发者无感**。插件只写本语言原生的代码，转换全由 rutis 做；正常使用不需要任何 rutis 专用类型或写法。唯一例外：凭空创建一个本语言没有的类型的值（如 JS 里的"只有日期"），用 SDK 的构造函数（2026-10-17 修订） |
| V1 | 0.11 一次做完全部十种值：时间点、本地日期时间、日期、时刻、二进制、Map、Set、大整数、Decimal、UUID；连同 #355 的一致性修复 |
| V2 | 原则：**默认转换，不丢信息**。每种值在每种语言有一个默认对应；对方没有同样的类型时，转成最接近且不丢信息的表示。只有确实会丢信息时报错 |
| V3 | 线上编码：与 `reference` 同级的新标记，不把普通 JSON 字段解释为类型 |
| V4 | 协商：每种值一个能力；对端没有声明时，**只让这次调用失败**，会话继续；不升协议版本号 |
| V5 | 没有时区的日期时间（Python 无 `tzinfo` 的 `datetime`、Kotlin `LocalDateTime`）是**独立的一种值**（本地日期时间，`datetime`），原样传递，不猜时区、不转成时间点（同 Arrow、JS Temporal `PlainDateTime`、`java.time.LocalDateTime`） |
| V6 | 整数超出 ±(2^53−1) 时**自动**按大整数传（Python `int`、Go `int64` / `uint64` / `*big.Int`、Rust 整数、Kotlin `Long` / `BigInteger`），不报错、不用包装。代价：JS 收到别的语言发来的整数时，范围内是 `number`、超出是 `bigint`（2026-10-17 修订，取代"显式才传"） |
| V7 | 时间点精度不同时，接收方按自己的精度**向负无穷截断**，写进边界规则，不报错 |
| V8 | Map 的键与 Set 的元素只能是"在每种语言里都是值"的东西（§五）；转换后键重复（会丢条目）时这次调用失败 |
| V9 | Go 结构体、带类型的切片和 map、顶层 `time.Time` 一律经同一个编码器逐个检查，不再走 `encoding/json` |
| V10 | 数据里的 `undefined` 在所有语言统一为 `null`（Python dict 不再删键） |
| V11 | Rust `Value` 加新变体并标 `#[non_exhaustive]`：不兼容变化只做这一次 |
| V12 | 测试工具与真实运行时对同一份期望文件比对，新值全部纳入 |

## 二、默认对应

| 值 | 标记 | JS（Node、Bun） | Python | Go | Rust | Kotlin | Swift |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 时间点 | `instant` | `Date` | `datetime`（带时区，UTC） | `time.Time`（UTC） | `Instant` | `java.time.Instant` | `Date` |
| 本地日期时间 | `datetime` | `"YYYY-MM-DDTHH:MM:SS[.f…]"` 字符串 | `datetime`（无时区） | `rutis.DateTime` | `DateTime` | `LocalDateTime` | `Rutis.DateTime` |
| 日期 | `date` | `"YYYY-MM-DD"` 字符串 | `date` | `rutis.Date` | `Date` | `LocalDate` | `Rutis.CalendarDate` |
| 时刻 | `time` | `"HH:MM:SS[.f…]"` 字符串 | `time`（无时区） | `rutis.TimeOfDay` | `TimeOfDay` | `LocalTime` | `Rutis.TimeOfDay` |
| 二进制 | `bytes` | `Uint8Array` | `bytes` | `[]byte` | `Vec<u8>` | `ByteArray` | `Data` |
| Map | `map` | `Map` | `dict` | `map[K]V` | `Vec<(Value, Value)>` | `Map` | `Dictionary` |
| Set | `set` | `Set` | `set` | 切片（声明为 `map[K]struct{}` 时为它） | `Vec<Value>` | `Set` | `Set` |
| 大整数 | `bigint` | `bigint` | `int`（收到的是 `int` 子类，见下） | `*big.Int` | `BigInt` | `BigInteger` | `Rutis.BigInt` |
| Decimal | `decimal` | 字符串 | `Decimal` | 字符串 | `Decimal`（字符串） | `BigDecimal` | `Decimal` |
| UUID | `uuid` | 字符串（小写） | `UUID` | 字符串（小写） | `Uuid` | `java.util.UUID` | `UUID` |

- Rust 的 `Instant`、`DateTime`、`Date`、`TimeOfDay`、`BigInt`、`Decimal`、`Uuid` 是 rutis-bridge 自己的小类型（不引入 chrono、num-bigint），各自提供与常用库互转的方法；Go 的 `rutis.DateTime`、`rutis.Date`、`rutis.TimeOfDay`，Swift 的 `Rutis.DateTime`、`Rutis.CalendarDate`、`Rutis.TimeOfDay`、`Rutis.BigInt` 同理。
- **JS 里的字符串**：本地日期时间、日期、时刻、Decimal、UUID 在 JS 没有对应类型，收到为**普通字符串**，内容完整，`===` 等照常可用。代价：这样的值经 JS 原样转发给别的语言时，类型不保留（Python 发出的 `date` 经 JS 转发回来是 `str`）；不做"记得类型的字符串对象"，因为对象的 `===` 按身份比较，会让 UUID 比较等常见写法静默出错。凭空创建、或要以原类型转发时，用 SDK 的构造函数：`rutis.datetime('2026-10-15T14:30')`、`rutis.date('2026-10-15')`、`rutis.time('14:30')`、`rutis.decimal('1.10')`、`rutis.uuid('…')`；普通字符串照旧是字符串。Go 的 Decimal、UUID 同理（`rutis.Decimal`、`rutis.UUID` 命名字符串类型）。
- **Python 的大整数**：普通 `int` 超出范围时自动按大整数发出，不需要任何写法。收到的 `bigint` 是 `int` 的子类（开发者当 `int` 用即可），原样发回仍是大整数，所以 JS 发来的 `5n` 经 Python 转发回 JS 仍是 `5n`。Go 收到的 `bigint` 到 `any` 时是 `*big.Int`，同理。
- **Map**：JS 里字符串键的 `Map` 到 Python 是 `dict` 的子类，原样转发仍是 Map；Python 普通的字符串键 `dict` 照旧按对象传。开发者无需区分。
- **Swift 的 Decimal**：Foundation `Decimal` 只有 38 位有效数字，超出时这次调用失败（会丢信息）。
- **Go 的 Set**：Go 没有 set 类型；按声明的目标类型解码，目标是 `any` 时得到去重后的切片。

## 三、线上编码

新值与 `reference`、`signal` 同级，含它们的数组与对象改用 `list` / `record` 编码（与现在含引用时相同）。

| 标记 | `value` | 规则 |
| --- | --- | --- |
| `instant` | `"2026-10-15T03:22:15.123456789Z"` | RFC 3339，UTC（`Z`），0–9 位小数，年份 0001–9999 |
| `datetime` | `"2026-10-15T14:30:05.25"` | 无时区，0–9 位小数，年份 0001–9999 |
| `date` | `"2026-10-15"` | 年份 0001–9999 |
| `time` | `"14:30:05.25"` | 24 小时制，0–9 位小数，无时区 |
| `bytes` | `"aGVsbG8="` | base64（RFC 4648 §4，带填充） |
| `bigint` | `"-123456789012345678901"` | 十进制，无前导零，至多 4300 位（Python 默认的整数字符串上限） |
| `decimal` | `"-12.3400"` | 十进制，可带小数，不用指数；保留末尾零（`1.10` 与 `1.1` 不同）；至多 4300 位；NaN、无穷拒绝 |
| `uuid` | `"0f8fad5b-d9cb-469f-a165-70867728950e"` | 小写，8-4-4-4-12 |
| `map` | `[[k, v], …]` | 键值对数组，保持发送方的迭代顺序 |
| `set` | `[e, …]` | 元素数组，保持发送方的迭代顺序 |

例：`{"type":"instant","value":"2026-10-15T03:22:15Z"}`；`{"type":"map","value":[[{"type":"data","value":"a"},{"type":"data","value":1}]]}`。

## 四、能力与协商

- 能力名：`value-instant`、`value-datetime`、`value-date`、`value-time`、`value-bytes`、`value-map`、`value-set`、`value-bigint`、`value-decimal`、`value-uuid`。endpoint 与 compat 两种握手都声明（compat 同 `sync-wait`：旧实现忽略握手里多出的字段；Rust 的 `COMPAT_DECLARED` 加上这些名字）。
- 发送方只向声明了相应能力的对端发送该标记；对端没有声明时，**这次调用失败**，错误说明哪种值、对端不支持，会话继续。
- 宿主转发时按**下游**对端的能力再检查：上游能发、下游不能收时，被转发的这次调用失败。
- 新实现（Swift、Kotlin）可以只实现一部分：没声明的类型，对端不会发给它。

## 五、Map 的键与 Set 的元素

- 允许：字符串、有限数字、布尔、`bigint`、`datetime`、`date`、`time`、`decimal`、`uuid`。
- 不允许：`instant`（JS `Date` 是对象，作键按身份比较）、`bytes`、`null`、数组、对象、`map`、`set`、引用。发送方在发出前拒绝。
- 接收方转换后键重复时（例如 JS `Map` 的键 `1` 与 `true` 到 Python dict 会合并），这次调用失败：会丢条目。
- 字符串键的 `Map` 仍按 `map` 标记传（不是对象），对方收到 `Map` / `dict`，不变成普通对象：JS 普通对象照旧按对象（`data`）传。

## 六、各语言的发送规则

| 语言 | 规则 |
| --- | --- |
| JS | `Date` → `instant`（无效日期报错）；`Uint8Array`、`ArrayBuffer`、`DataView`、`Buffer` → `bytes`；`Map` → `map`；`Set` → `set`；`bigint` → `bigint`；SDK 构造函数 → `datetime` / `date` / `time` / `decimal` / `uuid` |
| Python | 带时区 `datetime` → `instant`；无时区 `datetime` → `datetime`（V5）；`date` → `date`；`time`（带时区的拒绝）→ `time`；`bytes`、`bytearray`、`memoryview` → `bytes`；`set`、`frozenset` → `set`；非字符串键的 `dict`、收到的 Map（`dict` 子类）→ `map`；超出 ±(2^53−1) 的 `int`、收到的大整数 → `bigint`；`Decimal` → `decimal`；`UUID` → `uuid`。`datetime`、`Decimal`、`UUID` 不再按对象引用传（#355） |
| Go | `time.Time` → `instant`；`rutis.DateTime` / `rutis.Date` / `rutis.TimeOfDay` → `datetime` / `date` / `time`；`[]byte` → `bytes`；非字符串键的 map → `map`；`*big.Int` / `big.Int` → `bigint`；`rutis.Decimal` / `rutis.UUID` → `decimal` / `uuid`。结构体字段、带类型的切片与 map 递归经同一编码器（V9），不再静默变成字符串或 base64 |
| Rust | `Value` 的新变体（V11）；`Value::json()` 遇到新值报错（同引用）；Cordis 生成器把 `Uint8Array`、`Date`、`bigint`、`Map`、`Set` 映射到上面的 Rust 类型 |
| Kotlin、Swift | 按 §二 的类型；值层用 SDK 自己的编码器逐个检查，不依赖 `JSONEncoder` / kotlinx 的默认编码（它们会把日期编成数字、二进制编成 base64） |

时间点精度（V7）：JS 毫秒、Python 微秒、Swift 约微秒（`Date` 是 `Double`）、Go / Rust / Kotlin 纳秒；收到更细的值时向负无穷截断。

## 七、边界规则（写进插件 API 文档）

1. 默认转换按 §二，不丢信息；会丢信息时这次调用失败，错误说明原因。
2. 没有时区的日期时间是本地日期时间，原样传递，不猜时区；它与时间点是两种值，不互相转换。
3. 时间点按接收方精度向负无穷截断。
4. 整数超出 ±(2^53−1) 自动按大整数传；JS 收到时为 `bigint`，范围内为 `number`。JS 自己的 `number` 超出范围时照旧不精确（JS 本身的限制），要精确用 `bigint`。
5. Map 键、Set 元素的限制见 §五；转换后键重复则失败。
6. 对端不支持某种值时，这次调用失败，会话继续。
7. 节点之间的事件转发（`NodeEvent`，参数为 JSON）不承载新值：含新值的事件在转发时拒绝。同一运行时进程内的叶子事件照常传递。
8. `undefined` 在数据里统一为 `null`。

## 八、不兼容变化（0.11 迁移说明）

- Python：`datetime`、`Decimal`、`UUID` 过去按对象引用传，现在按值传；对端拿到的是值，不是代理。
- Go：结构体内的 `time.Time`、`[]byte`、超界整数过去经 `encoding/json` 变成字符串、base64 或不检查，现在按 §六 编码或报错。
- Python dict 里值为 `undefined` 的键过去被删掉，现在为 `null`。
- Rust：`Value` 加变体并标 `#[non_exhaustive]`，穷尽匹配要加一支。
- Python、Go 的整数超出 ±(2^53−1) 过去报错，现在自动按大整数传。
- 过去被拒绝的值（JS `Date`、`Map`、`Set`、二进制、`bigint` 等）现在可以传，但只发给声明了能力的对端：与旧版本运行时之间，这些调用照旧失败。

## 九、测试

| 测试 | 内容 |
| --- | --- |
| 期望文件 `weather.expected.json` | 每种值的往返样例（各语言发、各语言收），把现在的 `throws` 改为 `returns`；截断、键重复、超界等失败样例 |
| `tests/value_roundtrip.rs` | 属性测试生成器加十种值与 §五 的键规则 |
| 各语言单测 | 编解码、边界、错误文本 |
| 跨版本（#209） | 上一个发布版本的运行时 ↔ 当前宿主：新值的调用失败、会话继续；宿主转发给旧下游时同样 |
| 测试工具 | Node `@arcships/rutis/testing`、Python `rutis.testing`、Go `rutistest` 与真实运行时同一份期望 |

不做 Q6.2.8 的模型检查：只增加值标记，不改多方交互规则。

## 十、阶段

| 阶段 | 内容 |
| --- | --- |
| 1 | 协议规范：本文的编码与能力写进协议设计 §六；期望文件先加样例（标为待实现） |
| 2 | Rust：`Value` 变体、编解码、能力、转发检查 |
| 3 | Node、Bun、Python、Go：各自编解码、能力、测试工具（可并行） |
| 4 | 一致性：期望文件全部改为 `returns`、属性测试、跨版本测试、文档（插件 API 值表、各语言指南、guarantees） |

阶段 3 依赖阶段 2；#355 的一致性修复随阶段 3 各语言一起做（不先拒绝再支持）。

## 十一、与草案的出入

| 草案 | 本文 | 理由 |
| --- | --- | --- |
| 0.11 只做二进制与日期，其余拒绝 | 十种值全部在 0.11 | 维护者决定：插件作者只适应一次，协议只改一次，Swift / Kotlin SDK 拿到完整规则 |
| 无时区 `datetime` 拒绝 | 独立的本地日期时间值，原样传递 | 维护者决定：不报错、不猜时区、不丢信息（参考 §十二） |
| Python `date`、`time` 拒绝 | `date`、`time` 两个标记 | #360 |
| 标记名 `date` 表示时间点 | 时间点为 `instant`，另有 `datetime`、`date`、`time` | 四种时间值各有其名，同 JS Temporal 与 `java.time` |
| Python 超界整数按大小自动发 `bigint` 的选项 | 自动按大整数传（V6） | 2026-10-17 维护者决定：开发者无感（V0）优先于"JS 收到的类型固定"（#360 原要求） |
| （阶段 3 的实现）收到的值用"记得类型的字符串对象" | JS 收到普通字符串 | 对象的 `===` 按身份比较，会让常见写法静默出错；经 JS 转发丢类型的代价写进文档 |
| Decimal、UUID 在没有对应类型的语言里用包装对象 | 用字符串，SDK 提供构造函数发出原类型 | 维护者决定：默认转换，内容不丢 |

## 十二、原则与参考

**原则**

0. 开发者无感（V0）：写原生代码，转换由 rutis 做，这一条优先于下面各条。
1. 类型写在明处：带标记的值，不靠字段名猜类型（协议设计 §六）。
2. 不悄悄丢信息：能转就转，会丢信息才报错（设计哲学 §七 原则 7）。
3. 收到的类型尽量稳定；与 0 冲突时让位（整数在 JS 里随大小为 `number` 或 `bigint`）。
4. 用各语言顺手的类型：一张统一的语义表，加每种语言的对应表。
5. 新旧共存：按类型协商，不支持的只让这次调用失败。
6. 测试工具与真实运行时同一份期望（一致性测试即契约）。

**参考**

| 项目 / 标准 | 借鉴 |
| --- | --- |
| Protocol Buffers（proto3 JSON 映射、一致性测试套件） | 每种语言的对应表；int64 在 JSON 里用字符串（同 V6 的问题）；所有实现跑同一套一致性测试 |
| Apache Arrow | 时间类型区分带时区与不带时区（V5）；Decimal 的精度语义 |
| JS Temporal 提案、`java.time` | 时间点 / 本地日期时间 / 日期 / 时刻四分（`Instant`、`PlainDateTime`、`PlainDate`、`PlainTime`） |
| Smithy（AWS） | 一份模型、多语言 SDK 的类型表；2.0 删去 set 类型——set 的跨语言语义难统一，所以本文限制 Set 元素（§五） |
| WebAssembly 组件模型（WIT） | 数据按值、资源按引用，与 rutis 的值 / 引用划分相同；刻意少而精的内置类型 |
| GraalVM Polyglot 互操作 | 按能力描述值（是否日期、是否 map、能否放进 long），与按类型协商相近 |
| CBOR（RFC 8949）类型标签 | 用标签扩展类型（时间、大整数、十进制、UUID、set、非字符串键 map），是"带标记的值"的先例 |
| HTML structured clone | JS 一侧支持的类型集合（`Date`、`Map`、`Set`、`Uint8Array`、`bigint`） |
| RFC 3339、RFC 4648、RFC 9562、I-JSON（RFC 7493） | 时间文本格式、base64、UUID、JSON 整数的安全范围 ±(2^53−1) |
| MongoDB Extended JSON、superjson | 反例：用 `$date` 一类特殊字段名标类型，会与用户数据撞名 |
