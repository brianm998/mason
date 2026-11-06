// SPDX-License-Identifier: GPL-3.0-or-later

//! A bounded worker pool. Blocking work (git, the store, processes) goes here,
//! never onto the async event loop (Part 2). When the queue is full the caller
//! is told so, rather than piling up unbounded work.

use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread;

type Work = Box<dyn FnOnce() + Send + 'static>;

#[derive(Clone)]
pub struct Pool {
    tx: SyncSender<Work>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PoolError {
    /// The queue is full.
    Busy,
    /// The work panicked or the pool shut down.
    Lost,
}

impl Pool {
    pub fn new(name: &str, threads: usize, queue: usize) -> Pool {
        let (tx, rx) = sync_channel::<Work>(queue);
        let rx: Arc<Mutex<Receiver<Work>>> = Arc::new(Mutex::new(rx));
        for i in 0..threads {
            let rx = rx.clone();
            thread::Builder::new()
                .name(format!("{name}-{i}"))
                .spawn(move || loop {
                    let work = { rx.lock().unwrap_or_else(|e| e.into_inner()).recv() };
                    match work {
                        Ok(w) => {
                            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(w));
                        }
                        Err(_) => break,
                    }
                })
                .expect("spawn pool thread");
        }
        Pool { tx }
    }

    pub fn spawn(&self, f: impl FnOnce() + Send + 'static) -> Result<(), PoolError> {
        match self.tx.try_send(Box::new(f)) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(PoolError::Busy),
            Err(TrySendError::Disconnected(_)) => Err(PoolError::Lost),
        }
    }

    /// Run `f` on the pool and await its result without blocking the caller's thread.
    pub async fn run<T: Send + 'static>(
        &self,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T, PoolError> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.spawn(move || {
            let _ = tx.send(f());
        })?;
        rx.await.map_err(|_| PoolError::Lost)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn work_runs_on_pool_threads_and_the_queue_is_bounded() {
        let pool = Pool::new("t", 1, 2);
        let gate = Arc::new(Mutex::new(()));
        let held = gate.lock().unwrap();
        let done = Arc::new(AtomicUsize::new(0));
        let mut busy = 0;
        let mut accepted = 0;
        for _ in 0..10 {
            let (g, d) = (gate.clone(), done.clone());
            match pool.spawn(move || {
                let _x = g.lock();
                d.fetch_add(1, Ordering::SeqCst);
            }) {
                Ok(()) => accepted += 1,
                Err(PoolError::Busy) => busy += 1,
                Err(e) => panic!("{e:?}"),
            }
        }
        assert!(
            busy >= 7,
            "one running + two queued, the rest refused: accepted {accepted}, busy {busy}"
        );
        drop(held);
        let end = std::time::Instant::now() + Duration::from_secs(5);
        while done.load(Ordering::SeqCst) < accepted && std::time::Instant::now() < end {
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(done.load(Ordering::SeqCst), accepted);
    }

    #[tokio::test]
    async fn run_returns_the_result_and_survives_a_panic() {
        let pool = Pool::new("t", 1, 4);
        assert_eq!(pool.run(|| 21 * 2).await, Ok(42));
        assert_eq!(
            pool.run(|| -> u32 { panic!("boom") }).await,
            Err(PoolError::Lost)
        );
        assert_eq!(pool.run(|| "still alive").await, Ok("still alive"));
    }
}
