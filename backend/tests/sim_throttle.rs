//! Standalone, deterministic SANDBOX simulation for the Reddit rate-limiter
//! starvation bug. This is NOT a test of production code — it is a self-contained
//! discrete-event model used to (a) reproduce the observed "2 succeed, 1 starves"
//! equilibrium under the *current* control law and (b) compare candidate pacing
//! strategies against several plausible upstream-throttle models, so we can pick
//! the SIMPLEST strategy that avoids starvation and converges under ALL of them
//! without per-model tuning.
//!
//! Run it:
//!     cargo test --test sim_throttle -- --nocapture
//!
//! Everything here is pure and deterministic: virtual time is an `f64` seconds
//! counter, there is no wall clock, no async, no tokio, and a fixed RNG seed, so
//! runs are reproducible. No production code is touched and no new crate deps are
//! added.
//!
//! ## Why this is faithful to production (read alongside the real files)
//!
//! - `backend/src/collectors/scheduler.rs::run_targeted_pass` processes the
//!   channel's targets **sequentially in a fixed order** each pass; each target
//!   does `lane.acquire().await` then a fetch, then reports Success/Throttled/
//!   Failure. In steady state a head fetch is a single page (no new content), so
//!   that's effectively **one acquire+fetch per target per pass**. We model
//!   exactly that.
//! - All Reddit targets share **ONE lane** (`lane = "reddit"`) — the rate limit
//!   is per-IP/global, so per-target lanes are not an option. We model one shared
//!   controller across all 3 targets.
//! - Between passes the collector sleeps `poll_interval` (prod 120s). The limiter
//!   state persists across passes (it lives in `AppState`). During that idle
//!   sleep a `max_burst=1` token bucket refills, so the FIRST acquire of every
//!   pass returns immediately regardless of how low AIMD drove the rate — this is
//!   the idle-burst bug, mechanism #1.
//! - `S0_current` is a faithful port of `AdaptiveController` (token bucket,
//!   burst=1, slow time-gated AIMD, multiplicative decrease, idle refill) with the
//!   PRODUCTION params from `scheduler.rs::default_throttles`.

use std::collections::VecDeque;
use std::time::Duration;

use pulp::ratelimit::adaptive::{AdaptiveConfig, AdaptiveController};
use tokio::time::Instant as TokioInstant;

// ───────────────────────────────────────────────────────────────────────────
// Virtual time + deterministic RNG
// ───────────────────────────────────────────────────────────────────────────

/// Tiny deterministic LCG so runs are reproducible without a crate dep. We only
/// use it where a model needs jitter; the core dynamics are deterministic.
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1))
    }
    #[allow(dead_code)]
    fn next_f64(&mut self) -> f64 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        ((x.wrapping_mul(0x2545_F491_4F6C_DD1D)) >> 11) as f64 / (1u64 << 53) as f64
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Upstream throttle models (the server side) — at least 3, behind one trait.
// ───────────────────────────────────────────────────────────────────────────

/// What the upstream did with a request that arrived at virtual time `now`.
#[derive(Debug, Clone, Copy, PartialEq)]
enum ServerResp {
    Ok,
    /// 429; optional server-advertised Retry-After (seconds).
    Throttled(Option<f64>),
}

/// A black-box upstream policy. `request(now)` is called the instant the client
/// actually issues a request (after its own pacing), and mutates server state.
trait ServerModel {
    fn name(&self) -> &'static str;
    fn request(&mut self, now: f64) -> ServerResp;
    /// Reset between strategy runs so each (strategy × model) pair starts clean.
    fn reset(&mut self);
}

/// TokenBucket: capacity C, refill r tokens/sec. A request consumes a token or
/// 429s. LENIENT: a cold request after a long idle always succeeds (bucket is
/// full), which is precisely what lets the idle-burst bug hide.
struct TokenBucketServer {
    capacity: f64,
    refill_per_sec: f64,
    tokens: f64,
    last: f64,
    inited: bool,
}
impl TokenBucketServer {
    fn new(capacity: f64, refill_per_sec: f64) -> Self {
        Self {
            capacity,
            refill_per_sec,
            tokens: capacity,
            last: 0.0,
            inited: false,
        }
    }
}
impl ServerModel for TokenBucketServer {
    fn name(&self) -> &'static str {
        "TokenBucket"
    }
    fn request(&mut self, now: f64) -> ServerResp {
        if !self.inited {
            self.last = now;
            self.inited = true;
        }
        let dt = (now - self.last).max(0.0);
        self.tokens = (self.tokens + self.refill_per_sec * dt).min(self.capacity);
        self.last = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            ServerResp::Ok
        } else {
            ServerResp::Throttled(None)
        }
    }
    fn reset(&mut self) {
        self.tokens = self.capacity;
        self.last = 0.0;
        self.inited = false;
    }
}

