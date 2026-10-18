//! 派发尾链并发回归测试(D31):两个任务并发 emit 同一事件名,
//! 链不得分叉——各次 emit 的调用按发出顺序开始。
//!
//! 背景:emit 的"取 prev → spawn → 存尾"必须单次持锁原子完成;
//! 若 remove/insert 分两段锁,并发同名 emit 会各自拿到 prev,
//! 派发任务并发执行,顺序保证失效(修复前实测 max_in_flight=4)。
//!
//! #464 起同名 emit 的调用只按顺序*开始*,慢调用不再挡住后面的 emit。
//! 这里的监听器不让出(整个调用在开始那一步里跑完),所以链不分叉时
//! 调用仍然不会重叠;同时检查每个发射者的事件按它发出的顺序开始。

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use rutis::{Call, Ctx, Event, EventKey, Listener};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct ProbeEvent {
    emitter: usize,
    n: usize,
}

impl Event for ProbeEvent {
    const NAME: &'static str = "probe/fork";
    type Value = ();
}

/// 监听器持 Arc 原子句柄(注册移走监听器,断言侧仍可读)。
struct ChainProbe {
    in_flight: Arc<AtomicUsize>,
    max_in_flight: Arc<AtomicUsize>,
    completed: Arc<AtomicUsize>,
    /// 每个发射者开始的序号,按开始顺序。
    started: Arc<Mutex<[Vec<usize>; 2]>>,
}

impl Listener<ProbeEvent> for ChainProbe {
    fn call<'a>(
        &'a self,
        _ctx: &'a Ctx,
        e: &'a ProbeEvent,
        _call: Call<'a, ()>,
    ) -> rutis::BoxFuture<'a, Result<Option<()>, rutis::CordisError>> {
        let in_flight = self.in_flight.clone();
        let max_in_flight = self.max_in_flight.clone();
        let completed = self.completed.clone();
        let started = self.started.clone();
        let (emitter, n) = (e.emitter, e.n);
        Box::pin(async move {
            let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            max_in_flight.fetch_max(now, Ordering::SeqCst);
            started.lock().unwrap()[emitter].push(n);
            // 一点工作量,放大重叠窗口:若链分叉,并发派发在此明显重叠
            std::thread::sleep(std::time::Duration::from_micros(50));
            in_flight.fetch_sub(1, Ordering::SeqCst);
            completed.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        })
    }
}

const EMITS_PER_EMITTER: usize = 2_000;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispatch_chain_does_not_fork_under_concurrent_emit() {
    let root = Ctx::root().unwrap();
    let key = EventKey::<ProbeEvent>::of();
    let in_flight = Arc::new(AtomicUsize::new(0));
    let max_in_flight = Arc::new(AtomicUsize::new(0));
    let completed = Arc::new(AtomicUsize::new(0));
    let started: Arc<Mutex<[Vec<usize>; 2]>> = Arc::default();
    root.events()
        .on(
            &root,
            &key,
            ChainProbe {
                in_flight: in_flight.clone(),
                max_in_flight: max_in_flight.clone(),
                completed: completed.clone(),
                started: started.clone(),
            },
        )
        .unwrap();

    let emitters: Vec<_> = (0..2)
        .map(|emitter| {
            let root = root.clone();
            let key = key.clone();
            tokio::spawn(async move {
                for n in 0..EMITS_PER_EMITTER {
                    root.events()
                        .emit(&root, &key, Arc::new(ProbeEvent { emitter, n }))
                        .expect("default event dispatch");
                }
            })
        })
        .collect();
    for emitter in emitters {
        emitter.await.unwrap();
    }

    // 确定性排干:全部派发完成(completed == 总 emit 数)后读终值
    let total = EMITS_PER_EMITTER * 2;
    tokio::time::timeout(std::time::Duration::from_secs(60), async {
        while completed.load(Ordering::SeqCst) < total {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("chain drains");

    assert_eq!(
        max_in_flight.load(Ordering::SeqCst),
        1,
        "同名 emit 的调用必须按顺序开始:链分叉了"
    );
    let expected: Vec<usize> = (0..EMITS_PER_EMITTER).collect();
    for (emitter, ns) in started.lock().unwrap().iter().enumerate() {
        assert_eq!(ns, &expected, "发射者 {emitter} 的事件乱序开始");
    }
}
