// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ModifyEventSubscription`.
//!
//! Every call goes over the wire: the service is mounted on a `MultiServiceRouter`
//! exactly as the production router mounts it (through `db_instances::emulator`, the one
//! mounting helper this test target has), and each request is a real awsQuery form POST
//! carrying the SigV4 credential scope the router keys on. The assertions read the XML
//! the emulator answers with, so a handler that computed nothing could not pass them.
//!
//! The rule under test: a modify writes the members it names onto the STORED subscription
//! and leaves every other member exactly as it was, every later read reports what it
//! wrote, the subscription it leaves behind has to satisfy the same constraints a create
//! of it would (a category belongs to the resulting source type, which a category needs),
//! a refused modify changes nothing at all, and the answer reports the subscription in the
//! settled `active` status it keeps, since a change to a notification rule is in force by
//! the time the call answers.
//!
//! The tables below span the classes that rule generalises over: which members the request
//! names (none, each one alone, and all of them at once), both settings of `Enabled` from
//! both starting settings, category lists that shrink, grow and stay, a source type
//! changed with and without its categories, the subscription that names no source type,
//! the whole published source-type list with each type's whole published category list,
//! the regional scope the subscription name resolves in, and each way a request is refused
//! — including the published count and length bounds plus one past each.

use service_com_amazonaws_rds_amazonrdsv19::state::{
    event_categories_of, EVENT_SUBSCRIPTION_SOURCE_TYPES, MAX_EVENT_CATEGORIES_PER_REQUEST,
    MAX_EVENT_CATEGORY_LEN,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, call_in, emulator, expect_error, expect_ok, optional_text, text, texts, wait_for_settle,
    REGION,
};

/// A second region, so the regional scope of a subscription is observable.
const OTHER_REGION: &str = "eu-west-1";

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// The SNS topic a subscription publishes to, by topic name, in `region`.
fn topic_arn(region: &str, topic: &str) -> String {
    format!("arn:aws:sns:{region}:{ACCOUNT}:{topic}")
}

/// The topic every subscription below starts out publishing to.
const BASE_TOPIC: &str = "rds-events";

/// `CreateEventSubscription` in `region`, naming the members a row starts from.
async fn create_subscription(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    source_type: Option<&str>,
    categories: &[&str],
    enabled: Option<bool>,
) -> (u16, String) {
    let mut params: Vec<(String, String)> = vec![
        ("SubscriptionName".to_string(), name.to_string()),
        ("SnsTopicArn".to_string(), topic_arn(region, BASE_TOPIC)),
    ];
    if let Some(source_type) = source_type {
        params.push(("SourceType".to_string(), source_type.to_string()));
    }
    params.extend(category_params(categories));
    if let Some(enabled) = enabled {
        params.push(("Enabled".to_string(), enabled.to_string()));
    }
    call_in(router, region, "CreateEventSubscription", &params).await
}

/// The `EventCategories.EventCategory.N` parameters of one call.
fn category_params<S: AsRef<str>>(categories: &[S]) -> Vec<(String, String)> {
    categories
        .iter()
        .enumerate()
        .map(|(index, category)| {
            (
                format!("EventCategories.EventCategory.{}", index + 1),
                category.as_ref().to_string(),
            )
        })
        .collect()
}

/// `ModifyEventSubscription` in `region`, naming only the members a row carries.
async fn modify_in(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    source_type: Option<&str>,
    categories: Option<&[&str]>,
    topic: Option<&str>,
    enabled: Option<bool>,
) -> (u16, String) {
    let mut params: Vec<(String, String)> = vec![("SubscriptionName".to_string(), name.to_string())];
    if let Some(source_type) = source_type {
        params.push(("SourceType".to_string(), source_type.to_string()));
    }
    if let Some(categories) = categories {
        params.extend(category_params(categories));
    }
    if let Some(topic) = topic {
        params.push(("SnsTopicArn".to_string(), topic_arn(region, topic)));
    }
    if let Some(enabled) = enabled {
        params.push(("Enabled".to_string(), enabled.to_string()));
    }
    call_in(router, region, "ModifyEventSubscription", &params).await
}

