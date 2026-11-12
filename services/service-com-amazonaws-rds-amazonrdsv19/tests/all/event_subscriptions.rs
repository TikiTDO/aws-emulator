// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for this service's event notification slice:
//! `CreateEventSubscription` and the `DescribeEventSubscriptions` report that reads what
//! it persisted.
//!
//! Every call goes over the wire: the service is mounted on a `MultiServiceRouter`
//! exactly as the production router mounts it (through `db_instances::emulator`, the one
//! mounting helper this test target has), and each request is a real awsQuery form POST
//! carrying the SigV4 credential scope the router keys on. The assertions read the XML
//! the emulator answers with, so a handler that computed nothing could not pass them.
//!
//! The rule under test: a resource a create persisted is visible to every later call that
//! looks it up, and a lookup answers the modelled not-found fault only for a resource
//! nothing created. Here that is `CreateEventSubscription` ->
//! `DescribeEventSubscriptions`: the subscription the create acknowledged is reported, and
//! a name no create stored is the modelled `SubscriptionNotFoundFault`.
//!
//! The tables below span the classes those rules generalise over: the source types
//! `CreateEventSubscriptionMessage$SourceType` publishes (including ones with no published
//! category map and the absent source type that subscribes to everything), both settings
//! of `Enabled` and its default, subscriptions with and without `EventCategories` and
//! `SourceIds`, the regional scope a name resolves in, the published name-length bound and
//! one character past it, and the published per-region allowance plus one create past it.

use std::thread::sleep;
use std::time::Duration;

use service_com_amazonaws_rds_amazonrdsv19::state::{
    EVENT_SUBSCRIPTION_QUOTA_PER_REGION, EVENT_SUBSCRIPTION_SOURCE_TYPES,
    MAX_EVENT_CATEGORIES_PER_REQUEST, MAX_EVENT_CATEGORY_LEN, MAX_SOURCE_IDS_PER_REQUEST,
    MAX_SUBSCRIPTION_NAME_LEN, MAX_TAGS_PER_RESOURCE, MAX_TAG_VALUE_LEN, STATUS_TRANSITION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, call_in, emulator, expect_error, expect_ok, optional_text, text, texts, REGION,
};

/// A second region, so the regional scope of a subscription is observable.
const OTHER_REGION: &str = "eu-west-1";

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// The SNS topic every subscription below publishes to, in [`REGION`].
fn topic_arn(region: &str) -> String {
    format!("arn:aws:sns:{region}:{ACCOUNT}:rds-events")
}

/// How long a test waits before expecting the transition a create owes to have settled:
/// the span itself plus slack for a loaded test machine.
fn wait_for_settle() {
    sleep(STATUS_TRANSITION_DELAY + Duration::from_millis(200));
}

/// One row of the create/describe round trip.
struct SubscriptionCase {
    case: &'static str,
    name: &'static str,
    /// The `SourceType` the create names, if any: "If this value isn't specified, all
    /// events are returned."
    source_type: Option<&'static str>,
    /// The `EventCategories` the create names, each a published category of that source
    /// type.
    event_categories: &'static [&'static str],
    /// The `Enabled` the create names, if any.
    enabled: Option<bool>,
    /// What the subscription reports for `Enabled`.
    reported_enabled: bool,
}

/// The subscriptions the round trip creates and reads back.
///
/// The rows differ along what the rule generalises over: the source type (a source type
/// with a published category map, one whose categories this emulator does not publish, and
/// none at all), whether categories are named, and `Enabled` (both settings and its
/// default).
const SUBSCRIPTION_CASES: &[SubscriptionCase] = &[
    SubscriptionCase {
        case: "db-instance, two categories, enabled by default",
        name: "emu-sub-instance",
        source_type: Some("db-instance"),
        event_categories: &["availability", "backup"],
        enabled: None,
        reported_enabled: true,
    },
    SubscriptionCase {
        case: "db-snapshot, one category, explicitly disabled",
        name: "emu-sub-snapshot",
        source_type: Some("db-snapshot"),
        event_categories: &["creation"],
        enabled: Some(false),
        reported_enabled: false,
    },
    SubscriptionCase {
        case: "db-proxy, no categories, explicitly enabled",
        name: "emu-sub-proxy",
        source_type: Some("db-proxy"),
        event_categories: &[],
        enabled: Some(true),
        reported_enabled: true,
    },
    SubscriptionCase {
        case: "zero-etl, a source type with no published category map",
        name: "emu-sub-zero-etl",
        source_type: Some("zero-etl"),
        event_categories: &[],
        enabled: None,
        reported_enabled: true,
    },
    SubscriptionCase {
        case: "no source type, so every source of the account",
        name: "emu-sub-everything",
        source_type: None,
        event_categories: &[],
        enabled: Some(false),
        reported_enabled: false,
    },
];

