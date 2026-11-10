// SPDX-License-Identifier: CC0-1.0

//! The SCHEDULE a rule fires on: when a rule carrying a `ScheduleExpression` is due, and
//! what firing one hands its targets.
//!
//! "Rules with `ScheduleExpressions` self-trigger based on the given schedule" (`PutRule`):
//! such a rule is matched by no published event, so nothing a client does makes it fire —
//! its own schedule does. That is what this module performs: the clock
//! ([`drive_schedule_clock`]) wakes up, asks the store which schedule rules the clock has
//! made due, and hands each one's targets EventBridge's own scheduled-event envelope through
//! the very delivery path a published event's targets are handed theirs
//! ([`crate::ops::put_events::deliver`]) — so a state-machine target is really started, a
//! function target is really invoked, the target's `RoleArn` decides the delivery exactly as
//! it does for a published event, and what came of it is in the invocation log.
//!
//! ## What a firing carries
//!
//! EventBridge's own envelope, with a FRESH `id` per firing, `aws.events` as its `source`,
//! `Scheduled Event` as its `detail-type`, the rule's ARN as the one resource it names and an
//! empty `detail` — the document the EventBridge user guide publishes for a scheduled event.
//! Nothing of the rule or the target is folded into it: a target that declares `Input`,
//! `InputPath` or an `InputTransformer` is handed what that override states (the delivery path
//! applies those, as it does for a published event), and a target declaring none is handed
//! the envelope itself.
//!
//! ## When a rule is due
//!
//! The `ScheduleExpression` decides it: a `rate(<value> <unit>)` fires every `value` units,
//! and a `cron(<six fields>)` fires at the instants that expression names, read through the
//! maintained `cron` crate rather than a hand-written parser. The CADENCE is the caller's
//! own figure and keeps its value. WHEN the first firing of a new rule falls is not something
//! the expression states — AWS aligns a rate rule's firings to its own clock, so the first
//! one lands somewhere inside the first period — and this emulator makes it due
//! [`FIRST_FIRING_DELAY`] after the rule became schedulable, which is inside that span for
//! every rate a caller can write.
//!
//! A firing that was due while nothing was running is not replayed: "EventBridge doesn't
//! guarantee that a rule fires at a precise time", and a schedule that fell many periods
//! behind catches up to the next instant ahead rather than firing once per missed period.
//!
//! A rule is on a schedule once it can reach something: see [`schedulable_rule`].

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use super::{
    event_envelope, rule_arn, service_state, BusKey, RuleRecord,
};

/// The `source` every scheduled event carries: EventBridge is what published it.
const SCHEDULED_EVENT_SOURCE: &str = "aws.events";

/// The `detail-type` every scheduled event carries.
const SCHEDULED_EVENT_DETAIL_TYPE: &str = "Scheduled Event";

/// The `detail` every scheduled event carries: a scheduled firing is about nothing but the
/// schedule, so the envelope's detail is the empty document.
const SCHEDULED_EVENT_DETAIL: &str = "{}";

/// How long after a rule becomes schedulable its FIRST firing falls due.
///
/// EMULATOR-CHOSEN, not an AWS duration: AWS states no instant for a new rule's first
/// firing — it aligns a rate rule to its own clock, so the first firing lands somewhere
/// inside the first period — and this is the span this emulator holds a new schedule rule
/// before firing it. Deliberately non-zero, so a client that creates a rule and then adds
/// its targets has them in place before the first firing, and well inside the shortest
/// period a `rate` can state (one minute).
pub const FIRST_FIRING_DELAY: Duration = Duration::from_secs(3);

/// How often the clock wakes up to ask what is due.
///
/// Emulator-chosen: fine enough that a firing is performed close to the instant it fell due,
/// and coarse enough that an idle emulator spends nothing noticeable on it.
const CLOCK_TICK: Duration = Duration::from_millis(500);

/// How many ticks a clock may take before it stands down, so a task started by one request
/// cannot outlive the process it was started in by much: at [`CLOCK_TICK`] a tick, two
/// hours. A later call that touches a schedule rule starts a fresh one
/// ([`drive_schedule_clock`]).
const MAX_CLOCK_TICKS: usize = 14_400;