/// `DescribeEventSubscriptions` for one subscription name, in `region`.
async fn describe_in(router: &MultiServiceRouter, region: &str, name: &str) -> (u16, String) {
    call_in(
        router,
        region,
        "DescribeEventSubscriptions",
        &[("SubscriptionName", name)],
    )
    .await
}

/// What a subscription reports once a row has been applied to it.
struct Reported {
    source_type: Option<&'static str>,
    categories: &'static [&'static str],
    topic: &'static str,
    enabled: bool,
}

/// Assert every member of a subscription on one answer, plus the status it is in and the
/// creation time it has carried since its create. `case` names the row AND the call the
/// answer came from, so a failure says which.
fn assert_reported(
    case: &str,
    xml: &str,
    region: &str,
    name: &str,
    expected: &Reported,
    statuses: &[&str],
    created_at: &str,
) {
    assert_eq!(
        text(xml, "CustSubscriptionId"),
        name,
        "case {case:?}: expected the subscription's own name, got {xml}"
    );
    assert_eq!(
        text(xml, "EventSubscriptionArn"),
        format!("arn:aws:rds:{region}:{ACCOUNT}:es:{name}"),
        "case {case:?}: expected the subscription's own ARN, got {xml}"
    );
    assert_eq!(
        text(xml, "CustomerAwsId"),
        ACCOUNT,
        "case {case:?}: expected the account the subscription belongs to, got {xml}"
    );
    assert_eq!(
        text(xml, "SnsTopicArn"),
        topic_arn(region, expected.topic),
        "case {case:?}: expected the topic the subscription now publishes to, got {xml}"
    );
    assert_eq!(
        optional_text(xml, "SourceType").as_deref(),
        expected.source_type,
        "case {case:?}: expected the source type the subscription now carries, got {xml}"
    );
    assert_eq!(
        texts(xml, "EventCategory"),
        expected
            .categories
            .iter()
            .map(|category| (*category).to_string())
            .collect::<Vec<String>>(),
        "case {case:?}: expected the categories the subscription now carries, got {xml}"
    );
    assert_eq!(
        text(xml, "Enabled"),
        expected.enabled.to_string(),
        "case {case:?}: expected the Enabled the subscription now carries, got {xml}"
    );
    let reported_status = text(xml, "Status");
    assert!(
        statuses.contains(&reported_status.as_str()),
        "case {case:?}: expected the subscription's status to be one of {statuses:?}, got \
         {reported_status:?} in {xml}"
    );
    // A modify changes the subscription, never re-creates it: the creation time it has
    // carried since its create is the same value afterwards.
    assert_eq!(
        text(xml, "SubscriptionCreationTime"),
        created_at,
        "case {case:?}: expected the creation time the subscription was minted with, got {xml}"
    );
}

/// One row of the "a modify writes what it names and leaves the rest alone" table: the
/// subscription it starts from, the members the modify names, and what the subscription
/// reports afterwards.
struct ModifyCase {
    case: &'static str,
    name: &'static str,
    /// The subscription the row starts from.
    base_source_type: Option<&'static str>,
    base_categories: &'static [&'static str],
    base_enabled: Option<bool>,
    /// The members the `ModifyEventSubscription` call names; `None` is a member it leaves
    /// out of the request entirely.
    source_type: Option<&'static str>,
    categories: Option<&'static [&'static str]>,
    topic: Option<&'static str>,
    enabled: Option<bool>,
    /// What every later read of the subscription reports.
    expected: Reported,
}

