// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHAT the published `ManagedNotificationConfiguration`
//! catalog contains, driven over HTTP through the same multi-service router the
//! emulator boots in production.
//!
//! The ordering, paging and filtering of the listing are covered by
//! `tests/all/managed_notification_configurations.rs` and the catalog's
//! membership by `tests/all/managed_catalog_membership.rs`; this suite is about
//! the catalog's CONTENTS and the rule they follow: the service publishes one
//! entry per event-type sub-category of every service whose events it covers,
//! each entry carrying its OWN display `name` and prose `description` — text
//! the service publishes rather than text computed from the arn's
//! `category`/`sub-category` tokens, so a caller that renders `name` shows
//! display text. Because nothing a caller does creates or removes a managed
//! configuration, that published set and its text are the SAME for every
//! account and every read: a fixed catalog, not a per-account or per-run value.
//!
//! Shape source for the bounds and the arn layout the rows below check:
//! `smithy_query(query="shape",shape="ManagedNotificationConfigurationStructure,ManagedNotificationConfigurationName,ManagedNotificationConfigurationDescription,ManagedNotificationConfigurationOsArn",model_path="services/service-com-amazonaws-notifications-notifications/smithy-model/com-amazonaws-notifications-notifications.json")`.

use serde_json::Value;

use crate::harness::{encode_path_label, managed_configuration_arn_segments, Emulator};

/// `ManagedNotificationConfigurationName`'s modelled `@length`: 1..64.
const NAME_MIN: usize = 1;
const NAME_MAX: usize = 64;

/// `ManagedNotificationConfigurationDescription`'s modelled `@length`: 0..256.
/// The published descriptions are prose, so each one is also asserted non-empty.
const DESCRIPTION_MAX: usize = 256;

/// The catalog the service publishes: `category`, `sub-category`, and the entry's
/// own published `name` and `description`.
///
/// Catalog source: the service's own published set, `MANAGED_NOTIFICATION_CATALOG`
/// in `src/state.rs` — the model enumerates no entries (only their shape, see
/// the module header's `smithy_query`), so this table is the published behaviour
/// and the rows span every category the service covers rather than one of them:
/// the Billing and Cost Management side, the health side, the Marketplace
/// SELLER side and the Marketplace BUYER side.
const PUBLISHED: &[(&str, &str, &str, &str)] = &[
    (
        "AWS-BCM",
        "Billing-and-Invoices",
        "Billing and Invoices",
        "Notifications about your bills and invoices, including invoice availability, charges that differ from your usual spend, and changes to the billing documents for your account.",
    ),
    (
        "AWS-BCM",
        "Financing",
        "Financing",
        "Notifications about AWS financing for your account, including financing application outcomes, installment schedules, and upcoming financed payments.",
    ),
    (
        "AWS-BCM",
        "Fraud-Prevention",
        "Fraud Prevention",
        "Notifications about suspected fraudulent activity on your account, including unusual usage, credentials that may be compromised, and the action needed to secure the account.",
    ),
    (
        "AWS-BCM",
        "Payment",
        "Payment",
        "Notifications about your payments, including upcoming and overdue payments, payment confirmations, failed payment methods, and refunds.",
    ),
    (
        "AWS-BCM",
        "Subscriptions",
        "Subscriptions",
        "Notifications about the subscriptions billed to your account, including upcoming renewals, expirations, and cancellations.",
    ),
    (
        "AWS-Health",
        "Issue",
        "Issue",
        "All account health issue events that are sent through email. By default, these are sent to the root email address.",
    ),
    (
        "AWS-Health",
        "Billing",
        "Health Billing",
        "Health events with Billing implications that are sent through email. By default, these are sent to the root email address and the billing alternate contact.",
    ),
    (
        "AWS-Health",
        "Operations",
        "Health Operations",
        "Health events with Operational implications that are sent through email. By default, these are sent to the root email address and the operations alternate contact.",
    ),
    (
        "AWS-Health",
        "Security",
        "Security",
        "Health events with Security implications that are sent through email. By default, these are sent to the root email address, operations alternate contact, and the security alternate contact.",
    ),
    (
        "AWS-Partner-Central",
        "PCProductListings",
        "Product listings",
        "Marketplace events related to your published AMI, container, and SaaS product catalog, including listing change set status, vulnerability scan findings, open product tasks, and Vendor Insights security profile snapshots.",
    ),
    (
        "AWS-Partner-Central",
        "PCOffersAndAgreements",
        "Offers and Agreements",
        "Marketplace events related to private offers, reseller activity, and post-acceptance agreement lifecycle, including offer publishing, acceptance, agreement creation, amendment, cancellation, compliance failures, and ISV entitlement events.",
    ),
    (
        "AWS-Partner-Central",
        "PCPaymentsAndDisbursements",
        "Payments and disbursements",
        "Marketplace events with payments and financial implications, including disbursement failures and pauses, payment failures, payment request rejections, billing adjustments on purchase agreements, and invoice submission outcomes.",
    ),
    (
        "AWS-Partner-Central",
        "PCAccountManagement",
        "Account management",
        "Marketplace events related to seller account standing, including business and bank account verification status.",
    ),
    (
        "AWS-MP-Buyer",
        "MPBuyerProducts",
        "Products and solutions",
        "Notifications about changes to products you've subscribed to, including new versions, instance type updates, visibility changes, region restrictions, and security profile snapshots.",
    ),
    (
        "AWS-MP-Buyer",
        "MPBuyerPrivateOffers",
        "Private offers",
        "Notifications about private offers extended to you, including new offers, offer set availability, and channel partner offer lifecycle updates.",
    ),
    (
        "AWS-MP-Buyer",
        "MPBuyerPricing",
        "Pricing changes",
        "Notifications about price changes to products you're subscribed to, including hourly, monthly, and usage-based pricing updates.",
    ),
    (
        "AWS-MP-Buyer",
        "MPBuyerAgreements",
        "Agreements and subscriptions",
        "Notifications about your agreements and subscriptions, including new agreements, amendments, cancellations, expirations, renewals, free trial reminders, payment requests, and billing adjustments.",
    ),
];

