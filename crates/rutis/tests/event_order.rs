//! 事件与顺序的核心保证(#177 1/2):状态事件按提交顺序到达(D24)、
//! 同类型 emit 按发出顺序投递(D31)、事件不按 isolate 过滤(D29)、
//! 异步 waterfall 回调 panic 传给调用者(design-rust-port §八 第 4 条)。
//!
//! #464 起同名 emit 的调用只按发出顺序*开始*,彼此并发运行(慢调用不再
//! 挡住后面的 emit),完成顺序不作契约。所以监听器在调用开始时记录,
//! 然后再让出若干轮调度,让各次调用真正重叠:链一旦分叉,开始顺序就会
//! 乱,断言因此能看到。让出的轮数由事件内容确定,不用随机数,也不用
//! 固定时长的等待;等待一律有 `HANG_GUARD` 兜底(质量规范 §7)。

use std::collections::BTreeMap;
use std::future::{Future, IntoFuture};
use std::sync::Arc;

use rutis::events::{Call, Listener};
use rutis::host::{
    BoxFuture, CtxHost, DynPlugin, Effect, ErrorKind, FiberState, FiberStatusChanged, PluginId,
    TypeKey,
};
use rutis::{Ctx, Error, Event};
use rutis_test_support::within;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

// ── 测试助手 ─────────────────────────────────────────────────────

/// 带发射者编号的序号事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
struct Seq {
    emitter: usize,
    n: usize,
}

impl Event for Seq {
    const NAME: &'static str = "test::event_order::Seq";
    type Value = ();
}

fn seq(emitter: usize, n: usize) -> Seq {
    Seq { emitter, n }
}

/// 监听器:调用开始时执行 `body`,然后让出 `delay(e)` 轮调度。
fn slow<E, D, B>(delay: D, body: B) -> impl Listener<E>
where
    E: Event,
    D: Fn(&E) -> usize + Send + Sync + 'static,
    B: Fn(&Ctx, &E) + Send + Sync + 'static,
{
    let parts = Arc::new((delay, body));
    move |e: Arc<E>, call: Call<E>| {
        let parts = parts.clone();
        async move {
            let (delay, body) = &*parts;
            body(call.ctx(), &e);
            for _ in 0..delay(&e) {
                tokio::task::yield_now().await;
            }
            Ok(None)
        }
    }
}

/// 收 `count` 条消息,整体受 `HANG_GUARD` 约束。
async fn take<T>(what: &str, rx: &mut mpsc::UnboundedReceiver<T>, count: usize) -> Vec<T> {
    within(what, async {
        let mut out = Vec::with_capacity(count);
        while out.len() < count {
            out.push(rx.recv().await.expect("sender alive"));
        }
        out
    })
    .await
}

/// 什么都不做的插件。
struct Noop;

impl DynPlugin for Noop {
    fn name(&self) -> &str {
        "noop"
    }
    fn apply<'a>(&'a self, _ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async { Ok(Effect::Done) })
    }
}

// ── D24 状态事件按提交顺序分发 ────────────────────────────────────