/// After how long with no tick recorded another caller may take the clock over.
///
/// The claim below is held by whichever clock last recorded a tick; a task that ENDED
/// without releasing it (a panic, a runtime shut down under it) would otherwise keep
/// schedules from ever firing again, so the claim lapses after this much silence and the
/// next call that touches a schedule rule starts a clock that takes it.
///
/// It is longer than ONE firing can take, so a clock busy delivering is never taken over
/// mid-firing: a target's own call is bounded by the service it is made to (a state machine's
/// `StartExecution` by that interpreter's own ceilings), and this is wider than the widest of
/// those.
const CLAIM_LAPSES_AFTER: Duration = Duration::from_secs(120);

/// What the schedule clock has done, and which clock is running it: the whole of this
/// module's state, held in the store so it survives the task that reads it.
#[derive(Debug, Default)]
pub struct Schedules {
    /// When each schedule rule's NEXT firing falls due, by the rule's own ARN. A rule the
    /// clock has not seen yet has no entry, and gets one — [`FIRST_FIRING_DELAY`] ahead —
    /// the first time the clock looks at it, so it is not fired the instant it is created.
    due: BTreeMap<String, SystemTime>,
    /// The generation of the clock currently driving these schedules, with the instant it
    /// last recorded a tick: see [`CLAIM_LAPSES_AFTER`].
    clock: Option<(u64, SystemTime)>,
    /// The generations handed out so far, so no two clocks of one run share one.
    generation: u64,
    /// The task the clock of that generation runs as, so it stays ABORTABLE: a clock whose
    /// claim is taken over — because it stopped recording ticks — is stopped rather than left
    /// running beside the one that took it, and the handle is what stops it.
    running: Option<::tokio::task::JoinHandle<()>>,
}

impl Schedules {
    /// Claim the single clock for this store, answering the generation the caller's clock
    /// runs under — `None` when a clock is really running, which is what keeps one clock per
    /// store however many calls ask.
    fn claim(&mut self, now: SystemTime) -> Option<u64> {
        if let Some((_, ticked)) = self.clock {
            let lapsed = now
                .duration_since(ticked)
                .is_ok_and(|silent| silent >= CLAIM_LAPSES_AFTER);
            if !lapsed {
                return None;
            }
        }
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.clock = Some((generation, now));
        Some(generation)
    }

    /// Record that the clock of `generation` is still running, answering whether it still
    /// holds the claim: one that has been taken over stands down rather than firing beside
    /// the clock that took it.
    fn ticked(&mut self, generation: u64, now: SystemTime) -> bool {
        match self.clock {
            Some((held, _)) if held == generation => {
                self.clock = Some((generation, now));
                true
            }
            _ => false,
        }
    }

    /// Release the claim held by `generation`, leaving a claim taken over since alone.
    fn release(&mut self, generation: u64) {
        if matches!(self.clock, Some((held, _)) if held == generation) {
            self.clock = None;
            self.running = None;
        }
    }

    /// Hold the task the clock just started, stopping whichever task the claim it took was
    /// held by: at most one clock task is alive per store, and it can always be stopped.
    fn set_running(&mut self, running: ::tokio::task::JoinHandle<()>) {
        if let Some(previous) = self.running.replace(running) {
            previous.abort();
        }
    }

    /// Which rules are DUE now, and the instant each one's next firing moves to.
    ///
    /// Both halves happen here, under one lock: the firing is performed outside it, so the
    /// due instant has to move BEFORE the delivery rather than after it — otherwise a slow
    /// delivery would make the same firing due twice.
    fn take_due(&mut self, schedulable: &[Schedulable], now: SystemTime) -> Vec<Schedulable> {
        let mut firing = Vec::new();
        let mut known: BTreeMap<String, SystemTime> = BTreeMap::new();
        for rule in schedulable {
            let due = match self.due.get(&rule.rule_arn) {
                Some(due) => *due,
                // A rule the clock has not seen yet: its first firing falls due ahead of
                // now, so it is not fired the instant it was created.
                None => {
                    known.insert(rule.rule_arn.clone(), after(now, FIRST_FIRING_DELAY));
                    continue;
                }
            };
            if due > now {
                known.insert(rule.rule_arn.clone(), due);
                continue;
            }
            // A schedule with no instant ahead of now fires no more: it keeps no due instant,
            // so the clock neither fires it nor looks at it again.
            if let Some(next) = rule.schedule.next_after(due, now) {
                known.insert(rule.rule_arn.clone(), next);
                firing.push(rule.clone());
            }
        }
        // A rule that is no longer schedulable — deleted, disabled, or no longer carrying a
        // schedule — keeps no due instant, so re-enabling one starts its schedule afresh.
        self.due = known;
        firing
    }
}

