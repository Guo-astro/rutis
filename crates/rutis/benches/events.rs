//! Run with `cargo bench -p rutis --bench events`. Local samples are not
//! portable latency guarantees or hard CI performance thresholds.
use std::hint::black_box;
use std::time::Instant;

use std::sync::Arc;

use rutis::events::{Call, Mode, SyncCall};
use rutis::host::CtxHost;
use rutis::internal::CtxInternal;
use rutis::{Ctx, Event};

#[derive(serde::Serialize, serde::Deserialize)]
struct Ping;
impl Event for Ping {
    const NAME: &'static str = "bench/ping";
    type Value = u64;
}
/// No result; in a waterfall, passes to the rest of the chain.
async fn through(_: Arc<Ping>, call: Call<Ping>) -> rutis::Result<Option<u64>> {
    if call.mode() == Mode::Waterfall {
        call.next().await
    } else {
        Ok(None)
    }
}
/// The synchronous `through`.
fn sync_through(_: Arc<Ping>, call: SyncCall<'_, Ping>) -> rutis::Result<Option<u64>> {
    if call.mode() == Mode::WaterfallSync {
        call.next()
    } else {
        Ok(None)
    }
}
fn measure(mut call: impl FnMut(), iterations: usize) -> f64 {
    for _ in 0..1000 {
        call();
    }
    let start = Instant::now();
    for _ in 0..iterations {
        call();
    }
    start.elapsed().as_nanos() as f64 / iterations as f64
}
fn report(name: &str, ns: f64) {
    println!("{name:40} {ns:12.1} ns/op");
}

fn main() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let _entered = runtime.enter();
    let iterations = std::env::var("RUTIS_BENCH_ITERATIONS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(100_000)
        .max(1);
    println!("iterations={iterations}; release build; 2 runtime workers");
    for listeners in [0, 1, 8, 64] {
        let root = Ctx::root().unwrap();
        for _ in 0..listeners {
            // One table: the same listeners serve both modes.
            root.on_sync::<Ping>(sync_through).unwrap();
        }
        let ping = Arc::new(Ping);
        report(
            &format!("bail_sync exact listeners={listeners}"),
            measure(
                || {
                    black_box(root.bail::<Ping>(ping.clone()).unwrap());
                },
                iterations,
            ),
        );
        report(
            &format!("waterfall_sync exact listeners={listeners}"),
            measure(
                || {
                    black_box(
                        root.waterfall_sync::<Ping, _>(ping.clone(), |_| Ok(Some(1)))
                            .unwrap(),
                    );
                },
                iterations,
            ),
        );
        runtime.block_on(root.shutdown()).unwrap();
    }
    // The awaited dispatch modes, with empty listeners: serial runs every
    // listener (each returns None), parallel runs them concurrently, and
    // waterfall passes through every listener to the terminal.
    for listeners in [0, 1, 8, 64] {
        let root = Ctx::root().unwrap();
        for _ in 0..listeners {
            root.listen::<Ping>().name("awaited").on(through).unwrap();
        }
        let loops = if listeners > 8 {
            iterations / 10 + 1
        } else {
            iterations
        };
        let ping = Arc::new(Ping);
        // Measured inside a runtime task, where plugins call them; parallel
        // spawns one task per listener.
        let task_root = root.clone();
        let [serial, parallel, waterfall] = runtime
            .block_on(runtime.spawn(async move {
                let root = task_root;
                let awaited = root.named("awaited");
                let mut samples = [0.0; 3];
                for (mode, sample) in samples.iter_mut().enumerate() {
                    let once = || async {
                        match mode {
                            0 => {
                                black_box(awaited.serial::<Ping>(ping.clone()).await.unwrap());
                            }
                            1 => awaited.parallel::<Ping>(ping.clone()).await.unwrap(),
                            _ => {
                                black_box(
                                    awaited
                                        .waterfall::<Ping>(ping.clone())
                                        .terminal(|_| async { Ok(Some(1)) })
                                        .await
                                        .unwrap(),
                                );
                            }
                        }
                    };
                    for _ in 0..1000 {
                        once().await;
                    }
                    let start = Instant::now();
                    for _ in 0..loops {
                        once().await;
                    }
                    *sample = start.elapsed().as_nanos() as f64 / loops as f64;
                }
                samples
            }))
            .unwrap();
        report(&format!("serial exact listeners={listeners}"), serial);
        report(&format!("parallel exact listeners={listeners}"), parallel);
        report(&format!("waterfall exact listeners={listeners}"), waterfall);
        runtime.block_on(root.shutdown()).unwrap();
    }
    for patterns in [0, 1, 8, 64, 1024] {
        let root = Ctx::root().unwrap();
        root.listen::<Ping>().name("room/hit").on(through).unwrap();
        let ping = Arc::new(Ping);
        for i in 0..patterns {
            root.listen::<Ping>()
                .prefix(if i == 0 {
                    "room/".to_owned()
                } else {
                    format!("miss/{i}/")
                })
                .on(|_, _| async { Ok(None) })
                .unwrap();
        }
        let loops = if patterns > 64 {
            iterations / 10 + 1
        } else {
            iterations
        };
        let samples = runtime.block_on(async {
            let hit = root.named("room/hit");
            for _ in 0..1000 {
                hit.serial::<Ping>(ping.clone()).await.unwrap();
            }
            let start = Instant::now();
            for _ in 0..loops {
                black_box(hit.serial::<Ping>(ping.clone()).await.unwrap());
            }
            start.elapsed().as_nanos() as f64 / loops as f64
        });
        report(&format!("serial exact + patterns={patterns}"), samples);
        runtime.block_on(root.shutdown()).unwrap();
    }
    // emit returns once the dispatch is queued; the calls of one name's
    // emits start in emit order. "accept" is the caller's cost, "drain" the time per
    // event until the chain behind one listener is empty.
    for listeners in [0, 1] {
        let root = Ctx::root().unwrap();
        for _ in 0..listeners {
            root.listen::<Ping>().name("queue").on(through).unwrap();
        }
        let ping = Arc::new(Ping);
        let queue = root.named("queue");
        let loops = iterations / 10 + 1;
        let start = Instant::now();
        for _ in 0..loops {
            queue.emit::<Ping>(ping.clone()).unwrap();
        }
        let accept = start.elapsed().as_nanos() as f64 / loops as f64;
        runtime.block_on(async {
            while !root.diagnostics().event_backlogs.is_empty() {
                tokio::task::yield_now().await;
            }
        });
        let drain = start.elapsed().as_nanos() as f64 / loops as f64;
        report(
            &format!("emit accept same key listeners={listeners}"),
            accept,
        );
        report(&format!("emit drain same key listeners={listeners}"), drain);
        runtime.block_on(root.shutdown()).unwrap();
    }
}
