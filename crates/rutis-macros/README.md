# rutis-macros

The procedural macros of [rutis](https://github.com/arcships/rutis), used through the `rutis` crate:

```rust
use rutis::service;

#[service]
impl Calendar {
    pub async fn add(&self, entry: NewEntry) -> Result<Entry, CalendarError> { /* … */ }
}
```

See the Rust host guide, "Provide Rust services to plugins".
