// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `EventSubscription` envelope every event-subscription
//! operation answers with.
//!
//! Every call goes over the wire: the service is mounted on a `MultiServiceRouter`
//! exactly as the production router mounts it (through `db_instances::emulator`, the one
//! mounting helper this test target has), and each request is a real awsQuery form POST
//! carrying the SigV4 credential scope the router keys on. The assertions read the XML
//! the emulator answers with, so a handler that computed nothing could not pass them.
//!
//! The rule under test, over every operation that serializes an `EventSubscription`:
//! `SubscriptionCreationTime` is the String member the service renders the creation
//! instant into, in the one form this service writes it in — the calendar date and the
//! time of day to the millisecond, space-separated, in UTC — and a subscription is a
//! notification rule that provisions nothing, so a change to one is in force by the time
//! the call answers and the envelope reports the SETTLED `active` status rather than a
//! transient one.
//!
//! The tables below span the classes that rule generalises over: every operation that
//! answers with the envelope (the create, taking a source on, the modify, dropping a
//! source, the describe and the delete), the shape of the subscription the envelope
//! belongs to (a source type whose identifiers resolve against stored resources, one this
//! service holds no records of, and a subscription naming no source type at all), both
//! settings of `Enabled`, two regions — and, for the rendered form itself, instants
//! spanning the epoch, single-digit calendar fields, a leap day, the end of a year and
//! sub-millisecond readings.

use service_com_amazonaws_rds_amazonrdsv19::state::event_subscription_creation_time;

use crate::db_instances::{call_in, emulator, expect_error, expect_ok, text, REGION};
use crate::event_subscription_sources::{
    add_source_in, create_subscription, seed_sources, OTHER_REGION, SEEDED_CLUSTER,
};

/// The form `EventSubscription$SubscriptionCreationTime` is written in, as a `chrono`
/// pattern: the calendar date, then the time of day to the millisecond, separated by a
/// space and carrying no zone marker.
///
/// Stated here rather than read from the service, so this suite pins the form instead of
/// agreeing with whatever the service happens to produce.
const CREATION_TIME_FORM: &str = "%Y-%m-%d %H:%M:%S%.3f";

/// Assert that `value` is a `SubscriptionCreationTime` in [`CREATION_TIME_FORM`].
///
/// It is read back through `chrono`'s parser and re-rendered: a value that round-trips
/// through the form carries exactly the fields the form names, each padded as the form
/// pads it — so a value in any other form (an RFC-3339 instant, a value with more or
/// fewer fractional digits, one carrying a zone) fails here.
pub(crate) fn assert_creation_time_form(case: &str, value: &str) {
    let parsed = ::chrono::NaiveDateTime::parse_from_str(value, CREATION_TIME_FORM);
    let Ok(parsed) = parsed else {
        panic!(
            "case {case:?}: expected a SubscriptionCreationTime in {CREATION_TIME_FORM:?}, \
             got {value:?}"
        );
    };
    let rendered = parsed.format(CREATION_TIME_FORM).to_string();
    assert_eq!(
        rendered, value,
        "case {case:?}: expected a SubscriptionCreationTime in {CREATION_TIME_FORM:?}, got \
         {value:?}"
    );
}

/// Assert the `Status` and the `SubscriptionCreationTime` of one answered envelope.
fn assert_envelope(case: &str, xml: &str, status: &str, created_at: &str) {
    let reported_status = text(xml, "Status");
    assert_eq!(
        reported_status, status,
        "case {case:?}: expected the subscription's status to be {status:?}, got \
         {reported_status:?} in {xml}"
    );
    let reported_created_at = text(xml, "SubscriptionCreationTime");
    assert_creation_time_form(case, &reported_created_at);
    // A change to a subscription never re-creates it: every envelope carries the instant
    // the create minted, read off the stored record.
    assert_eq!(
        reported_created_at, created_at,
        "case {case:?}: expected the creation time the subscription was minted with \
         ({created_at:?}), got {reported_created_at:?} in {xml}"
    );
}