/// SlidingWindow: at most K requests per window W seconds; over → 429. STICKY:
/// well-spaced requests still accumulate within the window, so a cold probe can
/// still 429 if recent history filled the window. This best explains the observed
/// "cold first probe still 429s". Optionally advertises Retry-After = time until
/// the oldest in-window request ages out.
struct SlidingWindowServer {
    k: usize,
    window: f64,
    advertise_retry_after: bool,
    /// If true, a REJECTED attempt also occupies a window slot — the "abuse
    /// detector counts every request you make, accepted or not" variant. This is
    /// exactly what makes a fixed-phase cold probe SELF-POISON: its own rejected
    /// attempt 120s ago is still on the books when it probes again, so it can
    /// never clear its own slot — the perpetual first-target starvation.
    count_rejected: bool,
    times: VecDeque<f64>,
}
impl SlidingWindowServer {
    fn new(k: usize, window: f64, advertise_retry_after: bool, count_rejected: bool) -> Self {
        Self {
            k,
            window,
            advertise_retry_after,
            count_rejected,
            times: VecDeque::new(),
        }
    }
}
impl ServerModel for SlidingWindowServer {
    fn name(&self) -> &'static str {
        "SlidingWindow"
    }
    fn request(&mut self, now: f64) -> ServerResp {
        while let Some(&front) = self.times.front() {
            if now - front >= self.window {
                self.times.pop_front();
            } else {
                break;
            }
        }
        if self.times.len() < self.k {
            self.times.push_back(now);
            ServerResp::Ok
        } else {
            let oldest = *self.times.front().unwrap();
            let retry = if self.advertise_retry_after {
                Some((oldest + self.window - now).max(0.0))
            } else {
                None
            };
            if self.count_rejected {
                // The attempt counts against you anyway: it pushes the window
                // forward so an over-eager poller keeps itself locked out.
                self.times.push_back(now);
                self.times.pop_front();
            }
            ServerResp::Throttled(retry)
        }
    }
    fn reset(&mut self) {
        self.times.clear();
    }
}

/// PenaltyBox: once a 429 is hit, ALL requests 429 for a lockout L seconds
/// (regardless of spacing), then resume. HARSH: punishes probing. Underneath the
/// lockout it is a sliding window, so over-fast steady traffic keeps re-tripping.
struct PenaltyBoxServer {
    k: usize,
    window: f64,
    lockout: f64,
    times: VecDeque<f64>,
    locked_until: f64,
}
impl PenaltyBoxServer {
    fn new(k: usize, window: f64, lockout: f64) -> Self {
        Self {
            k,
            window,
            lockout,
            times: VecDeque::new(),
            locked_until: f64::NEG_INFINITY,
        }
    }
}
impl ServerModel for PenaltyBoxServer {
    fn name(&self) -> &'static str {
        "PenaltyBox"
    }
    fn request(&mut self, now: f64) -> ServerResp {
        if now < self.locked_until {
            // Locked out: every request is rejected, advertise time remaining.
            return ServerResp::Throttled(Some(self.locked_until - now));
        }
        while let Some(&front) = self.times.front() {
            if now - front >= self.window {
                self.times.pop_front();
            } else {
                break;
            }
        }
        if self.times.len() < self.k {
            self.times.push_back(now);
            ServerResp::Ok
        } else {
            // Trip the penalty box.
            self.locked_until = now + self.lockout;
            self.times.clear();
            ServerResp::Throttled(Some(self.lockout))
        }
    }
    fn reset(&mut self) {
        self.times.clear();
        self.locked_until = f64::NEG_INFINITY;
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Client pacing strategies (the thing we are choosing) — behind one trait.
// ───────────────────────────────────────────────────────────────────────────

/// A client pacing strategy. Mirrors the real `try_acquire`/`on_success`/
/// `on_throttled(retry_after)` interface, all time-injected (take `now`).
///
/// `earliest_issue(now)` returns the earliest virtual time the client is allowed
/// to issue its NEXT request, given everything that's happened so far. The sim
/// fast-forwards to that instant. This captures both token-bucket pacing and a
/// pure "earliest-next-instant" interval, uniformly.
trait Strategy {
    fn name(&self) -> &'static str;
    /// Earliest time the next request may be issued (>= now).
    fn earliest_issue(&mut self, now: f64) -> f64;
    /// Record that a request was issued at `now` (consume a token / set the next
    /// interval anchor). Called right before the server sees it.
    fn on_issue(&mut self, now: f64);
    fn on_success(&mut self, now: f64);
    fn on_throttled(&mut self, retry_after: Option<f64>, now: f64);
    /// Short human string of internal state, for tracing only.
    fn debug_state(&self) -> String {
        String::new()
    }
}

// ── S0_current: faithful port of the production AdaptiveController ──────────
//
// Token bucket + slow time-gated AIMD, burst=1, idle refill. PRODUCTION params
// from scheduler.rs::default_throttles (per-second).
struct S0Current {
    // config
    min_rate: f64,
    max_rate: f64,
    inc_step: f64,
    inc_interval: f64,
    dec_factor: f64,
    max_burst: f64,
    max_retry_after: f64,
    // state
    rate: f64,
    tokens: f64,
    last_refill: f64,
    next_increase_at: f64,
    paused_until: Option<f64>,
    started: bool,
}
impl S0Current {
    fn production() -> Self {
        // default_throttles(): initial 8/min, min 0.5/min, max 10/min,
        // +1/min step once per 60s, x0.5 on 429, burst 1, retry cap 300s.
        Self {
            min_rate: 0.5 / 60.0,
            max_rate: 10.0 / 60.0,
            inc_step: 1.0 / 60.0,
            inc_interval: 60.0,
            dec_factor: 0.5,
            max_burst: 1.0,
            max_retry_after: 300.0,
            rate: 8.0 / 60.0,
            tokens: 1.0, // starts full (max_burst)
            last_refill: 0.0,
            next_increase_at: 60.0,
            paused_until: None,
            started: false,
        }
    }
    fn refill(&mut self, now: f64) {
        let dt = (now - self.last_refill).max(0.0);
        if dt > 0.0 {
            self.tokens = (self.tokens + self.rate * dt).min(self.max_burst);
            self.last_refill = now;
        }
    }
}
impl Strategy for S0Current {
    fn name(&self) -> &'static str {
        "S0_current"
    }
    fn earliest_issue(&mut self, now: f64) -> f64 {
        if !self.started {
            self.last_refill = now;
            self.next_increase_at = now + self.inc_interval;
            self.started = true;
        }
        if let Some(until) = self.paused_until {
            if now < until {
                return until;
            }
            self.paused_until = None;
            self.last_refill = now;
        }
        self.refill(now);
        if self.tokens >= 1.0 {
            now
        } else {
            let deficit = 1.0 - self.tokens;
            now + deficit / self.rate
        }
    }
    fn on_issue(&mut self, now: f64) {
        self.refill(now);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
        } else {
            // Shouldn't happen (sim issues only at earliest_issue); be safe.
            self.tokens = 0.0;
        }
    }
    fn on_success(&mut self, now: f64) {
        self.refill(now);
        if now >= self.next_increase_at {
            self.rate = (self.rate + self.inc_step).min(self.max_rate);
            self.next_increase_at = now + self.inc_interval;
        }
    }
    fn on_throttled(&mut self, retry_after: Option<f64>, now: f64) {
        self.refill(now);
        self.rate = (self.rate * self.dec_factor).max(self.min_rate);
        self.next_increase_at = now + self.inc_interval;
        if let Some(ra) = retry_after {
            let capped = ra.min(self.max_retry_after);
            let until = now + capped;
            self.paused_until = Some(match self.paused_until {
                Some(e) if e > until => e,
                _ => until,
            });
        }
    }
    fn debug_state(&self) -> String {
        format!("rate={:.2}/min tok={:.2}", self.rate * 60.0, self.tokens)
    }
}

