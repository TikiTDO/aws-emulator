// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the MEMBERSHIP of the published managed-notification
//! catalog — which `(category, sub-category)` pairs the service publishes, and
//! that each of them is reachable, or unreachable, through every managed read
//! path alike. Driven over HTTP through the same multi-service router the
//! emulator boots in production.
//!
//! The rule under test: the service publishes one managed notification
//! configuration per event-type sub-category of every service whose events it
//! covers, and that published set is the whole of the catalog — a pair the
//! service publishes exists on EVERY path a caller reaches a managed
//! configuration by (the listing, a paged walk, the `channelIdentifier`-filtered
//! listing and `GetManagedNotificationConfiguration`), and a pair it does not
//! publish exists on NONE of them. Nothing a caller does creates or removes an
//! entry, so membership is a published value: the same set for every account and
//! every read.
//!
//! `tests/all/managed_notification_catalog.rs` pins each entry's published text
//! and `tests/all/managed_catalog_published_text.rs` pins that the text is
//! identical on every read path; this suite is about the SET, in both
//! directions, and about the paths agreeing on it.
//!
//! Shape source: `ManagedNotificationConfigurationOsArn`'s pattern
//! (`^arn:[a-z-]{3,10}:notifications::[0-9]{12}:managed-notification-configuration/category/[a-zA-Z0-9\-]{3,64}/sub-category/[a-zA-Z0-9\-]{3,64}$`)
//! is what makes a pair the key of an entry, and
//! `GetManagedNotificationConfiguration` declares `ResourceNotFoundException`
//! (HTTP 404) for an arn the service does not publish
//! (`smithy_query(query="operation",operation="GetManagedNotificationConfiguration,ListManagedNotificationConfigurations",model_path="services/service-com-amazonaws-notifications-notifications/smithy-model/com-amazonaws-notifications-notifications.json")`).

use serde_json::Value;
use service_com_amazonaws_notifications_notifications::state::MANAGED_DEFAULT_CHANNEL_IDENTIFIER;

use crate::harness::{encode_path_label, error_code, managed_configuration_arn_segments, Emulator};

/// The `(category, sub-category)` pairs the service publishes, one row per
/// published entry.
///
/// Catalog source: the service's own published set,
/// `MANAGED_NOTIFICATION_CATALOG` in `src/state.rs` — the model enumerates no
/// entry, so the membership is published behaviour. The rows span every covered
/// service rather than one of them: Billing and Cost Management, AWS Health, the
/// Marketplace seller side and the Marketplace buyer side.
const PUBLISHED_PAIRS: &[(&str, &str)] = &[
    ("AWS-BCM", "Billing-and-Invoices"),
    ("AWS-BCM", "Financing"),
    ("AWS-BCM", "Fraud-Prevention"),
    ("AWS-BCM", "Payment"),
    ("AWS-BCM", "Subscriptions"),
    ("AWS-Health", "Issue"),
    ("AWS-Health", "Billing"),
    ("AWS-Health", "Operations"),
    ("AWS-Health", "Security"),
    ("AWS-Partner-Central", "PCProductListings"),
    ("AWS-Partner-Central", "PCOffersAndAgreements"),
    ("AWS-Partner-Central", "PCPaymentsAndDisbursements"),
    ("AWS-Partner-Central", "PCAccountManagement"),
    ("AWS-MP-Buyer", "MPBuyerProducts"),
    ("AWS-MP-Buyer", "MPBuyerPrivateOffers"),
    ("AWS-MP-Buyer", "MPBuyerPricing"),
    ("AWS-MP-Buyer", "MPBuyerAgreements"),
];

/// Pairs the service does NOT publish, each well-formed under the arn pattern so
/// the answer is the modelled not-found rather than a validation failure.
///
/// The rows vary which part of the pair is unpublished: a sub-category that
/// belongs to ANOTHER covered service, a sub-category of no covered service, a
/// sub-category spelled as an event-type name a caller might expect under a
/// published category, an unpublished category carrying a published
/// sub-category, and both segments unpublished.
const UNPUBLISHED_PAIRS: &[(&str, &str, &str)] = &[
    (
        "a sub-category of another covered service",
        "AWS-Health",
        "MPBuyerPricing",
    ),
    (
        "a sub-category of no covered service",
        "AWS-Health",
        "AccountNotification",
    ),
    (
        "an event-type name under a published category",
        "AWS-Health",
        "ScheduledChange",
    ),
    (
        "another event-type name under a published category",
        "AWS-Health",
        "Investigation",
    ),
    (
        "a sub-category the billing category does not publish",
        "AWS-BCM",
        "Credits",
    ),
    (
        "an unpublished category with a published sub-category",
        "AWS-Billing",
        "Payment",
    ),
    ("both segments unpublished", "Zzz-Service", "Qqq-Category"),
];

