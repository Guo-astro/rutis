# Go: a provider's own object, inside one process

[中文](design-go-local-objects-2026-11-01.md)

Status: being implemented (branch `claude/same-process`, not merged). Date: 2026-11-01. The code §1 describes is the one in this design's parent commit.
Issue: none yet; opened with this design. Related: `sdk/optional-deps` (`docs/guarantees.md`), [optional dependencies](design-optional-deps-2026-10-28.en.md) §4.5, [object references](design-object-refs-2026-10-27.en.md).

## 1. Today

`docs/guarantees.md`'s `sdk/optional-deps` says, for all six languages, "in the same process `get` and `use` give the provider's object itself, not a handle." [The plugin API](guide/plugin-api.en.md) says the same on its `ctx.use` row. Rust's `ctx.require::<T>()` gives `Arc<T>`, that very object; Node, Bun and Python's `use` give the object; Kotlin's `use<I>()` returns the object with a `cast` when the provider in this process implements `I`; Swift's `Ctx.proxy` returns it when the object conforms to the protocol.

Go cannot. `ctx.Use(name, &target)` accepts two kinds of target:

```
Service.bind(target)                        go/rutis/internal/runtime/runtime.go
├─ ① target is *advanced.Service / advanced.Service → filled (Local() is true in this process)
├─ ② not a struct → "use %s: the target must point to a struct of function fields or a *advanced.Service, not %s"
└─ ③ each function field of the struct
   ├─ CheckSignature(field.Type, true)       → an error last, at most one other result
   ├─ in this process and the method's type is the field's → the field is the provider's method value (no conversion)
   └─ otherwise MakeFunc: the call converts its arguments as a crossing one does
```

Measured on this repository's Go runtime (two rows in `newRuntime`, one `Provide`, one `Use`):

| What is written | Result |
| --- | --- |
| `var g Greeter; ctx.Use("hello", &g)`, `Greeter` an interface the provider implements | `use hello: the target must point to a struct of function fields or a *advanced.Service, not rutis.Greeter` |
| `var x any; ctx.Use("hello", &x)` | the same line, `not interface {}` |
| `struct{ Draw func(w string) string }`, the provider's method is exactly that | `use canvas: field Draw: func(string) string must return an error last` |
| `var s *advanced.Service` | works, `s.Local()` is `true`, `s.Call` converts its arguments |

The third line is the telling one: `bind` already has a "same process, same type: give the method value" path, but it sits behind `CheckSignature`, and the rule for crossing a process gets there first.

So two Go plugins of one process that want to call each other through a Go interface (methods taking `func(*ui.Context)`, returning `(Doc, bool)`) have to pass the object through a package-level variable and skip rutis.

## 2. Decisions

| # | Decision | Why; what was dropped |
| --- | --- | --- |
| D1 | When the target's type can hold the provider's object (`reflect.TypeOf(object).AssignableTo(target type)`: an interface it implements, its own type, `any`), the target is that object | The rule `sdk/optional-deps` writes for all six, and Rust's `Arc<T>`. The test is Go's own assignment rule, not a new one. It comes before function fields: when the provider's object is itself a struct and so is the target, the target gets the object (by value or pointer as `Provide` gave it, as in Go) |
| D2 | An `any` target with the provider in this process is the object; for a service of another process it is `*advanced.Service` | An object of another process received as `any` is a `*advanced.Service` ([object references](design-object-refs-2026-10-27.en.md), `ObjectFromPeer`); this is the same answer. Dropped: an error — `any` asks for the object itself, and outside this process that is this proxy |
| D3 | An interface target, the provider in this process but its object not implementing it: an error naming the method it lacks or has with another type | Go's own compiler words (`missing method M`, `wrong type for method M`), so the plugin author matches them at a glance. Dropped: falling back to the proxy — an interface is Go's type constraint, and a caller whose type does not satisfy it should not run |
| D4 | An interface target (other than `any`) with the provider in another process: an error saying only a service provided in this process fills it, and that for another process one writes a struct of function fields or `*advanced.Service` | A service of another process has methods, not a Go type, and nothing can make a value implementing an arbitrary interface for it (`reflect` cannot generate an interface implementation at run time). Dropped: generating a proxy — it cannot be done, and Swift's and Kotlin's proxies are generated at compile time (`@RutisClient`) |
| D5 | A struct of function fields, the provider in this process: a field whose type is exactly the method's takes the method value, before any crossing signature check; other fields as before | That method runs in this process, where the crossing rules (an error last, arguments that encode) do not apply. The two branches of ③ only swap places |
| D6 | The providing side is unchanged: a `Provide` without `Provides` still checks no signature, and `MethodsOf` still refuses a method with more than one non-error result | `MethodsOf` is the table announced to other processes, and Rust requires a service's methods to cross too (a compile error when they cannot, `sdk/rust-compile-errors`). Dropped: skipping the methods that cannot cross and announcing the rest silently — the announced table would lack one, and another process would see `has no method` without knowing why. A service used only in this process is not written into `Provides` |
| D7 | Lifetime: the object carries no handle, as in the other five | For a service in `Inject`, rutis stops its user's row when the provider leaves (`loader/rows-gate-on-services`; `gl` in `multilang_go.rs::a_go_provider_that_goes_stops_its_users_first` is a Go row using another row's service in the same process), so the object `Use` gave is not called after the provider stops. A service in `Optional` gates nothing: the object `Get` gave stays in the variable after the provider leaves, as `sdk/optional-deps` says; read it again when its current state matters |