/// One row of the envelope table: the subscription the row drives, and the source
/// identifier it takes on and drops again (`None` for a subscription that covers every
/// source, which carries no source list to change).
struct EnvelopeCase {
    case: &'static str,
    name: &'static str,
    region: &'static str,
    source_type: Option<&'static str>,
    sources: &'static [&'static str],
    /// The identifier the row adds and then removes.
    moved_source: Option<&'static str>,
    /// The `Enabled` the modify asks for.
    enabled: bool,
}

/// The rows differ along what the rule generalises over: the source type the subscription
/// names (one whose identifiers resolve against stored resources, one this service holds
/// no records of, and none at all), the sources it starts with, whether a source list is
/// changed at all, both settings of `Enabled`, and the region the subscription lives in.
const ENVELOPE_CASES: &[EnvelopeCase] = &[
    EnvelopeCase {
        case: "db-instance, sources resolved against stored instances",
        name: "emu-env-sub-instance",
        region: REGION,
        source_type: Some("db-instance"),
        sources: &["emu-src-instance-a"],
        moved_source: Some("emu-src-instance-b"),
        enabled: false,
    },
    EnvelopeCase {
        case: "db-cluster, a source resolved against a stored DB cluster",
        name: "emu-env-sub-cluster",
        region: REGION,
        source_type: Some("db-cluster"),
        sources: &[],
        moved_source: Some(SEEDED_CLUSTER),
        enabled: true,
    },
    EnvelopeCase {
        case: "db-security-group, a source type this service holds no records of",
        name: "emu-env-sub-security-group",
        region: REGION,
        source_type: Some("db-security-group"),
        sources: &["emu-src-security-group-a", "emu-src-security-group-b"],
        moved_source: Some("emu-src-security-group-c"),
        enabled: false,
    },
    EnvelopeCase {
        case: "no source type at all, so no source list to change",
        name: "emu-env-sub-everything",
        region: REGION,
        source_type: None,
        sources: &[],
        moved_source: None,
        enabled: true,
    },
    EnvelopeCase {
        case: "a subscription in a second region",
        name: "emu-env-sub-other-region",
        region: OTHER_REGION,
        source_type: Some("custom-engine-version"),
        sources: &["emu-src-engine-version-a"],
        moved_source: None,
        enabled: false,
    },
];

#[tokio::test]
async fn every_envelope_carries_the_rendered_creation_time_and_the_settled_status() {
    let router = emulator();
    seed_sources(&router).await;

    for row in ENVELOPE_CASES {
        let named = |operation: &str| format!("{} ({operation})", row.case);

        let created = expect_ok(
            row.case,
            create_subscription(&router, row.region, row.name, row.source_type, row.sources).await,
        );
        let created_at = text(&created, "SubscriptionCreationTime");
        assert_creation_time_form(&named("CreateEventSubscription"), &created_at);
        // RDS answers the create with the subscription already active: creating one writes
        // a notification rule and provisions nothing.
        assert_envelope(
            &named("CreateEventSubscription"),
            &created,
            "active",
            &created_at,
        );

        if let Some(source) = row.moved_source {
            let added = expect_ok(
                row.case,
                add_source_in(&router, row.region, row.name, source).await,
            );
            assert_envelope(
                &named("AddSourceIdentifierToSubscription"),
                &added,
                "active",
                &created_at,
            );
        }

        let modified = expect_ok(
            row.case,
            call_in(
                &router,
                row.region,
                "ModifyEventSubscription",
                &[
                    ("SubscriptionName", row.name),
                    ("Enabled", if row.enabled { "true" } else { "false" }),
                ],
            )
            .await,
        );
        assert_envelope(
            &named("ModifyEventSubscription"),
            &modified,
            "active",
            &created_at,
        );
        // The change the modify wrote is in force, so the subscription read straight after
        // it is active and reports what the modify asked for.
        let described = expect_ok(
            row.case,
            call_in(
                &router,
                row.region,
                "DescribeEventSubscriptions",
                &[("SubscriptionName", row.name)],
            )
            .await,
        );
        assert_envelope(
            &named("DescribeEventSubscriptions straight after the modify"),
            &described,
            "active",
            &created_at,
        );
        let reported_enabled = text(&described, "Enabled");
        assert_eq!(
            reported_enabled,
            row.enabled.to_string(),
            "case {:?}: expected the Enabled the modify asked for, got {reported_enabled:?}",
            row.case
        );

        if let Some(source) = row.moved_source {
            let removed = expect_ok(
                row.case,
                call_in(
                    &router,
                    row.region,
                    "RemoveSourceIdentifierFromSubscription",
                    &[
                        ("SubscriptionName", row.name),
                        ("SourceIdentifier", source),
                    ],
                )
                .await,
            );
            assert_envelope(
                &named("RemoveSourceIdentifierFromSubscription"),
                &removed,
                "active",
                &created_at,
            );
        }

        // The delete answers with the subscription it accepted the delete for, which is the
        // one status an envelope reports other than `active`.
        let deleted = expect_ok(
            row.case,
            call_in(
                &router,
                row.region,
                "DeleteEventSubscription",
                &[("SubscriptionName", row.name)],
            )
            .await,
        );
        assert_envelope(
            &named("DeleteEventSubscription"),
            &deleted,
            "deleting",
            &created_at,
        );
        // The delete was persisted, so the name resolves to nothing afterwards.
        expect_error(
            &named("DescribeEventSubscriptions after the delete"),
            call_in(
                &router,
                row.region,
                "DescribeEventSubscriptions",
                &[("SubscriptionName", row.name)],
            )
            .await,
            404,
            "SubscriptionNotFound",
        );
    }
}

