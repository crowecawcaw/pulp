//! Adaptive, self-tuning rate controller (interval pacing + AIMD on the interval).
//!
//! This is the engine that powers [`crate::ratelimit::RateLimiter`]. It is split
//! out so the control law is **pure and deterministically testable**: every
//! method that needs the current time takes it as an argument (a `tokio::time::Instant`),
//! so tests inject virtual time and never sleep on the wall clock.
//!
//! # What it does (and why)
//!
//! The controller paces request issuance to whatever the upstream currently
//! tolerates by self-tuning a single **minimum interval** between request starts,
//! using **AIMD** feedback (additive-increase / multiplicative-decrease — the same
//! family TCP congestion control uses), but applied to the *interval* rather than
//! a rate, and with **no token bucket / no burst**:
//!
//! * **Pure interval pacing, no burst, no idle credit.** The controller tracks a
//!   single `next_allowed` instant, anchored at the moment a request is *issued*.
//!   Every request — including the first one after a long idle — must wait until
//!   `next_allowed`. A long quiet stretch does **not** bank a free immediate
//!   request the way a refilling token bucket would. This is the key property: it
//!   kills the "first request of every pass is free" idle-burst that starved one
//!   of several targets sharing a lane.
//! * **Multiplicative tighten on throttle** ([`AdaptiveConfig::grow_factor`]):
//!   a 429 multiplies the interval (e.g. ×2, capped at `max_interval`) so we slow
//!   *down fast* when we're over budget, and optionally honors a `Retry-After`
//!   hard pause by pushing `next_allowed` out.
//! * **Gentle additive loosen on success** ([`AdaptiveConfig::shrink_step`]):
//!   each success subtracts a small fixed amount from the interval (floored at
//!   `min_interval`). Because tightening is multiplicative and loosening is
//!   additive, **failure dominates recovery**: a recurring throttle drives the
//!   interval up and *holds* it — it converges to a sustainable interval shared
//!   fairly by all targets instead of re-inflating back to the floor.
//! * **Hard floor and ceiling** ([`AdaptiveConfig::min_interval`] /
//!   [`AdaptiveConfig::max_interval`]): the interval is always clamped to
//!   `[min, max]`; loosening never drops below `min_interval` (the fastest rate),
//!   tightening never exceeds `max_interval` (the slowest rate / floor rate).
//!
//! Under a "succeeds when well-spaced, 429s when too fast" environment this
//! settles at the smallest interval the upstream tolerates: a 429 doubles it, a
//! run of successes nibbles it back down, but a *recurring* 429 keeps it elevated.

use std::time::Duration;

use tokio::time::Instant;

/// Tunable parameters for the [`AdaptiveController`].
///
/// The control law works in **interval space**: a single minimum spacing between
/// request starts, grown multiplicatively on a 429 and shrunk additively on
/// success, clamped to `[min_interval, max_interval]`.
#[derive(Debug, Clone)]
pub struct AdaptiveConfig {
    /// Interval the controller starts at. May be tighter or looser than what's
    /// sustainable — the controller will adapt it.
    pub initial_interval: Duration,
    /// Hard floor on the interval (the *fastest* allowed pacing). Additive
    /// loosening never drops below this. Must be `> 0`.
    pub min_interval: Duration,
    /// Hard ceiling on the interval (the *slowest* allowed pacing / floor rate).
    /// Multiplicative tightening never exceeds this. Clamped to `>= min_interval`.
    pub max_interval: Duration,
    /// Factor `> 1` the interval is multiplied by on each throttle (429). E.g.
    /// `2.0` doubles the interval (halves the rate). Values `<= 1` are clamped to
    /// `1` (no growth — used by tests to run effectively unthrottled).
    pub grow_factor: f64,
    /// Amount subtracted from the interval on each success (gentle additive
    /// loosen). Must be `>= 0`.
    pub shrink_step: Duration,
    /// Upper bound on any honored `Retry-After`, so a hostile/buggy header can't
    /// stall the controller forever.
    pub max_retry_after: Duration,
}

impl Default for AdaptiveConfig {
    fn default() -> Self {
        Self {
            initial_interval: Duration::from_secs(1),
            min_interval: Duration::from_millis(100),
            max_interval: Duration::from_secs(120),
            grow_factor: 2.0,
            shrink_step: Duration::from_secs(1),
            max_retry_after: Duration::from_secs(120),
        }
    }
}

