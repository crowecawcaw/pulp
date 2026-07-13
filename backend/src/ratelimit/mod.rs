//! Generic, API-agnostic adaptive rate limiting and throttling primitives.
//!
//! This module provides composable building blocks for politely talking to any
//! external HTTP API. Nothing in here knows anything about Reddit, GitHub,
//! HackerNews, or any particular collector — it is deliberately generic so the
//! same code can pace requests to *any* third-party service.
//!
//! # The pieces
//!
//! * [`RateLimiter`] — an adaptive **interval pacer** that paces request
//!   issuance. It uses **AIMD on a single minimum interval** (additive-increase /
//!   multiplicative-decrease, the same feedback family TCP congestion control
//!   uses): a 429 (throttle) multiplicatively *grows* the interval immediately
//!   (slows down fast), while a success *shrinks* it by one gentle additive step.
//!   Because tightening is multiplicative and loosening is additive, a recurring
//!   throttle drives the interval up and **holds** it — no re-inflation. There is
//!   **no token bucket and no burst**: a single persisted `next_allowed` instant,
//!   anchored at *issue* time, paces every request — including the first one after
//!   a long idle (so an idle stretch never banks a free immediate request). This
//!   lets a caller converge on the fastest spacing the remote API tolerates
//!   without hard-coding magic numbers, backing off the instant the API pushes
//!   back. It can also honor a hard `Retry-After` pause. The control law itself
//!   lives in [`adaptive::AdaptiveController`] (pure, time-injected for
//!   deterministic tests); this type wraps it for shared async use.
//!
//! * [`Throttle`] — a thin, ergonomic bundle around a [`RateLimiter`], exposing
//!   [`Throttle::run`] / [`Throttle::acquire`] + [`Throttle::report`].
//!
//! * [`KeyedThrottle`] — a generic, lazily-populated map of independent
//!   throttle "lanes" keyed by an arbitrary `K`. Use one lane per *endpoint
//!   class* (e.g. `"search"` vs `"feed"`) so that throttling on one endpoint
//!   does not starve another.
//!
//! All of these are cheaply cloneable (`Arc` inside) and `Send + Sync`, so a
//! single instance can be shared across many tasks.
//!
//! # Reusing this for another API
//!
//! ```no_run
//! use pulp::ratelimit::{AdaptiveConfig, KeyedThrottle, Outcome};
//!
//! // One shared config describing how politely to behave.
//! let cfg = AdaptiveConfig::default();
//!
//! // Independent lanes per endpoint class for some new API.
//! let throttles: KeyedThrottle<&'static str> = KeyedThrottle::new(cfg);
//!
//! # async fn demo(throttles: KeyedThrottle<&'static str>) -> Result<(), Box<dyn std::error::Error>> {
//! // In your request path:
//! let lane = throttles.lane("search");
//! let body = lane.run(|| async {
//!     // ... perform the HTTP call ...
//!     // Map the response to an `Outcome` so the throttle can adapt:
//!     //   Outcome::Success           -> on 2xx
//!     //   Outcome::Throttled { .. }  -> on 429 (pass the Retry-After if present)
//!     //   Outcome::Failure           -> on 5xx / transport error
//!     Ok::<_, std::io::Error>((Outcome::Success, "body"))
//! }).await?;
//! # let _ = body;
//! # Ok(())
//! # }
//! ```
//!
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::time::{sleep_until, Instant};

pub mod adaptive;
pub use adaptive::AdaptiveConfig;
use adaptive::AdaptiveController;

// ---------------------------------------------------------------------------
// Outcome
// ---------------------------------------------------------------------------

/// The result of a single request, as understood by the throttle.
///
/// Callers map their own response/error type onto this so the throttle can
/// adapt without knowing anything about the underlying API.
#[derive(Debug, Clone)]
pub enum Outcome {
    /// The request succeeded — gently increase the allowed rate.
    Success,
    /// The remote explicitly throttled us (e.g. HTTP 429). Multiplicatively
    /// decrease the rate and, if a `Retry-After` was supplied, pause until it
    /// elapses (capped by [`AdaptiveConfig::max_retry_after`]).
    Throttled {
        /// The server-advertised cool-down, if any (e.g. parsed from a
        /// `Retry-After` header).
        retry_after: Option<Duration>,
    },
    /// The request failed for some other reason (5xx, transport error, …). It
    /// does *not* change the adaptive rate (only an explicit 429 does).
    Failure,
}

// ---------------------------------------------------------------------------
// RateLimiter (adaptive interval pacer, AIMD)
// ---------------------------------------------------------------------------

