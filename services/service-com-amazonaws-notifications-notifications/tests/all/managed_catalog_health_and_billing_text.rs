// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the published display text of the `AWS-Health` and
//! `AWS-BCM` clusters of the managed notification catalog, driven over HTTP
//! through the same multi-service router the emulator boots in production.
//!
//! The rule under test: the `name` and `description` of a published managed
//! notification configuration are that entry's OWN fixed published text, keyed
//! by its `category`/`sub-category` and the same for every account and every
//! read path. They are therefore neither a rendering of the arn's tokens nor a
//! sentence template shared by the sub-categories of one category: two entries
//! of one category can carry display names of different forms and descriptions
//! that state OPPOSITE delivery behaviour, so each entry's text has to be
//! carried per entry rather than composed from its segments or from a sibling's
//! sentence.
//!
//! `tests/all/managed_notification_catalog.rs` pins the whole catalog's
//! contents on the unpaged listing and `tests/all/managed_catalog_published_text.rs`
//! pins the Marketplace BUYER cluster on every read path; this suite is the same
//! rule applied to the two account-level clusters, and to what makes their text
//! un-derivable: the per-entry display-name forms and the per-entry delivery
//! statement.
//!
//! Shape source: `ManagedNotificationConfigurationStructure.{arn,name,description}`
//! are all `@required`, `name` `@length` 1..64 and `description` `@length`
//! 0..256
//! (`smithy_query(query="shape",shape="ManagedNotificationConfigurationStructure,ManagedNotificationConfigurationName,ManagedNotificationConfigurationDescription",model_path="services/service-com-amazonaws-notifications-notifications/smithy-model/com-amazonaws-notifications-notifications.json")`).
//! The model enumerates no catalog entry, so the text itself is the service's
//! published behaviour.

use serde_json::Value;
use service_com_amazonaws_notifications_notifications::state::MANAGED_DEFAULT_CHANNEL_IDENTIFIER;

use crate::harness::{encode_path_label, managed_configuration_arn_segments, Emulator};

/// `ManagedNotificationConfigurationName`'s modelled `@length`: 1..64.
const NAME_BOUNDS: std::ops::RangeInclusive<usize> = 1..=64;

/// `ManagedNotificationConfigurationDescription`'s modelled `@length`: 0..256.
const DESCRIPTION_MAX: usize = 256;

/// The whole `AWS-Health` cluster of the published catalog, as
/// `(sub-category, name, description)`.
///
/// Catalog source: the service's published set, `MANAGED_NOTIFICATION_CATALOG`
/// in `src/state.rs` — the model enumerates no entry, so this table is the
/// published behaviour. The rows are the whole cluster rather than one entry of
/// it, including the sub-categories the issue this suite grew from did not cite
/// (`Operations` and `Security`).
const HEALTH_PUBLISHED: &[(&str, &str, &str)] = &[
    (
        "Issue",
        "Account-Specific Issues",
        "All account-specific Health events that do not send emails by default. These Health events provide frequent updates about the state of your AWS infrastructure, such as service events or maintenance notices.",
    ),
    (
        "Billing",
        "Billing Notification",
        "Health events with Billing implications that are sent through email. By default, these are sent to the root email address, operations alternate contact, and the billing alternate contact.",
    ),
    (
        "Operations",
        "Health Operations",
        "Health events with Operational implications that are sent through email. By default, these are sent to the root email address and the operations alternate contact.",
    ),
    (
        "Security",
        "Security",
        "Health events with Security implications that are sent through email. By default, these are sent to the root email address, operations alternate contact, and the security alternate contact.",
    ),
];

/// The arn `category/` segment of the cluster above.
const HEALTH_CATEGORY: &str = "AWS-Health";

/// The published display names of the whole `AWS-BCM` cluster, as
/// `(sub-category, name)`, from the same published set.
const BCM_PUBLISHED_NAMES: &[(&str, &str)] = &[
    ("Billing-and-Invoices", "Billing and Invoices"),
    ("Financing", "Financing"),
    ("Fraud-Prevention", "Fraud Prevention"),
    ("Payment", "Payment"),
    ("Subscriptions", "Subscriptions"),
];

/// The arn `category/` segment of the Billing and Cost Management cluster.
const BCM_CATEGORY: &str = "AWS-BCM";