// ── S1_pure_pacing_aimd: NO burst, single persisted earliest-next instant ───
//
// One global minimum interval between ANY two requests, persisted across passes
// via `next_allowed` (the earliest-next-request instant). The FIRST request of
// each pass is paced too — there is no idle-refill free pass. AIMD on the
// interval: 429 grows it (×2, capped), success shrinks it (gentle additive),
// floored. A recurring 429 dominates (×2 down-moves fast), so it converges and
// holds below the sustainable rate.
struct S1PurePacing {
    min_interval: f64,
    max_interval: f64,
    interval: f64,
    shrink_step: f64, // additive shrink per success (gentle)
    grow_factor: f64, // multiplicative grow per 429
    next_allowed: f64,
    started: bool,
}
impl S1PurePacing {
    fn new() -> Self {
        Self {
            // Floor: never faster than ~10/min (Reddit's documented budget) — same
            // intent as production max_rate. 1/(10/60) = 6s.
            min_interval: 6.0,
            // Ceiling: never slower than ~0.5/min (production floor) → 120s.
            max_interval: 120.0,
            interval: 60.0 / 8.0, // start at 8/min = 7.5s (production initial rate)
            shrink_step: 1.0,     // gently loosen 1s per success
            grow_factor: 2.0,     // double on a 429
            next_allowed: 0.0,
            started: false,
        }
    }
}
impl Strategy for S1PurePacing {
    fn name(&self) -> &'static str {
        "S1_pure_pacing_aimd"
    }
    fn earliest_issue(&mut self, now: f64) -> f64 {
        if !self.started {
            self.next_allowed = now;
            self.started = true;
        }
        self.next_allowed.max(now)
    }
    fn on_issue(&mut self, now: f64) {
        // The next request can't be issued before now + interval. Crucially this
        // is anchored at the ISSUE time, so a long idle does NOT bank credit —
        // the first request after a sleep is still paced relative to the last
        // actual issue (here, immediate), and the NEXT one waits a full interval.
        self.next_allowed = now + self.interval;
    }
    fn on_success(&mut self, now: f64) {
        let _ = now;
        self.interval = (self.interval - self.shrink_step).max(self.min_interval);
    }
    fn on_throttled(&mut self, retry_after: Option<f64>, now: f64) {
        self.interval = (self.interval * self.grow_factor).min(self.max_interval);
        // Respect a server Retry-After by pushing the next-allowed instant out,
        // but the interval growth is what actually converges the rate.
        if let Some(ra) = retry_after {
            self.next_allowed = self.next_allowed.max(now + ra.min(self.max_interval));
        }
    }
}

// ── S2_clean_pass_gate: interval AIMD that only loosens after a CLEAN streak ─
//
// Same single-interval pacing as S1 (no burst, persisted), but recovery is gated
// on N *consecutive* clean successes rather than loosening on every success. Any
// 429 doubles the interval AND resets the clean counter. This is the interval-
// space analogue of production's "sustained success" rule, but simpler: count
// successes, not wall-clock. Picked to be robust against the PenaltyBox (which
// punishes probing): loosening only after a sustained clean run avoids creeping
// back up too eagerly and re-tripping a lockout.
struct S2CleanPassGate {
    min_interval: f64,
    max_interval: f64,
    interval: f64,
    grow_factor: f64,
    shrink_step: f64,
    clean_needed: u32,
    clean_run: u32,
    next_allowed: f64,
    started: bool,
}
impl S2CleanPassGate {
    fn new() -> Self {
        Self {
            min_interval: 6.0,
            max_interval: 120.0,
            interval: 60.0 / 8.0, // 7.5s
            grow_factor: 2.0,
            shrink_step: 3.0, // bigger step, but gated behind a clean run
            clean_needed: 3,  // 3 consecutive clean successes before loosening
            clean_run: 0,
            next_allowed: 0.0,
            started: false,
        }
    }
}
impl Strategy for S2CleanPassGate {
    fn name(&self) -> &'static str {
        "S2_clean_pass_gate"
    }
    fn earliest_issue(&mut self, now: f64) -> f64 {
        if !self.started {
            self.next_allowed = now;
            self.started = true;
        }
        self.next_allowed.max(now)
    }
    fn on_issue(&mut self, now: f64) {
        self.next_allowed = now + self.interval;
    }
    fn on_success(&mut self, now: f64) {
        let _ = now;
        self.clean_run += 1;
        if self.clean_run >= self.clean_needed {
            self.interval = (self.interval - self.shrink_step).max(self.min_interval);
            self.clean_run = 0;
        }
    }
    fn on_throttled(&mut self, retry_after: Option<f64>, now: f64) {
        self.clean_run = 0;
        self.interval = (self.interval * self.grow_factor).min(self.max_interval);
        if let Some(ra) = retry_after {
            self.next_allowed = self.next_allowed.max(now + ra.min(self.max_interval));
        }
    }
}

