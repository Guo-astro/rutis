# rutis-macros

The procedural macros of [rutis](https://github.com/arcships/rutis), used through the `rutis` crate:

```rust
use rutis::service;

#[service]
impl Calendar {
    pub async fn add(&self, entry: NewEntry) -> Result<Entry, CalendarError> { /* … */ }
}
```

A plugin provides it as usual (`ctx.provide(Calendar::new())?`), and the rows of every language call it by name. See [Write a Rust plugin](https://github.com/arcships/rutis/blob/main/docs/guide/rust-plugin.en.md) and the Rust host guide, ["Provide Rust services to plugins"](https://github.com/arcships/rutis/blob/main/docs/guide/rust-host.en.md).