/// The published description of `AWS-BCM/Subscriptions`, the entry of that
/// cluster whose own published sentence this checkout carries.
const BCM_SUBSCRIPTIONS_DESCRIPTION: &str = "Updates about your AWS subscriptions, plans, credits, and billing status, including sign-ups, plan changes, and Marketplace activity.";

/// The clause the three `AWS-Health` sub-categories whose events AWS delivers by
/// email open their delivery statement with.
const SENT_BY_EMAIL: &str = "that are sent through email. By default, these are sent to the root email address";

/// One entry as a read path reported it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Reported {
    arn: String,
    name: String,
    description: String,
}

/// Every entry of a listing response, as this suite compares them.
fn reported(response: &Value, case: &str) -> Vec<Reported> {
    response["managedNotificationConfigurations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: a listing answers an array, got {response:?}"))
        .iter()
        .map(|entry| {
            let field = |member: &str| {
                entry[member]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case}: every entry carries {member}, got {entry:?}")
                    })
                    .to_string()
            };
            Reported {
                arn: field("arn"),
                name: field("name"),
                description: field("description"),
            }
        })
        .collect()
}

/// A listing with a raw query string; the empty string names no parameter at all.
async fn listing(emulator: &Emulator, case: &str, query: &str) -> Vec<Reported> {
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
    reported(&response, case)
}

/// The whole catalog walked one page at a time at `size` entries a page.
async fn paged_listing(emulator: &Emulator, case: &str, size: usize) -> Vec<Reported> {
    let mut seen: Vec<Reported> = Vec::new();
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
        seen.extend(reported(&response, case));
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

/// `GetManagedNotificationConfiguration` for one arn.
async fn read_entry(emulator: &Emulator, case: &str, arn: &str) -> (Reported, Value) {
    let uri = format!(
        "/managed-notification-configurations/{}",
        encode_path_label(arn)
    );
    let (status, response) = emulator.send("GET", &uri, None).await;
    assert_eq!(
        status, 200,
        "case {case}: a published arn must be readable, got {status} {response:?}"
    );
    let field = |member: &str| {
        response[member]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: the read answers {member}, got {response:?}"))
            .to_string()
    };
    (
        Reported {
            arn: arn.to_string(),
            name: field("name"),
            description: field("description"),
        },
        response,
    )
}

/// The listed entry whose arn carries `category`/`sub_category`.
fn entry_of<'a>(
    listed: &'a [Reported],
    case: &str,
    category: &str,
    sub_category: &str,
) -> &'a Reported {
    let suffix =
        format!("managed-notification-configuration/category/{category}/sub-category/{sub_category}");
    listed
        .iter()
        .find(|entry| entry.arn.ends_with(&suffix))
        .unwrap_or_else(|| {
            panic!(
                "case {case}: the listing must publish this sub-category, got {:?}",
                listed.iter().map(|entry| &entry.arn).collect::<Vec<_>>()
            )
        })
}

/// RULE applied to the whole `AWS-Health` cluster: each entry carries its own
/// published name and description, reported identically by the listing and by
/// the read of its arn, and both inside the bounds the model declares.
#[tokio::test]
async fn the_health_cluster_publishes_its_own_text_per_entry() {
    let emulator = Emulator::new();
    let listed = listing(&emulator, "the whole catalog", "maxResults=100").await;

    for (sub_category, name, description) in HEALTH_PUBLISHED {
        let case = format!("{HEALTH_CATEGORY}/{sub_category}");
        let found = entry_of(&listed, &case, HEALTH_CATEGORY, sub_category);
        assert_eq!(
            (found.name.as_str(), found.description.as_str()),
            (*name, *description),
            "case {case}: expected the published name {name:?} and description {description:?}, \
             got {:?} and {:?}",
            found.name,
            found.description
        );
        assert!(
            NAME_BOUNDS.contains(&name.chars().count())
                && description.chars().count() <= DESCRIPTION_MAX,
            "case {case}: the published text must stay inside the modelled bounds, got a name of \
             {} and a description of {} characters",
            name.chars().count(),
            description.chars().count()
        );

        let (read, response) = read_entry(&emulator, &case, &found.arn).await;
        assert_eq!(
            read, *found,
            "case {case}: a read must report the same published text as the listing, got {read:?}"
        );
        assert_eq!(
            (
                response["category"].as_str(),
                response["subCategory"].as_str()
            ),
            (Some(HEALTH_CATEGORY), Some(*sub_category)),
            "case {case}: the read must report the arn's own segments, got {response:?}"
        );
    }

    // Exactly that cluster: an entry added to or removed from it fails here.
    let published: std::collections::BTreeSet<String> = listed
        .iter()
        .filter_map(|entry| managed_configuration_arn_segments(&entry.arn))
        .filter(|(category, _)| category == HEALTH_CATEGORY)
        .map(|(_, sub_category)| sub_category)
        .collect();
    let expected: std::collections::BTreeSet<String> = HEALTH_PUBLISHED
        .iter()
        .map(|(sub_category, _, _)| (*sub_category).to_string())
        .collect();
    assert_eq!(
        published, expected,
        "case {HEALTH_CATEGORY}: expected exactly the published sub-categories {expected:?}, got \
         {published:?}"
    );
}