/// `CreateEventSubscription` for one row of the table, in `region`.
async fn create_case(
    router: &MultiServiceRouter,
    region: &str,
    row: &SubscriptionCase,
) -> (u16, String) {
    let mut params: Vec<(String, String)> = vec![
        ("SubscriptionName".to_string(), row.name.to_string()),
        ("SnsTopicArn".to_string(), topic_arn(region)),
    ];
    if let Some(source_type) = row.source_type {
        params.push(("SourceType".to_string(), source_type.to_string()));
    }
    for (index, category) in row.event_categories.iter().enumerate() {
        params.push((
            format!("EventCategories.EventCategory.{}", index + 1),
            (*category).to_string(),
        ));
    }
    if let Some(enabled) = row.enabled {
        params.push(("Enabled".to_string(), enabled.to_string()));
    }
    call_in(router, region, "CreateEventSubscription", &params).await
}

/// Assert every member of one row on an answer's `<EventSubscription>`, for a
/// subscription created in `region`: the ARN and the topic are that region's. `statuses`
/// are the statuses the subscription may be in at that point — a subscription is active
/// from its create onwards, so this is one value everywhere today, and the parameter is
/// what keeps each call site saying WHICH.
fn assert_reported(
    case: &str,
    label: &str,
    xml: &str,
    region: &str,
    row: &SubscriptionCase,
    statuses: &[&str],
) {
    let case = format!("{case} ({label})");
    assert_eq!(
        text(xml, "CustSubscriptionId"),
        row.name,
        "case {case:?}: expected the subscription's own name, got {xml}"
    );
    assert_eq!(
        text(xml, "EventSubscriptionArn"),
        format!("arn:aws:rds:{region}:{ACCOUNT}:es:{}", row.name),
        "case {case:?}: expected the subscription's own ARN, got {xml}"
    );
    assert_eq!(
        text(xml, "SnsTopicArn"),
        topic_arn(region),
        "case {case:?}: expected the topic the request named, got {xml}"
    );
    assert_eq!(
        text(xml, "CustomerAwsId"),
        ACCOUNT,
        "case {case:?}: expected the account the subscription belongs to, got {xml}"
    );
    assert_eq!(
        optional_text(xml, "SourceType").as_deref(),
        row.source_type,
        "case {case:?}: expected the source type the request named, got {xml}"
    );
    assert_eq!(
        texts(xml, "EventCategory"),
        row.event_categories
            .iter()
            .map(|category| (*category).to_string())
            .collect::<Vec<String>>(),
        "case {case:?}: expected the categories the request named, got {xml}"
    );
    assert_eq!(
        text(xml, "Enabled"),
        row.reported_enabled.to_string(),
        "case {case:?}: expected the Enabled the request asked for, got {xml}"
    );
    let reported_status = text(xml, "Status");
    assert!(
        statuses.contains(&reported_status.as_str()),
        "case {case:?}: expected the subscription's status to be one of {statuses:?}, got \
         {reported_status:?} in {xml}"
    );
    let created_at = text(xml, "SubscriptionCreationTime");
    // The one form this service renders the member in; `event_subscription_envelope` is
    // where that form is stated and asserted over the instant classes.
    crate::event_subscription_envelope::assert_creation_time_form(&case, &created_at);
}