/// 多个 fiber 在多个工作线程上同时装载、重启、卸载:每个 fiber 的状态
/// 事件按提交顺序到达(调用按提交顺序开始)——到达序列里 seq 从 1 起
/// 连续递增,且每条的 `from` 是上一条的 `to`。监听器按 seq 让出不同轮数,
/// 各次调用因此重叠;派发若不按提交顺序开始,后提交的事件会超车。
///
/// guarantee: core/D24
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn status_events_arrive_in_commit_order_across_concurrent_fibers() {
    const FIBERS: usize = 16;
    const RESTARTS: usize = 3;
    let root = Ctx::root().unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel::<FiberStatusChanged>();
    root.on::<FiberStatusChanged>(slow(
        |e: &FiberStatusChanged| (e.seq % 3) as usize * 4,
        move |_: &Ctx, e: &FiberStatusChanged| {
            let _ = tx.send(e.clone());
        },
    ))
    .unwrap();

    let barrier = Arc::new(tokio::sync::Barrier::new(FIBERS));
    let mut runs = Vec::new();
    for _ in 0..FIBERS {
        let root = root.clone();
        let barrier = barrier.clone();
        runs.push(tokio::spawn(async move {
            barrier.wait().await;
            let view = root.plugin(Noop);
            (&view).await.expect("load");
            for _ in 0..RESTARTS {
                view.restart().await.expect("restart");
            }
            view.dispose().await.expect("dispose");
        }));
    }
    within("fibers finish", async {
        for run in runs {
            run.await.expect("fiber task");
        }
    })
    .await;

    // 每个 fiber 最后一条是 → Disposed;收到 FIBERS 条即全部到齐
    let mut by_fiber: BTreeMap<PluginId, Vec<FiberStatusChanged>> = BTreeMap::new();
    within("status events arrive", async {
        let mut disposed = 0;
        while disposed < FIBERS {
            let e = rx.recv().await.expect("sender alive");
            if e.to == FiberState::Disposed {
                disposed += 1;
            }
            by_fiber.entry(e.plugin_id).or_default().push(e);
        }
    })
    .await;

    assert_eq!(by_fiber.len(), FIBERS);
    for (id, events) in &by_fiber {
        let seqs: Vec<u64> = events.iter().map(|e| e.seq).collect();
        let expected: Vec<u64> = (1..=events.len() as u64).collect();
        assert_eq!(seqs, expected, "{id:?}: 到达顺序不是提交顺序");
        assert_eq!(events[0].from, FiberState::Pending, "{id:?}");
        assert_eq!(events.last().unwrap().to, FiberState::Disposed, "{id:?}");
        for pair in events.windows(2) {
            assert_eq!(pair[0].to, pair[1].from, "{id:?}: 状态链断开 {pair:?}");
        }
    }
}

// ── D31 同类型 emit 按发出顺序投递 ────────────────────────────────

/// 多个任务在多个工作线程上同时 emit 同一名字:每个发射者的事件按它
/// 发出的顺序开始。监听器按序号让出不同轮数,链若分叉,同一发射者的
/// 后一个事件会超车。
///
/// guarantee: core/D31
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn emit_order_is_kept_per_emitter() {
    const EMITTERS: usize = 4;
    const PER_EMITTER: usize = 300;
    let root = Ctx::root().unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel::<Seq>();
    root.on::<Seq>(slow(
        |e: &Seq| (e.n * 7 + e.emitter) % 5,
        move |_: &Ctx, e: &Seq| {
            let _ = tx.send(*e);
        },
    ))
    .unwrap();

    let barrier = Arc::new(std::sync::Barrier::new(EMITTERS));
    let mut emitters = Vec::new();
    for emitter in 0..EMITTERS {
        let root = root.clone();
        let barrier = barrier.clone();
        // 各自的线程:emit 是同步调用,线程间真正并发
        emitters.push(std::thread::spawn(move || {
            barrier.wait();
            for n in 0..PER_EMITTER {
                root.emit(seq(emitter, n)).expect("emit accepted");
            }
        }));
    }
    for emitter in emitters {
        emitter.join().expect("emitter thread");
    }

    let arrived = take("all emits delivered", &mut rx, EMITTERS * PER_EMITTER).await;
    for emitter in 0..EMITTERS {
        let ns: Vec<usize> = arrived
            .iter()
            .filter(|e| e.emitter == emitter)
            .map(|e| e.n)
            .collect();
        let expected: Vec<usize> = (0..PER_EMITTER).collect();
        assert_eq!(ns, expected, "发射者 {emitter} 的事件乱序");
    }
}

/// 一个监听器在某次 emit 中 panic:panic 送到 ErrorSink,同一次 emit 中
/// 排在它后面的监听器照常收到,下一次 emit 也照常投递,顺序不变。
///
/// guarantee: core/D31
#[tokio::test]
async fn emit_after_listener_panic_is_still_delivered() {
    let (sink_tx, mut sink_rx) = mpsc::unbounded_channel::<String>();
    let root = Ctx::root_with_sink(
        tokio::runtime::Handle::current(),
        Arc::new(move |e: Arc<ErrorKind>| {
            let _ = sink_tx.send(e.to_string());
        }),
    );
    root.on::<Seq>(slow(
        |_: &Seq| 0,
        |_: &Ctx, e: &Seq| {
            if e.n == 1 {
                panic!("listener panic on 1");
            }
        },
    ))
    .unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel::<usize>();
    root.on::<Seq>(slow(
        |_: &Seq| 0,
        move |_: &Ctx, e: &Seq| {
            let _ = tx.send(e.n);
        },
    ))
    .unwrap();

    for n in 0..3 {
        root.emit(seq(0, n)).unwrap();
    }
    assert_eq!(take("emits delivered", &mut rx, 3).await, vec![0, 1, 2]);
    let reported = take("panic reported", &mut sink_rx, 1).await;
    assert!(
        reported[0].contains("listener panic on 1"),
        "sink got {reported:?}"
    );
    // 不多报:panic 只有一次
    root.shutdown().await.unwrap();
    assert!(sink_rx.try_recv().is_err(), "panic 被重复报告");
}

