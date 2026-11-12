// SPDX-License-Identifier: CC0-1.0

//! The clock a RUN's time-based bounds are read against.
//!
//! Which bound stopped a run is the HOST's own reading and never the guest's, and the
//! DEADLINE is the one bound whose reading is a span of TIME: a run that reaches any other
//! bound slowly enough is answered by the deadline instead, so on a slow or loaded host the
//! bound that answers is decided by how fast the host happens to be running rather than by
//! what the run did. Which clock those readings come from is therefore a property of the run
//! rather than a call to the host buried inside each check.
//!
//! Every invocation this emulator serves is bounded against this host's own clock
//! ([`Clock::host`]), which is what [`super::python::run`] and [`super::node::run`] pass and
//! what the operation path therefore always uses: no request chooses a run's clock, and no
//! configuration selects one. A run driven to establish that ONE named bound is what stops it
//! is bounded against a clock whose span this host's speed cannot spend: one that stands
//! STILL ([`Clock::still`]), against which a deadline never falls, so only a bound that is
//! not a span of time can answer the run; or one its own READINGS drive ([`Clock::driven`]),
//! against which a deadline falls after a number of readings — the run's traced events and
//! the host's own samples of it — rather than after a span of this host's time.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Where a run's time-based bounds read the current instant from.
///
/// Shared by both engines ([`super::python::Bounds`], [`super::node::Bounds`]): the bound a
/// run is stopped on is read the same way whichever language the handler is written in.
#[derive(Clone, Debug)]
pub struct Clock {
    /// The instant this clock reads before a reading has driven it, or `None` for this
    /// host's own clock, whose readings nothing but time drives.
    base: Option<Instant>,
    /// How far ONE reading drives this clock.
    tick: Duration,
    /// How many readings it has answered, which is what drives it. Shared between the run's
    /// own thread and the host's watch over it, which read the same clock.
    readings: Arc<AtomicU64>,
}

impl Clock {
    /// This HOST's own clock, which every invocation this emulator serves is bounded
    /// against: a reading of it is the instant it is taken at, and nothing but time moves
    /// it.
    #[must_use]
    pub fn host() -> Self {
        Self {
            base: None,
            tick: Duration::ZERO,
            readings: Arc::new(AtomicU64::new(0)),
        }
    }

    /// A clock that stands STILL: every reading answers the instant it was built at.
    ///
    /// A deadline read against it never falls, however long the run takes of this host's
    /// time, so the bound that stops a run judged against it is one of the bounds that is
    /// not a span of time — the step budget, the memory ceiling, the response cap. It is for
    /// DRIVING a run, from this crate's own tests: no invocation this emulator serves is
    /// bounded against it, because the operation path passes [`Self::host`].
    #[must_use]
    pub fn still() -> Self {
        Self {
            base: Some(Instant::now()),
            tick: Duration::ZERO,
            readings: Arc::new(AtomicU64::new(0)),
        }
    }

    /// A clock its own READINGS drive: each reading answers `tick` after the one before it.
    ///
    /// A span of such a clock is a NUMBER of readings rather than a span of this host's
    /// time, so a deadline read against it falls after the run has delivered that many
    /// traced events — the same number on an idle host and on a loaded one. For DRIVING a
    /// run, like [`Self::still`], and reached from no request path either.
    #[must_use]
    pub fn driven(tick: Duration) -> Self {
        Self {
            base: Some(Instant::now()),
            tick,
            readings: Arc::new(AtomicU64::new(0)),
        }
    }

    /// The instant this clock reads NOW, which on a driven clock is also the reading that
    /// drives it.
    #[must_use]
    pub fn now(&self) -> Instant {
        let Some(base) = self.base else {
            return Instant::now();
        };
        let readings = self.readings.fetch_add(1, Ordering::Relaxed);
        base + self
            .tick
            .saturating_mul(u32::try_from(readings).unwrap_or(u32::MAX))
    }
}

impl Default for Clock {
    /// The host's own clock: a run with nothing said about its clock is bounded against the
    /// clock every invocation is bounded against.
    fn default() -> Self {
        Self::host()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How many readings each row below takes, which is more than one so a clock that moves
    /// per reading is told apart from one that does not.
    const READINGS: u32 = 8;

    /// What one reading of each kind of clock answers — one row per kind, each read the same
    /// number of times, so the rows differ only along which clock they read.
    #[test]
    fn a_reading_moves_the_clock_its_own_kind_says_it_does() {
        // The tick a driven row is built with, and the one a still row's readings must not
        // move it by.
        let tick = Duration::from_millis(7);
        let cases: &[(&str, Clock, Option<Duration>)] = &[
            ("the host's own clock", Clock::host(), None),
            ("a clock that stands still", Clock::still(), Some(Duration::ZERO)),
            ("a clock its readings drive", Clock::driven(tick), Some(tick)),
        ];
        for (case, clock, per_reading) in cases {
            let first = clock.now();
            let mut taken = first;
            for reading in 1..=READINGS {
                let next = clock.now();
                assert!(
                    next >= taken,
                    "case {case}: reading {reading} went backwards, {next:?} after {taken:?}"
                );
                if let Some(per_reading) = per_reading {
                    let expected = first + per_reading.saturating_mul(reading);
                    assert_eq!(
                        next, expected,
                        "case {case}: expected reading {reading} at {expected:?}, got {next:?}"
                    );
                }
                taken = next;
            }
        }
    }

    /// A clock that stands still answers the SAME instant to every reader of it, which is
    /// what a run and the host's own watch over that run both read.
    #[test]
    fn a_still_clock_reads_the_same_instant_to_every_reader() {
        let clock = Clock::still();
        let taken = clock.now();
        let shared = clock.clone();
        let read_elsewhere = ::std::thread::spawn(move || shared.now())
            .join()
            .expect("the reading thread ended");
        assert_eq!(
            read_elsewhere, taken,
            "expected every reader of a still clock to read {taken:?}, got {read_elsewhere:?}"
        );
    }

    /// A driven clock is driven by EVERY reader of it: the readings a second holder takes
    /// move the clock the first holder reads, because a run and the watch over it spend one
    /// clock between them.
    #[test]
    fn a_driven_clock_is_moved_by_every_reader() {
        let tick = Duration::from_millis(3);
        let clock = Clock::driven(tick);
        let first = clock.now();
        let shared = clock.clone();
        ::std::thread::spawn(move || {
            let _ = shared.now();
        })
        .join()
        .expect("the reading thread ended");
        let after = clock.now();
        let expected = first + tick.saturating_mul(2);
        assert_eq!(
            after, expected,
            "expected the reading taken elsewhere to drive the clock to {expected:?}, got \
             {after:?}"
        );
    }
}