#[tokio::test]
async fn create_persists_every_member_and_describe_reads_it_back() {
    let router = emulator();

    for row in SUBSCRIPTION_CASES {
        let created = expect_ok(row.case, create_case(&router, REGION, row).await);
        // RDS answers the create with the subscription already active: creating one
        // writes a notification rule and provisions nothing, so there is no transient
        // status and no transition for a later read to settle.
        assert_reported(
            row.case,
            "CreateEventSubscription",
            &created,
            REGION,
            row,
            &["active"],
        );
        let described = expect_ok(
            row.case,
            call(
                &router,
                "DescribeEventSubscriptions",
                &[("SubscriptionName", row.name)],
            )
            .await,
        );
        // The round trip is what proves the record was PERSISTED rather than echoed,
        // which is the defect this suite pins.
        assert_reported(
            row.case,
            "DescribeEventSubscriptions",
            &described,
            REGION,
            row,
            &["active"],
        );
    }

    // A read does not move a status: the subscription was active when it was created and
    // is still active a transition span later, because nothing was ever owed.
    wait_for_settle();
    for row in SUBSCRIPTION_CASES {
        let described = expect_ok(
            row.case,
            call(
                &router,
                "DescribeEventSubscriptions",
                &[("SubscriptionName", row.name)],
            )
            .await,
        );
        assert_reported(
            row.case,
            "DescribeEventSubscriptions a transition span later",
            &described,
            REGION,
            row,
            &["active"],
        );
    }

    // Every created subscription is reported by the unnamed report too, in name order.
    let all = expect_ok(
        "every subscription of this region",
        call(
            &router,
            "DescribeEventSubscriptions",
            &[] as &[(&str, &str)],
        )
        .await,
    );
    let mut expected: Vec<&str> = SUBSCRIPTION_CASES.iter().map(|row| row.name).collect();
    expected.sort_unstable();
    assert_eq!(
        texts(&all, "CustSubscriptionId"),
        expected,
        "expected every created subscription in name order, got {all}"
    );
}

#[tokio::test]
async fn every_published_source_type_is_accepted_and_reported() {
    let router = emulator();

    // The published value list, from `CreateEventSubscriptionMessage$SourceType`: a
    // subscription may be created for each of them, including the four the
    // `DescribeEventCategories` member does not list.
    for (index, source_type) in EVENT_SUBSCRIPTION_SOURCE_TYPES.iter().enumerate() {
        let case = format!("source type {source_type}");
        let name = format!("emu-sub-type-{index}");
        expect_ok(
            &case,
            call(
                &router,
                "CreateEventSubscription",
                &[
                    ("SubscriptionName", name.as_str()),
                    ("SnsTopicArn", topic_arn(REGION).as_str()),
                    ("SourceType", *source_type),
                ],
            )
            .await,
        );
        let body = expect_ok(
            &case,
            call(
                &router,
                "DescribeEventSubscriptions",
                &[("SubscriptionName", name.as_str())],
            )
            .await,
        );
        assert_eq!(
            text(&body, "SourceType"),
            *source_type,
            "case {case:?}: expected the source type to be reported, got {body}"
        );
    }
}

#[tokio::test]
async fn a_subscription_resolves_only_in_the_region_it_was_created_in() {
    let router = emulator();

    expect_ok(
        "create in eu-west-1",
        call_in(
            &router,
            OTHER_REGION,
            "CreateEventSubscription",
            &[
                ("SubscriptionName", "emu-sub-regional"),
                ("SnsTopicArn", topic_arn(OTHER_REGION).as_str()),
                ("SourceType", "db-instance"),
            ],
        )
        .await,
    );
    let body = expect_ok(
        "describe in eu-west-1",
        call_in(
            &router,
            OTHER_REGION,
            "DescribeEventSubscriptions",
            &[("SubscriptionName", "emu-sub-regional")],
        )
        .await,
    );
    assert_eq!(
        text(&body, "EventSubscriptionArn"),
        format!("arn:aws:rds:{OTHER_REGION}:{ACCOUNT}:es:emu-sub-regional"),
        "expected the ARN to be scoped to the region it was created in, got {body}"
    );
    // The same name in another region names nothing, so one name can be a different
    // subscription in each region.
    expect_error(
        "the same name in us-west-2",
        call(
            &router,
            "DescribeEventSubscriptions",
            &[("SubscriptionName", "emu-sub-regional")],
        )
        .await,
        404,
        "SubscriptionNotFound",
    );
    expect_ok(
        "the same name created in us-west-2",
        call(
            &router,
            "CreateEventSubscription",
            &[
                ("SubscriptionName", "emu-sub-regional"),
                ("SnsTopicArn", topic_arn(REGION).as_str()),
                ("SourceType", "db-cluster"),
            ],
        )
        .await,
    );
    let body = expect_ok(
        "the us-west-2 subscription",
        call(
            &router,
            "DescribeEventSubscriptions",
            &[("SubscriptionName", "emu-sub-regional")],
        )
        .await,
    );
    assert_eq!(
        text(&body, "SourceType"),
        "db-cluster",
        "expected each region's own subscription, got {body}"
    );
}