/// The `channelIdentifier` values the published catalog is NOT seeded with, as
/// the `ChannelIdentifier` pattern spells them — a filtered listing on one of
/// them selects no entry, so it can never make an unpublished pair appear.
const UNASSOCIATED_CHANNELS: &[&str] = &[
    "ACCOUNT_ALTERNATE_BILLING",
    "ACCOUNT_ALTERNATE_OPERATIONS",
    "ACCOUNT_ALTERNATE_SECURITY",
];

/// The arn of one `(category, sub-category)` pair, as the service builds it.
fn pair_arn(category: &str, sub_category: &str) -> String {
    format!(
        "arn:aws:notifications::000000000000:managed-notification-configuration/category/{category}/sub-category/{sub_category}"
    )
}

/// The `(category, sub-category)` pairs one listing answered.
fn listed_pairs(response: &Value, case: &str) -> Vec<(String, String)> {
    response["managedNotificationConfigurations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: a listing answers an array, got {response:?}"))
        .iter()
        .map(|entry| {
            let arn = entry["arn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: every entry carries an arn, got {entry:?}"));
            managed_configuration_arn_segments(arn).unwrap_or_else(|| {
                panic!("case {case}: {arn} must match the modelled arn pattern")
            })
        })
        .collect()
}

/// One listing with a raw query string; the empty string names no parameter.
async fn listing(emulator: &Emulator, case: &str, query: &str) -> Vec<(String, String)> {
    let uri = if query.is_empty() {
        "/managed-notification-configurations".to_string()
    } else {
        format!("/managed-notification-configurations?{query}")
    };
    let (status, response) = emulator.send("GET", &uri, None).await;
    assert_eq!(
        status, 200,
        "case {case}: expected 200, got {status} {response:?}"
    );
    listed_pairs(&response, case)
}

/// The whole catalog walked one page at a time at `size` entries a page.
async fn paged_walk(emulator: &Emulator, case: &str, size: usize) -> Vec<(String, String)> {
    let mut seen: Vec<(String, String)> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let query = match &token {
            Some(value) => format!("maxResults={size}&nextToken={value}"),
            None => format!("maxResults={size}"),
        };
        let uri = format!("/managed-notification-configurations?{query}");
        let (status, response) = emulator.send("GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} {response:?}"
        );
        seen.extend(listed_pairs(&response, case));
        token = response["nextToken"].as_str().map(str::to_string);
        if token.is_none() {
            return seen;
        }
        assert!(
            seen.len() <= 1_000,
            "case {case}: paging did not terminate, saw {} entries",
            seen.len()
        );
    }
}

/// `GetManagedNotificationConfiguration` for one pair's arn.
async fn read_pair(emulator: &Emulator, category: &str, sub_category: &str) -> (u16, Value) {
    let uri = format!(
        "/managed-notification-configurations/{}",
        encode_path_label(&pair_arn(category, sub_category))
    );
    emulator.send("GET", &uri, None).await
}

/// Every published pair, sorted, as the cases below compare sets.
fn published_set() -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = PUBLISHED_PAIRS
        .iter()
        .map(|(category, sub_category)| ((*category).to_string(), (*sub_category).to_string()))
        .collect();
    pairs.sort();
    pairs
}

/// RULE: the catalog's membership is exactly the published set — every published
/// pair is there and nothing else is, on every path a caller enumerates it by.
#[tokio::test]
async fn every_enumeration_path_reports_exactly_the_published_set() {
    let emulator = Emulator::new();
    let want = published_set();

    // Rows: the ways a caller enumerates the catalog — no parameters at all,
    // each published bound of `maxResults`, the filter on the channel every
    // published entry starts associated with, and a walk at page sizes that
    // split the catalog, match it exactly and exceed it.
    let filter = format!(
        "channelIdentifier={}",
        encode_path_label(MANAGED_DEFAULT_CHANNEL_IDENTIFIER)
    );
    let mut paths: Vec<(String, Vec<(String, String)>)> = Vec::new();
    for (case, query) in [
        ("no parameters", ""),
        ("maxResults at the modelled maximum", "maxResults=100"),
        ("the channelIdentifier filter", filter.as_str()),
    ] {
        paths.push((case.to_string(), listing(&emulator, case, query).await));
    }
    for size in [1_usize, 4, want.len(), want.len() + 1, 100] {
        let case = format!("a paged walk at maxResults={size}");
        let walked = paged_walk(&emulator, &case, size).await;
        paths.push((case, walked));
    }

    for (case, pairs) in &paths {
        let mut sorted = pairs.clone();
        sorted.sort();
        assert_eq!(
            sorted, want,
            "case {case}: expected exactly the published pairs, got {sorted:?}"
        );
        assert_eq!(
            pairs.len(),
            want.len(),
            "case {case}: a path must report each published pair once, got {pairs:?}"
        );
    }
}