/// `at` plus `span`, saturating rather than panicking on a clock near the end of its range.
fn after(at: SystemTime, span: Duration) -> SystemTime {
    at.checked_add(span).unwrap_or(at)
}

/// One rule the clock may fire: where it lives, what it is called, and the schedule it fires
/// on.
#[derive(Debug, Clone)]
struct Schedulable {
    region: String,
    bus: String,
    rule: String,
    rule_arn: String,
    schedule: Schedule,
}

/// A rule's own `ScheduleExpression`, parsed.
#[derive(Debug, Clone)]
enum Schedule {
    /// `rate(<value> <unit>)`: one firing every `0` of these.
    Rate(Duration),
    /// `cron(<minutes> <hours> <day-of-month> <month> <day-of-week> <year>)`: a firing at
    /// each instant the expression names.
    Cron(Box<::cron::Schedule>),
}

impl Schedule {
    /// The schedule `expression` states, or `None` for text that states neither form — which
    /// `PutRule` refuses, so a stored rule carrying one was stored before it did.
    fn parse(expression: &str) -> Option<Self> {
        let trimmed = expression.trim();
        if let Some(body) = trimmed
            .strip_prefix("rate(")
            .and_then(|rest| rest.strip_suffix(')'))
        {
            return rate(body).map(Self::Rate);
        }
        let body = trimmed
            .strip_prefix("cron(")
            .and_then(|rest| rest.strip_suffix(')'))?;
        cron(body).map(|schedule| Self::Cron(Box::new(schedule)))
    }

    /// The instant the firing AFTER the one that was due at `due` falls on, given that the
    /// clock has reached `now` — or `None` when this schedule names no instant ahead at all,
    /// which is a schedule that fires no more (a `cron` expression whose year field is
    /// behind the clock): such a rule is dropped rather than looked at again.
    ///
    /// A schedule that fell behind — nothing drove it for a while — catches up to the next
    /// instant ahead of `now` rather than owing one firing per period it missed: "EventBridge
    /// doesn't guarantee that a rule fires at a precise time", and a burst of firings for a
    /// span nothing observed is not something the service performs.
    fn next_after(&self, due: SystemTime, now: SystemTime) -> Option<SystemTime> {
        match self {
            Self::Rate(period) => {
                let mut next = after(due, *period);
                while next <= now {
                    let moved = after(next, *period);
                    // An instant the clock cannot represent is one no period can move past:
                    // such a schedule names nothing ahead, so it fires no more.
                    if moved == next {
                        return None;
                    }
                    next = moved;
                }
                Some(next)
            }
            // A cron schedule names its own instants, so the next one is simply the first it
            // names after now.
            Self::Cron(schedule) => {
                let from: ::chrono::DateTime<::chrono::Utc> = now.into();
                schedule.after(&from).next().map(SystemTime::from)
            }
        }
    }
}

