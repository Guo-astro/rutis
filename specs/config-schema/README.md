# 配置 Schema 的共用用例

[English](README.en.md)

[配置 Schema 设计](../../docs/design-config-schema-2026-10-27.md) §8 的用例表：补默认值、校验与错误文字的规则，每种语言的 SDK（Node、Bun、Python、Go、Swift、Kotlin）、rutis-loader 和 `rutis-host check` 都逐条跑 [`cases.json`](cases.json)，逐字比较。这不是 TLA+ 模型，不在 CI 的 `specs` 任务里跑，由各实现自己的测试跑。

## 格式

```json
{ "cases": [ { "name": "…", "schema": { … }, "config": …, "then": { "config": … } | { "errors": ["config: …"] }, "warnings": ["config: …"] } ] }
```

- `config` 不写表示没有配置（和 `null` 一样）。
- `then.config`：通过，`apply` 拿到的值（补好默认值；通过 `integer` 的 `1.0` 写成整数 `1`）。按 JSON 相等比较；区分整数和小数的语言（Rust、Go、Python）还要比较是不是整数。
- `then.errors`：不通过，错误的每一行，也就是标记的配置错误的文字去掉行名（设计 §5.1：按路径排好，最多 20 行，余下的写成 `config: and N more problems`）。
- `warnings`：多余的键，同样的句子（测试工具和 loader 给出的警告）。

## 各实现怎么跑

- 每个用例跑一遍，比较 `then` 和 `warnings`。
- 通过的用例再跑一遍：先把第一次得到的配置里每个对象、数组原地改掉（加一个键、加一项），第二次的结果仍然等于 `then.config`（补上的默认值是复制，设计 §4、用例 21）。
- 这里只有各语言共用的规则。类型化语言自己的用例（Go 的零值与指针、Swift 的字面量初值、Kotlin 的默认参数、Rust 的 `serde(default)` 与 `deny_unknown_fields`、Rust `Definition` 的解码错误）、旧版 Go 清单忽略 `required`（设计 §7），都写在各自的测试里。

## 实现时定下的细节

设计没有逐字写到、为了 7 份实现一致而定下的，都有用例：

- 根是不是对象、根的 `default`、属性的 `default`，都顺着 `$ref` 和 `allOf` 找（先自己，再 `$ref`，再 `allOf` 成员，依次第一个）；schemars 生成的根常常只有一个 `$ref`。
- 默认值也补进数组的每一项和字典（`additionalProperties` 是 Schema）的每个值里的对象。
- 一个 Schema 「顺着 `$ref` 和 `allOf`」得到的各部分按深度优先的先序排：自己，然后它的 `$ref`（连同那里的 `$ref`、`allOf`），然后各 `allOf` 成员（各自同样展开），每个只算一次。
- 同一路径上的问题：先是类型（类型错就只报类型，任何一部分的 `type` 不符都算），然后按上面的顺序逐部分，每部分按 `enum` / `const`、上下界、`anyOf` / `oneOf` 的顺序；完全相同的问题只报一次。
- `additionalProperties` 取各部分里第一个写了的；没写时，多余的键在某一部分有 `properties` 时警告；写成 `true` 或 `{}` 时不警告。
- 在同一个值上，每个 `anyOf` / `oneOf` 分支（按它在 Schema 文档里写在哪里区分，即它的 JSON Pointer；用对象本身的身份也一样；`true` / `false` 分支不会递归，不用区分）只试一次：正在试时经 `$ref` 又走到它，算不匹配；试过的再遇到，用上次的结果（通过时连同整数改写后的值）。进到子键、数组项时重新算。这样自引用和互相引用的分支既不会无限递归，也不会指数级变慢。
- `$ref` 按 JSON Pointer 走，遇到数组按下标走（`#/$defs/a/anyOf/0`，下标不带前导零）；解析不了的 `$ref`（不是 `#/$defs/…`、`#/definitions/…`，或找不到）忽略，当作没写。
- 名为 `__proto__` 之类的键是普通的键。
- 路径里的键不截断（只有 `got` 的字符串超过 40 个才截断）。
- 根的路径写成 `config`；第一段不是标识符的键或下标时前面也写 `config`（`config["1st"]`）。
- `anyOf` / `oneOf` 的分支按 `const`、`enum`、`type` 的先后取第一个有的。
- `type` 里不认识的名字忽略；`items` 是数组（旧的元组写法）时不检查。
