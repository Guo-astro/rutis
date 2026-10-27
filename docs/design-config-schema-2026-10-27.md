# 0.18 的配置 Schema：补默认值、校验，再 `apply`

[English](design-config-schema-2026-10-27.en.md)

状态：定稿，未实现。日期：2026-10-27（同日修订：去掉"不检查的关键字"告警；解码器的错误改写成同样的句子；多出来的键由 loader 告警；写明 7 份实现要逐字一致的规则；再次修订：内核的 `Validation` 文字不变、去掉 `config_issues`，补上 dylib 行、应用运行时的行与 0.18 以前的二进制，简化解码器错误；第三次修订：写明 loader 里做这件事的位置、失败文字怎样写出，去掉旧 dylib 与旧 Swift 二进制的特例）。基准：`main` `85678323`（0.17.2 之后）。排期：0.18（不兼容，写进《迁移到 0.18》）。
Issue：[#669](https://github.com/arcships/rutis/issues/669)。与 [0.18 的诊断](design-diagnostics-2026-10-26.md) 一致：错误按它的 §4.3 带错误码，按 §4.7 记在配置写错的那一行，输出与它的 §8.1 故障 12 相同，不出现它 §4.5 的禁用词。

范围外：`pattern`、`format`、字符串长度等更细的约束（§3.1 的清单之外）；类型转换（`"5"` 不会变成 `5`）；生成配置表单或类型（#684）；Cordis 行由 Cordis 自己校验（§2.3），不再校验一遍；Standard Schema 作为 `definePlugin` 的配置。

## 一、现在怎样

插件 API 指南早就写着"JSON Schema；宿主据此校验和展示配置"，实际只展示（#714 已把指南改成实际情况）：

| 语言 | 现在 |
| --- | --- |
| TypeScript（Node、Bun）、Python | 配置原样交给 `apply`：不补默认值，`required`、类型都不看，裸字符串 `"Oslo"` 也接受 |
| Go | `json.Unmarshal` 进类型 `C`：类型错能报出字段，多余的键（`cty`）悄悄忽略；生成的 Schema 把没有 `omitempty` 的字段都列为 `required`，但从来不按它检查 |
| Swift、Kotlin | 解码进 `Decodable` / `@Serializable` 类型，出错文字是解码器的原文 |
| Rust（loader 的行） | `serde` 解码，错误只出现在 `ReconcileReport`：`config validation failed: ["invalid type: integer 42, expected a string"]`，没有字段路径，多余的键忽略 |
| Cordis 行 | Cordis 按插件的 `Config`（Standard Schema，例如 schemastery）补默认值并校验，与 Cordis 相同 |
| `rutis-host check` | 只打印 Schema |

## 二、决策

| # | 决策 | 章节 |
| --- | --- | --- |
| C1 | 每种语言的 SDK 在装载一行、调用 `apply` 之前补默认值并校验（运行时和测试工具走同一段代码）；`check` 用 Rust 实现做同样的事；loader 不对别的运行时的行再校验一遍 | 2.1 |
| C2 | 一个 JSON Schema 子集，14 个关键字；其他关键字不检查，也不告警 | 3.1 |
| C3 | 多出来的键：告警并原样交给 `apply`，告警由 loader 的 `EntryFactory` 按 Schema 算出，每行每个配置值打印一次；Schema 写了 `additionalProperties: false` 时是错误；`check --strict` 把告警当错误 | 3.2 |
| C4 | 类型化配置（Go、Swift、Kotlin、Rust）的 Schema 由类型生成，只写语言自己的解码器本来就有的规则；Schema 通过后解码器还报错时写 `is not a value the plugin accepts`，不出现解码器原文 | 3.3 |
| C5 | 默认值按"对象在才补它的属性"逐层补，跟着 `$ref`、`allOf`，不进 `anyOf` / `oneOf`；没给配置时对象根是 `{}` | 四 |
| C6 | 所有问题按路径排序一次列出，每个一行：`<行>: config: <路径> should be …, got …`；错误码 `bad-config` | 五 |
| C7 | 配置错时这一行失败，等它的行显示为等这一行；`dev`、`run`、测试工具、`check` 输出相同 | 五 |
| C8 | 一份共同的用例表 `specs/config-schema/cases.json`，每种语言和 `check` 都逐条跑，逐字比较；§六 的每条规则都有用例 | 六、八 |

### 2.1 为什么由各语言的 SDK 做

- **测试工具和真实运行时必须一样。** 测试工具里没有 loader，只有 SDK；SDK 装载一行的那段代码，运行时和测试工具共用。放在 SDK 里，测试过的配置规则就是运行时的规则。放在 loader 里，每种语言的测试工具还得各写一份，反而和运行时分成两套。
- **远程运行时自然一样。** 配置由运行它的那一端检查，不论运行时在本机、别的机器还是应用里。
- **类型化语言本来就要在这里解码。** Go、Swift、Kotlin、Rust 在交给 `apply` 之前把配置解码进类型；先按 Schema 补默认值、校验，再解码，出错文字就和其他语言一样。
- **一份规则，靠用例表保持一致**（§六、§八），而不是靠一份代码：反正每种语言都要有一份（测试工具），用例表让 7 份和 `check` 逐字相同。
- 0.18 以前的 SDK 构建的运行时，运行时不检查；`check` 照样检查它们的行（§七）。
- **进程内的 Rust 行没有单独的运行时**：loader 在 `EntryFactory`（`loader/plugins.rs`）里做：它的 `validate_config` 和 `build` 都从 `config.resolved.schema` 补出一份副本再校验，`build` 用这份副本。只对 `Resolved.foreign_scope` 为假的行（`Builtins` 登记的、dylib 的、嵌入者自己的解析器给的；dylib 的 Schema 来自它导出的 `rutis_plugin_config_schema`）；别的运行时的行由它们的 SDK 做，不在这里再做一遍（否则会做两遍、0.18 以前的 SDK 的行会失败、Cordis 行会按转换出来的 Schema 校验）。`EntryConfig.value` 与 `Running.config` 仍是原来的值，不存补过的：否则每次 reconcile 都会看到两者不同，把这一行再更新一遍。改配置（`view.update` / `view.set_config`）经内核同样到达 `EntryFactory`，所以第一次装载和改配置走的是同一处。

### 2.2 每一行经过的步骤

```text
行的 config（没有或 null 视为没给）
  → 补默认值（§四）
  → 按 Schema 校验（§3.1）；有错 → 这一行失败（§五），不调用 apply
  → 类型化语言：解码进类型；解码器还报错时，同样作为 bad-config，改写成 §5.1 的句子（§3.3）
  → apply(ctx, config)        // 补过默认值的副本，多出来的键原样保留
```

改配置（这一行重启）时同样走一遍。插件交给别人的定义（`ctx.plugin(def.at(&ctx, config))`）在定义所在的运行时里装载，也走一遍，失败记在这一份副本上，`ctx.plugin` 的调用方拿到同一个错误。

### 2.3 Cordis 行

Cordis 行（导出 `Config` 的 Cordis 插件）按 Cordis 的规则：Cordis 在装载时用 `Config` 校验，不再按转换出来的 JSON Schema 校验一遍（转换会丢掉 `transform` 等信息，两边会不一致）。Node 运行时把 Cordis 的 `ValidationError` 换成错误码 `bad-config` 的错误，按 §五 的格式写（路径取自 issue 的 `path`，文字取自 issue 的 `message`）。`check` 不检查 Cordis 行的配置值，只打印 Schema（与现在相同）；多出来的键也不告警。

`definePlugin({ config })` 只接受 JSON Schema。传入 schemastery 等 Standard Schema 对象（带 `~standard`）时，定义时就报错：`config of definePlugin should be a JSON Schema object, got a schemastery Schema`（不是 schemastery 时写 `got a Standard Schema`）。这样的代码以前也不对：Schema 只被打印，从没生效。

## 三、规则

### 3.1 支持的关键字

| 关键字 | 怎样检查 |
| --- | --- |
| `type` | `string`、`number`、`integer`、`boolean`、`object`、`array`、`null`，或它们的数组（`["object", "null"]`）。`integer` 接受 `1.0` |
| `properties` | 逐个属性检查 |
| `required` | 缺了就是错误 |
| `additionalProperties` | `false`：多出来的键是错误；一个 Schema：多出来的键按它检查（字典） |
| `items` | 数组的每一项按它检查（一个 Schema） |
| `enum`、`const` | 按 JSON 相等比较 |
| `minimum`、`maximum` | 数字的上下限（含等号） |
| `default` | 补默认值（§四） |
| `anyOf`、`oneOf` | 符合其中一个即可。`oneOf` 按 `anyOf` 检查，不要求只符合一个：这与 JSON Schema 不同，为的是 schemars 生成的 `oneOf` 在值同时符合几个分支时不误报 |
| `allOf` | 每一个都要符合 |
| `$ref` | 只认本文档内的 `#/$defs/…` 与 `#/definitions/…`（schemars、手写都够用） |

- 其他关键字（`format`、`pattern`、`minLength`、`uniqueItems`、`prefixItems`、`if` 等）不检查，也不告警：schemars、Kotlin 的生成器会自动写上其中一些，插件作者去不掉。各语言指南列出检查哪些关键字。
- 不做类型转换：`"5"` 不符合 `number`。

### 3.2 多出来的键

- 对象写了 `properties`、没写 `additionalProperties`：多出来的键告警，值原样交给 `apply`（JSON Schema 的本义，旧配置不会因此起不来）。拼写接近已有的键时给出建议（§6.1）。
- **已知的键**：对象的 Schema 与它经 `$ref`、`allOf` 合进来的部分里所有 `properties` 的键的并集；它也是拼写建议的候选。`anyOf` / `oneOf` 分支里的对象不告警多出来的键（不知道值是按哪个分支写的）。
- **告警由 loader 给**，不由运行时进程写：loader 在同一处（`EntryFactory`），对所有有 Schema 的行（不论 `foreign_scope`），用这一行已有的 Schema（`rows.schema` / `plugins.read` / 清单）和同一份 Rust 实现算出多出来的键，写进宿主的输出（`rutis-host` 写到标准错误，Rust 嵌入写进诊断设计 §七 的默认输出），不带插件输出的 `[weather]` 前缀。同一行同一个配置值只告警一次，重启不再重复；记录已告警的状态在 `commit.rs` 的试运行与真正提交之间共用，试运行不会让真正提交漏掉或重复告警。远程运行时、0.18 以前的 SDK 的行也由本机的 loader 告警。连进来的运行时（应用里的）的行在解析时没有 Schema（`Resolved.schema` 是 `None`），每个连接的 Schema 在实例的 `factory()` 里才有：由实例的 factory 告警，每个实例一次。SDK 只报错误。
- Cordis 行（loader 里看 `Resolved.meta["cordis"]`）不告警多出来的键，按 Cordis 的规则（§2.3）。
- 测试工具没有 loader，自己写同样的告警到标准错误。
- `additionalProperties: false`：由 SDK 作为错误报出（§五），这一行失败。
- `check --strict`：告警算错误，以 1 退出（与 `--strict` 对其他告警的处理相同）。
- 由 `ctx.plugin(def.at(...))` 装载的副本也经过 rutis-loader（`PluginMounter`，Schema 来自 `ForeignPlugin.schema()`），但**不告警**，只报错误：副本的配置是插件代码写的，不是人改的配置文件，告警没有人去改；测试工具对副本也不告警。

### 3.3 类型化配置怎样对上

生成 Schema 时只写语言自己的解码器本来就有的规则，不让开发者为 rutis 多写标注：

| 语言 | 必填 | 默认值 | 多出来的键 |
| --- | --- | --- | --- |
| Go（类型 `C`） | 不生成 `required`：没写的字段是零值，与 `encoding/json` 和 Go 插件指南相同（**改变**：以前把没有 `omitempty` 的字段列为 `required`）；要分清"没写"用指针 | 零值；手写 `Schema` 里的 `default` 照常补 | 告警 |
| Swift（`@RutisConfig`） | 可选类型（`T?`）和带字面量初始值的 `var` 不必填，其余必填 | `var units = "metric"` 的字面量成为 `default`（字符串、数字、布尔、由它们组成的数组和字典）；非字面量的初始值不算默认值，字段必填 | 告警 |
| Kotlin（`@Serializable`） | 有默认参数（`isElementOptional`）或可空的不必填 | 由 kotlinx 解码时填（`SerialDescriptor` 看不到默认值，Schema 里没有，`check` 不显示） | 告警 |
| Rust（`serde` + `schemars`） | schemars 已有的规则：`Option<T>`、`#[serde(default)]` 不必填 | schemars 写进 Schema 的 `default`；先补进 JSON，`serde(default)` 照常有效，两者是同一个值 | 告警；`#[serde(deny_unknown_fields)]` 生成 `additionalProperties: false`，是错误 |
| Rust（没有 Schema：`Definition`，以及 `export_plugin!` 没写 `schema:` 的 dylib） | `serde` 解码 | `serde(default)` | `serde` 的规则 |

**解码器的错误**：有 Schema 时先按 Schema 校验，通过了解码器还报错，说明 Schema 和类型不一致，一律写 `<路径> is not a value the plugin accepts`，不出现解码器的原文。路径由各语言的解码器给出；Kotlin 的 `RutisFormat` 现在的 `ValueException` 不带路径，要补上。

没有 Schema 的 Rust 解码按种类改写：`Definition`（配置类型只要求 `DeserializeOwned`），以及 `export_plugin!` 没写 `schema:` 的 dylib（`schema:` 是可选的，多数不写）：

| 种类 | 句子 |
| --- | --- |
| `invalid_type` | `<路径> should be <期望>, got <值>`；`<期望>` 取自被调用的 `deserialize_*` 方法（`deserialize_str` → `a string`），不取 serde 的 `expected`（那是内部类型名，诊断设计 §4.5） |
| `missing_field` | `<路径> is required` |
| `unknown_variant` | `<路径> should be "a" or "b", got "c"`（§5.1 的 `enum` 句子） |
| `unknown_field` | `<路径> is not a config key (did you mean …?)` |
| 其他 | `<路径> is not a value the plugin accepts` |

为此 Rust 要一个基于 `serde_json::Value` 的 `Deserializer`，用自己的错误类型（`serde::de::value` 对错误类型是泛型的），解码时记下路径和被调用的方法，不解析 serde 的文字。它放在 `rutis` 里，rutis-sdk（dylib）也用它；有 Schema 时同一个解码器只给出那一句 `is not a value the plugin accepts`，没有 Schema 时按上表的种类。

## 四、默认值

- **没给配置**（没写 `config` 或写了 `null`）：根 Schema 有 `default` 时用它；根是对象（`type` 是 `"object"` 或包含 `"object"`）时从 `{}` 开始；都不是时按"没有值"检查（根不允许时报 `config is required`）。只写了 `properties`、没写 `type` 的根不算对象：别的规则会改变 JSON Schema 的意思。没有 Schema 的插件照旧，收到什么就是什么。
- **逐层补**：一个对象在（包括刚开始的 `{}`），它缺的属性有 `default` 时补上，再进入补上的或原有的子对象继续补。
- **跟着 `$ref`、`allOf`，不进 `anyOf` / `oneOf`**（与 ajv 相同）。
- **不凭空造对象**：缺的子对象自己没有 `default` 时不补，`apply` 里它就是没有；想让嵌套的默认值总在，给子对象写 `default: {}`。这是 ajv `useDefaults` 的规则，JavaScript 开发者最熟悉的那一种。
- 补进去的是副本，`default` 写的对象不会被插件改动影响到下一次。
- 符合 `integer` 的值（如 `1.0`）在交给插件的副本里写成整数（`1`）：否则 Go 的 `encoding/json` 和 serde 解码进整数类型时会拒绝它。
- 补完再校验：`default` 本身不符合 Schema 时，报的就是那条路径的普通错误（`units should be "metric" or "imperial", got "kelvin"`），这一行失败；运行时与 `check` 相同。
- 判断"配置里有没有这个键"的代码会看到默认值已经补上：这是本稿要的行为，写进迁移说明。

## 五、错误

### 5.1 文字

一个问题一行，所有问题一次列出（排序见 §6.4；最多 20 个，多的在最后一行写成 `<行>: config: and N more problems`）：

```text
<行>: config: <路径> <原因>
```

| 问题 | 文字 |
| --- | --- |
| 类型不对 | `city should be a string, got 42` |
| 根类型不对 | `config should be an object, got "Oslo"` |
| 缺必填 | `city is required` |
| 不在 `enum` 里 | `units should be "metric" or "imperial", got "kelvin"`；三个以上：`should be one of "a", "b", "c"` |
| `const` | `mode should be "fast", got "slow"` |
| 上下限 | `days should be at least 1, got 0`；`days should be at most 14, got 30` |
| 不是整数 | `days should be an integer, got 1.5` |
| `anyOf` / `oneOf` | `proxy should be a string or an object, got 3`（分支怎样写见 §6.5） |
| 不允许的键（错误），多出来的键（告警） | `cty is not a config key (did you mean city?)`；没有接近的键时没有括号部分。告警与错误同一句话，告警只是不让这一行失败（值照样交给 `apply`） |

- 路径：`home.lat`、`cities[2]`；不是标识符（`[A-Za-z_][A-Za-z0-9_]*`）的键写成 JSON 字符串加方括号：`headers["x-api"]`。根写 `config`。
- `got` 的写法见 §6.2、§6.3。
- 一条路径上类型不对时，只报类型这一条：不再报它的 `enum`、上下限，也不进入它的属性或项。
- 不出现诊断设计 §4.5 的禁用词（serde、`ValidationError`、内部类型名、平台名字）。

### 5.2 结构

- 错误是 `BindingError`，错误码 `bad-config`。诊断设计 §4.3 写明"表在实施时可以加行，不改已有的行"，本稿在那张表加一行：`bad-config`，什么时候：配置不符合 Schema，`data`：`{ issues: [{ path: ["home", "lat"], message: "home.lat should be a number, got \"north\"" }] }`（`path` 的数组项是键或下标）。
- SDK 只发 `code` 与 `data`；`site`（这一行、`doing: "apply"`，诊断设计 §4.1）由 loader 在边界上补；测试工具里没有 `site`。rutis 打印时见到 `bad-config` 写 `config:`，不写 `failed in apply:`。
- Cordis 行的 `ValidationError` 也是 `bad-config`（§2.3）。
- Rust 的行（`foreign_scope` 为假的行）：`EntryFactory` 的配置步骤失败时，返回一个带标记的错误（loader 自己的类型包着 `ErrorKind::Validation { issues }`，`issues` 是上面每一行的文字）。一行的错误只在 `Loader::failures` 里变成字符串（`reconcile.rs` 的 `rejected.to_string()`），这个字符串同时给 `Failure.error` 和等待原因 `WaitReason::ProviderFailed`（`status.rs` 打印它）。`failures()` 见到带标记的错误时写成 `config: <问题>`，每个问题一行，于是 `ReconcileReport` 与 §九 的等待行（`chat: waiting for weather (row weather failed: config: …)`）都对；也和插件自己 `validate_config` 返回的错误、内核的 `config type mismatch` 分得开。内核的 `ErrorKind::Validation` 不只用于配置，它的显示文字不改。
- 宿主认出其他语言的行（含 Cordis 行）的配置错误，用诊断设计 §4.2 / §8.2 的 `rutis_bridge::thrown(&e)` 读错误码：`code() == "bad-config"`。不按文字匹配。

### 5.3 记在哪一行，之后怎样

- 记在配置写错的那一行（诊断设计 §4.7）：这一行失败，不调用 `apply`；依赖它的行显示 `chat: waiting for weather (row weather failed: config: units should be …)`，不显示成调用方的错误。
- 改好配置后照常重新装载（`dev` 监视配置文件时自动，`run` 下按现有的重新装载方式）。
- `dev` 与 `run` 规则相同，区别只在诊断设计 §七 的输出前缀；配置的错误和告警由 rutis 打印，不带插件输出的前缀。

## 六、逐字一致的规则

7 份实现与 `check` 要逐字相同的地方，每条在用例表里都有用例：

### 6.1 拼写建议

与 rutis-host 给服务名的建议相同，用 `rutis-loader` `loader/waits.rs` 的 `closest`（`status.rs` 只打印 loader 算出的结果）：

- 编辑距离按 Unicode 标量值计，一次编辑是插入、删除、替换、交换相邻两个（OSA 距离）。
- 键长 0–2 个标量：不建议；3–6 个：距离不超过 1；7 个以上：不超过 2。
- 候选是这个对象已知的键（§3.2，不含它自己）；取距离最小的，距离相同时取按 Unicode 码点最小的（配置模块先把候选排序再交给 `closest`，`closest` 本身不改）。

### 6.2 `got` 的值

- 字符串、数字、布尔、`null` 写成 JSON；对象写 `an object`，数组写 `an array of N`（`N` 是项数）。不写出对象和数组的内容，不涉及键的顺序。
- 数字：值是整数且绝对值不超过 2^53 时不带小数点（`1.0` 写成 `1`）；其他写最短的往返形式（与 JavaScript 的 `String(number)` 相同：`1.5`、`1e+21`）。超出 2^53 的整数各语言精度不同，用例表不含。
- 字符串：按 JSON 转义 `"`、`\` 与控制字符（`\n`、`\t` 等有短写的用短写，其余写 `\u00XX`，小写十六进制）；非 ASCII 字符原样写出，不转义。

### 6.3 长度上限

字符串的内容超过 40 个 Unicode 标量值时，只写前 40 个，后接 `…`（U+2026），再加引号：`"<前 40 个>…"`。计数在转义之前。

### 6.4 问题的顺序

按路径排序：路径逐段比较，键按 Unicode 码点比较，下标按数值，一条路径是另一条的前缀时前者在先（父在子之前）。同一路径上的先后：类型、`enum` / `const`、上下限、`anyOf` / `oneOf`，`allOf` 各成员的问题按成员的顺序插在后面。缺必填的键和多出来的键都在子路径上，与其他子路径一起按键排序。缺必填的键按它自己的路径（`home.lat`）参与排序。这样不依赖配置里键的顺序（Go 的 map 没有顺序），也不依赖 Schema 里 `properties` 的顺序（Go 生成的 Schema 是 map）。

### 6.5 `anyOf` / `oneOf` 失败时写出的分支

- 每个分支先跟着 `$ref`、`allOf` 找到它的 `type` / `const` / `enum`，写成：`type` → `a string`、`a number`、`an integer`、`a boolean`、`an object`、`an array`、`null`；`const` 与 `enum` → 它们的值（§6.2 的写法）；三者都没有的分支写 `another accepted value`。
- 按分支的顺序写，去掉重复，两个用 `or` 连接，三个以上写 `one of a, b, c`（与 `enum` 相同）。

## 七、`rutis-host check`

- 对每一行用它已经打印的 Schema、按同样的规则补默认值并校验（Rust 实现，与 loader 的行共用），不运行插件。
- 有错误：按现有版式写 `<行> (<插件>): cannot run: config: …`，每个问题一行，以 1 退出。多出来的键是告警，写到标准错误，以 0 退出；`--strict` 时以 1 退出。
- 0.18 以前的 SDK 构建的 Go 二进制（清单里的 `sdk` 早于 0.18）：`check` 不看清单里的 `required`，其余照常检查。旧的 Go 清单把没有 `omitempty` 的字段都列为必填，按它检查会误报。Swift 的旧清单不需要特例：旧的宏按「是否可选」写 `required`，合成的 `Decodable` 要的正是这些键。
- 检查时不知道 Schema 的行（没连上的远程运行时、`peer:` 行）写 `config not checked: its schema is known only when it runs`，不算告警。Cordis 行见 §2.3。
- `check` 的输出与运行时的错误逐字相同（同一份用例表锁住）。

## 八、用例表

`specs/config-schema/cases.json`：每条是 `{ name, schema, config, then: { config } | { errors: [...] }, warnings: [...] }`，`errors` 与 `warnings` 是 §5.1 的完整文字。每种语言的 SDK 测试（Node、Bun、Python、Go、Swift、Kotlin）、rutis-loader 与 `check` 都逐条跑，逐字比较（`warnings` 由测试工具与 loader 的告警比较）。用例至少有：

| # | 用例 |
| --- | --- |
| 1 | 没给配置，根是对象：得到 `{}` 加默认值；根是 `["object", "null"]` 同样；只有 `properties` 没有 `type` 的根：不造 `{}` |
| 2 | `null` 与没给相同 |
| 3 | 裸字符串当配置：`config should be an object, got "Oslo"` |
| 4 | 缺 `required` |
| 5 | 类型不对（字符串、数字、整数、布尔、数组、对象各一条） |
| 6 | `enum` 两个值、三个值；`const` |
| 7 | `minimum`、`maximum` 边界（等于上下限通过） |
| 8 | 嵌套对象在：补它的默认值；不在：不造对象；子对象 `default: {}`：造出来并补 |
| 9 | 默认值跟着 `$ref` 与 `allOf`，不进 `anyOf` / `oneOf` |
| 10 | 数组项出错：路径 `cities[2]` |
| 11 | 字典（`additionalProperties` 是 Schema） |
| 12 | 多出来的键：告警、原样保留；建议的三种长度界限、距离相同时的取舍、交换相邻两个算一次；没有接近的键时的写法 |
| 13 | `additionalProperties: false`：错误 |
| 14 | `anyOf` / `oneOf`；`["string", "null"]`；分支是 `$ref`、`const`、什么都没有时的写法；`oneOf` 同时符合两个分支时通过 |
| 15 | `$ref` 到 `$defs` 与 `definitions` |
| 16 | 不检查的关键字（`pattern`、`format`、`uniqueItems`）：值照常通过，没有告警 |
| 17 | 多个问题一次列出的顺序（键的码点顺序、父在子前、同一路径上的先后）；超过 20 个截断 |
| 18 | `default` 本身不符合 Schema：运行时是普通的路径错误 |
| 19 | 非标识符的键的路径 |
| 20 | `got` 的写法：`1.0` 写成 `1`、`1.5`、`1e+21`、转义（`"`、`\`、`\n`、`\u0001`）、非 ASCII 原样、超过 40 个标量值截断（含多字节字符）、对象与数组 |
| 21 | 默认值是副本：插件改了它，下一次装载不受影响 |
| 22 | 已知的键包括 `$ref`、`allOf` 合进来的 `properties`；`anyOf` / `oneOf` 分支里的对象不告警 |
| 23 | 符合 `integer` 的 `1.0` 在得到的配置里是 `1` |
| 24 | 类型不对的路径上不再报 `enum`、上下限与子路径 |

类型化语言另有自己的用例（Go 的零值与指针、Swift 的字面量初始值、Kotlin 的默认参数、Rust 的 `serde(default)` 与 `deny_unknown_fields`、Rust `Definition` 的解码器错误种类），各在自己的测试里。

## 九、开发者看到什么

`rutis.json` 里 `weather` 的配置写错时（`dev`，TypeScript 插件；其他语言输出相同）：

```text
# 值不在 enum 里
weather: config: units should be "metric" or "imperial", got "kelvin"

# 裸字符串
weather: config: config should be an object, got "Oslo"

# 缺必填、类型不对，一次列出（按路径排序）
weather: config: city is required
weather: config: days should be an integer, got 1.5
weather: config: home.lat should be a number, got "north"

# 拼错键（告警，这一行照常起来，city 用默认值）
weather: config: cty is not a config key (did you mean city?)

# 依赖它的行
chat: waiting for weather (row weather failed: config: units should be "metric" or "imperial", got "kelvin")

# Rust 插件（类型错由 Schema 先查出，与其他语言相同）
weather: config: city should be a string, got 42

# Rust 里做的定义（没有 Schema），解码器的错误改写后
demo: config: city should be a string, got 42
```

`rutis-host check`：

```text
weather (weather): cannot run: config: units should be "metric" or "imperial", got "kelvin"
rutis-host: warning: weather: config: cty is not a config key (did you mean city?)
rutis-host: 1 row(s) or binaries cannot run
$ echo $?
1
```

（沿用 `check` 现有的版式 `<行> (<插件>): cannot run: …`；一行有几个问题时每个问题一行。）

测试工具（以 Node 为例）：`load(plugin, { units: 'kelvin' })` 失败，抛出的 `BindingError` 的 `code` 是 `bad-config`，消息是 `config: units should be "metric" or "imperial", got "kelvin"`；Go 的 `rutistest.Load` 返回同样的错误（`errors.As` 得到 `*rutis.BindingError`）。

## 十、兼容与迁移

写进《迁移到 0.18》（两份）：

- **以前写错也能运行的配置现在让这一行失败**：类型不对、缺 `required` 的字段（Python 以前完全不看 `required`）、裸字符串当配置、`additionalProperties: false` 下多出来的键。**升级前先用 0.18 的 `rutis-host check` 跑一遍**，它列出每一行不合格的配置。
- **测试也会失败**：给测试工具传了不完整或写错的配置的测试，现在和运行时一样失败；按 Schema 补全配置，或让 Schema 如实写出哪些字段可以不写。
- **默认值现在会补上**：判断"配置里有没有这个键"的插件代码会看到默认值；为默认值写的兜底可以去掉，也可以留着。
- **Go**：生成的 Schema 不再有 `required`（与 Go 的零值一致；0.18 以前的二进制，`check` 也不看它的 `required`）；手写的 `Schema` 里的 `default` 现在会补上（Go 插件指南"不会替你填默认值"一句改写）。
- **Swift**：带字面量初始值的 `var` 字段不再必填，没写时用初始值。
- **Rust**：loader 的行的配置错误在 `ReconcileReport` 里的文字从 `config validation failed: [...]` 变为 `config: <路径> …`；类型不变，内核 `ErrorKind::Validation` 的显示文字不变。dylib 的行在 `build` 之前由 loader 校验；dylib 照每次发布一样要用 0.18 重新构建（rutis-dylib 核对 SDK 的标识与散列，旧的 dylib 不会被装载）。
- **嵌入者自己的解析器**：用 `with_schema` 给了 Schema 的行，现在会按它补默认值并校验（`Resolved.schema` 的文档注释同步写明）。
- **Node、Bun**：`definePlugin` 的 `config` 传 Standard Schema 对象时报错。只影响本来就不对的代码：这样的 Schema 以前只被打印，从没生效。
- **0.18 以前的 SDK 构建的运行时**：运行时不检查它们的行的配置；`check` 照样检查。升级 SDK 后才在运行时生效。
- **多出来的键**：只告警；`check --strict` 下是错误。
- 发布说明写明：这是兑现文档早就写着的行为（插件 API 指南的"宿主据此校验"）。

## 十一、实施

| 步 | PR 内容 |
| --- | --- |
| 1a | 用例表 `specs/config-schema/`；Rust 实现（rutis-loader 的 `config` 模块：补默认值、校验、§六 的规则）；`check` 的检查、告警与退出码 |
| 1b | loader：`EntryFactory` 的 `validate_config` / `build` 对 `foreign_scope` 为假的行补默认值并校验（原值不变）；`failures()` 对带标记的错误写 `config: …`；对多出来的键的告警（含实例的 factory、试运行共用的去重）；`site` 的补法；`Resolved.schema` 的文档注释。`rutis` 里基于 `Value` 的 `Deserializer` 与 §3.3 的错误改写，`Definition` 与 rutis-sdk 的 dylib 解码都用它 |
| 2 | Node 与 Bun：`definePlugin` 的行与测试工具；Cordis `ValidationError` 换成 `bad-config`；拒绝 Standard Schema 对象 |
| 3 | Python：模块行、`define_plugin` 的行与测试工具 |
| 4 | Go：校验后再解码；生成的 Schema 去掉 `required`；`rutistest` |
| 5 | Swift 与 Kotlin：`@RutisConfig` 的字面量默认值；Kotlin `RutisFormat` 的错误带路径；两者的运行时、应用里的运行与测试工具 |
| 6 | 文档：插件 API 指南（列出检查的关键字）、rutis-host 指南"检查配置"、各语言指南（撤回 #714 写的"目前不检查"）、诊断设计（两份）§4.3 的表加 `bad-config` 一行、《迁移到 0.18》、各语言模板去掉代码里重复的默认值（改写在 Schema 或类型里）、`docs/guarantees.md` 新条目"配置在 `apply` 之前补默认值并校验"（写明验证它的用例表） |

每一步合并时，它那一列的用例全部通过；#669 在第 6 步合并、7 列都通过后关闭。