/// The rows of that table.
///
/// They differ along what the rule generalises over: which members the request names (none
/// at all, each one on its own, and every one at once), both settings of `Enabled` reached
/// from both starting settings, a category list that shrinks, one that grows and one left
/// untouched, a source type changed together with its categories and one changed on its
/// own, and the subscription that names no source type. Every category named is one its
/// source type publishes in [`EVENT_CATEGORIES`].
const MODIFY_CASES: &[ModifyCase] = &[
    ModifyCase {
        case: "no member named beyond the subscription: every member unchanged",
        name: "emu-mod-nothing",
        base_source_type: Some("db-instance"),
        base_categories: &["availability", "backup"],
        base_enabled: None,
        source_type: None,
        categories: None,
        topic: None,
        enabled: None,
        expected: Reported {
            source_type: Some("db-instance"),
            categories: &["availability", "backup"],
            topic: BASE_TOPIC,
            enabled: true,
        },
    },
    ModifyCase {
        case: "the topic alone: the other members keep their values",
        name: "emu-mod-topic",
        base_source_type: Some("db-instance"),
        base_categories: &["availability", "backup"],
        base_enabled: None,
        source_type: None,
        categories: None,
        topic: Some("rds-alerts"),
        enabled: None,
        expected: Reported {
            source_type: Some("db-instance"),
            categories: &["availability", "backup"],
            topic: "rds-alerts",
            enabled: true,
        },
    },
    ModifyCase {
        case: "Enabled alone, deactivating an activated subscription",
        name: "emu-mod-disable",
        base_source_type: Some("db-instance"),
        base_categories: &["availability"],
        base_enabled: Some(true),
        source_type: None,
        categories: None,
        topic: None,
        enabled: Some(false),
        expected: Reported {
            source_type: Some("db-instance"),
            categories: &["availability"],
            topic: BASE_TOPIC,
            enabled: false,
        },
    },
    ModifyCase {
        case: "Enabled alone, activating a deactivated subscription",
        name: "emu-mod-enable",
        base_source_type: Some("db-instance"),
        base_categories: &["availability"],
        base_enabled: Some(false),
        source_type: None,
        categories: None,
        topic: None,
        enabled: Some(true),
        expected: Reported {
            source_type: Some("db-instance"),
            categories: &["availability"],
            topic: BASE_TOPIC,
            enabled: true,
        },
    },
    ModifyCase {
        case: "a shorter category list replaces the stored one",
        name: "emu-mod-fewer-categories",
        base_source_type: Some("db-instance"),
        base_categories: &["availability", "backup", "failure"],
        base_enabled: None,
        // A request that names categories names the source type they belong to: "if
        // SourceType is null, Categories must also be null".
        source_type: Some("db-instance"),
        categories: Some(&["failure"]),
        topic: None,
        enabled: None,
        expected: Reported {
            source_type: Some("db-instance"),
            categories: &["failure"],
            topic: BASE_TOPIC,
            enabled: true,
        },
    },
    ModifyCase {
        case: "a longer category list replaces the stored one",
        name: "emu-mod-more-categories",
        base_source_type: Some("db-instance"),
        base_categories: &["availability"],
        base_enabled: None,
        source_type: Some("db-instance"),
        categories: Some(&["availability", "backup", "failure", "maintenance"]),
        topic: None,
        enabled: None,
        expected: Reported {
            source_type: Some("db-instance"),
            categories: &["availability", "backup", "failure", "maintenance"],
            topic: BASE_TOPIC,
            enabled: true,
        },
    },
    ModifyCase {
        case: "a source type and the categories of that type together",
        name: "emu-mod-source-type",
        base_source_type: Some("db-instance"),
        base_categories: &["availability"],
        base_enabled: None,
        source_type: Some("db-cluster"),
        categories: Some(&["failover"]),
        topic: None,
        enabled: None,
        expected: Reported {
            source_type: Some("db-cluster"),
            categories: &["failover"],
            topic: BASE_TOPIC,
            enabled: true,
        },
    },
    ModifyCase {
        case: "a source type alone, keeping categories the new type also publishes",
        name: "emu-mod-source-type-alone",
        base_source_type: Some("db-instance"),
        base_categories: &["notification"],
        base_enabled: None,
        source_type: Some("db-cluster"),
        categories: None,
        topic: None,
        enabled: None,
        expected: Reported {
            source_type: Some("db-cluster"),
            categories: &["notification"],
            topic: BASE_TOPIC,
            enabled: true,
        },
    },
    ModifyCase {
        case: "every member at once",
        name: "emu-mod-everything",
        base_source_type: Some("db-instance"),
        base_categories: &["availability", "backup"],
        base_enabled: Some(true),
        source_type: Some("db-snapshot"),
        categories: Some(&["creation", "deletion"]),
        topic: Some("rds-snapshot-events"),
        enabled: Some(false),
        expected: Reported {
            source_type: Some("db-snapshot"),
            categories: &["creation", "deletion"],
            topic: "rds-snapshot-events",
            enabled: false,
        },
    },
    ModifyCase {
        case: "a subscription that names no source type keeps naming none",
        name: "emu-mod-everything-source",
        base_source_type: None,
        base_categories: &[],
        base_enabled: Some(false),
        source_type: None,
        categories: None,
        topic: Some("rds-all-events"),
        enabled: Some(true),
        expected: Reported {
            source_type: None,
            categories: &[],
            topic: "rds-all-events",
            enabled: true,
        },
    },
];