/// 监听器在处理中再 emit 同一名字:新事件排在已发出的事件之后,不会
/// 插队,也不会死锁。前三个事件在任何监听器运行之前已经发出,所以
/// 期望的开始顺序是 0、1、2,然后才是监听器发出的 100、101。
///
/// guarantee: core/D31
#[tokio::test]
async fn emit_inside_listener_goes_to_the_back() {
    let root = Ctx::root().unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel::<usize>();
    root.on::<Seq>(slow(
        // 事件 0 的调用最慢(#464 起它不再挡住 1、2,只是开始在前)
        |e: &Seq| if e.n == 0 { 8 } else { 0 },
        move |ctx: &Ctx, e: &Seq| {
            if e.n == 0 {
                ctx.emit(seq(0, 100)).unwrap();
                ctx.emit(seq(0, 101)).unwrap();
            }
            let _ = tx.send(e.n);
        },
    ))
    .unwrap();

    for n in 0..3 {
        root.emit(seq(0, n)).unwrap();
    }
    assert_eq!(
        take("re-entrant emits delivered", &mut rx, 5).await,
        vec![0, 1, 2, 100, 101]
    );
}

// ── D29 事件不按 isolate 过滤 ─────────────────────────────────────

struct Scoped;

/// 在装载时注册一个监听器的插件:收到的事件连同自己的名字一起发出。
struct ListenIn {
    name: &'static str,
    tx: mpsc::UnboundedSender<(&'static str, usize)>,
}

impl DynPlugin for ListenIn {
    fn name(&self) -> &str {
        self.name
    }
    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        let name = self.name;
        let tx = self.tx.clone();
        Box::pin(async move {
            ctx.on::<Seq>(slow(
                |_: &Seq| 0,
                move |_: &Ctx, e: &Seq| {
                    let _ = tx.send((name, e.emitter));
                },
            ))?;
            Ok(Effect::Done)
        })
    }
}