/// The period a `rate(...)` body states, or `None` when it states none.
///
/// "rate(value unit) […] value: a positive number. unit: the unit of time — minute, minutes,
/// hour, hours, day, days", and "if the value is equal to 1, then the unit must be singular"
/// — a rule whose body is not that is one `PutRule` refuses, so nothing here guesses a period
/// for it.
fn rate(body: &str) -> Option<Duration> {
    let mut fields = body.split_whitespace();
    let value: u64 = fields.next()?.parse().ok()?;
    let unit = fields.next()?;
    if fields.next().is_some() || value == 0 {
        return None;
    }
    let seconds = match unit {
        "minute" | "minutes" => 60,
        "hour" | "hours" => 60 * 60,
        "day" | "days" => 24 * 60 * 60,
        _ => return None,
    };
    // A singular unit states one of it: "rate(1 minutes)" and "rate(5 minute)" are the forms
    // AWS refuses, so they state no period here either.
    if (value == 1) != !unit.ends_with('s') {
        return None;
    }
    // A figure of units no span can represent states no period at all: it is refused rather
    // than saturated into one, which would be a period the schedule could never move past.
    Some(Duration::from_secs(value.checked_mul(seconds)?))
}

/// The cron schedule a `cron(...)` body states, or `None` when it states none.
///
/// "cron(fields) […] Minutes Hours Day-of-month Month Day-of-week Year" — a six-field,
/// minute-resolution expression, which the `cron` crate reads in its seven-field form
/// (seconds first), so a literal `0` seconds field is prepended. A `?` ("no specific value")
/// in the day-of-month or day-of-week field is the same statement as `*` in the grammar that
/// crate parses.
fn cron(body: &str) -> Option<::cron::Schedule> {
    /// The fields the service documents, and so the count a body must carry.
    const CRON_FIELDS: usize = 6;
    /// The indexes of the day-of-month and day-of-week fields, the two `?` is a statement in.
    const NO_SPECIFIC_VALUE_FIELDS: [usize; 2] = [2, 4];

    let fields: Vec<&str> = body.split_whitespace().collect();
    if fields.len() != CRON_FIELDS {
        return None;
    }
    let normalised: Vec<&str> = ::std::iter::once("0")
        .chain(fields.iter().enumerate().map(|(index, field)| {
            if *field == "?" && NO_SPECIFIC_VALUE_FIELDS.contains(&index) {
                "*"
            } else {
                *field
            }
        }))
        .collect();
    normalised.join(" ").parse().ok()
}

/// Every rule of this store that fires on a schedule: one carrying a `ScheduleExpression`
/// this module can read, which is `ENABLED`.
///
/// "DISABLED: EventBridge does not match any events against the rule" — a disabled rule is
/// not triggered by anything, its schedule included.
fn schedulable(store: &super::ComAmazonawsEventbridgeAwseventsState) -> Vec<Schedulable> {
    let mut schedulable = Vec::new();
    for (key, bus) in &store.buses {
        for rule in bus.rules.values() {
            if let Some(found) = schedulable_rule(key, rule) {
                schedulable.push(found);
            }
        }
    }
    schedulable
}

/// The schedulable form of one rule of the bus `key` names, or `None` when it fires on no
/// schedule.
///
/// A rule with NO TARGET is not one either: a firing of it reaches nobody, so nothing of it
/// is observable, and the span before the first firing ([`FIRST_FIRING_DELAY`]) is measured
/// from the moment the rule can reach something rather than from a moment at which a firing
/// would have been delivered nowhere. A rule whose targets are put after it — which is the
/// order `PutRule` and `PutTargets` are two calls for — therefore fires that span after the
/// targets arrive, not one whole period later.
fn schedulable_rule(key: &BusKey, rule: &RuleRecord) -> Option<Schedulable> {
    if rule.state == super::RULE_STATE_DISABLED || rule.targets.is_empty() {
        return None;
    }
    let schedule = Schedule::parse(rule.schedule_expression.as_deref()?)?;
    Some(Schedulable {
        region: key.region.clone(),
        bus: key.name.clone(),
        rule: rule.name.clone(),
        rule_arn: rule_arn(&key.region, &key.name, &rule.name),
        schedule,
    })
}