#[tokio::test]
async fn modify_writes_the_members_it_names_and_leaves_the_rest_alone() {
    let router = emulator();

    let mut created_times: Vec<String> = Vec::new();
    for row in MODIFY_CASES {
        let created = expect_ok(
            row.case,
            create_subscription(
                &router,
                REGION,
                row.name,
                row.base_source_type,
                row.base_categories,
                row.base_enabled,
            )
            .await,
        );
        created_times.push(text(&created, "SubscriptionCreationTime"));
    }

    for (row, created_at) in MODIFY_CASES.iter().zip(&created_times) {
        let modified = expect_ok(
            row.case,
            modify_in(
                &router,
                REGION,
                row.name,
                row.source_type,
                row.categories,
                row.topic,
                row.enabled,
            )
            .await,
        );
        // RDS answers the modify with the subscription in the `active` status it keeps:
        // the change is applied before the call answers, so no transient status is
        // reported for it.
        assert_reported(
            &format!("{} (ModifyEventSubscription)", row.case),
            &modified,
            REGION,
            row.name,
            &row.expected,
            &["active"],
            created_at,
        );
        // The round trip is what proves the change was PERSISTED rather than echoed.
        let described = expect_ok(row.case, describe_in(&router, REGION, row.name).await);
        assert_reported(
            &format!(
                "{} (DescribeEventSubscriptions straight after the modify)",
                row.case
            ),
            &described,
            REGION,
            row.name,
            &row.expected,
            &["active"],
            created_at,
        );
    }

    // A transition span later every modified subscription is still active, and still
    // reports exactly what the modify wrote: no change was owed for it.
    wait_for_settle();
    for (row, created_at) in MODIFY_CASES.iter().zip(&created_times) {
        let described = expect_ok(row.case, describe_in(&router, REGION, row.name).await);
        assert_reported(
            &format!(
                "{} (DescribeEventSubscriptions a transition span later)",
                row.case
            ),
            &described,
            REGION,
            row.name,
            &row.expected,
            &["active"],
            created_at,
        );
    }
}