## 3. What developers see

### Plugin authors

In one process, one row provides the editor's workspace and another takes it through a Go interface:

```go
package editor

type Workspace interface {
	Open(path string) (Doc, bool)
	OnDraw(draw func(c *ui.Context))
}

var Files = rutis.Define(rutis.Plugin[rutis.NoConfig]{
	Name: "files",
	Apply: func(ctx *rutis.Ctx, _ rutis.NoConfig) error {
		// Not in Provides: this service is used in this process only (D6)
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

Normally: `ws` is the `*files` that `files` provided, and its calls are plain Go calls; arguments and results are not encoded. `var x any` gets it too, and so does `ctx.Get` with `Optional`.

When it goes wrong (verbatim):

| How it is triggered | Output | How to fix it |
| --- | --- | --- |
| An interface target, `workspace` provided by another process (another language, another Go process, the host) | `use workspace: editor.Workspace is an interface, which only a service provided in this process fills: for one of another process, use a struct of function fields or *advanced.Service` | Use a struct of function fields, or `*advanced.Service` |
| An interface target, this process's object lacks a method | `use workspace: *editor.files does not implement editor.Workspace (missing method OnDraw)` | Add the method to `*editor.files`, or change the interface |
| An interface target, the object has the method with another signature | `use workspace: *editor.files does not implement editor.Workspace (wrong type for method Open)` | Make the two signatures the same |
| An interface target, `Provide` given a value whose methods are on its pointer | `use workspace: editor.files does not implement editor.Workspace (method OnDraw has pointer receiver)` | Provide a pointer (`newFiles()` already is one), or put the method on the value |
| A function field whose type differs from this process's method and does not end with an error | `use canvas: field Draw: func(string) string must return an error last` (unchanged) | Write the field as the method's type, or in the shape that crosses |

These are the `error` `Use` / `Get` return, without an error code (like the other lines `bind` writes), and they are not written to stderr.

### The test tool

`rutistest.Load` provides its services in the same runtime (the `rutistest` package documentation), so a plugin taking one through an interface gets the object the test gave. Tests therefore cannot catch "the provider moved to another process and the interface target refuses" — the limit [the plugin API](guide/plugin-api.en.md) states for "the test's services and the plugin are in one runtime".

### Concepts to learn

None new. A Go plugin author already knows "the same process gives the object, another process gives a proxy" (plugin API); this design makes Go's spelling of that sentence true.

## 4. Compatibility

- Only looser: the three targets that failed before (an interface, `any`, and a function field matching this process's method type without an error) now work; nothing that worked changes.
- The protocol, the announced method tables and `MethodsOf` do not change.
- A plugin that `Use`s a service through an interface breaks (with D4's line) when that service is moved to another process by the deployment. That is the stated limit: an interface target holds in one process.

## 5. Implementation

| # | Where | Change | Cases |
| --- | --- | --- | --- |
| 1 | `Service.bind` in `go/rutis/internal/runtime/runtime.go` | D1–D5: after `*advanced.Service`, the object of this process when the target can hold it; the two interface errors; a function field takes this process's method before the signature check | `go/rutis/internal/runtime/local_objects_test.go`: an interface gets the same pointer, with methods taking a function and returning two values; `any` (the object here, `*advanced.Service` from the host); the two lines for a missing method and a different one, word for word; the line for a host service's interface target, word for word; a function field of the method's type without an error; `Get` the same |
| 2 | `go/rutis/rutistest` | nothing | one case in `rutistest`: the test's service taken through an interface is the object the test gave |
| 3 | Documentation | `docs/guide/go-plugin.md`, the Go part of `docs/guide/plugin-api.md`; the Go cases listed in `docs/guarantees.md`'s `sdk/optional-deps` | — |

## 6. Later

- Swift: `Ctx.proxy` already gives this process's object first, but `use<P: RutisProxy>` requires the protocol to be marked `@RutisClient` (methods `async throws`) and `provide` requires `@RutisService` (methods that cross). A protocol used only in this process, with synchronous methods or closure arguments, cannot be written on either side. Adding a `use` overload that asks for no macro meets the ambiguity of the overload `@RutisClient` generates for each protocol, and changes the public API snapshot: a design of its own.
- Kotlin: #1006 fixed the order in `use`; `provide` of an object that is not a `@RutisService` still fails (no KSP-generated table), so a service used only in this process must still be marked. A design of its own.