/// A snapshot of a [`RateLimiter`]'s internal state, for observability/tests.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RateLimiterState {
    /// The current effective rate (`1 / interval`), in requests per second.
    pub rate_per_sec: f64,
    /// The current minimum spacing between requests, in seconds.
    pub interval_secs: f64,
    /// Whether the next request is currently gated to a future instant (a hard
    /// `Retry-After` pause, or simply the next paced slot).
    pub paused: bool,
}

/// An adaptive interval-pacing rate limiter using AIMD on the interval.
///
/// A thin, shareable async wrapper over the pure [`AdaptiveController`]: it holds
/// the controller behind a mutex, supplies it `Instant::now()`, and turns its
/// "wait until this instant" answers into async sleeps. All the control-law
/// behavior (additive loosen on success, multiplicative tighten on throttle,
/// non-bursting interval pacing, `Retry-After` pauses) lives in the controller.
///
/// Clone freely — clones share the same underlying controller.
#[derive(Debug, Clone)]
pub struct RateLimiter {
    inner: Arc<Mutex<AdaptiveController>>,
}

impl RateLimiter {
    /// Build a limiter from `cfg`. The config is validated/clamped by the
    /// underlying [`AdaptiveController`] (`min_interval > 0`,
    /// `max_interval >= min_interval`, `grow_factor >= 1`).
    pub fn new(cfg: AdaptiveConfig) -> Self {
        let controller = AdaptiveController::new(cfg, Instant::now());
        Self {
            inner: Arc::new(Mutex::new(controller)),
        }
    }

    /// Acquire one token, waiting (asynchronously) until one is available given
    /// the current adaptive rate and any active `Retry-After` pause.
    ///
    /// This never spins: it asks the controller exactly when the next token (or
    /// the end of a pause) is due and sleeps until then.
    pub async fn acquire(&self) {
        loop {
            let wait_until = {
                let mut inner = self.inner.lock().unwrap();
                inner.try_acquire(Instant::now())
            };
            match wait_until {
                Ok(()) => return,
                Err(t) => sleep_until(t).await,
            }
        }
    }

    /// Gentle additive loosen: feed one success. The interval shrinks by one
    /// `shrink_step` (floored at `min_interval`). A single success only nibbles
    /// the interval down; it cannot undo a multiplicative tighten in one step.
    pub fn on_success(&self) {
        self.inner.lock().unwrap().on_success(Instant::now());
    }

    /// Multiplicative tighten: a throttle response grows the interval by
    /// `grow_factor` (capped at `max_interval`), slowing issuance fast. If
    /// `retry_after` is given, refuse to issue requests until at least that long
    /// has elapsed (capped by `max_retry_after`).
    pub fn on_throttled(&self, retry_after: Option<Duration>) {
        self.inner
            .lock()
            .unwrap()
            .on_throttled(retry_after, Instant::now());
    }

    /// The current adaptive rate, in tokens per second.
    pub fn current_rate(&self) -> f64 {
        self.inner.lock().unwrap().rate()
    }

    /// A snapshot of internal state. Primarily for observability and tests.
    pub fn state(&self) -> RateLimiterState {
        let inner = self.inner.lock().unwrap();
        let now = Instant::now();
        RateLimiterState {
            rate_per_sec: inner.rate(),
            interval_secs: inner.interval().as_secs_f64(),
            paused: inner.is_paused(now),
        }
    }
}

// ---------------------------------------------------------------------------
// Throttle (ergonomic RateLimiter bundle)
// ---------------------------------------------------------------------------

/// An ergonomic wrapper around a [`RateLimiter`].
///
/// Clone freely — clones share the same underlying limiter.
#[derive(Debug, Clone)]
pub struct Throttle {
    limiter: RateLimiter,
}

impl Throttle {
    /// Build a throttle from an [`AdaptiveConfig`].
    pub fn new(cfg: AdaptiveConfig) -> Self {
        Self {
            limiter: RateLimiter::new(cfg),
        }
    }

    /// Access the underlying rate limiter.
    pub fn limiter(&self) -> &RateLimiter {
        &self.limiter
    }

    /// Wait for a rate-limiter token. Pair with [`report`](Self::report) once
    /// the request completes so the limiter can adapt.
    pub async fn acquire(&self) {
        self.limiter.acquire().await;
    }

    /// Feed the outcome of a request back into the limiter so it can adapt.
    pub fn report(&self, outcome: &Outcome) {
        match outcome {
            Outcome::Success => self.limiter.on_success(),
            Outcome::Throttled { retry_after } => self.limiter.on_throttled(*retry_after),
            // A non-throttle failure doesn't change the rate.
            Outcome::Failure => {}
        }
    }