impl AdaptiveConfig {
    /// Validate/clamp the config so the control math stays well-behaved:
    /// `min_interval > 0`, `max_interval >= min_interval`, `grow_factor >= 1`,
    /// `shrink_step >= 0` (all `Duration`s are already non-negative).
    fn sanitized(mut self) -> Self {
        if self.min_interval.is_zero() {
            self.min_interval = Duration::from_nanos(1);
        }
        if self.max_interval < self.min_interval {
            self.max_interval = self.min_interval;
        }
        if self.grow_factor.is_nan() || self.grow_factor < 1.0 {
            // NaN or < 1 → no growth (clamped). 1.0 means "never tighten".
            self.grow_factor = 1.0;
        }
        self
    }
}

/// The pure, time-injected adaptive controller. Not cloneable / not internally
/// synchronized — [`crate::ratelimit::RateLimiter`] wraps it in a mutex and an
/// `Arc` to make it shareable, and feeds it `Instant::now()`.
///
/// The entire state is two values: the current minimum `interval` and the
/// `next_allowed` instant (the earliest the next request may be issued). There is
/// no token bucket and no burst capacity.
#[derive(Debug)]
pub struct AdaptiveController {
    cfg: AdaptiveConfig,
    /// Current minimum spacing between request starts.
    interval: Duration,
    /// Earliest instant the next request may be issued. Anchored at *issue* time,
    /// so a long idle does not grant a free immediate request.
    next_allowed: Instant,
}

impl AdaptiveController {
    /// Build a controller from `cfg`, anchored at time `now`. The first request is
    /// allowed immediately (`next_allowed = now`); every subsequent request is
    /// paced by `interval`.
    pub fn new(cfg: AdaptiveConfig, now: Instant) -> Self {
        let cfg = cfg.sanitized();
        let interval = cfg
            .initial_interval
            .clamp(cfg.min_interval, cfg.max_interval);
        Self {
            cfg,
            interval,
            next_allowed: now,
        }
    }

    /// Try to issue one request *now*. Returns `Ok(())` if allowed (and arms the
    /// next slot at `now + interval`), or `Err(when)` with the [`Instant`] the
    /// caller must wait until before retrying.
    ///
    /// Because `next_allowed` is set forward to `now + interval` only when a
    /// request actually issues — and is never advanced by idle time — there is no
    /// burst and no idle credit: every request, including the first of a pass, is
    /// paced by `interval`.
    pub fn try_acquire(&mut self, now: Instant) -> Result<(), Instant> {
        if now >= self.next_allowed {
            self.next_allowed = now + self.interval;
            Ok(())
        } else {
            Err(self.next_allowed)
        }
    }

    /// Feed a successful outcome: gently loosen the interval by one additive
    /// `shrink_step`, floored at `min_interval`. A single success only nibbles the
    /// interval down — it cannot undo a multiplicative tighten in one step, which
    /// is what makes failure dominate recovery.
    pub fn on_success(&mut self, _now: Instant) {
        self.interval = self
            .interval
            .saturating_sub(self.cfg.shrink_step)
            .max(self.cfg.min_interval);
    }

    /// Feed a throttle (429): multiplicatively tighten the interval (×`grow_factor`,
    /// capped at `max_interval`), and — if a `Retry-After` was supplied — push
    /// `next_allowed` out to at least `now + min(retry_after, max_retry_after)`
    /// (extended, never shortened).
    pub fn on_throttled(&mut self, retry_after: Option<Duration>, now: Instant) {
        let grown = self.interval.mul_f64(self.cfg.grow_factor);
        self.interval = grown.min(self.cfg.max_interval).max(self.cfg.min_interval);

        if let Some(ra) = retry_after {
            let capped = ra.min(self.cfg.max_retry_after);
            let until = now + capped;
            if until > self.next_allowed {
                self.next_allowed = until;
            }
        }
    }

    /// Current minimum spacing between requests.
    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// Current effective rate (req/sec), `1 / interval`. Kept for continuity and
    /// observability now that the controller is interval-based.
    pub fn rate(&self) -> f64 {
        let secs = self.interval.as_secs_f64();
        if secs > 0.0 {
            1.0 / secs
        } else {
            f64::INFINITY
        }
    }

    /// Whether a hard `Retry-After` pause is in effect at `now` (i.e. the next
    /// allowed instant is in the future). A side-effect-free peek for
    /// observability/state snapshots.
    pub fn is_paused(&self, now: Instant) -> bool {
        self.next_allowed > now
    }