/// RULE: an entry's text is not a template filled from its category and
/// sub-category — within ONE category the display names take several forms and
/// the descriptions state different delivery behaviour, which no derivation from
/// the arn's segments could produce.
#[tokio::test]
async fn one_categorys_entries_do_not_share_a_derived_form() {
    let emulator = Emulator::new();
    let listed = listing(&emulator, "the whole catalog", "maxResults=100").await;

    // Rows: the display-name forms the one `AWS-Health` category publishes —
    // prose neither segment spells, prose the sub-category segment opens, the
    // `Health <segment>` form, and the bare segment.
    for (sub_category, expected_form) in [
        ("Issue", "neither segment"),
        ("Billing", "the segment and a word it does not spell"),
        ("Operations", "the category's word and the segment"),
        ("Security", "the segment alone"),
    ] {
        let case = format!("{HEALTH_CATEGORY}/{sub_category}");
        let entry = entry_of(&listed, &case, HEALTH_CATEGORY, sub_category);
        let derived = [
            format!("{HEALTH_CATEGORY}-{sub_category}"),
            format!("{HEALTH_CATEGORY}/{sub_category}"),
            format!("{HEALTH_CATEGORY}{sub_category}"),
            format!("{sub_category}-{HEALTH_CATEGORY}"),
            HEALTH_CATEGORY.to_string(),
        ];
        for rendering in &derived {
            assert_ne!(
                &entry.name, rendering,
                "case {case} ({expected_form}): the name is published display text, not the arn \
                 rendering {rendering:?}"
            );
        }
    }

    let names: std::collections::BTreeSet<&str> = HEALTH_PUBLISHED
        .iter()
        .map(|(_, name, _)| *name)
        .collect();
    let spelled_by_segment = HEALTH_PUBLISHED
        .iter()
        .filter(|(sub_category, name, _)| name == sub_category)
        .count();
    assert!(
        names.len() == HEALTH_PUBLISHED.len() && spelled_by_segment < HEALTH_PUBLISHED.len(),
        "case {HEALTH_CATEGORY}: one category's names must be per-entry text, got {names:?} with \
         {spelled_by_segment} spelled by their own segment"
    );

    // And the delivery statements disagree inside the one category: the
    // account-specific issue events do NOT send email by default, while the
    // other three sub-categories' events do — so no single sentence template
    // serves the cluster.
    for (sub_category, _, description) in HEALTH_PUBLISHED {
        let case = format!("{HEALTH_CATEGORY}/{sub_category}");
        let entry = entry_of(&listed, &case, HEALTH_CATEGORY, sub_category);
        let emails_by_default = *sub_category != "Issue";
        assert_eq!(
            entry.description.contains(SENT_BY_EMAIL),
            emails_by_default,
            "case {case}: expected the published delivery statement (emails by default: \
             {emails_by_default}) of {description:?}, got {:?}",
            entry.description
        );
        if !emails_by_default {
            assert!(
                entry.description.contains("do not send emails by default"),
                "case {case}: the published sentence must state that these events do not send \
                 email by default, got {:?}",
                entry.description
            );
        }
    }
}