    /// Run `f` under the throttle: wait for a token, run the closure, then report
    /// its [`Outcome`] back to the limiter.
    ///
    /// `f` must return `Ok((Outcome, T))` on completion (mapping its own
    /// response to an [`Outcome`]) or `Err(E)` on an internal error. An `Err`
    /// is propagated to the caller unchanged.
    pub async fn run<F, Fut, T, E>(&self, f: F) -> Result<T, E>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<(Outcome, T), E>>,
    {
        self.acquire().await;
        let (outcome, value) = f().await?;
        self.report(&outcome);
        Ok(value)
    }
}

// ---------------------------------------------------------------------------
// KeyedThrottle
// ---------------------------------------------------------------------------

/// A map of independent [`Throttle`] "lanes" keyed by `K`.
///
/// Lanes are created lazily from a shared [`AdaptiveConfig`] the first time a
/// key is requested. Use one key per *endpoint class* so that throttling on one
/// endpoint does not affect the others.
///
/// Clone freely — clones share the same lane map.
#[derive(Debug, Clone)]
pub struct KeyedThrottle<K: Eq + Hash + Clone> {
    cfg: AdaptiveConfig,
    lanes: Arc<Mutex<HashMap<K, Throttle>>>,
}

impl<K: Eq + Hash + Clone> KeyedThrottle<K> {
    /// Build a keyed throttle whose lanes are all created from `cfg`.
    pub fn new(cfg: AdaptiveConfig) -> Self {
        Self {
            cfg,
            lanes: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Get (creating if necessary) the [`Throttle`] lane for `key`. The
    /// returned `Throttle` is a cheap clone sharing the lane's state.
    pub fn lane(&self, key: K) -> Throttle {
        let mut lanes = self.lanes.lock().unwrap();
        lanes
            .entry(key)
            .or_insert_with(|| Throttle::new(self.cfg.clone()))
            .clone()
    }

    /// Non-creating peek: return a clone of the [`Throttle`] lane for `key` if
    /// it has already been materialized, without creating one as a side effect.
    /// Used by read-only observability handlers that must not spin up a lane.
    pub fn peek(&self, key: &K) -> Option<Throttle> {
        self.lanes.lock().unwrap().get(key).cloned()
    }

    /// Number of lanes currently materialized.
    pub fn len(&self) -> usize {
        self.lanes.lock().unwrap().len()
    }

    /// Whether no lanes have been materialized yet.
    pub fn is_empty(&self) -> bool {
        self.lanes.lock().unwrap().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{advance, Duration};

    fn fast_cfg() -> AdaptiveConfig {
        // Deterministic interval pacing. Initial interval 0.5s (rate 2/s),
        // floor 0.125s (rate 8/s), grow ×2 on a 429, shrink 0.25s per success.
        AdaptiveConfig {
            initial_interval: Duration::from_millis(500),
            min_interval: Duration::from_millis(125),
            max_interval: Duration::from_secs(8),
            grow_factor: 2.0,
            shrink_step: Duration::from_millis(250),
            max_retry_after: Duration::from_secs(60),
        }
    }

    /// The first request issues immediately, then each subsequent request is
    /// paced by one `interval` (0.5s) of virtual time — no burst.
    #[tokio::test(start_paused = true)]
    async fn acquire_paces_according_to_interval() {
        let rl = RateLimiter::new(fast_cfg()); // interval 0.5s
        let start = Instant::now();

        // First acquire issues immediately.
        rl.acquire().await;
        assert_eq!(Instant::now(), start);

        // Second acquire must wait one interval = 0.5s.
        let h = tokio::spawn({
            let rl = rl.clone();
            async move { rl.acquire().await }
        });
        // Not enough time yet.
        advance(Duration::from_millis(499)).await;
        assert!(!h.is_finished());
        // Now enough.
        advance(Duration::from_millis(2)).await;
        h.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn acquire_n_requests_takes_n_minus_one_intervals() {
        // interval 0.5s. 5 requests: first immediate, remaining 4 at 0.5s each
        // = 2.0s total.
        let rl = RateLimiter::new(fast_cfg());
        let start = Instant::now();
        let h = tokio::spawn({
            let rl = rl.clone();
            async move {
                for _ in 0..5 {
                    rl.acquire().await;
                }
            }
        });
        advance(Duration::from_millis(1999)).await;
        assert!(!h.is_finished());
        advance(Duration::from_millis(2)).await;
        h.await.unwrap();
        assert!(Instant::now() >= start + Duration::from_secs(2));
    }

    #[tokio::test(start_paused = true)]
    async fn on_success_additively_loosens_clamped_to_min() {
        // Each success shrinks the interval by 0.25s, floored at 0.125s.
        let rl = RateLimiter::new(fast_cfg()); // interval 0.5s → rate 2/s
        assert_eq!(rl.current_rate(), 2.0);

        rl.on_success(); // 0.5 → 0.25s → rate 4/s
        assert_eq!(rl.current_rate(), 4.0);
        rl.on_success(); // 0.25 → 0.125s (floor) → rate 8/s
        assert_eq!(rl.current_rate(), 8.0);
        // Already at the floor; stays clamped.
        rl.on_success();
        assert_eq!(rl.current_rate(), 8.0);
    }

    #[tokio::test(start_paused = true)]
    async fn on_throttled_multiplicatively_tightens_clamped() {
        let rl = RateLimiter::new(fast_cfg()); // interval 0.5s, ×2, max 8s
        rl.on_throttled(None); // 0.5 → 1s → rate 1/s
        assert_eq!(rl.current_rate(), 1.0);
        rl.on_throttled(None); // 1 → 2s → rate 0.5/s
        assert_eq!(rl.current_rate(), 0.5);
        // Drive to the ceiling (8s → rate 0.125/s) and confirm clamping.
        for _ in 0..10 {
            rl.on_throttled(None);
        }
        assert_eq!(rl.state().interval_secs, 8.0);
    }

    #[tokio::test(start_paused = true)]
    async fn retry_after_hard_pause_is_honored() {
        let rl = RateLimiter::new(fast_cfg());
        // Issue once so next_allowed is armed.
        rl.acquire().await;

        rl.on_throttled(Some(Duration::from_secs(5)));
        assert!(rl.state().paused);

        let h = tokio::spawn({
            let rl = rl.clone();
            async move { rl.acquire().await }
        });
        // The hard pause blocks issuance until the retry-after elapses.
        advance(Duration::from_millis(4999)).await;
        assert!(!h.is_finished());
        advance(Duration::from_millis(2)).await;
        // The acquire only completes once the ~5s pause elapses (proving it was
        // honored). Note: `paused` now means "next slot is in the future", which
        // is true immediately after any issue (the just-armed +interval slot), so
        // it is not a meaningful post-issue assertion under interval pacing.
        h.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn retry_after_is_capped_by_max_retry_after() {
        let mut cfg = fast_cfg();
        cfg.max_retry_after = Duration::from_secs(2);
        let rl = RateLimiter::new(cfg);
        rl.acquire().await; // arm next_allowed

        // Ask for a hostile 1-hour pause; it must be capped to 2s.
        rl.on_throttled(Some(Duration::from_secs(3600)));
        let h = tokio::spawn({
            let rl = rl.clone();
            async move { rl.acquire().await }
        });
        advance(Duration::from_millis(1999)).await;
        assert!(!h.is_finished());
        advance(Duration::from_millis(2)).await;
        h.await.unwrap();
    }

    // --- KeyedThrottle ----------------------------------------------------

    #[tokio::test(start_paused = true)]
    async fn keyed_throttle_lazily_creates_and_caches_lanes() {
        let kt: KeyedThrottle<&'static str> = KeyedThrottle::new(fast_cfg());
        assert!(kt.is_empty());
        let a1 = kt.lane("search");
        assert_eq!(kt.len(), 1);
        let a2 = kt.lane("search");
        assert_eq!(kt.len(), 1); // cached, same lane
                                 // Same underlying state: adapting via a1 is visible via a2.
        a1.limiter().on_throttled(None);
        assert_eq!(a1.limiter().current_rate(), a2.limiter().current_rate());
    }

    #[tokio::test(start_paused = true)]
    async fn keyed_throttle_isolates_lanes() {
        let kt: KeyedThrottle<&'static str> = KeyedThrottle::new(fast_cfg());
        let search = kt.lane("search");
        let feed = kt.lane("feed");
        assert_eq!(kt.len(), 2);

        // Throttle "search" hard; "feed" must be untouched.
        for _ in 0..10 {
            search.limiter().on_throttled(None);
        }
        assert_eq!(search.limiter().state().interval_secs, 8.0); // ceiling
        assert_eq!(feed.limiter().current_rate(), 2.0); // unaffected
    }

    // --- Throttle convenience --------------------------------------------

    #[tokio::test(start_paused = true)]
    async fn throttle_run_success_returns_value_and_loosens() {
        let t = Throttle::new(fast_cfg());
        // Tighten first so there's headroom to loosen into.
        t.report(&Outcome::Throttled { retry_after: None });
        let after_throttle = t.limiter().current_rate();

        let v: i32 = t
            .run(|| async { Ok::<_, std::convert::Infallible>((Outcome::Success, 42)) })
            .await
            .unwrap();
        assert_eq!(v, 42);
        // A success loosens the interval (raises the rate).
        assert!(t.limiter().current_rate() > after_throttle);
    }

    #[tokio::test(start_paused = true)]
    async fn throttle_run_err_propagates() {
        let t = Throttle::new(fast_cfg());
        let res: Result<(), &'static str> = t.run(|| async { Err("boom") }).await;
        assert_eq!(res.unwrap_err(), "boom");
    }
}