    /// The earliest instant the next request may be issued (side-effect-free
    /// peek). Equal to what [`try_acquire`](Self::try_acquire) would return on the
    /// `Err` path. Exposed for observability and for deterministic simulators that
    /// want to fast-forward to the next slot without consuming it.
    pub fn next_allowed(&self) -> Instant {
        self.next_allowed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> AdaptiveConfig {
        AdaptiveConfig {
            initial_interval: Duration::from_secs(2),
            min_interval: Duration::from_secs(1),
            max_interval: Duration::from_secs(64),
            grow_factor: 2.0,
            shrink_step: Duration::from_millis(500),
            max_retry_after: Duration::from_secs(60),
        }
    }

    #[test]
    fn starts_at_initial_interval_clamped() {
        let t0 = Instant::now();
        assert_eq!(
            AdaptiveController::new(cfg(), t0).interval(),
            Duration::from_secs(2)
        );

        // Initial above max is clamped down to max.
        let mut hi = cfg();
        hi.initial_interval = Duration::from_secs(1000);
        assert_eq!(
            AdaptiveController::new(hi, t0).interval(),
            Duration::from_secs(64)
        );

        // Initial below min is clamped up to min.
        let mut lo = cfg();
        lo.initial_interval = Duration::from_millis(10);
        assert_eq!(
            AdaptiveController::new(lo, t0).interval(),
            Duration::from_secs(1)
        );
    }

    #[test]
    fn rate_is_inverse_of_interval() {
        let t0 = Instant::now();
        let c = AdaptiveController::new(cfg(), t0); // 2s interval
        assert!((c.rate() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn first_request_is_immediate_then_paced_by_interval() {
        let t0 = Instant::now();
        let mut c = AdaptiveController::new(cfg(), t0); // interval 2s
                                                        // First request issues immediately.
        assert!(c.try_acquire(t0).is_ok());
        // Immediately asking again is denied until t0 + interval (2s).
        let next = c.try_acquire(t0).unwrap_err();
        assert_eq!(next, t0 + Duration::from_secs(2));
        // Just before: still denied.
        assert!(c.try_acquire(t0 + Duration::from_millis(1999)).is_err());
        // At/after: issued, and the NEXT one is one more interval out.
        assert!(c.try_acquire(t0 + Duration::from_secs(2)).is_ok());
        let next2 = c.try_acquire(t0 + Duration::from_secs(2)).unwrap_err();
        assert_eq!(next2, t0 + Duration::from_secs(4));
    }

    /// The whole point of the redesign: a long idle does NOT grant a free
    /// immediate request. After issuing, the next request is paced by `interval`
    /// no matter how long we idle — there is no banked burst.
    #[test]
    fn no_idle_credit_no_burst() {
        let t0 = Instant::now();
        let mut c = AdaptiveController::new(cfg(), t0); // interval 2s
        assert!(c.try_acquire(t0).is_ok());
        // Idle a long time (1 hour), then try again at issue time t0+3600.
        let later = t0 + Duration::from_secs(3600);
        // Allowed (the slot at t0+2s is long past) — issues exactly once...
        assert!(c.try_acquire(later).is_ok());
        // ...and the very next request is paced a full interval out from the
        // ISSUE instant; idle did not bank a second free request.
        let next = c.try_acquire(later).unwrap_err();
        assert_eq!(next, later + Duration::from_secs(2));
    }

    #[test]
    fn success_additively_loosens_clamped_to_min() {
        let t0 = Instant::now();
        let mut c = AdaptiveController::new(cfg(), t0); // 2s, shrink 0.5s, min 1s
        c.on_success(t0);
        assert_eq!(c.interval(), Duration::from_millis(1500));
        c.on_success(t0);
        assert_eq!(c.interval(), Duration::from_secs(1));
        c.on_success(t0); // already at floor
        assert_eq!(c.interval(), Duration::from_secs(1));
    }

    #[test]
    fn throttle_multiplicatively_tightens_clamped_to_max() {
        let t0 = Instant::now();
        let mut c = AdaptiveController::new(cfg(), t0); // 2s, ×2, max 64s
        c.on_throttled(None, t0);
        assert_eq!(c.interval(), Duration::from_secs(4));
        c.on_throttled(None, t0);
        assert_eq!(c.interval(), Duration::from_secs(8));
        // Drive to the ceiling.
        for _ in 0..10 {
            c.on_throttled(None, t0);
        }
        assert_eq!(c.interval(), Duration::from_secs(64));
    }

    #[test]
    fn retry_after_pushes_next_allowed_capped() {
        let t0 = Instant::now();
        let mut cfg = cfg();
        cfg.max_retry_after = Duration::from_secs(2);
        let mut c = AdaptiveController::new(cfg, t0);
        c.try_acquire(t0).unwrap(); // arm next_allowed at t0+2s
                                    // Ask for a hostile 1h pause; capped to 2s from now.
        c.on_throttled(Some(Duration::from_secs(3600)), t0);
        assert!(c.is_paused(t0));
        // Blocked until t0 + 2s (the cap), not the bare interval.
        let when = c.try_acquire(t0 + Duration::from_secs(1)).unwrap_err();
        assert_eq!(when, t0 + Duration::from_secs(2));
        assert!(!c.is_paused(t0 + Duration::from_secs(2)));
    }

    #[test]
    fn retry_after_never_shortens_existing_wait() {
        let t0 = Instant::now();
        let mut c = AdaptiveController::new(cfg(), t0);
        c.try_acquire(t0).unwrap(); // next_allowed = t0 + 2s (interval)
                                    // A 429 with a tiny retry-after (0.5s) must NOT pull the wait in.
        c.on_throttled(Some(Duration::from_millis(500)), t0);
        let when = c.try_acquire(t0).unwrap_err();
        assert_eq!(when, t0 + Duration::from_secs(2));
    }

    /// CORE ANTI-RE-INFLATION PROPERTY. Repeat "pass" = (1 throttle + 2 successes).
    /// Multiplicative grow (×2) must outweigh the two additive shrinks, so the
    /// interval is driven UP and HELD elevated — it must NOT collapse back to
    /// `min_interval`. This is the fix for the shared-lane re-inflation bug.
    #[test]
    fn recurring_throttle_drives_interval_up_and_holds() {
        let t0 = Instant::now();
        let mut c = AdaptiveController::new(cfg(), t0); // 2s, ×2, shrink 0.5s, min 1s, max 64s
        let start = c.interval();

        let mut now = t0;
        let mut after_first_pass = None;
        for pass in 0..30 {
            // one throttle...
            c.on_throttled(None, now);
            now += Duration::from_secs(1);
            // ...then two successes.
            c.on_success(now);
            now += Duration::from_secs(1);
            c.on_success(now);
            now += Duration::from_secs(1);
            if pass == 0 {
                after_first_pass = Some(c.interval());
            }
        }
        let settled = c.interval();

        // After the very first pass it is already well above where it started
        // (×2 then −0.5 −0.5 = net up by ~1s from 2s).
        assert!(
            after_first_pass.unwrap() > start,
            "one pass should already raise the interval: {:?} > {:?}",
            after_first_pass.unwrap(),
            start
        );
        // It stays elevated — pinned at the ceiling, NOT collapsed back to min.
        assert!(
            settled > start && settled >= Duration::from_secs(8),
            "recurring throttle must HOLD the interval elevated (no re-inflation back to min): {:?}",
            settled
        );
        assert_ne!(
            settled, c.cfg.min_interval,
            "must not collapse to min_interval"
        );
    }

    /// Converse: a clean run of successes (no throttles) loosens all the way back
    /// down to `min_interval` and holds there — confirming the additive loosen
    /// works when nothing pushes back.
    #[test]
    fn sustained_success_loosens_to_min() {
        let t0 = Instant::now();
        let mut c = AdaptiveController::new(cfg(), t0);
        // Tighten hard first.
        for _ in 0..5 {
            c.on_throttled(None, t0);
        }
        assert!(c.interval() > Duration::from_secs(8));
        // Then many successes: walks down to the floor.
        let mut t = t0;
        for _ in 0..1000 {
            t += Duration::from_secs(1);
            c.on_success(t);
        }
        assert_eq!(c.interval(), Duration::from_secs(1));
    }

    #[test]
    fn grow_factor_one_means_no_tightening() {
        let t0 = Instant::now();
        let mut cfg = cfg();
        cfg.grow_factor = 1.0; // test-mode style: never grow
        let mut c = AdaptiveController::new(cfg, t0);
        let before = c.interval();
        c.on_throttled(None, t0);
        assert_eq!(c.interval(), before, "grow_factor 1.0 must not tighten");
    }
}