#[tokio::test]
async fn every_published_source_type_and_its_categories_are_accepted() {
    let router = emulator();

    // The published source-type list, from `ModifyEventSubscriptionMessage$SourceType`,
    // with each type's published categories from `EVENT_CATEGORIES` — so the rows cover
    // source types and categories the issue cited none of, including the types this
    // emulator publishes no category map for.
    for (index, source_type) in EVENT_SUBSCRIPTION_SOURCE_TYPES.iter().enumerate() {
        let case = format!("source type {source_type}");
        let name = format!("emu-mod-type-{index}");
        expect_ok(
            &case,
            create_subscription(&router, REGION, &name, Some(source_type), &[], None).await,
        );
        // A type with no published category map has nothing to resolve a category
        // against, so any category is accepted for it.
        let published = event_categories_of(source_type);
        let categories: Vec<&str> = published.map_or_else(|| vec!["notification"], |all| all.to_vec());
        let modified = expect_ok(
            &case,
            modify_in(
                &router,
                REGION,
                &name,
                Some(source_type),
                Some(&categories),
                None,
                None,
            )
            .await,
        );
        assert_eq!(
            text(&modified, "SourceType"),
            *source_type,
            "case {case:?}: expected the source type the modify named, got {modified}"
        );
        assert_eq!(
            texts(&modified, "EventCategory"),
            categories
                .iter()
                .map(|category| (*category).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: expected the categories the modify named, got {modified}"
        );
        // A category the type DOES publish a map for is resolved against that map, so one
        // outside it is the modelled 404; for a type with no map there is nothing to
        // resolve against and the same request is served.
        let unpublished: &[&str] = &["emu-unpublished-category"];
        let response = modify_in(
            &router,
            REGION,
            &name,
            Some(source_type),
            Some(unpublished),
            None,
            None,
        )
        .await;
        if published.is_some() {
            expect_error(&case, response, 404, "SubscriptionCategoryNotFound");
        } else {
            expect_ok(&case, response);
        }
    }
}

#[tokio::test]
async fn modify_leaves_the_source_identifiers_alone() {
    let router = emulator();

    // "You can't modify the source identifiers using this call": `db-security-group` is a
    // source type this service holds no records of, so the create stores the identifiers
    // it names without resolving them.
    let name = "emu-mod-sources";
    let sources = ["emu-security-group-a", "emu-security-group-b"];
    let mut params: Vec<(String, String)> = vec![
        ("SubscriptionName".to_string(), name.to_string()),
        ("SnsTopicArn".to_string(), topic_arn(REGION, BASE_TOPIC)),
        ("SourceType".to_string(), "db-security-group".to_string()),
    ];
    for (index, source) in sources.iter().enumerate() {
        params.push((
            format!("SourceIds.SourceId.{}", index + 1),
            (*source).to_string(),
        ));
    }
    expect_ok(
        "create with sources",
        call(&router, "CreateEventSubscription", &params).await,
    );

    let modified = expect_ok(
        "modify the categories",
        modify_in(
            &router,
            REGION,
            name,
            // The categories' own source type, which a request naming categories carries.
            Some("db-security-group"),
            Some(&["failure"]),
            Some("rds-alerts"),
            Some(false),
        )
        .await,
    );
    let expected: Vec<String> = sources.iter().map(|id| (*id).to_string()).collect();
    assert_eq!(
        texts(&modified, "SourceId"),
        expected,
        "expected the modify to leave the subscription's sources alone, got {modified}"
    );
    let described = expect_ok(
        "describe after the modify",
        describe_in(&router, REGION, name).await,
    );
    assert_eq!(
        texts(&described, "SourceId"),
        expected,
        "expected the stored sources to be unchanged by the modify, got {described}"
    );
}

#[tokio::test]
async fn an_unknown_subscription_name_is_the_modelled_not_found() {
    let router = emulator();

    // A name nothing created, and a name created in ANOTHER region: a subscription
    // belongs to the region it was created in, so only that region's subscriptions are in
    // scope for a call signed for it.
    expect_ok(
        "create in the other region",
        create_subscription(
            &router,
            OTHER_REGION,
            "emu-mod-elsewhere",
            Some("db-instance"),
            &["availability"],
            None,
        )
        .await,
    );
    for name in ["emu-mod-absent", "emu-mod-elsewhere"] {
        let case = format!("modify {name} from {REGION}");
        let body = expect_error(
            &case,
            modify_in(&router, REGION, name, None, None, None, Some(false)).await,
            404,
            "SubscriptionNotFound",
        );
        assert_eq!(
            text(&body, "Message"),
            format!("Event Subscription {name} not found."),
            "case {case:?}: expected the fault to name the subscription, got {body}"
        );
    }

    // The same call signed for the region that holds it is served, and the subscription
    // the other region holds under that name stays as it was.
    let modified = expect_ok(
        "modify in the region that holds it",
        modify_in(
            &router,
            OTHER_REGION,
            "emu-mod-elsewhere",
            None,
            None,
            None,
            Some(false),
        )
        .await,
    );
    assert_eq!(
        text(&modified, "Enabled"),
        "false",
        "expected the modify to deactivate the subscription of its own region, got {modified}"
    );
    assert_eq!(
        text(&modified, "EventSubscriptionArn"),
        format!("arn:aws:rds:{OTHER_REGION}:{ACCOUNT}:es:emu-mod-elsewhere"),
        "expected the ARN of the region the subscription was created in, got {modified}"
    );
}

/// One row of the "a refused modify changes nothing" table: the members the request names
/// and the refusal it gets.
struct RefusalCase {
    case: &'static str,
    source_type: Option<&'static str>,
    categories: Option<&'static [&'static str]>,
    /// A raw `SnsTopicArn` value, named as-is rather than built from a topic name.
    sns_topic_arn: Option<&'static str>,
    enabled: Option<bool>,
    status: u16,
    code: &'static str,
}