/// Start the one in-process task that fires this store's schedule rules as the clock makes
/// them due, if one is not already running.
///
/// A client puts a schedule rule and its targets and then watches the TARGET — it makes no
/// further EventBridge call — so a firing that only the next call to this service could
/// perform would never happen at all. This is what makes a scheduled rule observable without
/// one, and it does not change WHEN a firing happens: the clock performs what each rule's own
/// schedule has already made due.
///
/// At most one clock runs per store ([`Schedules::claim`]), it stays abortable
/// ([`Schedules::set_running`]), and with no async runtime to spawn onto there is no clock —
/// which is the case in a unit test that drives a handler directly, where nothing is waiting
/// for a firing either.
///
/// `callback` is the in-process emulator every delivery is made through, cloned from the
/// request that started the clock: the firings this clock performs reach the same handlers a
/// published event's deliveries reach.
pub fn drive_schedule_clock<Eng: Send + Sync + 'static>(
    engine: &::std::sync::Arc<Eng>,
    callback: &::smithy_server_runtime::EmbeddedEmulator,
) {
    let Some(state) = service_state(&**engine) else {
        return;
    };
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    let now = ::std::time::SystemTime::now();
    let Some(generation) = state.lock().schedules.claim(now) else {
        return;
    };
    let callback = callback.clone();
    let claimed = ::engine::ServiceState::clone(&state);
    let engine = ::std::sync::Arc::clone(engine);
    let running = runtime.spawn(async move {
        for _ in 0..MAX_CLOCK_TICKS {
            ::tokio::time::sleep(CLOCK_TICK).await;
            let Some(state) = service_state(&*engine) else {
                return;
            };
            let now = ::std::time::SystemTime::now();
            let firing = {
                let mut store = state.lock();
                if !store.schedules.ticked(generation, now) {
                    return;
                }
                let schedulable = schedulable(&store);
                store.schedules.take_due(&schedulable, now)
            };
            for rule in firing {
                fire(&*engine, &state, &callback, &rule, now).await;
            }
        }
        state.lock().schedules.release(generation);
    });
    claimed.lock().schedules.set_running(running);
}