/// One row of the rendered-form table: the instant, and the string the member carries for
/// it.
struct RenderCase {
    case: &'static str,
    seconds: i64,
    nanos: u32,
    expected: &'static str,
}

/// The rows differ along the calendar and sub-second classes the form has to render: the
/// epoch itself, single-digit month and day fields (which the form zero-pads), a leap
/// day, the last second of a year, a reading with no sub-second part, one whose
/// millisecond field needs padding, and one carrying more precision than the member
/// publishes.
const RENDER_CASES: &[RenderCase] = &[
    RenderCase {
        case: "the epoch",
        seconds: 0,
        nanos: 0,
        expected: "1970-01-01 00:00:00.000",
    },
    RenderCase {
        case: "single-digit month and day",
        seconds: 1_000_000_000,
        nanos: 0,
        expected: "2001-09-09 01:46:40.000",
    },
    RenderCase {
        case: "milliseconds",
        seconds: 1_234_567_890,
        nanos: 987_000_000,
        expected: "2009-02-13 23:31:30.987",
    },
    RenderCase {
        case: "a reading finer than the member's millisecond resolution",
        seconds: 1_234_567_890,
        nanos: 987_654_321,
        expected: "2009-02-13 23:31:30.987",
    },
    RenderCase {
        case: "a millisecond field that needs padding",
        seconds: 1_234_567_890,
        nanos: 1_000_000,
        expected: "2009-02-13 23:31:30.001",
    },
    RenderCase {
        case: "an afternoon, in 24-hour form",
        seconds: 1_700_000_000,
        nanos: 0,
        expected: "2023-11-14 22:13:20.000",
    },
    RenderCase {
        case: "a leap day",
        seconds: 1_709_208_000,
        nanos: 500_000_000,
        expected: "2024-02-29 12:00:00.500",
    },
    RenderCase {
        case: "the last second of a year",
        seconds: 1_798_761_599,
        nanos: 999_000_000,
        expected: "2026-12-31 23:59:59.999",
    },
];

#[tokio::test]
async fn the_creation_time_is_rendered_in_one_form_for_every_instant() {
    for row in RENDER_CASES {
        let rendered = event_subscription_creation_time(
            ::aws_smithy_types::DateTime::from_secs_and_nanos(row.seconds, row.nanos),
        );
        assert_eq!(
            rendered, row.expected,
            "case {:?}: expected {:?}, got {rendered:?}",
            row.case, row.expected
        );
        assert_creation_time_form(row.case, &rendered);
    }
}