/// The rows of that table: each way this operation refuses a request, against the one
/// subscription below.
const REFUSAL_CASES: &[RefusalCase] = &[
    RefusalCase {
        case: "a topic ARN that is not an ARN at all",
        source_type: None,
        categories: None,
        sns_topic_arn: Some("rds-events"),
        enabled: None,
        status: 400,
        code: "SNSInvalidTopic",
    },
    RefusalCase {
        case: "an ARN of another service",
        source_type: None,
        categories: None,
        sns_topic_arn: Some("arn:aws:sqs:us-west-2:000000000000:rds-events"),
        enabled: None,
        status: 400,
        code: "SNSInvalidTopic",
    },
    RefusalCase {
        case: "an SNS ARN naming no topic",
        source_type: None,
        categories: None,
        sns_topic_arn: Some("arn:aws:sns:us-west-2:000000000000:"),
        enabled: None,
        status: 400,
        code: "SNSInvalidTopic",
    },
    RefusalCase {
        // A value outside the set the model's `SourceType` enum carries; `db-shard-group`
        // IS one of them, so the row has to name something no source type.
        case: "a source type a subscription may not name",
        source_type: Some("db-replica"),
        categories: None,
        sns_topic_arn: None,
        enabled: None,
        status: 400,
        code: "InvalidParameterValue",
    },
    RefusalCase {
        case: "a published source type in the wrong case",
        source_type: Some("DB-Instance"),
        categories: None,
        sns_topic_arn: None,
        enabled: None,
        status: 400,
        code: "InvalidParameterValue",
    },
    RefusalCase {
        // The categories name their own source type, as a request carrying categories has
        // to: what is refused is the category, not the pair.
        case: "a category the stored source type does not publish",
        source_type: Some("db-instance"),
        categories: Some(&["flooding"]),
        sns_topic_arn: None,
        enabled: None,
        status: 404,
        code: "SubscriptionCategoryNotFound",
    },
    RefusalCase {
        case: "a category list of which only one entry is unpublished",
        source_type: Some("db-instance"),
        categories: Some(&["availability", "global-failover"]),
        sns_topic_arn: None,
        enabled: None,
        status: 404,
        code: "SubscriptionCategoryNotFound",
    },
    RefusalCase {
        case: "a source type the stored categories do not belong to",
        source_type: Some("db-parameter-group"),
        categories: None,
        sns_topic_arn: None,
        enabled: None,
        status: 404,
        code: "SubscriptionCategoryNotFound",
    },
];