/// The page size the listing uses when a request names none: "Defaults to 20",
/// from the `maxResults` member's own documentation
/// (`smithy_query(query="operation",operation="ListManagedNotificationConfigurations")`).
const DEFAULT_PAGE_SIZE: usize = 20;

/// Every entry the unpaged listing answers, as `(arn, name, description)`.
async fn listed_catalog(emulator: &Emulator) -> Vec<(String, String, String)> {
    // `maxResults`' modelled maximum is 100, which is more than the catalog
    // holds, so one page carries all of it.
    let (status, response) = emulator
        .send("GET", "/managed-notification-configurations?maxResults=100", None)
        .await;
    assert_eq!(
        status, 200,
        "case unpaged listing: expected 200, got {status} {response:?}"
    );
    assert_eq!(
        response["nextToken"],
        Value::Null,
        "case unpaged listing: the whole catalog fits one page of 100, got {response:?}"
    );
    response["managedNotificationConfigurations"]
        .as_array()
        .unwrap_or_else(|| panic!("case unpaged listing: expected an array, got {response:?}"))
        .iter()
        .map(|entry| {
            let field = |name: &str| {
                entry[name]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case unpaged listing: every entry carries {name}, got {entry:?}")
                    })
                    .to_string()
            };
            (field("arn"), field("name"), field("description"))
        })
        .collect()
}

/// `GetManagedNotificationConfiguration` for one arn.
async fn read_catalog_entry(emulator: &Emulator, arn: &str) -> (u16, Value) {
    let uri = format!(
        "/managed-notification-configurations/{}",
        encode_path_label(arn)
    );
    emulator.send("GET", &uri, None).await
}