/// RULE applied in the positive direction: a published pair is reachable by its
/// own arn, and the read agrees with the listing about the pair it names.
#[tokio::test]
async fn every_published_pair_is_readable_by_its_arn() {
    let emulator = Emulator::new();

    for (category, sub_category) in PUBLISHED_PAIRS {
        let case = format!("{category}/{sub_category}");
        let (status, response) = read_pair(&emulator, category, sub_category).await;
        assert_eq!(
            status, 200,
            "case {case}: a published pair must be readable, got {status} {response:?}"
        );
        assert_eq!(
            response["arn"].as_str(),
            Some(pair_arn(category, sub_category).as_str()),
            "case {case}: expected the arn it was asked for, got {response:?}"
        );
        assert_eq!(
            response["category"].as_str(),
            Some(*category),
            "case {case}: expected category {category}, got {response:?}"
        );
        assert_eq!(
            response["subCategory"].as_str(),
            Some(*sub_category),
            "case {case}: expected subCategory {sub_category}, got {response:?}"
        );
        // The entry carries display text of its own on this path too, so a
        // published pair is never reachable as a nameless record.
        for member in ["name", "description"] {
            let value = response[member].as_str().unwrap_or_default();
            assert!(
                !value.is_empty(),
                "case {case}: expected published {member} text, got {response:?}"
            );
        }
    }
}

/// RULE applied in the negative direction: a pair the service does not publish
/// is reachable through NO managed read path — the read answers the modelled
/// not-found, and no listing, paged walk or filtered listing carries it.
#[tokio::test]
async fn a_pair_the_service_does_not_publish_is_reachable_nowhere() {
    let emulator = Emulator::new();
    let everything = listing(&emulator, "the whole catalog", "maxResults=100").await;
    let walked = paged_walk(&emulator, "a paged walk at maxResults=1", 1).await;
    let filter = format!(
        "channelIdentifier={}",
        encode_path_label(MANAGED_DEFAULT_CHANNEL_IDENTIFIER)
    );
    let filtered = listing(&emulator, "the channelIdentifier filter", &filter).await;

    for (case, category, sub_category) in UNPUBLISHED_PAIRS {
        let pair = ((*category).to_string(), (*sub_category).to_string());
        assert!(
            !published_set().contains(&pair),
            "case {case}: the row must name a pair the service does not publish, got {pair:?}"
        );
        for (path, pairs) in [
            ("the whole catalog", &everything),
            ("a paged walk at maxResults=1", &walked),
            ("the channelIdentifier filter", &filtered),
        ] {
            assert!(
                !pairs.contains(&pair),
                "case {case}: {path} must not carry {pair:?}, got {pairs:?}"
            );
        }

        let (status, response) = read_pair(&emulator, category, sub_category).await;
        assert_eq!(
            status, 404,
            "case {case}: expected 404 for {pair:?}, got {status} {response:?}"
        );
        assert!(
            error_code(&response).contains("ResourceNotFoundException"),
            "case {case}: expected ResourceNotFoundException, got {response:?}"
        );
    }
}

/// RULE: membership is a published value, so a filter on a channel the catalog
/// is not associated with selects nothing rather than inventing an entry, and
/// repeated reads leave the set exactly as it was.
#[tokio::test]
async fn membership_is_the_same_set_for_every_read_and_every_store() {
    let first = Emulator::new();
    let baseline = {
        let mut pairs = listing(&first, "the whole catalog", "maxResults=100").await;
        pairs.sort();
        pairs
    };
    assert_eq!(
        baseline,
        published_set(),
        "case the whole catalog: expected the published set, got {baseline:?}"
    );

    for channel in UNASSOCIATED_CHANNELS {
        let case = format!("channelIdentifier={channel}");
        let query = format!("channelIdentifier={}", encode_path_label(channel));
        let selected = listing(&first, &case, &query).await;
        assert!(
            selected.is_empty(),
            "case {case}: a channel the catalog is not associated with selects no entry, got {selected:?}"
        );
    }

    let repeated = {
        let mut pairs = listing(&first, "a repeated listing", "maxResults=100").await;
        pairs.sort();
        pairs
    };
    assert_eq!(
        repeated, baseline,
        "case a repeated listing: a read must not change the published membership"
    );

    let second = Emulator::new();
    let other_store = {
        let mut pairs = listing(&second, "another store", "maxResults=100").await;
        pairs.sort();
        pairs
    };
    assert_eq!(
        other_store, baseline,
        "case another store: every store publishes the same membership"
    );
}