#[tokio::test]
async fn a_refused_modify_changes_nothing() {
    let router = emulator();

    let name = "emu-mod-refused";
    let base = Reported {
        source_type: Some("db-instance"),
        categories: &["availability", "backup"],
        topic: BASE_TOPIC,
        enabled: true,
    };
    let created = expect_ok(
        "create the subscription the rows are refused against",
        create_subscription(
            &router,
            REGION,
            name,
            base.source_type,
            base.categories,
            None,
        )
        .await,
    );
    let created_at = text(&created, "SubscriptionCreationTime");

    for row in REFUSAL_CASES {
        let mut params: Vec<(String, String)> =
            vec![("SubscriptionName".to_string(), name.to_string())];
        if let Some(source_type) = row.source_type {
            params.push(("SourceType".to_string(), source_type.to_string()));
        }
        if let Some(categories) = row.categories {
            params.extend(category_params(categories));
        }
        if let Some(arn) = row.sns_topic_arn {
            params.push(("SnsTopicArn".to_string(), arn.to_string()));
        }
        if let Some(enabled) = row.enabled {
            params.push(("Enabled".to_string(), enabled.to_string()));
        }
        expect_error(
            row.case,
            call(&router, "ModifyEventSubscription", &params).await,
            row.status,
            row.code,
        );
        // A refused modify stores nothing: every member is the one the create wrote, and
        // the subscription never entered the transient status an accepted modify puts it
        // in.
        let described = expect_ok(row.case, describe_in(&router, REGION, name).await);
        assert_reported(
            &format!("{} (DescribeEventSubscriptions after the refusal)", row.case),
            &described,
            REGION,
            name,
            &base,
            &["active"],
            &created_at,
        );
    }
}

#[tokio::test]
async fn categories_are_refused_without_a_source_type_and_past_the_stored_bounds() {
    let router = emulator();

    // A subscription that names no source type has no type for a category to belong to,
    // which is the combination a create of the same subscription is refused for.
    let everything = "emu-mod-no-source-type";
    expect_ok(
        "create a subscription covering every source",
        create_subscription(&router, REGION, everything, None, &[], None).await,
    );
    expect_error(
        "EventCategories without a source type",
        modify_in(
            &router,
            REGION,
            everything,
            None,
            Some(&["availability"]),
            None,
            None,
        )
        .await,
        400,
        "InvalidParameterCombination",
    );

    // The count and length bounds this service stores categories under, each at the bound
    // and one past it.
    let bounded = "emu-mod-bounded";
    expect_ok(
        "create the subscription the bounds are checked against",
        create_subscription(&router, REGION, bounded, Some("zero-etl"), &[], None).await,
    );
    let at_count: Vec<String> = (0..MAX_EVENT_CATEGORIES_PER_REQUEST)
        .map(|index| format!("emu-category-{index}"))
        .collect();
    let mut past_count = at_count.clone();
    past_count.push(format!("emu-category-{MAX_EVENT_CATEGORIES_PER_REQUEST}"));
    let at_length = "c".repeat(MAX_EVENT_CATEGORY_LEN);
    let past_length = "c".repeat(MAX_EVENT_CATEGORY_LEN + 1);
    for (case, categories, expected) in [
        ("as many categories as the bound allows", at_count, None),
        (
            "one category past the count bound",
            past_count,
            Some("InvalidParameterValue"),
        ),
        ("a category as long as the bound allows", vec![at_length], None),
        (
            "a category one character past the length bound",
            vec![past_length],
            Some("InvalidParameterValue"),
        ),
    ] {
        let mut params: Vec<(String, String)> = vec![
            ("SubscriptionName".to_string(), bounded.to_string()),
            // The categories' own source type, which a request that names categories has
            // to carry: the bounds under test are on the LIST, not on the pair.
            ("SourceType".to_string(), "zero-etl".to_string()),
        ];
        params.extend(category_params(&categories));
        let response = call(&router, "ModifyEventSubscription", &params).await;
        match expected {
            None => {
                let body = expect_ok(case, response);
                assert_eq!(
                    texts(&body, "EventCategory"),
                    categories,
                    "case {case:?}: expected the categories the modify named, got {body}"
                );
            }
            Some(code) => {
                expect_error(case, response, 400, code);
            }
        }
    }
}