#[tokio::test]
async fn a_second_create_of_one_name_is_refused_and_changes_nothing() {
    let router = emulator();

    expect_ok(
        "the first create",
        call(
            &router,
            "CreateEventSubscription",
            &[
                ("SubscriptionName", "emu-sub-twice"),
                ("SnsTopicArn", topic_arn(REGION).as_str()),
                ("SourceType", "db-instance"),
            ],
        )
        .await,
    );
    expect_error(
        "the second create of the same name",
        call(
            &router,
            "CreateEventSubscription",
            &[
                ("SubscriptionName", "emu-sub-twice"),
                ("SnsTopicArn", topic_arn(REGION).as_str()),
                ("SourceType", "db-cluster"),
            ],
        )
        .await,
        400,
        "SubscriptionAlreadyExist",
    );
    let body = expect_ok(
        "the stored subscription after the refusal",
        call(
            &router,
            "DescribeEventSubscriptions",
            &[("SubscriptionName", "emu-sub-twice")],
        )
        .await,
    );
    assert_eq!(
        text(&body, "SourceType"),
        "db-instance",
        "expected the refused create to change nothing, got {body}"
    );
}

#[tokio::test]
async fn a_subscription_may_name_only_sources_this_service_holds() {
    let router = emulator();

    // `SourceIds`: "If the source type is a DB instance, a DBInstanceIdentifier value
    // must be supplied" — so an entry is resolved against the DB instances this region
    // holds, and one that names none is the modelled not-found.
    expect_error(
        "a DB instance nothing created",
        call(
            &router,
            "CreateEventSubscription",
            &[
                ("SubscriptionName", "emu-sub-sources"),
                ("SnsTopicArn", topic_arn(REGION).as_str()),
                ("SourceType", "db-instance"),
                ("SourceIds.SourceId.1", "emu-absent-instance"),
            ],
        )
        .await,
        404,
        "SourceNotFound",
    );
    expect_error(
        "the refused create stored nothing",
        call(
            &router,
            "DescribeEventSubscriptions",
            &[("SubscriptionName", "emu-sub-sources")],
        )
        .await,
        404,
        "SubscriptionNotFound",
    );

    // The same request against an instance this service DOES hold is accepted, and the
    // source is reported back.
    expect_ok(
        "the DB instance the subscription names",
        call(
            &router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "emu-sub-source-instance"),
                ("Engine", "mysql"),
                ("DBInstanceClass", "db.m5.large"),
            ],
        )
        .await,
    );
    expect_ok(
        "a subscription naming a stored DB instance",
        call(
            &router,
            "CreateEventSubscription",
            &[
                ("SubscriptionName", "emu-sub-sources"),
                ("SnsTopicArn", topic_arn(REGION).as_str()),
                ("SourceType", "db-instance"),
                ("SourceIds.SourceId.1", "emu-sub-source-instance"),
            ],
        )
        .await,
    );
    let body = expect_ok(
        "the stored subscription",
        call(
            &router,
            "DescribeEventSubscriptions",
            &[("SubscriptionName", "emu-sub-sources")],
        )
        .await,
    );
    assert_eq!(
        texts(&body, "SourceId"),
        vec!["emu-sub-source-instance".to_string()],
        "expected the source the request named, got {body}"
    );
}

