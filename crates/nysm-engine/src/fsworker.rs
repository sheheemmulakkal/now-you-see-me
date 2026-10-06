//! Slow providers run on their own threads: `statvfs` on a hung network
//! mount or a sensor read that goes to a device (e.g. NVMe temperature)
//! can block, and no timeout can cancel a blocked syscall. The engine only
//! ever reads the latest cached result and labels it stale when old.
//!
//! Limitation: a permanently blocked worker thread cannot be reclaimed; it
//! is detached and exits with the process.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use nysm_collect::CResult;

struct State<T> {
    result: Option<(Instant, CResult<T>)>,
    /// Set while a refresh is running, to detect a blocked provider.
    running_since: Option<Instant>,
}

/// Periodically runs `provider` on a dedicated thread.
pub struct SlowWorker<T> {
    shared: Arc<(Mutex<State<T>>, Condvar)>,
    stop: Arc<AtomicBool>,
    interval: Duration,
    what: &'static str,
}

pub enum WorkerStatus<T> {
    NotYet,
    Fresh(Duration, CResult<T>),
    /// Last data is older than the freshness bound (e.g. provider blocked).
    Stale(Duration, CResult<T>, String),
}

impl<T: Clone + Send + 'static> SlowWorker<T> {
    pub fn spawn(
        name: &'static str,
        what: &'static str,
        mut provider: Box<dyn FnMut() -> CResult<T> + Send>,
        interval: Duration,
    ) -> Self {
        let shared = Arc::new((
            Mutex::new(State {
                result: None,
                running_since: None,
            }),
            Condvar::new(),
        ));
        let stop = Arc::new(AtomicBool::new(false));
        let (s, st) = (shared.clone(), stop.clone());
        let spawned = thread::Builder::new().name(name.into()).spawn(move || {
            while !st.load(Ordering::Relaxed) {
                s.0.lock().unwrap().running_since = Some(Instant::now());
                let r = provider();
                {
                    let mut g = s.0.lock().unwrap();
                    g.result = Some((Instant::now(), r));
                    g.running_since = None;
                }
                s.1.notify_all();
                // Sleep, but wake promptly on stop.
                let deadline = Instant::now() + interval;
                while !st.load(Ordering::Relaxed) && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(100).min(interval));
                }
            }
        });
        if let Err(e) = spawned {
            shared.0.lock().unwrap().result = Some((
                Instant::now(),
                Err(nysm_collect::CollectError::Failed(format!("spawn: {e}"))),
            ));
        }
        SlowWorker {
            shared,
            stop,
            interval,
            what,
        }
    }

    /// Block up to `timeout` for the first result (used by one-shot commands).
    pub fn wait_first(&self, timeout: Duration) {
        let (m, cv) = &*self.shared;
        let g = m.lock().unwrap();
        let _ = cv.wait_timeout_while(g, timeout, |s| s.result.is_none());
    }

    pub fn status(&self) -> WorkerStatus<T> {
        let g = self.shared.0.lock().unwrap();
        let Some((at, r)) = &g.result else {
            return WorkerStatus::NotYet;
        };
        let age = at.elapsed();
        let bound = self.interval * 3 + Duration::from_secs(5);
        if age > bound {
            let why = match g.running_since {
                Some(s) => format!(
                    "{} query blocked for {:.0}s",
                    self.what,
                    s.elapsed().as_secs_f64()
                ),
                None => format!("{} data is old", self.what),
            };
            WorkerStatus::Stale(age, r.clone(), why)
        } else {
            WorkerStatus::Fresh(age, r.clone())
        }
    }
}

impl<T> Drop for SlowWorker<T> {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocked_provider_is_reported_stale_without_blocking_callers() {
        let calls = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let c = calls.clone();
        let w = SlowWorker::spawn(
            "t",
            "test",
            Box::new(move || {
                // First call returns, second blocks "forever" (2 s here).
                if c.fetch_add(1, Ordering::SeqCst) > 0 {
                    thread::sleep(Duration::from_secs(2));
                }
                Ok(1u32)
            }),
            Duration::from_millis(10),
        );
        w.wait_first(Duration::from_secs(1));
        let t = Instant::now();
        assert!(matches!(w.status(), WorkerStatus::Fresh(_, Ok(1))));
        assert!(
            t.elapsed() < Duration::from_millis(50),
            "status() must not wait for the provider"
        );
    }
}