// ── S3_real_controller: the REAL shipped AdaptiveController, driven directly ─
//
// This is the regression guard that matters: it drives the ACTUAL production
// `pulp::ratelimit::adaptive::AdaptiveController` (the type `RateLimiter` wraps)
// through the same f64 discrete-event harness as the sandbox strategies. The
// controller is pure and time-injected — its methods take a `tokio::time::Instant`
// and it never reads the clock itself — so we map the sim's f64 virtual seconds
// onto a single `base` Instant captured once (`base + Duration::from_secs_f64(t)`),
// which is pure arithmetic needing no tokio runtime.
//
// Config mirrors `scheduler.rs::default_throttles` PRODUCTION values: initial
// 7.5s (8/min), floor 6s (10/min), ceiling 120s, grow ×2, shrink 1s, retry cap
// 300s. If this starves a target under any upstream model, the shipped code does.
struct S3RealController {
    ctrl: AdaptiveController,
    base: TokioInstant,
}
impl S3RealController {
    fn new() -> Self {
        let base = TokioInstant::now();
        let cfg = AdaptiveConfig {
            initial_interval: Duration::from_secs_f64(60.0 / 8.0), // 7.5s
            min_interval: Duration::from_secs_f64(60.0 / 10.0),    // 6s
            max_interval: Duration::from_secs(120),
            grow_factor: 2.0,
            shrink_step: Duration::from_secs(1),
            max_retry_after: Duration::from_secs(300),
        };
        Self {
            ctrl: AdaptiveController::new(cfg, base),
            base,
        }
    }
    fn at(&self, t: f64) -> TokioInstant {
        self.base + Duration::from_secs_f64(t.max(0.0))
    }
}
impl Strategy for S3RealController {
    fn name(&self) -> &'static str {
        "S3_real_controller"
    }
    fn earliest_issue(&mut self, now: f64) -> f64 {
        // Side-effect-free peek at the next allowed slot (does NOT consume).
        let secs = self
            .ctrl
            .next_allowed()
            .saturating_duration_since(self.base)
            .as_secs_f64();
        secs.max(now)
    }
    fn on_issue(&mut self, now: f64) {
        // The harness calls this at exactly `earliest_issue`'s returned time, so
        // `try_acquire` succeeds here and arms `next_allowed` at now+interval —
        // identical to the real `RateLimiter::acquire` loop consuming a slot.
        let inst = self.at(now);
        let _ = self.ctrl.try_acquire(inst);
    }
    fn on_success(&mut self, now: f64) {
        let inst = self.at(now);
        self.ctrl.on_success(inst);
    }
    fn on_throttled(&mut self, retry_after: Option<f64>, now: f64) {
        let inst = self.at(now);
        let ra = retry_after.map(Duration::from_secs_f64);
        self.ctrl.on_throttled(ra, inst);
    }
    fn debug_state(&self) -> String {
        format!(
            "interval={:.1}s rate={:.2}/min",
            self.ctrl.interval().as_secs_f64(),
            self.ctrl.rate() * 60.0
        )
    }
}

// ───────────────────────────────────────────────────────────────────────────
// The simulation harness: faithful port of run_targeted_pass dynamics.
// ───────────────────────────────────────────────────────────────────────────

const N_TARGETS: usize = 3;
const POLL_INTERVAL: f64 = 120.0; // prod poll_interval
const HORIZON: f64 = 6.0 * 3600.0; // simulate 6 virtual hours

#[derive(Default, Clone)]
struct Stats {
    successes: [u64; N_TARGETS],
    throttles: [u64; N_TARGETS],
    /// Longest run of CONSECUTIVE 429s a target suffered with no success in
    /// between (the live bug reported "104 consecutive 429s, 0 successes"). The
    /// max of this across targets is the starvation severity headline.
    max_consec_429: [u64; N_TARGETS],
    cur_consec_429: [u64; N_TARGETS],
    /// First virtual time (s) at which EVERY target had >= 1 success — a coarse
    /// "everyone is making progress" convergence marker. None if never reached.
    all_progress_at: Option<f64>,
    /// First time at which a full clean pass (all 3 targets OK in one pass)
    /// occurred — a stricter steady-state marker. None if never.
    first_clean_pass_at: Option<f64>,
}

fn run_one(strategy: &mut dyn Strategy, server: &mut dyn ServerModel, _seed: u64) -> Stats {
    run_one_inner(strategy, server, false, HORIZON)
}