/// One row of the bound table: the case, the create's parameters, and whether a request
/// at that size is served (`true`) or refused (`false`).
type BoundRow = (&'static str, Vec<(String, String)>, bool);

#[tokio::test]
async fn every_stored_request_list_is_bounded_at_its_published_cap() {
    let router = emulator();

    // The lists a create STORES, each at this emulator's cap and one entry (or one
    // character) past it: a request at the bound is served, one past it is refused and
    // stores nothing. The caps are this service's own, so the test reads them from the
    // constants the handler enforces rather than repeating numbers.
    let rows: Vec<BoundRow> = vec![
        (
            "source ids at the cap",
            source_ids_params("emu-sub-at-cap", MAX_SOURCE_IDS_PER_REQUEST),
            true,
        ),
        (
            "one source id past the cap",
            source_ids_params("emu-sub-past-cap", MAX_SOURCE_IDS_PER_REQUEST + 1),
            false,
        ),
        (
            "event categories at the cap",
            categories_params("emu-sub-cats-at-cap", MAX_EVENT_CATEGORIES_PER_REQUEST),
            true,
        ),
        (
            "one event category past the cap",
            categories_params("emu-sub-cats-past-cap", MAX_EVENT_CATEGORIES_PER_REQUEST + 1),
            false,
        ),
        (
            "an event category one character past the stored length",
            vec![
                ("SubscriptionName".to_string(), "emu-sub-long-cat".to_string()),
                ("SnsTopicArn".to_string(), topic_arn(REGION)),
                ("SourceType".to_string(), "zero-etl".to_string()),
                (
                    "EventCategories.EventCategory.1".to_string(),
                    "c".repeat(MAX_EVENT_CATEGORY_LEN + 1),
                ),
            ],
            false,
        ),
        (
            "tags at the published quota",
            tags_params("emu-sub-tags-at-cap", MAX_TAGS_PER_RESOURCE),
            true,
        ),
        (
            "one tag past the published quota",
            tags_params("emu-sub-tags-past-cap", MAX_TAGS_PER_RESOURCE + 1),
            false,
        ),
        (
            "a tag value one character past the published length",
            vec![
                ("SubscriptionName".to_string(), "emu-sub-long-tag".to_string()),
                ("SnsTopicArn".to_string(), topic_arn(REGION)),
                ("Tags.Tag.1.Key".to_string(), "owner".to_string()),
                (
                    "Tags.Tag.1.Value".to_string(),
                    "v".repeat(MAX_TAG_VALUE_LEN + 1),
                ),
            ],
            false,
        ),
    ];

    for (case, params, served) in rows {
        let name = params
            .iter()
            .find(|(key, _)| key == "SubscriptionName")
            .map(|(_, value)| value.clone())
            .expect("every row names its subscription");
        let answer = call(&router, "CreateEventSubscription", &params).await;
        if served {
            expect_ok(case, answer);
            let body = expect_ok(
                case,
                call(
                    &router,
                    "DescribeEventSubscriptions",
                    &[("SubscriptionName", name.as_str())],
                )
                .await,
            );
            assert_eq!(
                text(&body, "CustSubscriptionId"),
                name,
                "case {case:?}: expected a request at the bound to be stored, got {body}"
            );
        } else {
            expect_error(case, answer, 400, "InvalidParameterValue");
            expect_error(
                case,
                call(
                    &router,
                    "DescribeEventSubscriptions",
                    &[("SubscriptionName", name.as_str())],
                )
                .await,
                404,
                "SubscriptionNotFound",
            );
        }
    }
}

/// A create naming `count` source ids of a source type whose resources this service holds
/// none of, so the row tests the COUNT rather than the resolution.
fn source_ids_params(name: &str, count: usize) -> Vec<(String, String)> {
    let mut params = vec![
        ("SubscriptionName".to_string(), name.to_string()),
        ("SnsTopicArn".to_string(), topic_arn(REGION)),
        ("SourceType".to_string(), "db-security-group".to_string()),
    ];
    for index in 0..count {
        params.push((
            format!("SourceIds.SourceId.{}", index + 1),
            format!("emu-source-{index}"),
        ));
    }
    params
}

/// A create naming `count` event categories of a source type this emulator publishes no
/// category map for, so the row tests the COUNT rather than the membership.
fn categories_params(name: &str, count: usize) -> Vec<(String, String)> {
    let mut params = vec![
        ("SubscriptionName".to_string(), name.to_string()),
        ("SnsTopicArn".to_string(), topic_arn(REGION)),
        ("SourceType".to_string(), "zero-etl".to_string()),
    ];
    for index in 0..count {
        params.push((
            format!("EventCategories.EventCategory.{}", index + 1),
            format!("category-{index}"),
        ));
    }
    params
}

/// A create carrying `count` tags.
fn tags_params(name: &str, count: usize) -> Vec<(String, String)> {
    let mut params = vec![
        ("SubscriptionName".to_string(), name.to_string()),
        ("SnsTopicArn".to_string(), topic_arn(REGION)),
    ];
    for index in 0..count {
        params.push((format!("Tags.Tag.{}.Key", index + 1), format!("key-{index}")));
        params.push((
            format!("Tags.Tag.{}.Value", index + 1),
            format!("value-{index}"),
        ));
    }
    params
}

/// One row of the refusal table: the case, the create's parameters, and the HTTP status
/// and wire error code it must be refused with.
type RefusalRow = (
    &'static str,
    Vec<(String, String)>,
    u16,
    &'static str,
);

#[tokio::test]
async fn a_create_this_service_refuses_stores_nothing() {
    let router = emulator();

    /// The parameters of one create: the two required members, then whatever the row
    /// adds. The topic is a parameter of its own so a row that is ABOUT the topic names
    /// it once rather than twice.
    fn params(name: &str, topic: &str, extra: &[(&str, &str)]) -> Vec<(String, String)> {
        let mut params = vec![
            ("SubscriptionName".to_string(), name.to_string()),
            ("SnsTopicArn".to_string(), topic.to_string()),
        ];
        for (key, value) in extra {
            params.push(((*key).to_string(), (*value).to_string()));
        }
        params
    }
    let base = |name: &str| params(name, &topic_arn(REGION), &[]);
    let with = |name: &str, extra: &[(&str, &str)]| params(name, &topic_arn(REGION), extra);

    // The published name bound ("The name must be less than 255 characters") and one
    // character past it: the first is accepted, the second refused.
    let longest = "a".repeat(MAX_SUBSCRIPTION_NAME_LEN);
    let too_long = "a".repeat(MAX_SUBSCRIPTION_NAME_LEN + 1);
    expect_ok(
        "the longest name the member admits",
        call(&router, "CreateEventSubscription", &base(&longest)).await,
    );

    let rows: Vec<RefusalRow> = vec![
        (
            "a name one character past the published bound",
            base(&too_long),
            400,
            "InvalidParameterValue",
        ),
        (
            "a topic ARN that is not an SNS topic ARN",
            params(
                "emu-sub-refused",
                "arn:aws:sqs:us-west-2:000000000000:rds-events",
                &[],
            ),
            400,
            "SNSInvalidTopic",
        ),
        (
            "a topic ARN that is not an ARN at all",
            params("emu-sub-refused", "rds-events", &[]),
            400,
            "SNSInvalidTopic",
        ),
        (
            "a source type AWS does not publish",
            with("emu-sub-refused", &[("SourceType", "db-tenant-database")]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a category the source type does not publish",
            with(
                "emu-sub-refused",
                &[
                    ("SourceType", "db-snapshot"),
                    ("EventCategories.EventCategory.1", "failover"),
                ],
            ),
            404,
            "SubscriptionCategoryNotFound",
        ),
        (
            "categories without the source type they belong to",
            with(
                "emu-sub-refused",
                &[("EventCategories.EventCategory.1", "backup")],
            ),
            400,
            "InvalidParameterCombination",
        ),
        (
            "source ids without the source type they belong to",
            with(
                "emu-sub-refused",
                &[("SourceIds.SourceId.1", "emu-some-instance")],
            ),
            400,
            "InvalidParameterCombination",
        ),
        (
            "a source id that is not an identifier at all",
            with(
                "emu-sub-refused",
                &[
                    ("SourceType", "db-instance"),
                    ("SourceIds.SourceId.1", "-leading-hyphen"),
                ],
            ),
            400,
            "InvalidParameterValue",
        ),
    ];

    for (case, params, status, code) in rows {
        expect_error(
            case,
            call(&router, "CreateEventSubscription", &params).await,
            status,
            code,
        );
        // A refused create must leave nothing behind for a later describe to find. The
        // last parameter list carries the name the row used; every row but the
        // over-long one uses the same one.
        expect_error(
            case,
            call(
                &router,
                "DescribeEventSubscriptions",
                &[("SubscriptionName", "emu-sub-refused")],
            )
            .await,
            404,
            "SubscriptionNotFound",
        );
    }
}

#[tokio::test]
async fn deleting_a_persisted_subscription_is_accepted_and_only_then_is_it_not_found() {
    let router = emulator();

    // The delete counterpart of the round trip above, over the same classes: with and
    // without a source type, and in each of two regions, so a delete reaches only the
    // region it was signed for.
    for (case, name, source_type) in [
        ("a db-instance subscription", "emu-sub-deletable", Some("db-instance")),
        ("a subscription for every source", "emu-sub-deletable-all", None),
    ] {
        for region in [REGION, OTHER_REGION] {
            let mut params: Vec<(String, String)> = vec![
                ("SubscriptionName".to_string(), name.to_string()),
                ("SnsTopicArn".to_string(), topic_arn(region)),
            ];
            if let Some(source_type) = source_type {
                params.push(("SourceType".to_string(), source_type.to_string()));
            }
            expect_ok(
                case,
                call_in(&router, region, "CreateEventSubscription", &params).await,
            );
        }
        let deleted = expect_ok(
            case,
            call(
                &router,
                "DeleteEventSubscription",
                &[("SubscriptionName", name)],
            )
            .await,
        );
        assert_eq!(
            text(&deleted, "CustSubscriptionId"),
            name,
            "case {case:?}: expected the delete to answer with the subscription it \
             removed, got {deleted}"
        );
        assert_eq!(
            text(&deleted, "Status"),
            "deleting",
            "case {case:?}: expected the accepted delete's status, got {deleted}"
        );
        // Only now is the name not found in this region — by the report and by a second
        // delete alike.
        for action in ["DescribeEventSubscriptions", "DeleteEventSubscription"] {
            expect_error(
                case,
                call(&router, action, &[("SubscriptionName", name)]).await,
                404,
                "SubscriptionNotFound",
            );
        }
        // The other region's subscription of the same name is untouched.
        let other = expect_ok(
            case,
            call_in(
                &router,
                OTHER_REGION,
                "DescribeEventSubscriptions",
                &[("SubscriptionName", name)],
            )
            .await,
        );
        assert_eq!(
            text(&other, "CustSubscriptionId"),
            name,
            "case {case:?}: expected the other region's subscription to survive, got {other}"
        );
    }
}

#[tokio::test]
async fn the_region_allowance_bounds_what_one_region_may_hold() {
    let router = emulator();

    for index in 0..EVENT_SUBSCRIPTION_QUOTA_PER_REGION {
        let case = format!("subscription {index} of the allowance");
        let name = format!("emu-sub-quota-{index}");
        expect_ok(
            &case,
            call(
                &router,
                "CreateEventSubscription",
                &[
                    ("SubscriptionName", name.as_str()),
                    ("SnsTopicArn", topic_arn(REGION).as_str()),
                ],
            )
            .await,
        );
    }
    expect_error(
        "one subscription past the region's allowance",
        call(
            &router,
            "CreateEventSubscription",
            &[
                ("SubscriptionName", "emu-sub-quota-over"),
                ("SnsTopicArn", topic_arn(REGION).as_str()),
            ],
        )
        .await,
        400,
        "EventSubscriptionQuotaExceeded",
    );
    // The allowance is the REGION's: another region still has its own.
    expect_ok(
        "the same name in another region",
        call_in(
            &router,
            OTHER_REGION,
            "CreateEventSubscription",
            &[
                ("SubscriptionName", "emu-sub-quota-over"),
                ("SnsTopicArn", topic_arn(OTHER_REGION).as_str()),
            ],
        )
        .await,
    );
}