/// RULE: the catalog covers more than one service's events — one entry per
/// event-type category of each covered service — and each entry carries the
/// `name` and `description` the service publishes for it, so a caller
/// enumerating it discovers every category with the text AWS states for it.
#[tokio::test]
async fn the_catalog_spans_every_published_category() {
    let emulator = Emulator::new();
    let listed = listed_catalog(&emulator).await;

    for (category, sub_category, name, description) in PUBLISHED {
        let case = format!("{category}/{sub_category}");
        let suffix = format!(
            "managed-notification-configuration/category/{category}/sub-category/{sub_category}"
        );
        let found = listed
            .iter()
            .find(|(arn, _, _)| arn.ends_with(&suffix))
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: the listing must publish this category, got {:?}",
                    listed.iter().map(|(arn, _, _)| arn).collect::<Vec<_>>()
                )
            });

        // The published display text, which is the entry's own and the same for
        // every account: no modelled operation creates, deletes or edits a
        // managed configuration.
        assert_eq!(
            (found.1.as_str(), found.2.as_str()),
            (*name, *description),
            "case {case}: expected the published name {name:?} and description \
             {description:?}, got {:?} and {:?}",
            found.1,
            found.2
        );

        // The entry is readable by the arn the listing answered, and the read
        // reports the same display text the listing did.
        let (status, response) = read_catalog_entry(&emulator, &found.0).await;
        assert_eq!(
            status, 200,
            "case {case}: a published arn must be readable, got {status} {response:?}"
        );
        assert_eq!(
            response["name"].as_str(),
            Some(*name),
            "case {case}: expected name {name:?}, got {response:?}"
        );
        assert_eq!(
            response["description"].as_str(),
            Some(*description),
            "case {case}: expected description {description:?}, got {response:?}"
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
    }

    let categories: std::collections::BTreeSet<String> = listed
        .iter()
        .filter_map(|(arn, _, _)| managed_configuration_arn_segments(arn))
        .map(|(category, _)| category)
        .collect();
    assert!(
        categories.len() >= 2,
        "case covered services: the catalog must cover more than one service's events, got {categories:?}"
    );
    for category in &categories {
        let entries = listed
            .iter()
            .filter_map(|(arn, _, _)| managed_configuration_arn_segments(arn))
            .filter(|(listed_category, _)| listed_category == category)
            .count();
        assert!(
            entries >= 2,
            "case {category}: a covered service publishes one entry per event-type category, got {entries}"
        );

        // And it publishes EXACTLY that service's categories: the published
        // cluster of each covered service is the set above, so an entry that
        // disappears from it, or one that appears without being published text
        // of its own, fails here.
        let expected: std::collections::BTreeSet<&str> = PUBLISHED
            .iter()
            .filter(|(published_category, _, _, _)| published_category == category)
            .map(|(_, sub_category, _, _)| *sub_category)
            .collect();
        let found: std::collections::BTreeSet<String> = listed
            .iter()
            .filter_map(|(arn, _, _)| managed_configuration_arn_segments(arn))
            .filter(|(listed_category, _)| listed_category == category)
            .map(|(_, sub_category)| sub_category)
            .collect();
        let found_refs: std::collections::BTreeSet<&str> =
            found.iter().map(String::as_str).collect();
        assert_eq!(
            found_refs, expected,
            "case {category}: expected exactly the published event-type categories {expected:?}, got {found_refs:?}"
        );
    }
}

/// RULE: the whole published catalog is answered to a caller that asks for no
/// page size, since the catalog is smaller than the default page — so a client
/// that enumerates the categories once, without paging, discovers every covered
/// service rather than a prefix of them.
#[tokio::test]
async fn the_default_page_carries_the_whole_published_catalog() {
    assert!(
        PUBLISHED.len() <= DEFAULT_PAGE_SIZE,
        "case catalog size: a catalog larger than the default page of \
         {DEFAULT_PAGE_SIZE} must be enumerated with a token, got {} entries",
        PUBLISHED.len()
    );

    let emulator = Emulator::new();
    // Rows: the two ways a caller asks for no particular page size — no query
    // string at all, and the default stated explicitly.
    for (case, uri) in [
        ("no maxResults", "/managed-notification-configurations"),
        (
            "maxResults at the documented default",
            "/managed-notification-configurations?maxResults=20",
        ),
    ] {
        let (status, response) = emulator.send("GET", uri, None).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} {response:?}"
        );
        assert_eq!(
            response["nextToken"],
            Value::Null,
            "case {case}: the whole catalog fits the default page, got {response:?}"
        );
        let listed: Vec<String> = response["managedNotificationConfigurations"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected an array, got {response:?}"))
            .iter()
            .filter_map(|entry| entry["arn"].as_str())
            .filter_map(managed_configuration_arn_segments)
            .map(|(category, sub_category)| format!("{category}/{sub_category}"))
            .collect();
        for (category, sub_category, _, _) in PUBLISHED {
            let wanted = format!("{category}/{sub_category}");
            assert!(
                listed.contains(&wanted),
                "case {case}: the first page must publish {wanted}, got {listed:?}"
            );
        }
    }
}

