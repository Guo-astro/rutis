# Go：同一进程里拿到提供方的对象本身

[English](design-go-local-objects-2026-11-01.en.md)

状态：实现中（分支 `claude/same-process`，未合并）。日期：2026-11-01。§一 描述的代码是本稿所在提交的父提交里的。
Issue：尚无，随本稿开。相关：`sdk/optional-deps`（`docs/guarantees.md`）、[可选依赖](design-optional-deps-2026-10-28.md) §4.5、[对象引用](design-object-refs-2026-10-27.md)。

## 一、现状

`docs/guarantees.md` 的 `sdk/optional-deps` 写着六种语言一致：“同一进程里 `get`、`use` 拿到的是提供方的对象本身，不经句柄。”[插件 API](guide/plugin-api.md) 的 `ctx.use` 一行也这样写。Rust 的 `ctx.require::<T>()` 返回 `Arc<T>`，就是提供方的那个对象；Node、Bun、Python 的 `use` 返回对象本身；Kotlin 的 `use<I>()` 在本进程的对象实现了 `I` 时直接 `cast` 返回；Swift 的 `Ctx.proxy` 在本进程的对象符合协议时直接返回它。

Go 做不到。`ctx.Use(name, &target)` 只认两种目标：

```
Service.bind(target)                        go/rutis/internal/runtime/runtime.go
├─ ① target 是 *advanced.Service / advanced.Service → 填进去（本进程时 Local() 为 true）
├─ ② 不是结构体 → "use %s: the target must point to a struct of function fields or a *advanced.Service, not %s"
└─ ③ 结构体的每个函数字段
   ├─ CheckSignature(field.Type, true)       → 必须以 error 结尾，最多一个其他返回值
   ├─ 本进程、方法的类型与字段完全相同 → 字段 = 提供方的方法值（不经转换）
   └─ 否则 MakeFunc：参数按跨进程的规则转换后调用
```

在本仓库的 Go 运行时上实测（`newRuntime` 里装两行，一行 `Provide`，一行 `Use`）：

| 写法 | 结果 |
| --- | --- |
| `var g Greeter; ctx.Use("hello", &g)`，`Greeter` 是接口，提供方的对象实现了它 | `use hello: the target must point to a struct of function fields or a *advanced.Service, not rutis.Greeter` |
| `var x any; ctx.Use("hello", &x)` | 同上一句，`not interface {}` |
| `struct{ Draw func(w string) string }`，提供方的方法正是 `Draw(w string) string` | `use canvas: field Draw: func(string) string must return an error last` |
| `var s *advanced.Service` | 成功，`s.Local()` 为 `true`，`s.Call` 经参数转换 |

第三行最说明问题：`bind` 已经有“本进程、类型相同就直接给方法值”这条路，但它排在 `CheckSignature` 之后，跨进程的签名规则先把它挡住了。

于是同一个 Go 进程里的两个插件，想用 Go 接口互相调用（方法带 `func(*ui.Context)` 参数、返回 `(Doc, bool)`），只能绕开 rutis 用包级全局变量传对象。

## 二、决策

