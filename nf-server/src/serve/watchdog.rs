//! Snapshot-stale watchdog: per-host alerting when this nf-server is
//! serving a snapshot older than the canonical voting-config height.
//!
//! ## Why this lives in-process
//!
//! We don't run a central Prometheus / Alertmanager today, but every
//! `nf-server` already has a Sentry DSN wired in (the deploy workflow
//! drops it into `/opt/nf-ingest/.env`). Sentry's Slack integration
//! already routes Error-level events to the on-call channel, so the
//! cheapest path to "page when served < expected for 30m" is to make
//! the binary watch its own gauges and emit a single Sentry event
//! when the threshold is crossed.
//!
//! ## Alert semantics
//!
//! The watchdog ticks every `tick_interval` (default 60s). On each
//! tick it reads `nf_snapshot_served_height` and
//! `nf_snapshot_expected_height` and decides whether the host is
//! "stale": `expected > 0 && served < expected`.
//!
//! * **Converged → stale**: start the stale timer, update the
//!   `nf_snapshot_stale_seconds` gauge on every tick.
//! * **Stale ≥ threshold (and not yet alerted)**: fire one Sentry
//!   event, mark this episode as alerted. We do *not* re-fire on every
//!   tick — Sentry would dedupe by fingerprint anyway, but a single
//!   event keeps the noise down.
//! * **Stale → converged**: clear the stale timer, reset the alerted
//!   flag, set the gauge back to 0. The next staleness episode is a
//!   fresh alert.
//!
//! Both `served > 0` and `served = 0` count as stale when
//! `expected > 0` — a host with no usable local snapshot is the worst
//! case of "stale".
//!
//! ## Why a pure `tick` function
//!
//! The decision logic is factored out of the spawned task so it can be
//! unit-tested without faking time, Sentry, or Prometheus. `tick`
//! takes the current `Instant` plus the gauge readings and returns an
//! `Action`; the task wrapper turns that into the actual Sentry call
//! and gauge update.

use std::time::{Duration, Instant};

use crate::metrics;

/// Decisions the watchdog can make on a single tick.
///
/// `tick` is pure: it takes immutable inputs and returns this enum.
/// The async wrapper interprets `FireAlert` / `Recovered` into
/// `sentry::capture_message` calls and gauge updates.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    /// Host is converged (`served >= expected`, or `expected == 0`).
    /// Reset internal state. The wrapper sets `nf_snapshot_stale_seconds`
    /// to 0.
    Converged,
    /// Host has just transitioned from stale-and-alerted back to
    /// converged. Wrapper logs an info-level recovery message and
    /// resets state.
    Recovered { gap: u64, stale_for: Duration },
    /// Host is stale but the threshold hasn't been crossed yet.
    /// Wrapper updates `nf_snapshot_stale_seconds` to `stale_for.as_secs()`.
    Stale { gap: u64, stale_for: Duration },
    /// Threshold just crossed. Wrapper emits one Sentry error event
    /// and updates the gauge.
    FireAlert {
        served: u64,
        expected: u64,
        gap: u64,
        stale_for: Duration,
    },
}

/// Internal state carried across ticks. Public so tests can drive it.
#[derive(Debug, Default, Clone, Copy)]
pub struct State {
    /// `Some(when)` if the most recent tick observed staleness; `None`
    /// if the most recent tick observed convergence. Updated on every
    /// transition.
    stale_since: Option<Instant>,
    /// True between the alert firing and the host recovering. Prevents
    /// repeated Sentry events for the same continuous staleness.
    alerted: bool,
}