/// `rigid_grid`: when true, model a fixed-tick poller (`tokio::time::interval`)
/// whose passes start on an exact POLL_INTERVAL grid (skipping a tick if a pass
/// overran). This locks the cold first probe's PHASE, reproducing the *first*-
/// target starvation specifically. When false, model the production `sleep`-
/// after-pass scheduler (cadence anchored at pass end, free to drift).
/// `horizon` lets the focused test observe the locked regime before production
/// S0's rate decay de-syncs the grid.
fn run_one_inner(
    strategy: &mut dyn Strategy,
    server: &mut dyn ServerModel,
    rigid_grid: bool,
    horizon: f64,
) -> Stats {
    server.reset();
    let trace = std::env::var("SIM_TRACE").is_ok();
    let mut pass_no = 0u32;
    let mut stats = Stats::default();
    let mut now = 0.0_f64;

    // The collector loop: each pass walks the 3 targets in FIXED order. Between
    // passes it sleeps POLL_INTERVAL. The strategy state persists across passes.
    while now < horizon {
        let pass_start = now;
        let mut clean_pass = true;

        for t in 0..N_TARGETS {
            // 1. Acquire (pace). Fast-forward virtual time to the earliest the
            //    strategy will let us issue.
            let issue_at = strategy.earliest_issue(now).max(now);
            now = issue_at;
            if now >= horizon {
                break;
            }
            strategy.on_issue(now);

            // 2. The upstream sees the request at `now`.
            let resp = server.request(now);
            if trace && pass_no < 12 {
                println!(
                    "    pass {:>2} t{} @ {:>7.1}s -> {:?}  [{}]",
                    pass_no,
                    t,
                    now,
                    resp,
                    strategy.debug_state()
                );
            }

            // 3. Report the outcome back to the strategy (Success / Throttled).
            match resp {
                ServerResp::Ok => {
                    stats.successes[t] += 1;
                    stats.cur_consec_429[t] = 0;
                    strategy.on_success(now);
                    if stats.successes.iter().all(|&c| c > 0) && stats.all_progress_at.is_none() {
                        stats.all_progress_at = Some(now);
                    }
                }
                ServerResp::Throttled(ra) => {
                    stats.throttles[t] += 1;
                    stats.cur_consec_429[t] += 1;
                    stats.max_consec_429[t] = stats.max_consec_429[t].max(stats.cur_consec_429[t]);
                    clean_pass = false;
                    strategy.on_throttled(ra, now);
                    // Production: on a 429 the head fetch records the failure and
                    // RETURNS (stops paging this target this pass). The next
                    // target still runs in the same pass. We model exactly that:
                    // continue to the next target.
                }
            }
        }

        if clean_pass && stats.first_clean_pass_at.is_none() {
            stats.first_clean_pass_at = Some(pass_start);
        }

        pass_no += 1;
        if rigid_grid {
            // Fixed-tick poller: next pass starts on the next POLL_INTERVAL grid
            // multiple at/after now (skip a tick if the pass overran). Keeps the
            // cold first probe on a rigid phase → first-target starvation locks.
            let _ = pass_start;
            now = ((now / POLL_INTERVAL).floor() + 1.0) * POLL_INTERVAL;
        } else {
            // Faithful to the real scheduler (`collectors::run_collector`): it runs
            // the whole sequential pass — where `lane.acquire().await` BLOCKS, so a
            // low rate stretches the pass — and only THEN `sleep(poll_interval)`.
            // So the inter-pass sleep is anchored at pass END (free to drift).
            let _ = pass_start;
            now += POLL_INTERVAL;
        }
    }

    stats
}

// ───────────────────────────────────────────────────────────────────────────
// Metrics
// ───────────────────────────────────────────────────────────────────────────

fn jains_fairness(counts: &[u64; N_TARGETS]) -> f64 {
    let sum: f64 = counts.iter().map(|&c| c as f64).sum();
    let sum_sq: f64 = counts.iter().map(|&c| (c as f64) * (c as f64)).sum();
    if sum_sq == 0.0 {
        return 1.0;
    }
    (sum * sum) / (N_TARGETS as f64 * sum_sq)
}

fn min_max_ratio(counts: &[u64; N_TARGETS]) -> f64 {
    let min = *counts.iter().min().unwrap() as f64;
    let max = *counts.iter().max().unwrap() as f64;
    if max == 0.0 {
        1.0
    } else {
        min / max
    }
}

