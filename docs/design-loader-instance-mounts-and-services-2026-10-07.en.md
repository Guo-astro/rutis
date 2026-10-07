# Loading Multiple Ordinary Plugins from One Configuration

[中文](design-loader-instance-mounts-and-services-2026-10-07.md)

Design proposal · Related to [#158](https://github.com/arcships/rutis/issues/158) and [#159](https://github.com/arcships/rutis/issues/159) · Code baseline `330ff51`

## 1. Basic Relationship

One configuration can load plugins beneath multiple host instances. Each expanded item is an ordinary plugin in the existing loader runtime table, using existing plugin management and lifecycle behavior.

```text
Configuration: assistant
      |
      +-- Document A ctx --> assistant@A --> ordinary plugin
      |
      +-- Document B ctx --> assistant@B --> ordinary plugin
```

Configuration retains the module name. Runtime records retain the relationship between the configuration row, host, and actual plugin.

## 2. Loading

```yaml
- id: assistant
  mount: document
  name: assistant-plugin
  inject: [document]
```

After creating a document, the host supplies its loading context through `register_mount("document", ctx)`. The loader expands matching configuration and loads plugins beneath that ctx. `mount` can appear on a top-level row or group and is inherited by descendants; groups and descendants retain their parent-child relationships.

The host ctx shares the loader's root. Mount records use the host instance identity and existing loading generation. Registering the same live mount twice returns an error.

Loading has two steps:

1. Generate a runtime identifier for the configuration row under this host and record its source.
2. Generate an instance runtime name, pass it to the existing `resolve(name)`, obtain the factory and dependencies, and follow the ordinary plugin loading path.

```text
assistant-plugin + host A --> resolve(runtime name A) --> factory A
assistant-plugin + host B --> resolve(runtime name B) --> factory B
```

The Resolver extracts the original module name and host information. Module lookup uses the original module name; resolution caches use the full runtime name. A factory can capture host information and return the corresponding instance dependencies.

`@A` and `@B` are display examples. Internal names use an unambiguous encoding containing module and mount identity, preserving special characters in the original module name.

## 3. Service Mapping

Services use existing registration mechanisms with a global or instance scope selection; global is the default. Rust type information and cross-language method descriptions are registered as before.

Instance services use existing keys:

```text
document@A --> host_key("document").with_instance(A)
document@B --> host_key("document").with_instance(B)
```

Plugins continue to access `document`. Before loading, the host determines the actual service keys used by dependencies, isolation, expressions, and cross-language access. Global services remain shared; instance services use the effective isolation location of their context.

Cross-language loading passes a name mapping through `rows.load`:

```text
names = { plugin-local service name: internal service name }
internal service name = encode(service name, owner, effective isolation location)
```

- Node uses `isolate(name, symbol)` beneath the existing Cordis root to map local names to service locations.
- Python looks up and provides services through the current row's mapping.
- The mapping covers declared instance services, allowing internal child plugins to continue using these local names.
- Bridge host registration, exports, notifications, and withdrawal use internal service names in existing string tables and reference counts.
- In-process native access retains original objects; cross-language calls use existing proxies and object handles.

## 4. Management and Unloading

Management operations target the expanded ordinary plugins. Configuration-row operations first select all corresponding plugins and then use existing processing paths. Source relationships select the targets.

| Operation | Target and handling |
| --- | --- |
| Edit a configuration row | All plugins expanded from that row, using existing validation, update, and recovery paths |
| Query or manage one running plugin | Use ordinary plugin operations with its runtime identifier |
| Plugin startup, dependency waiting, failure, or stopping | Use ordinary plugin states and lifecycle behavior, targeting that running plugin |
| Unload, disable, or delete a configuration row | Unload all plugins expanded from that row, including descendants for groups |
| Close a host or revoke its mount | Unload the plugins loaded by the loader beneath that host |
| Unload the loader | Unload all managed plugins, including those beneath external hosts |

Runtime plugin identifiers are separate from template identifiers: individual lifecycle handling targets the plugin, while configuration operations target the template and its expanded results.

Unloading follows ordinary plugin cleanup and waits for the corresponding cleanup to finish. Configuration-to-plugin and host-to-plugin relationships are maintained during loading and unloading.

## 5. Implementation Areas and Acceptance

| Area | Change |
| --- | --- |
| Loader | Accept host ctx, expand configuration into the existing runtime table, and select management and unloading targets by source |
| Resolver | Decode runtime names, locate code by original module name, construct factories with host information, and use consistent names for caching and refresh |
| Service registration and lookup | Compute actual keys from scope and owner using existing instance keys and isolation queries |
| Bridge, Node, Python | Pass name mappings at load time and use mapped names in existing service indexes |

Acceptance uses ordinary plugin behavior as the baseline:

- Two hosts each load a plugin with independent construction and instance service access; global services remain shared.
- Expanded plugins behave like directly loaded plugins during startup, updates, failures, stopping, and restarting.
- Editing a configuration row affects all expanded results; unloading it cleans up all results.
- Closing one host cleans up only its plugins; unloading the loader cleans up all managed plugins.
- Rust, Node, and Python agree on instance service mappings while preserving native Node objects and child-plugin behavior.
- Old objects and cleanup actions cannot affect newly loaded plugins after a reload.

Implementation references: [runtime tables](../crates/rutis-loader/src/loader/mod.rs#L228), [loading and cleanup](../crates/rutis-loader/src/loader/reconcile.rs#L64), [update flow](../crates/rutis-loader/src/loader/commit.rs#L85), [Resolver](../crates/rutis-loader/src/resolver.rs#L15), [instance service keys](../crates/rutis/src/key.rs#L166), and [cross-language loading](../crates/rutis-bridge/src/runtime/process.rs#L570).

Status: design proposal, pending implementation and validation.