/// Pure decision function: classify the current observation and
/// update state.
///
/// `now` is the wall clock for this tick (caller passes `Instant::now()`
/// in production, a faked `Instant` in tests).
pub fn tick(
    state: &mut State,
    now: Instant,
    served: u64,
    expected: u64,
    threshold: Duration,
) -> Action {
    // expected==0 is "no canonical height yet": treat as converged so
    // we don't alert during early startup or if voting-config is
    // misconfigured (the latter is its own class of problem and
    // would surface elsewhere).
    let is_stale = expected > 0 && served < expected;

    if !is_stale {
        let was_alerted = state.alerted;
        let stale_for = state
            .stale_since
            .map(|t| now.saturating_duration_since(t))
            .unwrap_or_default();
        state.stale_since = None;
        state.alerted = false;
        if was_alerted {
            return Action::Recovered {
                gap: expected.saturating_sub(served),
                stale_for,
            };
        }
        return Action::Converged;
    }

    let gap = expected - served;
    let started = *state.stale_since.get_or_insert(now);
    let stale_for = now.saturating_duration_since(started);

    if !state.alerted && stale_for >= threshold {
        state.alerted = true;
        return Action::FireAlert {
            served,
            expected,
            gap,
            stale_for,
        };
    }

    Action::Stale { gap, stale_for }
}

/// Spawn the watchdog as a background tokio task. Returns immediately;
/// the task runs until the binary exits.
///
/// `tick_interval` controls the polling cadence (≪ `threshold`);
/// `threshold` is how long staleness must persist before we page.
pub fn spawn(tick_interval: Duration, threshold: Duration) {
    tokio::spawn(async move {
        let mut state = State::default();
        let mut ticker = tokio::time::interval(tick_interval);
        // We don't care about the immediate tick at t=0; let the timer
        // pace us at `tick_interval` from now on.
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            let served = metrics::served_height_get();
            let expected = metrics::expected_height_get();
            let action = tick(&mut state, Instant::now(), served, expected, threshold);
            apply(action);
        }
    });
}