fn fmt_hms(secs: Option<f64>) -> String {
    match secs {
        None => "never".to_string(),
        Some(s) => {
            let m = (s / 60.0).round() as i64;
            format!("{}m", m)
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────
// The driver: every strategy × every server model, printed as a table.
// ───────────────────────────────────────────────────────────────────────────

fn make_strategies() -> Vec<Box<dyn Strategy>> {
    vec![
        Box::new(S0Current::production()),
        Box::new(S1PurePacing::new()),
        Box::new(S2CleanPassGate::new()),
        Box::new(S3RealController::new()),
    ]
}

fn make_servers() -> Vec<Box<dyn ServerModel>> {
    // Parameters chosen so the SUSTAINABLE rate is BELOW what "3 big queries every
    // 120s" wants AND the stickiness window is LONG relative to a pass, so the
    // previous pass's (paced, late) requests are still "on the books" when the
    // next pass's COLD first probe (target 0) fires. That is exactly the regime in
    // which mechanism #1 (idle-burst free first request) makes target 0 the
    // perpetual casualty: it always probes into a not-yet-recovered window, while
    // AIMD's rate cut only spaces targets 1..N out into the recovered tail.
    //
    // Sustainable ceiling here is ~2/min; a pass wants 3/120s = 1.5/min but in a
    // BURST, and the windows are minutes long, so the burst pattern is what trips
    // them — not the long-run average.
    vec![
        // Lenient bucket: capacity 1.5, refill 2/min. A cold probe after a long
        // idle still succeeds (bucket refilled), so the idle-burst bug hides best
        // here — yet shared-lane re-inflation (mechanism #2) still skews fairness.
        Box::new(TokenBucketServer::new(1.5, 2.0 / 60.0)),
        // Sticky sliding window: <=4 requests per 300s, advertises Retry-After.
        // The window (5 min) is far longer than the 120s inter-pass gap, so the
        // (paced) requests from the previous pass are still on the books when the
        // next pass's cluster fires — the cluster's victim probe keeps walking
        // into a not-yet-recovered window. This is the "2 succeed, 1 starves"
        // regime: the window serves two of the three but not the third, and under
        // S0 the casualty is stable (a positional victim of the shared-lane
        // burst + AIMD spacing). Best explains the live "one target 429s for
        // hours" behavior.
        Box::new(SlidingWindowServer::new(4, 300.0, true, false)),
        // Harsh penalty box: <=3 requests / 180s, then a 240s global lockout.
        // Punishes any probing burst; the lockout straddles the next pass start so
        // the cold first probe keeps walking into an active lockout.
        Box::new(PenaltyBoxServer::new(3, 180.0, 240.0)),
    ]
}

#[test]
fn sim_throttle_strategy_comparison() {
    const SEED: u64 = 0xC0FFEE;
    let _ = Rng::new(SEED); // seed reserved for any jitter models; dynamics are deterministic

    println!();
    println!("================================================================================");
    println!(
        " Rate-limiter starvation simulation — {} targets share ONE lane",
        N_TARGETS
    );
    println!(
        " Horizon: {:.0}h virtual, poll interval: {:.0}s, seed: {:#x}",
        HORIZON / 3600.0,
        POLL_INTERVAL,
        SEED
    );
    println!(
        " A pass demands {} requests / {:.0}s; sustainable upstream rate is set BELOW that,",
        N_TARGETS, POLL_INTERVAL
    );
    println!(" so the channel MUST slow down (not all 3 can succeed every pass).");
    println!("================================================================================");

    // Starvation, defined two ways (a target is starved if EITHER holds):
    //   * fairness: its success count is < 25% of the busiest target's, OR
    //   * stall: it suffered a long unbroken run of 429s (>= 20 consecutive,
    //     the live-bug signature — "104 consecutive 429s, 0 successes").
    fn classify_starved(succ: &[u64; N_TARGETS], consec: &[u64; N_TARGETS]) -> (bool, usize) {
        let max_succ = *succ.iter().max().unwrap();
        let mut worst = usize::MAX;
        let mut starved = false;
        for t in 0..N_TARGETS {
            let unfair = max_succ > 0 && (succ[t] as f64) < 0.25 * (max_succ as f64);
            let stalled = consec[t] >= 20;
            if succ[t] == 0 || unfair || stalled {
                starved = true;
                if worst == usize::MAX || succ[t] < succ[worst] {
                    worst = t;
                }
            }
        }
        (starved, worst)
    }

    // Collect results for a final verdict pass.
    struct Row {
        strat: &'static str,
        server: &'static str,
        consec: [u64; N_TARGETS],
        thr_total: u64,
        succ_total: u64,
        starved: bool,
        jain: f64,
        mmr: f64,
        all_prog: Option<f64>,
        clean: Option<f64>,
    }
    let mut rows: Vec<Row> = Vec::new();

    for server_idx in 0..make_servers().len() {
        let server_name = make_servers()[server_idx].name();
        println!();
        println!(
            "┌─ Upstream model: {} ─────────────────────────────────────────",
            server_name
        );
        println!(
            "│ {:<22} {:>8} {:>8} {:>8} {:>7} {:>9} {:>7} {:>8} {:>10}",
            "strategy",
            "t0_ok",
            "t1_ok",
            "t2_ok",
            "429s",
            "succ_tot",
            "Jain",
            "min/max",
            "maxRun429"
        );
        println!("│ {}", "-".repeat(96));

        for strat_idx in 0..make_strategies().len() {
            let mut strat = make_strategies().into_iter().nth(strat_idx).unwrap();
            let mut server = make_servers().into_iter().nth(server_idx).unwrap();
            let stats = run_one(strat.as_mut(), server.as_mut(), SEED);

            let succ_total: u64 = stats.successes.iter().sum();
            let thr_total: u64 = stats.throttles.iter().sum();
            let (starved, worst_target) = classify_starved(&stats.successes, &stats.max_consec_429);
            let jain = jains_fairness(&stats.successes);
            let mmr = min_max_ratio(&stats.successes);
            let max_run = *stats.max_consec_429.iter().max().unwrap();

            let star = if starved {
                format!(" ⚠STARVE(t{})", worst_target)
            } else {
                String::new()
            };
            println!(
                "│ {:<22} {:>8} {:>8} {:>8} {:>7} {:>9} {:>7.3} {:>8.3} {:>10}{}",
                strat.name(),
                stats.successes[0],
                stats.successes[1],
                stats.successes[2],
                thr_total,
                succ_total,
                jain,
                mmr,
                max_run,
                star,
            );

            rows.push(Row {
                strat: strat.name(),
                server: server_name,
                consec: stats.max_consec_429,
                thr_total,
                succ_total,
                starved,
                jain,
                mmr,
                all_prog: stats.all_progress_at,
                clean: stats.first_clean_pass_at,
            });
        }
        println!("└{}", "─".repeat(78));
    }

    // ── Convergence detail ───────────────────────────────────────────────────
    println!();
    println!("Convergence detail (first clean all-3-OK pass / first time ALL targets progressed):");
    for r in &rows {
        println!(
            "  {:<22} × {:<14}  clean_pass={:<7}  all_progress={:<7}  starved={}",
            r.strat,
            r.server,
            fmt_hms(r.clean),
            fmt_hms(r.all_prog),
            r.starved
        );
    }

    // ── VALIDATION 1: S0_current reproduces the starvation bug ────────────────
    // The bug: under sustained throttling a stable victim target emerges (the
    // live system saw one target rack up 104 consecutive 429s with 0 successes).
    // We require S0 to produce, under at least one upstream model, BOTH a long
    // unbroken 429 run AND severe unfairness.
    let s0_rows: Vec<&Row> = rows.iter().filter(|r| r.strat == "S0_current").collect();
    let s0_starves = s0_rows.iter().any(|r| r.starved);
    let s0_worst_run = s0_rows
        .iter()
        .map(|r| *r.consec.iter().max().unwrap())
        .max()
        .unwrap();
    let s0_worst_mmr = s0_rows.iter().map(|r| r.mmr).fold(f64::INFINITY, f64::min);
    println!();
    println!(
        "VALIDATION 1 — S0_current reproduces starvation: {}  (worst 429-run={}, worst min/max={:.3})",
        s0_starves, s0_worst_run, s0_worst_mmr
    );
    assert!(
        s0_starves,
        "S0_current must reproduce the observed starvation (a stable victim target)."
    );
    assert!(
        s0_worst_run >= 20,
        "S0_current must reproduce a long consecutive-429 run (live: 104); got {}",
        s0_worst_run
    );

    // ── VALIDATION 2: the recommended strategy avoids starvation everywhere ───
    println!();
    println!("Robustness across ALL upstream models:");
    let mut clean_strategies: Vec<&'static str> = Vec::new();
    for strat_name in [
        "S0_current",
        "S1_pure_pacing_aimd",
        "S2_clean_pass_gate",
        "S3_real_controller",
    ] {
        let strat_rows: Vec<&Row> = rows.iter().filter(|r| r.strat == strat_name).collect();
        let any_starve = strat_rows.iter().any(|r| r.starved);
        let worst_jain = strat_rows
            .iter()
            .map(|r| r.jain)
            .fold(f64::INFINITY, f64::min);
        let worst_mmr = strat_rows
            .iter()
            .map(|r| r.mmr)
            .fold(f64::INFINITY, f64::min);
        let worst_run = strat_rows
            .iter()
            .map(|r| *r.consec.iter().max().unwrap())
            .max()
            .unwrap();
        let total_succ: u64 = strat_rows.iter().map(|r| r.succ_total).sum();
        let total_thr: u64 = strat_rows.iter().map(|r| r.thr_total).sum();
        if !any_starve {
            clean_strategies.push(strat_name);
        }
        println!(
            "  {:<22} starves_under_any={:<5}  worst_Jain={:.3}  worst_min/max={:.3}  worstRun429={:<4} Σsucc={} Σ429={}",
            strat_name, any_starve, worst_jain, worst_mmr, worst_run, total_succ, total_thr
        );
    }

    println!();
    println!(
        "Strategies with NO starvation under ANY model: {:?}",
        clean_strategies
    );
    // The whole point: at least one of the candidate fixes must clear ALL models.
    assert!(
        clean_strategies.iter().any(|s| *s != "S0_current"),
        "At least one candidate fix must avoid starvation under every upstream model."
    );
    // And S1 (the simplest single-interval AIMD) must be among them — it is the
    // recommendation; if this regresses, the recommendation needs revisiting.
    assert!(
        clean_strategies.contains(&"S1_pure_pacing_aimd"),
        "S1_pure_pacing_aimd is the recommended fix and must avoid starvation everywhere."
    );

    // ── VALIDATION 3: the REAL SHIPPED controller avoids starvation everywhere ─
    // This is the regression guard that drives production code. The sim above
    // ran `S3_real_controller`, which wraps the actual
    // `pulp::ratelimit::adaptive::AdaptiveController` with production config. It
    // must NOT starve under ANY upstream model: every target must get MULTIPLE
    // successes and no target may suffer a long unbroken 429 run.
    println!();
    let s3_rows: Vec<&Row> = rows
        .iter()
        .filter(|r| r.strat == "S3_real_controller")
        .collect();
    assert!(
        clean_strategies.contains(&"S3_real_controller"),
        "The REAL shipped AdaptiveController must avoid starvation under every upstream model."
    );
    for r in &s3_rows {
        // Recompute per-target detail from the stored max-consec run + fairness.
        assert!(
            !r.starved,
            "real controller starved under {} (Jain={:.3}, min/max={:.3}, worstRun429={})",
            r.server,
            r.jain,
            r.mmr,
            r.consec.iter().max().unwrap()
        );
        assert!(
            *r.consec.iter().max().unwrap() < 20,
            "real controller suffered a long 429 run under {}: {:?}",
            r.server,
            r.consec
        );
    }
    // Every target gets multiple successes under every model (no near-starvation).
    for server_idx in 0..make_servers().len() {
        let mut strat = S3RealController::new();
        let mut server = make_servers().into_iter().nth(server_idx).unwrap();
        let stats = run_one(&mut strat, server.as_mut(), SEED);
        assert!(
            stats.successes.iter().all(|&c| c >= 2),
            "real controller: every target must get multiple successes under {} (got {:?})",
            server.name(),
            stats.successes
        );
        println!(
            "  S3_real_controller × {:<14}  successes={:?}  maxRun429={:?}",
            server.name(),
            stats.successes,
            stats.max_consec_429
        );
    }

    println!();
    println!("RECOMMENDATION: S1_pure_pacing_aimd — a single persisted minimum-interval,");
    println!("grown ×2 on a 429 and shrunk gently on success, floored/capped. No burst, so");
    println!("the FIRST request of every pass is paced too (kills the idle-burst free probe);");
    println!("a recurring 429 dominates the AIMD so the rate converges DOWN and HOLDS. One");
    println!("knob family, no per-server-model tuning, robust across TokenBucket / SlidingWindow");
    println!(
        "/ PenaltyBox. See the report for the exact control law and the AdaptiveController map."
    );
    println!();
}

/// Focused reproduction of the EXACT live signature: the FIRST target (target 0)
/// starves while the others succeed periodically. This uses the rigid fixed-tick
/// poller grid (a faithful model of `tokio::time::interval`), which locks the
/// cold first probe's phase, plus a sticky window that counts every attempt
/// (accepted or rejected) so the cold probe self-poisons its own slot. Under
/// S0_current target 0 racks up a long unbroken 429 run (the live system saw
/// 104); under S1 every target makes steady progress.
#[test]
fn sim_throttle_first_target_starvation_locked() {
    println!();
    println!("==== Focused: FIRST-target starvation under a fixed-tick poller grid ====");

    // Sticky window: <=2 successful requests per 150s, no Retry-After. The 150s
    // window > 120s grid, so the previous tick's two SUCCESSES (targets that won
    // their slots) are still on the books when the next tick's cold first probe
    // fires — it walks into a full window and 429s, every tick. Meanwhile after
    // it 429s, AIMD halves the rate and the next targets are pushed late enough
    // that the older successes age out → they win the slots. The window serves
    // exactly two of the three; on the rigid grid the loser is always the cold
    // first probe (target 0). This is the "2 succeed, 1 starves" equilibrium.
    let mk_server = || SlidingWindowServer::new(3, 125.0, false, true);

    // Observe the LOCKED regime: with production S0's config the cold first probe
    // is phase-locked into the full window for the first several passes, racking
    // up consecutive 429s with ZERO successes while targets 1 and 2 succeed every
    // pass — the exact live signature. (After ~5 passes the gated AIMD has halved
    // the rate enough times that the pass overruns the 120s tick, the grid
    // de-syncs, and the victim rotates — that recovery is itself only because the
    // rate collapsed. We measure the locked window where the bug bites.)
    let focus_horizon = 5.5 * POLL_INTERVAL;

    // --- S0_current on the rigid grid: target 0 must starve. ---
    let mut s0 = S0Current::production();
    let mut srv = mk_server();
    let st0 = run_one_inner(&mut s0, &mut srv, true, focus_horizon);
    println!(
        "  S0_current   successes t0={} t1={} t2={}  | max consec-429 t0={} t1={} t2={}",
        st0.successes[0],
        st0.successes[1],
        st0.successes[2],
        st0.max_consec_429[0],
        st0.max_consec_429[1],
        st0.max_consec_429[2],
    );

    // --- S1 on the same grid+server: nobody starves. ---
    let mut s1 = S1PurePacing::new();
    let mut srv1 = mk_server();
    let st1 = run_one_inner(&mut s1, &mut srv1, true, focus_horizon);
    println!(
        "  S1_pacing    successes t0={} t1={} t2={}  | max consec-429 t0={} t1={} t2={}",
        st1.successes[0],
        st1.successes[1],
        st1.successes[2],
        st1.max_consec_429[0],
        st1.max_consec_429[1],
        st1.max_consec_429[2],
    );
    println!();

    // Validation: S0 makes TARGET 0 the dominant victim — far fewer successes
    // than the others and the longest unbroken 429 run — while the other two make
    // real progress. (The grid locks t0's cold-probe phase so it walks into a
    // not-yet-recovered window every tick. Note: production S0's time-gated
    // recovery means a persistent per-pass 429 eventually decays the rate to the
    // floor, which can de-sync the rigid grid into a different bad equilibrium;
    // so we assert the FIRST-target victimhood + a clearly sustained run, not a
    // perfectly permanent lock — the severity headline is `max_consec_429`.)
    let min_other = st0.successes[1].min(st0.successes[2]);
    let t0_run = st0.max_consec_429[0];
    // Target 0 wins only the warmup pass 0 (empty window), then 429s every pass:
    // it is the strict minimum and far below the others, with the longest run.
    assert!(
        st0.successes[0] <= 1 && st0.successes[0] < min_other,
        "target 0 must starve under S0 on the grid (t0={}, others>= {})",
        st0.successes[0],
        min_other
    );
    assert!(
        t0_run >= 5
            && t0_run
                > *[st0.max_consec_429[1], st0.max_consec_429[2]]
                    .iter()
                    .max()
                    .unwrap(),
        "target 0 must suffer the longest, sustained 429 run (live: 104); got t0={}, all={:?}",
        t0_run,
        st0.max_consec_429
    );
    assert!(
        st0.successes[1] > 0 && st0.successes[2] > 0,
        "the OTHER targets must still succeed periodically (the 2-succeed-1-starves equilibrium)"
    );

    // Validation: S1 keeps ALL targets progressing, with no long 429 run.
    assert!(
        st1.successes.iter().all(|&c| c > 0),
        "under S1 every target must make progress"
    );
    assert!(
        *st1.max_consec_429.iter().max().unwrap() < 10,
        "under S1 no target should suffer a long 429 run"
    );
}
