# 0.18 的配置 Schema：补默认值、校验，再 `apply`

[English](design-config-schema-2026-10-27.en.md)

状态：定稿，未实现。日期：2026-10-27。基准：`main` `85678323`（0.17.2 之后）。排期：0.18（不兼容，写进《迁移到 0.18》）。
Issue：[#669](https://github.com/arcships/rutis/issues/669)。与 [0.18 的诊断](design-diagnostics-2026-10-26.md) 一致：错误按它的 §4.3 带错误码，按 §4.7 记在配置写错的那一行，输出与它的 §8.1 故障 12 相同。

范围外：`pattern`、`format`、字符串长度等更细的约束（§3.1 的清单之外）；类型转换（`"5"` 不会变成 `5`）；生成配置表单或类型（#684）；Cordis 行由 Cordis 自己校验（§2.3），rutis 不再校验一遍。

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
| C1 | 每种语言的 SDK 在装载一行、调用 `apply` 之前做（运行时和测试工具走同一段代码）；`check` 用 rutis 的 Rust 实现做同样的事；loader 不对别的运行时的行再做一遍 | 2.1 |
| C2 | 一个 JSON Schema 子集，14 个关键字；其他关键字忽略，`check` 告警 | 3.1 |
| C3 | 多出来的键：告警并原样交给 `apply`；Schema 写了 `additionalProperties: false` 时是错误；`check --strict` 把告警当错误 | 3.2 |
| C4 | 类型化配置（Go、Swift、Kotlin、Rust）的 Schema 由类型生成，只写语言自己的解码器本来就有的规则 | 3.3 |
| C5 | 默认值按"对象在才补它的属性"逐层补；没给配置时根是 `{}` | 四 |
| C6 | 所有问题一次列出，每个一行：`<行>: config: <路径> should be …, got …`；错误码 `bad-config` | 五 |
| C7 | 配置错时这一行失败，等它的行显示为等这一行；`dev`、`run`、测试工具、`check` 输出相同 | 五 |
| C8 | 一份共同的用例表 `specs/config-schema/cases.json`，每种语言和 `check` 都逐条跑 | 七 |

### 2.1 为什么由各语言的 SDK 做

- **测试工具和真实运行时必须一样。** 测试工具里没有 loader，只有 SDK；SDK 装载一行的那段代码，运行时和测试工具共用。放在 SDK 里，测试过的配置规则就是运行时的规则。放在 loader 里，每种语言的测试工具还得各写一份，反而和运行时分成两套。
- **远程运行时自然一样。** 配置由运行它的那一端检查，不论运行时在本机、别的机器还是应用里。
- **类型化语言本来就要在这里解码。** Go、Swift、Kotlin、Rust 在交给 `apply` 之前把配置解码进类型；先按 Schema 补默认值、校验，再解码，出错文字就和其他语言一样。
- **一份规则，靠用例表保持一致**（§七），而不是靠一份代码：反正每种语言都要有一份（测试工具），用例表让 7 份和 `check` 逐字相同。
- 0.18 以前的 SDK 构建的运行时不检查；`check` 照样检查它们的行。

### 2.2 每一行经过的步骤

```text
行的 config（没有或 null 视为没给）
  → 补默认值（§四）
  → 按 Schema 校验（§3.1）；有错 → 这一行失败（§五），不调用 apply
  → 类型化语言：解码进类型；解码器还报错时，同样作为 bad-config，能给路径就给
  → apply(ctx, config)        // 补过默认值的副本，多出来的键原样保留
```

改配置（这一行重启）时同样走一遍。插件交给别人的定义（`ctx.plugin(def.at(&ctx, config))`）在定义所在的运行时里装载，也走一遍，失败记在这一份副本上，`ctx.plugin` 的调用方拿到同一个错误。

### 2.3 Cordis 行

Cordis 行（导出 `Config` 的 Cordis 插件）按 Cordis 的规则：Cordis 在装载时用 `Config` 校验，rutis 不再按转换出来的 JSON Schema 校验一遍（转换会丢掉 `transform` 等信息，两边会不一致）。rutis 只把 Cordis 的 `ValidationError` 按 §五 的格式打印（路径取自 issue 的 `path`）。`check` 不检查 Cordis 行的配置值，只打印 Schema（与现在相同）。

`definePlugin({ config })` 只接受 JSON Schema。传入 schemastery 等 Standard Schema 对象时，定义时就报错：`config of definePlugin is a JSON Schema; a schemastery Schema belongs in a Cordis plugin's exported Config`。

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
| `anyOf`、`oneOf` | 符合其中一个即可（`oneOf` 按 `anyOf` 检查）；分支里的 `default` 不补 |
| `allOf` | 每一个都要符合 |
| `$ref` | 只认本文档内的 `#/$defs/…` 与 `#/definitions/…`（schemars、手写都够用） |

- 只作说明的关键字不检查，也不告警：`$schema`、`$id`、`title`、`description`、`examples`、`deprecated`、`readOnly`、`writeOnly`、`x-` 开头的（如 `x-volatile`）。
- 其他关键字（`pattern`、`minLength`、`format`、`prefixItems`、`if` 等）不检查。`check` 对每个 Schema 告警一次：`weather: config schema: pattern is not checked by rutis; check it in the plugin`；运行时不告警。
- 不做类型转换：`"5"` 不符合 `number`。

### 3.2 多出来的键

- 对象写了 `properties`、没写 `additionalProperties`：多出来的键告警，值原样交给 `apply`（JSON Schema 的本义，旧配置不会因此起不来）。拼写接近已有的键时给出建议。
- 告警由检查它的那一方写到标准错误（运行时、测试工具、`check` 各写一次）：`weather: config: cty is not a config key (did you mean city?); it is ignored`。
- `additionalProperties: false`：同样的文字作为错误，这一行失败。
- `check --strict`：告警算错误，以 1 退出（与 `--strict` 对其他告警的处理相同）。

### 3.3 类型化配置怎样对上

生成 Schema 时只写语言自己的解码器本来就有的规则，不让开发者为 rutis 多写标注：

| 语言 | 必填 | 默认值 | 多出来的键 |
| --- | --- | --- | --- |
| Go（类型 `C`） | 不生成 `required`：没写的字段是零值，与 `encoding/json` 和 Go 插件指南相同（**改变**：以前把没有 `omitempty` 的字段列为 `required`）；要分清"没写"用指针 | 零值；手写 `Schema` 里的 `default` 照常补 | 告警 |
| Swift（`@RutisConfig`） | 可选类型（`T?`）和带字面量初始值的 `var` 不必填，其余必填 | `var units = "metric"` 的字面量成为 `default`（字符串、数字、布尔、由它们组成的数组和字典）；非字面量的初始值不算默认值，字段必填 | 告警 |
| Kotlin（`@Serializable`） | 有默认参数（`isElementOptional`）或可空的不必填 | 由 kotlinx 解码时填（`SerialDescriptor` 看不到默认值，Schema 里没有，`check` 不显示） | 告警 |
| Rust（`serde` + `schemars`） | schemars 已有的规则：`Option<T>`、`#[serde(default)]` 不必填 | schemars 写进 Schema 的 `default`；先补进 JSON，`serde(default)` 照常有效，两者是同一个值 | 告警；`#[serde(deny_unknown_fields)]` 生成 `additionalProperties: false`，是错误 |
| Rust（Rust 里做的定义，没有 Schema） | `serde` 解码 | `serde(default)` | `serde` 的规则 |

Rust 里做的定义（`Definition`，配置类型只要求 `DeserializeOwned`）没有 Schema，只走解码；解码错误也按 §五 的格式写，路径由解码时记下（`serde_path_to_error`）。

## 四、默认值

- **没给配置**（没写 `config` 或写了 `null`）：根 Schema 有 `default` 时用它；根是对象时从 `{}` 开始；都不是时按"没有值"检查（根不允许时报错）。没有 Schema 的插件照旧，收到什么就是什么。
- **逐层补**：一个对象在（包括刚开始的 `{}`），它缺的属性有 `default` 时补上，再进入补上的或原有的子对象继续补。
- **不凭空造对象**：缺的子对象自己没有 `default` 时不补，`apply` 里它就是没有；想让嵌套的默认值总在，给子对象写 `default: {}`。这是 ajv `useDefaults` 的规则，JavaScript 开发者最熟悉的那一种。
- 补进去的是副本，`default` 写的对象不会被插件改动影响到下一次。
- 补完再校验：`default` 本身不符合 Schema 是插件的错误，这一行失败，`check` 也报错：`weather: config schema: the default of units ("kelvin") should be "metric" or "imperial"`。
- 判断"配置里有没有这个键"的代码会看到默认值已经补上：这是本稿要的行为，写进迁移说明。

## 五、错误

### 5.1 文字

一个问题一行，所有问题一次列出（最多 20 个，多的写成 `and N more`）：

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
| `anyOf` / `oneOf` | `proxy should be a string or an object, got 3` |
| 不允许的键 | `cty is not a config key (did you mean city?)` |
| 解码器的错误（§2.2） | `home.lat: <解码器的原话，去掉内部类型名>` |

- 路径：`home.lat`、`cities[2]`；不是标识符的键加引号：`headers["x-api"]`。
- `got` 写 JSON，超过 40 个字符时写类型（`an object`、`an array of 12`）。
- 不出现诊断设计 §4.5 的禁用词（serde、`ValidationError`、内部类型名）。

### 5.2 结构

- 错误是 `BindingError`，错误码 `bad-config`（诊断设计 §4.3 的表加一行），`data`：`{ issues: [{ path: ["home", "lat"], message: "home.lat should be a number, got \"north\"" }] }`。`site` 是这一行、`doing: "apply"`（诊断设计 §4.1）；rutis 打印时见到 `bad-config` 写 `config:`，不写 `failed in apply:`。
- Rust：loader 的行的配置错误仍是 `ErrorKind::Validation { issues }`，`issues` 是上面的每一行文字，显示为 `config: …`（去掉 `config validation failed: [...]`）；`ReconcileReport` 的 `Failure` 跟着变文字，类型不变。

### 5.3 记在哪一行，之后怎样

- 记在配置写错的那一行（诊断设计 §4.7）：这一行失败，不调用 `apply`；依赖它的行显示 `chat: waiting for weather (row weather failed: config: units should be …)`，不显示成调用方的错误。
- 改好配置后照常重新装载（`dev` 监视配置文件时自动，`run` 下按现有的重新装载方式）。
- `dev` 与 `run` 规则相同，区别只在诊断设计 §七 的输出前缀。

## 六、`rutis-host check`

- 对每一行用它已经打印的 Schema、按同样的规则补默认值并校验（Rust 实现，与 loader 的行共用），不运行插件。
- 有错误：按现有版式写 `<行> (<插件>): cannot run: config: …`，每个问题一行，以 1 退出。告警（多出来的键、不检查的关键字）写到标准错误，以 0 退出；`--strict` 时以 1 退出。
- 检查时不知道 Schema 的行（没连上的远程运行时、`peer:` 行）写 `config not checked: its schema is known only when it runs`，不算告警。Cordis 行见 §2.3。
- `check` 的输出与运行时的错误逐字相同（同一份用例表锁住）。

## 七、用例表

`specs/config-schema/cases.json`：每条是 `{ name, schema, config, then: { config } | { errors: [...] }, warnings: [...] }`，`errors` 与 `warnings` 是 §5.1 的完整文字。每种语言的 SDK 测试（Node、Bun、Python、Go、Swift、Kotlin）、rutis-loader 与 `check` 都逐条跑，逐字比较。用例至少有：

| # | 用例 |
| --- | --- |
| 1 | 没给配置，根是对象：得到 `{}` 加默认值 |
| 2 | `null` 与没给相同 |
| 3 | 裸字符串当配置：`config should be an object, got "Oslo"` |
| 4 | 缺 `required` |
| 5 | 类型不对（字符串、数字、整数、布尔、数组、对象各一条） |
| 6 | `enum` 两个值、三个值；`const` |
| 7 | `minimum`、`maximum` 边界（等于上下限通过） |
| 8 | 嵌套对象在：补它的默认值；不在：不造对象；子对象 `default: {}`：造出来并补 |
| 9 | 数组项出错：路径 `cities[2]` |
| 10 | 字典（`additionalProperties` 是 Schema） |
| 11 | 多出来的键：告警、原样保留、给出建议 |
| 12 | `additionalProperties: false`：错误 |
| 13 | `anyOf` / `oneOf`；`["string", "null"]` |
| 14 | `$ref` 到 `$defs` 与 `definitions` |
| 15 | 不检查的关键字（`pattern`）：值照常通过 |
| 16 | 多个问题一次列出；超过 20 个截断 |
| 17 | `default` 本身不符合 Schema |
| 18 | 非标识符的键的路径；很长的 `got` |
| 19 | 默认值是副本：插件改了它，下一次装载不受影响 |

类型化语言另有自己的用例（Go 的零值与指针、Swift 的字面量初始值、Kotlin 的默认参数、Rust 的 `serde(default)` 与 `deny_unknown_fields`），各在自己的测试里。

## 八、开发者看到什么

`rutis.json` 里 `weather` 的配置写错时（`dev`，TypeScript 插件；其他语言输出相同）：

```text
# 值不在 enum 里
weather: config: units should be "metric" or "imperial", got "kelvin"

# 裸字符串
weather: config: config should be an object, got "Oslo"

# 缺必填、类型不对，一次列出
weather: config: city is required
weather: config: days should be an integer, got 1.5
weather: config: home.lat should be a number, got "north"

# 拼错键（告警，这一行照常起来，city 用默认值）
[weather] weather: config: cty is not a config key (did you mean city?); it is ignored

# 依赖它的行
chat: waiting for weather (row weather failed: config: units should be "metric" or "imperial", got "kelvin")

# 插件写的 default 本身不对
weather: config schema: the default of units ("kelvin") should be "metric" or "imperial"
```

`rutis-host check`：

```text
weather (weather): cannot run: config: units should be "metric" or "imperial", got "kelvin"
rutis-host: warning: weather: config: cty is not a config key (did you mean city?); it is ignored
rutis-host: warning: weather: config schema: pattern is not checked by rutis; check it in the plugin
rutis-host: 1 row(s) or binaries cannot run
$ echo $?
1
```

（沿用 `check` 现有的版式 `<行> (<插件>): cannot run: …`；一行有几个问题时每个问题一行。）

测试工具（以 Node 为例）：`load(plugin, { units: 'kelvin' })` 失败，抛出的 `BindingError` 的 `code` 是 `bad-config`，消息是 `config: units should be "metric" or "imperial", got "kelvin"`；Go 的 `rutistest.Load` 返回同样的错误（`errors.As` 得到 `*rutis.BindingError`）。

## 九、兼容与迁移

写进《迁移到 0.18》（两份）：

- **以前写错也能运行的配置现在让这一行失败**：类型不对、缺 `required` 的字段、裸字符串当配置、`additionalProperties: false` 下多出来的键。**升级前先用 0.18 的 `rutis-host check` 跑一遍**，它列出每一行不合格的配置。
- **默认值现在会补上**：判断"配置里有没有这个键"的插件代码会看到默认值；为默认值写的兜底可以去掉，也可以留着。
- **Go**：生成的 Schema 不再有 `required`（与 Go 的零值一致）；手写的 `Schema` 里的 `default` 现在会补上（Go 插件指南"不会替你填默认值"一句改写）。
- **Swift**：带字面量初始值的 `var` 字段不再必填，没写时用初始值。
- **Rust**：配置错误的文字从 `config validation failed: [...]` 变为 `config: <路径> …`；`ErrorKind::Validation`、`ReconcileReport` 类型不变。
- **Node、Bun**：`definePlugin` 的 `config` 传 Standard Schema 对象时报错。
- **多出来的键**：只告警；`check --strict` 下是错误。
- 发布说明写明：这是兑现文档早就写着的行为（插件 API 指南的"宿主据此校验"）。

## 十、实施

| 步 | PR 内容 |
| --- | --- |
| 1 | 用例表 `specs/config-schema/`；Rust 实现（rutis-loader 的 `config` 模块）：`Builtins::register` / `register_with` / `register_raw` 的行、`Definition` 的解码路径；`ErrorKind::Validation` 的文字；`check` 的检查、告警与退出码 |
| 2 | Node 与 Bun：`definePlugin` 的行与测试工具；Cordis `ValidationError` 按 §5.1 打印；拒绝 Standard Schema 对象 |
| 3 | Python：模块行、`define_plugin` 的行与测试工具 |
| 4 | Go：校验后再解码；生成的 Schema 去掉 `required`；`rutistest` |
| 5 | Swift 与 Kotlin：`@RutisConfig` 的字面量默认值；两者的运行时、应用里的运行与测试工具 |
| 6 | 文档：插件 API 指南、rutis-host 指南"检查配置"、各语言指南（撤回 #714 写的"目前不检查"）、《迁移到 0.18》、`docs/guarantees.md` 新条目"配置在 `apply` 之前补默认值并校验"（写明验证它的用例表） |

每一步合并时，它那一列的用例全部通过；#669 在第 6 步合并、7 列都通过后关闭。