/// RULE applied to the Billing and Cost Management cluster: it publishes exactly
/// the sub-categories of the table above with those display names, and the
/// entry whose published sentence this service carries reports it on every read
/// path.
#[tokio::test]
async fn the_billing_cluster_publishes_its_own_text_per_entry() {
    let emulator = Emulator::new();
    let listed = listing(&emulator, "the whole catalog", "maxResults=100").await;

    for (sub_category, name) in BCM_PUBLISHED_NAMES {
        let case = format!("{BCM_CATEGORY}/{sub_category}");
        let entry = entry_of(&listed, &case, BCM_CATEGORY, sub_category);
        assert_eq!(
            entry.name.as_str(),
            *name,
            "case {case}: expected the published name {name:?}, got {:?}",
            entry.name
        );
        assert!(
            NAME_BOUNDS.contains(&entry.name.chars().count())
                && !entry.description.is_empty()
                && entry.description.chars().count() <= DESCRIPTION_MAX
                && entry.description.ends_with('.'),
            "case {case}: the published text must be prose inside the modelled bounds, got {entry:?}"
        );

        let (read, _) = read_entry(&emulator, &case, &entry.arn).await;
        assert_eq!(
            read, *entry,
            "case {case}: a read must report the same published text as the listing, got {read:?}"
        );
    }

    let published: std::collections::BTreeSet<String> = listed
        .iter()
        .filter_map(|entry| managed_configuration_arn_segments(&entry.arn))
        .filter(|(category, _)| category == BCM_CATEGORY)
        .map(|(_, sub_category)| sub_category)
        .collect();
    let expected: std::collections::BTreeSet<String> = BCM_PUBLISHED_NAMES
        .iter()
        .map(|(sub_category, _)| (*sub_category).to_string())
        .collect();
    assert_eq!(
        published, expected,
        "case {BCM_CATEGORY}: expected exactly the published sub-categories {expected:?}, got \
         {published:?}"
    );

    let subscriptions = entry_of(
        &listed,
        "AWS-BCM/Subscriptions",
        BCM_CATEGORY,
        "Subscriptions",
    );
    assert_eq!(
        subscriptions.description.as_str(),
        BCM_SUBSCRIPTIONS_DESCRIPTION,
        "case {BCM_CATEGORY}/Subscriptions: expected the published description \
         {BCM_SUBSCRIPTIONS_DESCRIPTION:?}, got {:?}",
        subscriptions.description
    );
}

/// RULE: the published text is the same on every path a caller reaches an entry
/// by and in every store, since nothing a caller does creates, edits or deletes
/// a managed configuration.
#[tokio::test]
async fn every_path_and_every_store_reports_the_same_account_cluster_text() {
    let emulator = Emulator::new();

    // The rows differ along the way a caller enumerates the catalog: no
    // parameter at all, an explicit page size at the modelled maximum, a walk
    // at several page sizes, and the filter on the channel every published
    // entry starts associated with.
    let filter = format!(
        "channelIdentifier={}",
        encode_path_label(MANAGED_DEFAULT_CHANNEL_IDENTIFIER)
    );
    let mut paths: Vec<(String, Vec<Reported>)> = vec![
        (
            "no parameters".to_string(),
            listing(&emulator, "no parameters", "").await,
        ),
        (
            "maxResults at the modelled maximum".to_string(),
            listing(&emulator, "maxResults at the modelled maximum", "maxResults=100").await,
        ),
        (
            "the channelIdentifier filter".to_string(),
            listing(&emulator, "the channelIdentifier filter", filter.as_str()).await,
        ),
    ];
    for size in [1_usize, 2, 7] {
        let case = format!("a paged walk at maxResults={size}");
        let walked = paged_listing(&emulator, &case, size).await;
        paths.push((case, walked));
    }
    // And a second, independently booted store: the catalog is a published set,
    // not per-store state.
    let other = Emulator::new();
    paths.push((
        "another store".to_string(),
        listing(&other, "another store", "maxResults=100").await,
    ));

    for (case, entries) in &paths {
        for (sub_category, name, description) in HEALTH_PUBLISHED {
            let row = format!("{case}: {HEALTH_CATEGORY}/{sub_category}");
            let entry = entry_of(entries, &row, HEALTH_CATEGORY, sub_category);
            assert_eq!(
                (entry.name.as_str(), entry.description.as_str()),
                (*name, *description),
                "case {row}: expected the published name {name:?} and description \
                 {description:?}, got {:?} and {:?}",
                entry.name,
                entry.description
            );
        }
        for (sub_category, name) in BCM_PUBLISHED_NAMES {
            let row = format!("{case}: {BCM_CATEGORY}/{sub_category}");
            let entry = entry_of(entries, &row, BCM_CATEGORY, sub_category);
            assert_eq!(
                entry.name.as_str(),
                *name,
                "case {row}: expected the published name {name:?}, got {:?}",
                entry.name
            );
        }
    }
}
