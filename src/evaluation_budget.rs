//! One evaluation's budget, shared by every connection and concurrent call.
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(crate) const MAX_REQUESTS: u64 = 4096;
pub(crate) const MAX_DURATION: Duration = Duration::from_secs(300);

#[derive(Clone, Debug)]
pub(crate) struct Budget(Arc<State>);

#[derive(Debug)]
struct State {
    deadline: Instant,
    requests: AtomicU64,
    max_requests: u64,
    denied: AtomicBool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluation_budget_reservations_are_atomic_across_workers() {
        let budget = Budget::new(Duration::from_secs(10), 100);
        let workers: Vec<_> = (0..16)
            .map(|_| {
                let budget = budget.clone();
                std::thread::spawn(move || (0..100).filter(|_| budget.acquire().is_ok()).count())
            })
            .collect();
        let allowed: usize = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .sum();
        assert_eq!(allowed, 100);
        assert!(budget.check().is_err());
    }

    #[test]
    fn evaluation_budget_enforces_empty_and_single_request_limits() {
        for limit in [0, 1] {
            let budget = Budget::new(Duration::from_secs(10), limit);
            for _ in 0..limit {
                assert!(budget.acquire().is_ok());
            }
            assert!(budget.acquire().is_err());
            assert!(budget.acquire().is_err());
            assert!(budget.check().is_err());
            assert_eq!(budget.0.requests.load(Ordering::Relaxed), limit);
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Exhausted;

impl std::fmt::Display for Exhausted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("evaluation budget exhausted")
    }
}

impl std::error::Error for Exhausted {}

impl Default for Budget {
    fn default() -> Self {
        Self::new(MAX_DURATION, MAX_REQUESTS)
    }
}

impl Budget {
    pub(crate) fn new(duration: Duration, max_requests: u64) -> Self {
        Self(Arc::new(State {
            deadline: Instant::now() + duration,
            requests: AtomicU64::new(0),
            max_requests,
            denied: AtomicBool::new(false),
        }))
    }

    pub(crate) fn check(&self) -> anyhow::Result<()> {
        if Instant::now() >= self.0.deadline || self.0.denied.load(Ordering::Relaxed) {
            return Err(Exhausted.into());
        }
        Ok(())
    }

    /// Reserve exactly one client request before touching the transport.
    pub(crate) fn acquire(&self) -> anyhow::Result<()> {
        self.check()?;
        let mut count = self.0.requests.load(Ordering::Relaxed);
        loop {
            if count >= self.0.max_requests {
                self.0.denied.store(true, Ordering::Relaxed);
                return Err(Exhausted.into());
            }
            match self.0.requests.compare_exchange_weak(
                count,
                count + 1,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return Ok(()),
                Err(current) => count = current,
            }
        }
    }

    pub(crate) fn timeout(&self, requested: Duration) -> anyhow::Result<Duration> {
        self.check()?;
        Ok(requested.min(self.0.deadline.saturating_duration_since(Instant::now())))
    }

    pub(crate) fn deadline(&self, requested: Duration) -> anyhow::Result<Instant> {
        self.check()?;
        Ok((Instant::now() + requested).min(self.0.deadline))
    }

    /// Budget exhaustion takes precedence over transport symptoms and partial verdicts.
    pub(crate) fn finish<T>(&self, result: anyhow::Result<T>) -> anyhow::Result<T> {
        self.check()?;
        result
    }
}