/// Side-effecting interpreter for the actions returned by `tick`.
/// Split out so the unit tests can verify the pure decision logic
/// without touching Sentry / the metrics registry.
fn apply(action: Action) {
    match action {
        Action::Converged => metrics::stale_seconds_set(0),
        Action::Stale { stale_for, .. } => metrics::stale_seconds_set(stale_for.as_secs()),
        Action::Recovered { gap, stale_for } => {
            metrics::stale_seconds_set(0);
            sentry::capture_message(
                &format!(
                    "snapshot height converged: gap closed after {}s (was {} blocks behind)",
                    stale_for.as_secs(),
                    gap,
                ),
                sentry::Level::Info,
            );
        }
        Action::FireAlert {
            served,
            expected,
            gap,
            stale_for,
        } => {
            metrics::stale_seconds_set(stale_for.as_secs());
            // Tags let the Sentry alert rule filter on this specific
            // event class without false positives from other Error
            // events. The shared message text + tags also give Sentry
            // a stable fingerprint per-host (issue stays grouped).
            sentry::with_scope(
                |scope| {
                    scope.set_tag("alert", "snapshot_stale");
                    scope.set_tag("served_height", served.to_string());
                    scope.set_tag("expected_height", expected.to_string());
                    scope.set_tag("gap_blocks", gap.to_string());
                    scope.set_tag("stale_seconds", stale_for.as_secs().to_string());
                },
                || {
                    sentry::capture_message(
                        &format!(
                            "snapshot stale: serving height {served}, expected {expected} \
                             ({gap} blocks behind for {}s)",
                            stale_for.as_secs(),
                        ),
                        sentry::Level::Error,
                    );
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(secs: u64) -> Duration {
        Duration::from_secs(secs)
    }

    /// `expected == 0` is treated as converged regardless of `served`
    /// so we don't alert during the bootstrap window.
    #[test]
    fn expected_zero_is_converged() {
        let mut st = State::default();
        let t0 = Instant::now();
        assert_eq!(tick(&mut st, t0, 0, 0, s(1800)), Action::Converged);
        assert_eq!(tick(&mut st, t0 + s(1), 100, 0, s(1800)), Action::Converged);
    }

    /// Equal heights are converged.
    #[test]
    fn equal_heights_are_converged() {
        let mut st = State::default();
        let t0 = Instant::now();
        assert_eq!(
            tick(&mut st, t0, 3312890, 3312890, s(1800)),
            Action::Converged
        );
    }

    /// Stale-but-below-threshold returns `Stale` with the running
    /// duration; the gauge is updated but Sentry is not called.
    #[test]
    fn stale_below_threshold_keeps_ticking() {
        let mut st = State::default();
        let t0 = Instant::now();

        let a1 = tick(&mut st, t0, 100, 110, s(1800));
        assert!(matches!(a1, Action::Stale { gap: 10, .. }));

        let a2 = tick(&mut st, t0 + s(60), 100, 110, s(1800));
        match a2 {
            Action::Stale { gap, stale_for } => {
                assert_eq!(gap, 10);
                assert_eq!(stale_for, s(60));
            }
            other => panic!("expected Stale, got {other:?}"),
        }

        assert!(!st.alerted);
        assert!(st.stale_since.is_some());
    }

    /// Crossing the threshold fires exactly one alert; subsequent
    /// stale ticks return `Stale`, not another `FireAlert`.
    #[test]
    fn fires_alert_once_when_threshold_crossed() {
        let mut st = State::default();
        let t0 = Instant::now();
        let threshold = s(1800);

        // Tick 1: stale starts.
        let _ = tick(&mut st, t0, 100, 110, threshold);
        // Tick 2: under threshold.
        let _ = tick(&mut st, t0 + s(900), 100, 110, threshold);
        // Tick 3: at threshold → FireAlert.
        let a = tick(&mut st, t0 + s(1800), 100, 110, threshold);
        assert!(
            matches!(
                a,
                Action::FireAlert {
                    served: 100,
                    expected: 110,
                    gap: 10,
                    ..
                }
            ),
            "got {a:?}"
        );
        assert!(st.alerted);

        // Tick 4: still stale, but no second FireAlert.
        let a2 = tick(&mut st, t0 + s(2400), 100, 110, threshold);
        assert!(matches!(a2, Action::Stale { gap: 10, .. }));
    }

    /// Recovery resets the alerted flag, returns `Recovered` once,
    /// and a subsequent staleness re-fires.
    #[test]
    fn recovery_resets_and_re_arms() {
        let mut st = State::default();
        let t0 = Instant::now();
        let threshold = s(1800);

        // Stale + alerted.
        let _ = tick(&mut st, t0, 100, 110, threshold);
        let _ = tick(&mut st, t0 + s(1800), 100, 110, threshold);
        assert!(st.alerted);

        // Recover (caught up).
        let r = tick(&mut st, t0 + s(2000), 110, 110, threshold);
        assert!(matches!(r, Action::Recovered { gap: 0, .. }), "got {r:?}");
        assert!(!st.alerted);
        assert!(st.stale_since.is_none());

        // Falls behind again — fresh stale window, fresh alert
        // after another full threshold elapses.
        let _ = tick(&mut st, t0 + s(2100), 110, 120, threshold);
        let mid = tick(&mut st, t0 + s(3000), 110, 120, threshold);
        assert!(matches!(mid, Action::Stale { .. }));
        let again = tick(&mut st, t0 + s(2100 + 1800), 110, 120, threshold);
        assert!(matches!(again, Action::FireAlert { gap: 10, .. }));
    }

    /// Convergence without a prior alert does not produce a `Recovered`
    /// event (no point spamming Sentry on every tick that wasn't
    /// already paging).
    #[test]
    fn convergence_without_alert_is_silent() {
        let mut st = State::default();
        let t0 = Instant::now();
        let threshold = s(1800);

        let _ = tick(&mut st, t0, 100, 110, threshold);
        // Recovers before threshold.
        let a = tick(&mut st, t0 + s(60), 110, 110, threshold);
        assert_eq!(a, Action::Converged);
    }

    /// `served == 0` with `expected > 0` is the worst-case stale
    /// (no usable local snapshot) and must alert just like a partial
    /// stale.
    #[test]
    fn served_zero_is_stale() {
        let mut st = State::default();
        let t0 = Instant::now();
        let threshold = s(60);

        let _ = tick(&mut st, t0, 0, 100, threshold);
        let a = tick(&mut st, t0 + s(60), 0, 100, threshold);
        assert!(
            matches!(
                a,
                Action::FireAlert {
                    served: 0,
                    expected: 100,
                    gap: 100,
                    ..
                }
            ),
            "got {a:?}"
        );
    }
}