/// RULE: an entry's `name` and `description` are its own published display
/// text, independent of the arn — not computed from the arn's
/// `category`/`sub-category` tokens, and both inside the bounds the model
/// declares for them.
///
/// Independent does not mean different: a sub-category named by a single word
/// publishes that word as its display name too (`AWS-BCM/Payment` is
/// `Payment`), so the rule is checked over the catalog — a derivation of the
/// arn's segments could not produce the names of the entries whose display text
/// their segments do not spell — rather than by demanding every entry's name
/// differ from its own segments.
#[tokio::test]
async fn every_entry_carries_display_text_that_is_not_its_arn_segments() {
    let emulator = Emulator::new();
    let listed = listed_catalog(&emulator).await;
    assert_eq!(
        listed.len(),
        PUBLISHED.len(),
        "case published catalog: expected {} entries, got {:?}",
        PUBLISHED.len(),
        listed.iter().map(|(arn, _, _)| arn).collect::<Vec<_>>()
    );

    for (arn, name, description) in &listed {
        let (category, sub_category) = managed_configuration_arn_segments(arn)
            .unwrap_or_else(|| panic!("case {arn}: the arn must match the modelled pattern"));
        let case = format!("{category}/{sub_category}");

        assert!(
            (NAME_MIN..=NAME_MAX).contains(&name.chars().count()),
            "case {case}: name must be {NAME_MIN}..{NAME_MAX} characters, got {name:?}"
        );
        assert!(
            !description.is_empty() && description.chars().count() <= DESCRIPTION_MAX,
            "case {case}: description must be prose within {DESCRIPTION_MAX} characters, got {description:?}"
        );

        // Every rendering of the arn's own two segments: a description is prose
        // about the covered events, so it is none of them.
        let renderings = [
            format!("{category}-{sub_category}"),
            format!("{category}/{sub_category}"),
            format!("{category}{sub_category}"),
            format!("{sub_category}-{category}"),
            category.clone(),
            sub_category.clone(),
        ];
        for rendering in &renderings {
            assert_ne!(
                description, rendering,
                "case {case}: description must be published prose, not the arn rendering {rendering:?}"
            );
        }
        // And the name is published text rather than a rendering of the whole
        // arn path: a two-segment rendering is never a display name, even where
        // a single-word sub-category and its display name coincide.
        for rendering in &renderings[..4] {
            assert_ne!(
                name, rendering,
                "case {case}: name must be published display text, not the arn rendering {rendering:?}"
            );
        }

        // Display text, not an identifier: it reads as words a console renders.
        assert!(
            name.contains(' ') || name.chars().filter(|c| c.is_uppercase()).count() <= 1,
            "case {case}: name must read as display text rather than a concatenated identifier, got {name:?}"
        );
        assert!(
            description.contains(' ') && description.ends_with('.'),
            "case {case}: description must be a prose sentence, got {description:?}"
        );
    }

    // The catalog as a whole proves the name is not computed from the arn: more
    // than one entry publishes a display name neither of its own segments
    // spells, which no derivation of those segments could produce.
    let not_a_segment = listed
        .iter()
        .filter(|(arn, name, _)| {
            managed_configuration_arn_segments(arn)
                .is_some_and(|(category, sub_category)| *name != category && *name != sub_category)
        })
        .count();
    assert!(
        not_a_segment >= 2,
        "case display text: the published names must be the entries' own text rather than a \
         rendering of their arn segments, got {} of {} entries whose name is a segment",
        listed.len() - not_a_segment,
        listed.len()
    );

    // No two entries share a name or an arn, so the catalog identifies each
    // published category distinctly.
    let mut names: Vec<&String> = listed.iter().map(|(_, name, _)| name).collect();
    names.sort();
    let unique = names.len();
    names.dedup();
    assert_eq!(
        names.len(),
        unique,
        "case distinct names: every published entry carries its own name, got {names:?}"
    );
}

/// RULE: the catalog is FIXED — nothing a caller does creates, removes or edits a
/// managed configuration, so every store publishes the same entries with the same
/// text, whatever else has happened in it.
#[tokio::test]
async fn the_published_catalog_is_the_same_set_for_every_store() {
    // Rows: a freshly booted store, the same store read again, a store that has
    // had an unrelated configuration created and tagged in it, and a second,
    // independently booted store — the catalog is the same in all of them, up to
    // the account segment of the arn, which is the environment's.
    let first = Emulator::new();
    let baseline = tail_of(&listed_catalog(&first).await);

    let repeated = tail_of(&listed_catalog(&first).await);
    assert_eq!(
        repeated, baseline,
        "case repeated read: a read must not change the published catalog"
    );

    let body = serde_json::json!({
        "name": "catalog-is-fixed",
        "description": "an unrelated configuration",
    });
    let (status, created) = first
        .send("POST", "/notification-configurations", Some(body))
        .await;
    assert_eq!(
        status, 201,
        "case unrelated write: create must answer 201, got {status} {created:?}"
    );
    let after_write = tail_of(&listed_catalog(&first).await);
    assert_eq!(
        after_write, baseline,
        "case unrelated write: creating a configuration must not change the published catalog"
    );

    let second = Emulator::new();
    let other_store = tail_of(&listed_catalog(&second).await);
    assert_eq!(
        other_store, baseline,
        "case another store: every store publishes the same catalog"
    );
}

/// Each listed entry as `(category/sub-category, name, description)` — the arn's
/// account segment dropped, since that is the environment's value and not part of
/// the published catalog.
fn tail_of(listed: &[(String, String, String)]) -> Vec<(String, String, String)> {
    let mut rows: Vec<(String, String, String)> = listed
        .iter()
        .map(|(arn, name, description)| {
            let (category, sub_category) = managed_configuration_arn_segments(arn)
                .unwrap_or_else(|| panic!("case {arn}: the arn must match the modelled pattern"));
            (
                format!("{category}/{sub_category}"),
                name.clone(),
                description.clone(),
            )
        })
        .collect();
    rows.sort();
    rows
}
