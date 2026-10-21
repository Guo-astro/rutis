//! Release-only integration fixture. tools/test-dylib.sh packages and runs it.
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
mod imp {
    use rutis::{BoxFuture, CordisError, Ctx, Effect, Plugin, Snapshot, TypeKey};
    use rutis_dylib::{DylibConfig, Loader};
    use std::sync::{Arc, Mutex};

    struct Consumer {
        log: Arc<Mutex<Vec<String>>>,
        injects: Vec<TypeKey>,
    }
    impl Plugin for Consumer {
        fn name(&self) -> &str {
            "consumer"
        }
        fn injects(&self) -> &[TypeKey] {
            &self.injects
        }
        fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
            Box::pin(async move {
                self.log
                    .lock()
                    .unwrap()
                    .push((*ctx.require::<String>().map_err(CordisError::from)?).clone());
                Ok(Effect::Done)
            })
        }
    }

    /// What the plugin's `#[rutis::service]` `Greeting` answers by name,
    /// as a row of another language calls it.
    async fn greeting(ctx: &Ctx) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        use rutis_bridge::session::{host_key, settle, HostDispatch, Value};
        let service = ctx
            .get_as::<dyn HostDispatch>(host_key("greeting"))
            .ok_or("the plugin's Greeting is not served by name")?;
        match settle(service.invoke("text", Value::List(Vec::new()))?).await? {
            Value::Data(rutis_sdk::serde_json::Value::String(text)) => Ok(text),
            other => Err(format!("unexpected {other:?}").into()),
        }
    }

    /// A consumer of `greeting` by name, gated on it as a row of another
    /// language is: it reloads when the plugin's version changes.
    struct ByName {
        log: Arc<Mutex<Vec<String>>>,
        injects: Vec<TypeKey>,
    }
    impl Plugin for ByName {
        fn name(&self) -> &str {
            "by-name"
        }
        fn injects(&self) -> &[TypeKey] {
            &self.injects
        }
        fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
            Box::pin(async move {
                let text = greeting(ctx).await.map_err(CordisError::PluginFailed)?;
                self.log.lock().unwrap().push(text);
                Ok(Effect::Done)
            })
        }
    }

    #[tokio::main]
    pub async fn main() -> Result<(), Box<dyn std::error::Error>> {
        let args: Vec<String> = std::env::args().collect();
        if args.get(1).is_some_and(|a| a == "--sdk-info") {
            println!("{} {}", rutis_sdk::SDK_ID, rutis_sdk::SDK_VERSION);
            return Ok(());
        }
        let expected = option_env!("RUTIS_SDK_ARTIFACT_SHA256")
            .ok_or("example host is not bound to an SDK artifact")?;
        let loader = Loader::new(
            expected,
            std::env::var_os("RUTIS_PLUGIN_CACHE")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::env::temp_dir().join("rutis-dylib-smoke-cache")),
            Default::default(),
            4,
        )?;
        if args.get(1).is_some_and(|a| a == "--load-only") {
            unsafe { loader.load(args.get(2).ok_or("missing plugin directory")?)? };
            return Ok(());
        }
        let first = args.get(1).ok_or("missing v1 directory")?;
        let second = args.get(2).ok_or("missing v2 directory")?;
        let v1 = unsafe { loader.load(first)? };
        let root = Ctx::root()?;
        let view = loader.spawn(&root, &v1, rutis_sdk::ConfigValue::Null)?;
        (&view).await?;
        assert_eq!(
            root.get::<String>().as_deref().map(String::as_str),
            Some("hello v1")
        );
        assert_eq!(root.get::<Snapshot>().unwrap().generation, 1);
        assert_eq!(
            greeting(&root)
                .await
                .map_err(|e| e as Box<dyn std::error::Error>)?,
            "hello v1"
        );
        if let Some(changed_dir) = args.get(3) {
            let changed = unsafe { loader.load(changed_dir)? };
            assert!(loader
                .swap(&view, &changed, rutis_sdk::ConfigValue::Null)
                .await
                .is_err());
            assert!(view
                .update(DylibConfig::new(changed, rutis_sdk::ConfigValue::Null,))
                .await
                .is_err());
            assert_eq!(root.get::<Snapshot>().unwrap().generation, 1);
            assert_eq!(
                view.current_config::<DylibConfig>().unwrap().module().id(),
                "greeter"
            );
        }
        let log = Arc::new(Mutex::new(Vec::new()));
        let consumer = root.plugin(Consumer {
            log: log.clone(),
            injects: vec![TypeKey::of::<String>()],
        });
        (&consumer).await?;
        let by_name_log = Arc::new(Mutex::new(Vec::new()));
        let by_name = root.plugin(ByName {
            log: by_name_log.clone(),
            injects: vec![rutis_bridge::session::host_key("greeting")],
        });
        (&by_name).await?;
        let v2 = unsafe { loader.load(second)? };
        loader
            .swap(&view, &v2, rutis_sdk::ConfigValue::Null)
            .await?;
        (&consumer).await?;
        (&by_name).await?;
        assert_eq!(
            root.get::<String>().as_deref().map(String::as_str),
            Some("hello v2")
        );
        assert_eq!(root.get::<Snapshot>().unwrap().generation, 2);
        // The new version's own type, served by name in its place.
        assert_eq!(
            greeting(&root)
                .await
                .map_err(|e| e as Box<dyn std::error::Error>)?,
            "hello v2"
        );
        if let Some(path) = std::env::var_os("RUTIS_PLUGIN_DROP_MARKER") {
            assert_eq!(std::fs::read_to_string(path)?, "drop\n");
        }
        assert_eq!(*log.lock().unwrap(), ["hello v1", "hello v2"]);
        // The by-name consumer reloaded onto the new generation's service.
        assert_eq!(*by_name_log.lock().unwrap(), ["hello v1", "hello v2"]);
        assert_eq!(
            loader.diagnostics().len(),
            if args.get(3).is_some() { 3 } else { 2 }
        );
        assert!(loader
            .diagnostics()
            .iter()
            .all(|item| item.mapped_bytes > 0 && item.usable));
        let limited = Loader::new(
            expected,
            std::env::var_os("RUTIS_PLUGIN_CACHE")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::env::temp_dir().join("rutis-dylib-smoke-cache")),
            Default::default(),
            1,
        )?;
        unsafe { limited.load(first)? };
        let rejected = unsafe { limited.load(second) }.err().unwrap();
        assert_eq!(rejected.step, "retention");
        assert_eq!(limited.diagnostics().len(), 1);
        if let Some(retry_dir) = args.get(4) {
            let retry = Loader::new(
                expected,
                std::env::var_os("RUTIS_PLUGIN_CACHE")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|| std::env::temp_dir().join("rutis-dylib-smoke-cache")),
                Default::default(),
                1,
            )?;
            assert_eq!(
                unsafe { retry.load(retry_dir) }.err().unwrap().step,
                "entry"
            );
            unsafe { retry.load(retry_dir)? };
            assert_eq!(retry.diagnostics().len(), 1);
            assert!(retry.diagnostics()[0].usable);
        }
        consumer.dispose().await?;
        by_name.dispose().await?;
        view.dispose().await?;
        // Unloaded, the plugin's service is no longer served by name.
        assert!(greeting(&root).await.is_err());
        root.shutdown().await?;
        println!("dylib swap and consumer reload passed");
        Ok(())
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    imp::main()
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn main() {}