/// Fire ONE rule: hand every target of it the scheduled-event envelope, and record what came
/// of each handover.
///
/// The envelope is built once per firing and carries a fresh event id, so two firings of one
/// rule are two events. The targets are read HERE, with the store locked only for as long as
/// reading them takes: a delivery reaches another service's handler, which takes that
/// service's own lock.
async fn fire<Eng: Send + Sync + 'static>(
    engine: &Eng,
    state: &::engine::ServiceState<super::ComAmazonawsEventbridgeAwseventsState>,
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    rule: &Schedulable,
    now: SystemTime,
) {
    let (event_id, targets) = {
        let mut store = state.lock();
        let event_id = store.next_event_id(now);
        let Some(stored) = store
            .buses
            .get(&BusKey::new(&rule.region, &rule.bus))
            .and_then(|bus| bus.rules.get(&rule.rule))
        else {
            return;
        };
        (
            event_id,
            crate::ops::put_events::compiled_targets(stored, &store),
        )
    };
    if targets.is_empty() {
        return;
    }
    let envelope = event_envelope(
        &event_id,
        &rule.region,
        Some(SCHEDULED_EVENT_SOURCE),
        Some(SCHEDULED_EVENT_DETAIL_TYPE),
        Some(SCHEDULED_EVENT_DETAIL),
        ::std::slice::from_ref(&rule.rule_arn),
        now,
    );
    let rendered = envelope.to_string();
    let iam = super::iam_state(engine);
    let cloudwatch = super::cloudwatch_state(engine);
    let glue = super::glue_state(engine);
    let kms = super::target_encryption::kms_state(engine);
    let stores = crate::ops::put_events::Stores {
        iam: iam.as_ref(),
        cloudwatch: cloudwatch.as_ref(),
        glue: glue.as_ref(),
        kms: kms.as_ref(),
    };
    let mut clients = crate::ops::put_events::Clients::default();
    let mut budget = crate::ops::put_events::DeliveryBudget::new();
    let mut invocations = Vec::with_capacity(targets.len());
    for target in &targets {
        let handover = crate::ops::put_events::Handover {
            event_id: &event_id,
            bus: &rule.bus,
            rule: &rule.rule,
            rule_arn: &rule.rule_arn,
            target,
            rendered: &rendered,
            // A scheduled firing is published by EventBridge itself, so there is no
            // publisher's trace header to carry.
            trace_header: None,
        };
        let Some(outcome) = crate::ops::put_events::deliver(
            callback,
            &rule.region,
            &mut clients,
            &stores,
            &handover,
            &mut budget,
        )
        .await
        else {
            break;
        };
        invocations.push(super::TargetInvocation {
            event_id: event_id.clone(),
            rule: rule.rule.clone(),
            target_id: target.id.clone(),
            target_arn: target.arn.clone(),
            outcome,
        });
    }
    let mut store = state.lock();
    // The firing itself is recorded as the delivery it is, so a client reading the routing
    // log sees the scheduled event beside the published ones.
    store.record_delivery(super::DeliveredEvent {
        event_id: event_id.clone(),
        bus: rule.bus.clone(),
        source: Some(SCHEDULED_EVENT_SOURCE.to_string()),
        detail_type: Some(SCHEDULED_EVENT_DETAIL_TYPE.to_string()),
        detail: Some(SCHEDULED_EVENT_DETAIL.to_string()),
        resources: vec![rule.rule_arn.clone()],
        time: now,
        matched_rules: vec![rule.rule.clone()],
        target_arns: targets.iter().map(|target| target.arn.clone()).collect(),
    });
    for invocation in invocations {
        store.record_invocation(invocation);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every form a `ScheduleExpression` states a cadence in, and the forms that state none.
    ///
    /// The rows differ along the unit, the value, the singular/plural agreement the service
    /// documents, and the two expression families.
    #[test]
    fn a_schedule_expression_states_its_own_cadence() {
        let cases: &[(&str, &str, Option<u64>)] = &[
            ("one minute", "rate(1 minute)", Some(60)),
            ("five minutes", "rate(5 minutes)", Some(300)),
            ("one hour", "rate(1 hour)", Some(3_600)),
            ("twelve hours", "rate(12 hours)", Some(43_200)),
            ("one day", "rate(1 day)", Some(86_400)),
            ("two days", "rate(2 days)", Some(172_800)),
            // "If the value is equal to 1, then the unit must be singular."
            ("a plural unit with one", "rate(1 minutes)", None),
            ("a singular unit with five", "rate(5 minute)", None),
            ("no unit at all", "rate(5)", None),
            ("a unit nothing measures", "rate(5 fortnights)", None),
            ("zero of a unit", "rate(0 minutes)", None),
            // A figure of units no span can represent states no period: it is refused here
            // rather than saturated into one a schedule could never move past.
            ("more units than a span holds", "rate(1000000000000000 days)", None),
            ("text that is neither form", "every minute", None),
        ];
        for (case, expression, expected) in cases {
            let parsed = Schedule::parse(expression);
            let seconds = match parsed {
                Some(Schedule::Rate(period)) => Some(period.as_secs()),
                _ => None,
            };
            assert_eq!(
                seconds, *expected,
                "case {case:?}: expected a period of {expected:?} seconds for {expression:?}, \
                 got {seconds:?}"
            );
        }
    }

    /// A six-field cron expression names its own instants, including with the `?` the service
    /// documents for "no specific value".
    #[test]
    fn a_cron_expression_names_the_instants_it_states() {
        let cases: &[(&str, &str, bool)] = &[
            ("every day at 20:00", "cron(0 20 * * ? *)", true),
            ("the first of every month", "cron(0 8 1 * ? *)", true),
            ("every weekday", "cron(0 18 ? * MON-FRI *)", true),
            ("five fields only", "cron(0 20 * * ?)", false),
            ("a field nothing parses", "cron(0 20 * * QQ *)", false),
        ];
        for (case, expression, readable) in cases {
            let parsed = Schedule::parse(expression);
            assert_eq!(
                matches!(parsed, Some(Schedule::Cron(_))),
                *readable,
                "case {case:?}: expected {expression:?} to be readable: {readable}, got \
                 {parsed:?}"
            );
        }
    }

    /// A rate schedule's firings are one period apart, and one that fell behind catches up to
    /// the next instant ahead rather than owing a firing per missed period.
    #[test]
    fn a_rate_schedule_moves_one_period_on() {
        let period = Duration::from_secs(60);
        let schedule = Schedule::Rate(period);
        let due = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let next = schedule.next_after(due, due);
        assert_eq!(
            next,
            Some(due + period),
            "case {:?}: expected the next firing one period on, got {next:?}",
            "on time"
        );
        let behind = due + Duration::from_secs(600);
        let caught_up = schedule
            .next_after(due, behind)
            .expect("a rate schedule always names an instant ahead");
        assert!(
            caught_up > behind && caught_up <= behind + period,
            "case {:?}: expected the next firing inside one period of now, got {caught_up:?}",
            "ten periods behind"
        );
        // A cron expression whose year is behind the clock names no instant ahead: such a
        // rule fires no more, which is what `None` says.
        let exhausted = Schedule::parse("cron(0 20 * * ? 2020)")
            .expect("the expression is one the service accepts");
        let answered = exhausted.next_after(due, SystemTime::now());
        assert!(
            answered.is_none(),
            "case {:?}: expected a schedule with nothing ahead to name no instant, got \
             {answered:?}",
            "a year behind the clock"
        );
        // And so does a period no instant can be moved past: the answer is never an instant
        // that is not ahead of now, which would make the rule due on every tick.
        let unreachable = Schedule::Rate(Duration::from_secs(u64::MAX));
        let answered = unreachable.next_after(due, SystemTime::now());
        assert!(
            answered.is_none(),
            "case {:?}: expected a period past the clock's range to name no instant, got \
             {answered:?}",
            "a period no instant can be moved past"
        );
    }

    /// One clock per store: a second caller gets nothing while the first keeps ticking, and
    /// the claim lapses once a clock stops recording ticks.
    #[test]
    fn one_clock_drives_a_store() {
        let mut schedules = Schedules::default();
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000);
        let first = schedules.claim(now);
        assert!(first.is_some(), "expected the first caller to claim the clock");
        assert!(
            schedules.claim(now).is_none(),
            "expected the second caller to find the clock claimed"
        );
        let generation = first.expect("the first caller claimed it");
        assert!(
            schedules.ticked(generation, now + Duration::from_secs(1)),
            "expected the holder's tick to be recorded"
        );
        let lapsed = now + Duration::from_secs(1) + CLAIM_LAPSES_AFTER;
        let taken_over = schedules.claim(lapsed);
        assert!(
            taken_over.is_some(),
            "expected a lapsed claim to be taken over, got none"
        );
        assert!(
            !schedules.ticked(generation, lapsed),
            "expected the taken-over clock to stand down"
        );
    }

    /// A rule the clock has not seen is not fired the instant it appears: its first firing
    /// falls one [`FIRST_FIRING_DELAY`] ahead, and the firings after it one period apart.
    #[test]
    fn a_new_rule_fires_after_the_first_firing_delay() {
        let mut schedules = Schedules::default();
        let rule = Schedulable {
            region: "us-west-2".to_string(),
            bus: "default".to_string(),
            rule: "poller".to_string(),
            rule_arn: "arn:aws:events:us-west-2:000000000000:rule/poller".to_string(),
            schedule: Schedule::Rate(Duration::from_secs(60)),
        };
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000);
        let seen = schedules.take_due(::std::slice::from_ref(&rule), now);
        assert!(
            seen.is_empty(),
            "expected a rule the clock has just seen not to fire yet, got {seen:?}"
        );
        let early = schedules.take_due(
            ::std::slice::from_ref(&rule),
            now + FIRST_FIRING_DELAY - Duration::from_millis(1),
        );
        assert!(
            early.is_empty(),
            "expected nothing to fire before the first firing is due, got {early:?}"
        );
        let fired = schedules.take_due(::std::slice::from_ref(&rule), now + FIRST_FIRING_DELAY);
        assert_eq!(
            fired.len(),
            1,
            "expected the first firing once it is due, got {fired:?}"
        );
        let again = schedules.take_due(::std::slice::from_ref(&rule), now + FIRST_FIRING_DELAY);
        assert!(
            again.is_empty(),
            "expected one firing per due instant, got {again:?}"
        );
    }
}