| # | 决策 | 为什么；放弃的做法 |
| --- | --- | --- |
| D1 | 目标的类型能装下本进程提供方的对象（`reflect.TypeOf(对象).AssignableTo(目标类型)`：它实现的接口、它自己的类型、`any`）时，目标就是这个对象 | 与 `sdk/optional-deps` 写下的六家规则、Rust 的 `Arc<T>` 相同。判断用 Go 自己的赋值规则，不另立规则。这一步排在函数字段之前：提供方对象的类型恰好是一个结构体、目标也是它时，给的是对象本身（是数据的副本还是指针，由提供方 `Provide` 的是值还是指针决定，与 Go 赋值相同） |
| D2 | `any` 目标、提供方不在本进程：`*advanced.Service` | 别的进程交来的对象以 `any` 收下时就是 `*advanced.Service`（[对象引用](design-object-refs-2026-10-27.md)，`ObjectFromPeer`），这里取同一个答案。放弃“报错”：`any` 是“给我它本身”，本进程外它本身就是这个代理 |
| D3 | 接口目标、提供方在本进程但对象没实现它：报错，写出缺的或类型不同的那个方法 | 写成 Go 编译器的说法（`missing method M`、`wrong type for method M`），插件作者一眼能对上。放弃回退到代理：接口是 Go 的类型约束，对象不满足时调用方的代码本来就不该运行 |
| D4 | 接口目标（`any` 以外）、提供方不在本进程：报错，说只有本进程的服务能这样用，另一进程的用函数字段的结构体或 `*advanced.Service` | 另一进程的服务只有方法表，没有 Go 类型，造不出一个实现任意接口的值（Go 的 `reflect` 不能在运行时生成接口实现）。放弃“生成代理”：做不到，且 Swift、Kotlin 的代理都要编译期生成（`@RutisClient`） |
| D5 | 函数字段的结构体、提供方在本进程：字段的类型与方法完全相同就直接给方法值，不做跨进程的签名检查；其余字段照旧 | 方法值在本进程里调用，跨进程的规则（以 error 结尾、参数能编码）不适用。第 ③ 步的两个分支只是换个先后 |
| D6 | 提供一侧不变：不写 `Provides` 的 `Provide` 照旧不查签名；`MethodsOf` 照旧拒绝返回两个以上非 error 值的方法 | `MethodsOf` 是对别的进程宣布的方法表，Rust 对服务方法同样要求能跨语言（不能时编译失败，`sdk/rust-compile-errors`）。放弃“跳过不能跨进程的方法、静默宣布其余的”：宣布的方法表少了一个，别的进程看到的是 `has no method`，不知道原因。只在本进程用的服务不写进 `Provides` |
| D7 | 生命周期：对象本身没有句柄，与另外五家相同 | `Inject` 里的服务，提供方离开时 rutis 先停用它的行（`loader/rows-gate-on-services`；`multilang_go.rs::a_go_provider_that_goes_stops_its_users_first` 里 `gl` 就是用同一进程另一行服务的 Go 行），所以 `Use` 拿到的对象不会在提供方停下后被调用。`Optional` 里的服务不门控：`Get` 得到的对象在提供方离开后仍在变量里，与 `sdk/optional-deps` 写下的一样；需要当前状态时再 `Get` 一次 |

## 三、开发者看到什么

### 插件作者

同一个进程里，一行提供编辑器的工作区，另一行用 Go 接口拿它：

```go
package editor

type Workspace interface {
	Open(path string) (Doc, bool)
	OnDraw(draw func(c *ui.Context))
}

var Files = rutis.Define(rutis.Plugin[rutis.NoConfig]{
	Name: "files",
	Apply: func(ctx *rutis.Ctx, _ rutis.NoConfig) error {
		// 不写进 Provides：这个服务只在本进程用（D6）
		ctx.Provide("workspace", newFiles())
		return nil
	},
})

var Tabs = rutis.Define(rutis.Plugin[rutis.NoConfig]{
	Name:   "tabs",
	Inject: []string{"workspace"},
	Apply: func(ctx *rutis.Ctx, _ rutis.NoConfig) error {
		var ws Workspace
		if err := ctx.Use("workspace", &ws); err != nil {
			return err
		}
		doc, ok := ws.Open("README.md")
		ws.OnDraw(func(c *ui.Context) { c.Text(doc.Title) })
		_ = ok
		return nil
	},
})
```

正常时：`ws` 就是 `files` 那一行 `Provide` 的 `*files`，调用是普通的 Go 方法调用，参数和返回值不经编码。`var x any` 也拿到它。`Optional` + `ctx.Get` 同样。

出错时（逐字）：