/// isolate 只隔离注册表,不隔离事件:在两个不同 isolate 里装载的插件、
/// 以及根上的监听器,收到从根、从任一 isolate 发出的全部事件。这是
/// 明确的"不做"(作用域事件用 `Ctx::mark_scope`,与 isolate 无关);本测试
/// 防止有人加上过滤。
///
/// guarantee: core/D29
#[tokio::test]
async fn events_are_not_filtered_by_isolate() {
    let root = Ctx::root().unwrap();
    let scoped = TypeKey::of::<Scoped>();
    let scope_a = root.isolate(scoped.clone(), "A");
    let scope_b = root.isolate(scoped, "B");
    let (tx, mut rx) = mpsc::unbounded_channel::<(&'static str, usize)>();
    let root_tx = tx.clone();
    root.on::<Seq>(slow(
        |_: &Seq| 0,
        move |_: &Ctx, e: &Seq| {
            let _ = root_tx.send(("root", e.emitter));
        },
    ))
    .unwrap();
    let in_a = scope_a.plugin(ListenIn {
        name: "in_a",
        tx: tx.clone(),
    });
    let in_b = scope_b.plugin(ListenIn {
        name: "in_b",
        tx: tx.clone(),
    });
    within("listeners loaded", async {
        (&in_a).await.expect("in_a");
        (&in_b).await.expect("in_b");
    })
    .await;

    // 发射者编号:0 = 根,1 = isolate A,2 = isolate B
    for (emitter, ctx) in [&root, &scope_a, &scope_b].into_iter().enumerate() {
        ctx.emit(seq(emitter, 0)).unwrap();
    }
    // 栅栏:根上的监听器最先注册,它收到栅栏时,之前各次 emit 的调用都
    // 已开始,而监听器在开始时就记录(同名按顺序开始,D31)。之后只看
    // 收到了什么,少收时立即失败。
    const FENCE: usize = 99;
    root.emit(seq(FENCE, 0)).unwrap();
    let mut got = within("fence delivered", async {
        let mut got = Vec::new();
        loop {
            let (listener, emitter) = rx.recv().await.expect("sender alive");
            if emitter == FENCE {
                if listener == "root" {
                    return got;
                }
                continue;
            }
            got.push((listener, emitter));
        }
    })
    .await;
    got.sort();
    let mut expected = Vec::new();
    for listener in ["in_a", "in_b", "root"] {
        for emitter in 0..3 {
            expected.push((listener, emitter));
        }
    }
    assert_eq!(got, expected);
}

// ── 异步 waterfall 回调 panic 传给调用者 ──────────────────────────

#[derive(Debug, Serialize, Deserialize)]
struct Wf;

impl Event for Wf {
    const NAME: &'static str = "test::event_order::Wf";
    type Value = u32;
}

/// 中间件:调用 next,结果加一(next 没有结果时当 0)。
async fn add_one(_: Arc<Wf>, call: Call<Wf>) -> rutis::Result<Option<u32>> {
    Ok(Some(call.next().await?.unwrap_or(0) + 1))
}

/// 让出一轮后 panic 的中间件(panic 发生在 poll 中,不在构造 future 时)。
async fn panics_after_yield(_: Arc<Wf>, _: Call<Wf>) -> rutis::Result<Option<u32>> {
    tokio::task::yield_now().await;
    panic!("async waterfall listener panic")
}

/// 终态续延:0。
async fn zero(_: Arc<Wf>) -> rutis::Result<Option<u32>> {
    Ok(Some(0))
}

/// panic 的终态续延(panic 发生在 poll 中)。
async fn panic_terminal(_: Arc<Wf>) -> rutis::Result<Option<u32>> {
    panic!("async waterfall terminal panic")
}

/// 调用 waterfall,返回 panic 转成的错误信息(waterfall 没有失败时测试
/// 失败,并给出它的返回值)。
async fn waterfall_panic<T, F>(root: &Ctx, terminal: T) -> String
where
    T: FnOnce(Arc<Wf>) -> F + Send + 'static,
    F: Future<Output = rutis::Result<Option<u32>>> + Send + 'static,
{
    let result = within(
        "waterfall caller",
        root.waterfall(Wf).terminal(terminal).into_future(),
    )
    .await;
    if let Err(error) = &result {
        if let ErrorKind::PluginFailed(message) = &**error {
            return message.to_string();
        }
    }
    panic!("panic did not reach the caller as an error; waterfall returned {result:?}")
}

/// 异步 waterfall 里的监听器或终态续延 panic:传给调用 waterfall 的一方
/// (穿过外层中间件),不被吞掉。出处:design-rust-port §八 第 4 条
/// "waterfall 监听器 panic 向分发者传播";登记表记在 D30 的备注里。
///
/// #464 起内核捕获每个监听器和终态的 panic,调用方收到的是
/// `PluginFailed(<panic 信息>)` 错误,不再是原样的 panic(调用方任务不再
/// panic)。
///
/// guarantee: core/D30
#[tokio::test]
async fn async_waterfall_panic_reaches_the_caller() {
    let root = Ctx::root().unwrap();
    root.on::<Wf>(add_one).unwrap();

    // 没有 panic 时链正常:0 + 1
    assert_eq!(root.waterfall(Wf).terminal(zero).await.unwrap(), Some(1));

    // 终态续延 panic,穿过外层中间件
    assert_eq!(
        waterfall_panic(&root, panic_terminal).await,
        "async waterfall terminal panic"
    );

    // 内层监听器 panic,穿过外层中间件
    let inner = root.on::<Wf>(panics_after_yield).unwrap();
    assert_eq!(
        waterfall_panic(&root, zero).await,
        "async waterfall listener panic"
    );
    inner.dispose().await.unwrap();
}
