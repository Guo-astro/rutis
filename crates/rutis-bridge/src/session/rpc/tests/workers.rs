//! The host's workers (#1103; `specs/cross-runtime-sync`'s
//! `policy-b-workers1`).
//!
//! A Rust service runs inside the host's runtime: `receive` spawns the call
//! and `execute` runs it there, so it holds one of the runtime's worker
//! threads for as long as it runs. The host's own synchronous call
//! (`request_sync`) blocks the thread that makes it — a worker, when a task
//! of the runtime made it. Both are the same pool, with one queue: a
//! service call that is not on a waiting thread's chain queues for a worker
//! and starts only once one is free.
//!
//! That is the shape `policy-b-workers1` has, and `waits.rs` has no node for
//! it: the table is keyed by (session, call id), and the host's threads are
//! not in it. The pool is as large as the host's runtime says: `rutis-host`
//! never sets the count (`main.rs`), so tokio's default applies —
//! `available_parallelism`, or `TOKIO_WORKER_THREADS` when set. A host with
//! one worker is a one-CPU machine, or that variable set to 1.

use super::*;
use std::time::Duration;

/// How long a step that must happen may take before it counts as hung.
const STEP_LIMIT: Duration = Duration::from_secs(20);
/// How long to watch for the service while the host's own call holds the
/// only worker. What it watches for cannot happen at all, so this is not a
/// race with the machine's speed: it is how long a bug takes to show.
const HELD: Duration = Duration::from_millis(500);

/// The Rust service of the shape: says where it ran, and when.
#[derive(Default)]
struct Service {
    /// The thread it ran on, and whether that thread was a thread of the
    /// host's runtime (a worker), not one of its own.
    report: Mutex<Option<(ThreadId, bool)>>,
    /// Signalled once it ran: its caller's call has been answered.
    ran_tx: Mutex<Option<mpsc::Sender<()>>>,
}
impl Dispatch for Service {
    fn invoke(&self, _: &Connection, _: &str, _: &str, _: Value) -> Reply {
        let on_runtime = Handle::try_current().is_ok();
        *self.report.lock().unwrap() = Some((std::thread::current().id(), on_runtime));
        if let Some(tx) = self.ran_tx.lock().unwrap().take() {
            let _ = tx.send(());
        }
        Ok(json!("ran").into())
    }
}

/// What the shape did on a host whose runtime has `workers` worker threads.
struct Observed {
    /// The service ran while the host's own synchronous call was still out.
    while_held: bool,
    /// It ran at all, once that call returned: the call queued, it was not
    /// lost.
    after_release: bool,
    /// It ran on a thread of the host's runtime, not on a thread of its own
    /// and not on the session's reader.
    on_runtime: bool,
    /// Its thread was not the one the host's own call blocked.
    other_thread: bool,
}

/// The shape of `policy-b-workers1`: a task of the runtime makes the host's
/// own synchronous call (calling `hold` on the far end, `HostRoot`), and
/// inside that wait the far end synchronously calls a Rust service that is
/// not on the call's chain. Then the far end answers the host's own call,
/// freeing its worker.
fn policy_b_workers(workers: usize) -> Observed {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(workers)
        .enable_all()
        .build()
        .unwrap();
    let (local, far) = far_pair();
    let (ran_tx, ran_rx) = mpsc::channel();
    let service = Arc::new(Service::default());
    *service.ran_tx.lock().unwrap() = Some(ran_tx);
    let peer = {
        let _entered = runtime.enter();
        Connection::open(local, service.clone() as Arc<dyn Dispatch>, rust()).unwrap()
    };
    // The thread the host's own call blocks: a worker of this runtime.
    let holder: Arc<Mutex<Option<ThreadId>>> = Arc::new(Mutex::new(None));
    let (sent_tx, sent_rx) = mpsc::channel::<()>();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let (done_tx, done_rx) = mpsc::channel::<()>();
    let far_thread = std::thread::spawn(move || {
        let mut far = Remote::start(far);
        // The host's own synchronous call, not answered yet: it holds the
        // worker until the far end answers it (#597).
        let Frame::Invoke { id, sync, .. } = far.read() else {
            panic!("the host's own call expected")
        };
        assert!(sync, "the host's own call waits for its reply");
        // A runtime's synchronous service call, made inside that wait and
        // on none of its chain: it needs a worker of its own.
        far.send(invoke("node:1", "run", true));
        sent_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        // The host's own call returns: its worker is free for the queue.
        far.ret(id, data(json!("held")));
        let Frame::Return { id, value } = far.read() else {
            panic!("the service's return expected")
        };
        assert_eq!(id, "node:1");
        assert_eq!(data_of(value), json!("ran"));
        done_rx.recv().unwrap();
    });
    // Greeted before anything is sent: `invoke` refuses a session whose
    // handshake is still on its way, and the far end greets only once it
    // runs.
    runtime.block_on(peer.ready()).unwrap();
    let host = peer.clone();
    let held_by = holder.clone();
    let host_call = runtime.spawn(async move {
        *held_by.lock().unwrap() = Some(std::thread::current().id());
        host.invoke("", "hold", Value::Undefined)
    });
    // The service call is sent only after the host's own call is out, so the
    // worker is held when it arrives.
    sent_rx
        .recv_timeout(STEP_LIMIT)
        .expect("the far end sent it");
    let while_held = ran_rx.recv_timeout(HELD).is_ok();
    release_tx.send(()).unwrap();
    // Queued, not lost: once the worker is free it runs.
    let after_release = while_held || ran_rx.recv_timeout(STEP_LIMIT).is_ok();
    let reply = runtime.block_on(host_call).unwrap();
    assert_eq!(reply.unwrap(), json!("held").into());
    let (thread, on_runtime) = service
        .report
        .lock()
        .unwrap()
        .expect("the service ran and said where");
    let other_thread = holder.lock().unwrap().is_some_and(|held| held != thread);
    done_tx.send(()).unwrap();
    far_thread.join().unwrap();
    Observed {
        while_held,
        after_release,
        on_runtime,
        other_thread,
    }
}

/// `policy-b-workers1`: with one worker the host's own call holds it, and the
/// runtime's synchronous service call waits for a free one. The model's
/// counterexample 12, on the implementation: policy B has no edge for it, so
/// nothing refuses the call and nothing can finish it.
/// guarantee: bridge/sync-wait-not-covered
#[test]
fn one_worker_is_held_by_the_hosts_own_call_and_the_service_call_waits() {
    let observed = policy_b_workers(1);
    assert!(
        !observed.while_held,
        "the service ran on the only worker while the host's own call held it"
    );
    assert!(
        observed.after_release,
        "the service call was lost, not queued for a worker"
    );
    assert!(
        observed.on_runtime,
        "the service did not run on the host's runtime, so it held no worker"
    );
}

/// The same shape with a second worker: the free one takes the call, so the
/// queue is the runtime's worker pool and nothing else. The deadlock takes as
/// many host waits as the pool has threads (`Workers` in the model).
/// guarantee: bridge/sync-wait-not-covered
#[test]
fn a_second_worker_takes_the_service_call_while_the_hosts_own_call_holds_one() {
    let observed = policy_b_workers(2);
    assert!(
        observed.while_held,
        "the service waited while a worker was free"
    );
    assert!(observed.on_runtime, "the service ran elsewhere");
    assert!(
        observed.other_thread,
        "the service ran on the thread the host's own call blocked"
    );
}