| 怎样触发 | 输出 | 怎样改 |
| --- | --- | --- |
| 接口目标，`workspace` 由别的进程（另一种语言、另一个 Go 进程、宿主）提供 | `use workspace: editor.Workspace is an interface, which only a service provided in this process fills: for one of another process, use a struct of function fields or *advanced.Service` | 用函数字段的结构体，或 `*advanced.Service` |
| 接口目标，本进程的对象少一个方法 | `use workspace: *editor.files does not implement editor.Workspace (missing method OnDraw)` | 给 `*editor.files` 补上方法，或改接口 |
| 接口目标，本进程的对象有这个方法但签名不同 | `use workspace: *editor.files does not implement editor.Workspace (wrong type for method Open)` | 让两边签名一致 |
| 函数字段的类型与本进程的方法不同，又没有以 error 结尾 | `use canvas: field Draw: func(string) string must return an error last`（不变） | 字段写成方法的类型，或写成跨进程的形状 |

这些错误是 `Use` / `Get` 返回的 `error`，不带错误码（与 `bind` 已有的几句相同），不写到 stderr。

### 测试工具

`rutistest.Load` 给的服务装在同一个运行时里（`rutistest` 包文档），插件以接口拿到的就是测试给的那个对象。测试因此发现不了“提供方换到别的进程后接口目标报错”这一类问题，与 [插件 API](guide/plugin-api.md) 里“测试给的服务与插件在同一个运行时里”那段写下的限制相同。

### 要懂的概念

不新增概念。Go 插件作者已知“同一进程是对象本身、别的进程是代理”（插件 API），本稿让 Go 的写法跟上这句话。

## 四、兼容

- 只放宽：以前报错的三种目标（接口、`any`、本进程方法类型相同但不以 error 结尾的函数字段）现在成功；以前成功的写法结果不变。
- 协议、宣布的方法表、`MethodsOf` 都不变。
- 一个插件以接口目标 `Use` 的服务，部署时如果被挪到别的进程，从“能用”变成 D4 的那句报错。这是写明的限制：接口目标只在同一进程成立。

## 五、实施

| # | 位置 | 改动 | 用例 |
| --- | --- | --- | --- |
| 1 | `go/rutis/internal/runtime/runtime.go` 的 `Service.bind` | D1–D5：`*advanced.Service` 之后先看本进程的对象能否赋给目标；接口目标的两句错误；函数字段先试本进程的方法值，再做签名检查 | `go/rutis/internal/runtime/local_objects_test.go`：接口拿到同一个指针、方法带函数参数与两个返回值；`any`（本进程是对象，宿主的是 `*advanced.Service`）；缺方法、方法类型不同的两句逐字；宿主服务的接口目标那句逐字；不以 error 结尾、类型相同的函数字段；`Get` 同样 |
| 2 | `go/rutis/rutistest` | 无改动 | `rutistest` 里一条：测试给的服务以接口拿到，是测试给的那个对象 |
| 3 | 文档 | `docs/guide/go-plugin.md`、`docs/guide/plugin-api.md` 的 Go 一段写上接口与 `any` 目标；`docs/guarantees.md` 的 `sdk/optional-deps` 列上 Go 的用例 | — |

## 六、后续

- Swift：`Ctx.proxy` 已经先给本进程的对象，但 `use<P: RutisProxy>` 要求协议标 `@RutisClient`（方法必须 `async throws`），`provide` 要求 `@RutisService`（方法能跨进程）。一个只在本进程用、方法是同步的或带闭包参数的协议，两头都写不出来。要加一个不要求宏的 `use` 重载，会碰到 `@RutisClient` 为每个协议生成的重载的歧义，并改公开 API 的快照，另写一稿。
- Kotlin：`use` 的顺序已由 #1006 改好；`provide` 一个不是 `@RutisService` 的对象仍然失败（找不到 KSP 生成的表），只在本进程用的服务因此也要标注。另写一稿。
