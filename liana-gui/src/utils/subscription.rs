use iced::{
    advanced::subscription::{from_recipe, EventStream, Hasher, Recipe},
    futures::{channel::mpsc, stream::BoxStream, Stream},
    Subscription,
};

use std::{
    hash::Hash,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

/// Set by the SIGINT handler, polled by [`interrupt`].
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// The handler runs on the signal stack, so it may only touch an atomic.
extern "C" fn on_interrupt(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

/// Emits once the process is interrupted with Ctrl+C.
///
/// Replaces `tokio::signal::ctrl_c`. Without it an interrupt would kill the process outright,
/// leaving the internal bitcoind running and its datadir locked.
pub fn interrupt() -> Subscription<()> {
    run_with_id("ctrl-c", interrupts())
}

fn interrupts() -> impl Stream<Item = ()> {
    const POLL_INTERVAL: Duration = Duration::from_millis(100);

    let (mut sender, receiver) = mpsc::channel(1);
    thread::spawn(move || {
        // SAFETY: the handler only stores into an atomic.
        unsafe { libc::signal(libc::SIGINT, on_interrupt as libc::sighandler_t) };
        loop {
            thread::sleep(POLL_INTERVAL);
            if INTERRUPTED.load(Ordering::SeqCst) {
                let _ = sender.try_send(());
                return;
            }
            if sender.is_closed() {
                return;
            }
        }
    });
    receiver
}

/// Emits an [`Instant`] every `interval`.
///
/// Replacement for `iced::time::every`, which only exists when iced runs on a timer-carrying
/// executor. The ticks are produced by a thread; one that arrives while the previous one is still
/// unread is dropped rather than queued.
pub fn every(interval: Duration) -> Subscription<Instant> {
    run_with_id(interval, ticker(interval))
}

fn ticker(interval: Duration) -> impl Stream<Item = Instant> {
    let (mut sender, receiver) = mpsc::channel(1);
    thread::spawn(move || loop {
        thread::sleep(interval);
        if let Err(e) = sender.try_send(Instant::now()) {
            if e.is_disconnected() {
                return;
            }
        }
    });
    receiver
}

/// Replacement for `Subscription::run_with_id` which was removed in iced 0.14.
///
/// Creates a [`Subscription`] that will asynchronously run the given [`Stream`],
/// using `id` to uniquely identify the subscription.
pub fn run_with_id<I, S, T>(id: I, stream: S) -> Subscription<T>
where
    I: Hash + 'static,
    S: Stream<Item = T> + Send + 'static,
    T: 'static + Send,
{
    from_recipe(IdRunner { id, stream })
}

struct IdRunner<I, S> {
    id: I,
    stream: S,
}

impl<I, S> Recipe for IdRunner<I, S>
where
    I: Hash + 'static,
    S: Stream + Send + 'static,
    S::Item: 'static + Send,
{
    type Output = S::Item;

    fn hash(&self, state: &mut Hasher) {
        self.id.hash(state);
        std::any::TypeId::of::<S>().hash(state);
    }

    fn stream(self: Box<Self>, _input: EventStream) -> BoxStream<'static, Self::Output> {
        Box::pin(self.stream)
    }
}
