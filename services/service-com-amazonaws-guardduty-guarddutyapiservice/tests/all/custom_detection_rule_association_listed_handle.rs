// SPDX-License-Identifier: CC0-1.0

//! The handle a `ListCustomDetectionRuleAssociations` entry advertises, read off the
//! BYTES the service answered with and then used the way a client uses it.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! serialization and the handlers are exercised together. Unlike the sibling identity
//! suite, which asserts over the PARSED payload, this one reads the response body as
//! the service serialized it: parsing collapses a member's JSON type, its case and the
//! whitespace around it, and those are the dimensions a listed handle has to agree with
//! its own ARN in before a client can act on it.
//!
//! The rules under test, each asserted over a SET of inputs rather than one call:
//!
//! 1. In the bytes a listing answers with, every entry's `associationId` is a plain
//!    JSON STRING whose characters are exactly the trailing `/association/<id>`
//!    segment of that same entry's own `arn`, and exactly the id the create minted for
//!    that association: `AssociationId` is "the unique identifier for the association"
//!    and `Arn` is "the Amazon Resource Name (ARN) of the association"
//!    (`smithy_query(shape="AssociationSummary")`), so the two members of one summary
//!    name one resource. Asserted over every published rule, in both
//!    `AssociationMode`s, through two Regions, and over the unfiltered listing, each
//!    filter the operation takes and a page walk one entry at a time.
//! 2. That id is the association's HANDLE: taken out of the listing — never out of the
//!    create — it is accepted as the `AssociationId` @httpLabel of
//!    `GetCustomDetectionRuleAssociation`, `UpdateCustomDetectionRuleAssociation` and
//!    `DeleteCustomDetectionRuleAssociation` (`smithy_query(operation=
//!    "GetCustomDetectionRuleAssociation,UpdateCustomDetectionRuleAssociation,DeleteCustomDetectionRuleAssociation")`)
//!    and addresses the association the listing reported it for, so list-then-act
//!    resolves instead of answering the modelled 404.
//!
//! No `AssociationId` is written into this suite: every one is read off the emulator's
//! own response, because the digits of an id are minted per run and only its form, its
//! stability and its usability are requirements.

use serde_json::{json, Value};

use crate::custom_detection_rule_association_identity::{
    arn_association_id, associate, entry_with_arn,
};
use crate::detector_lifecycle::{
    call_bytes_in, call_in, error_code, new_router, published_rule_ids, Router,
};

/// `ListCustomDetectionRuleAssociations` is `GET /custom-detection-rule/association`
/// (`smithy_query(operation="ListCustomDetectionRuleAssociations")`).
const ASSOCIATIONS_PATH: &str = "/custom-detection-rule/association";

/// The read, the update and the delete of one association are bound at
/// `/custom-detection-rule/rule/{RuleId}/association/{AssociationId}`
/// (`smithy_query(operation="GetCustomDetectionRuleAssociation,UpdateCustomDetectionRuleAssociation,DeleteCustomDetectionRuleAssociation")`).
const RULES_PATH: &str = "/custom-detection-rule/rule";

/// Every `AssociationMode` member (`smithy_query(shape="AssociationMode")`).
const MODES: [&str; 2] = ["DRY_RUN", "LIVE"];

/// Two Regions, so a listed handle is asserted to be the Region's own rather than
/// assumed.
const REGIONS: [&str; 2] = ["us-east-1", "ap-south-1"];

/// `MaxResults` is `range 1..100`
/// (`smithy_query(operation="ListCustomDetectionRuleAssociations")`), so this page size
/// reports every association a case makes at once.
const MAX_RESULTS: i64 = 100;

/// The modelled error shape an association the account does not hold is refused with
/// (`smithy_query(operation="GetCustomDetectionRuleAssociation")`).
const NOT_FOUND: &str = "ResourceNotFoundException";

/// One listing answer: the bytes it arrived as, and the entries and page token parsed
/// out of those same bytes.
struct Listing {
    raw: String,
    entries: Vec<Value>,
    next_token: Option<String>,
}

/// Read one page of the listing `region`'s endpoint reports for the query string
/// `query`, failing the test when it does not succeed.
async fn listing(router: &Router, region: &str, query: &str, case: &str) -> Listing {
    let (status, raw) = call_bytes_in(
        router,
        region,
        "GET",
        &format!("{ASSOCIATIONS_PATH}{query}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: list-associations expected 200, got {status} {raw:?}"
    );
    let parsed: Value = serde_json::from_str(&raw)
        .unwrap_or_else(|error| panic!("case {case:?}: expected a JSON body, got {raw:?} ({error})"));
    Listing {
        entries: parsed["ruleAssociations"]
            .as_array()
            .cloned()
            .unwrap_or_default(),
        next_token: parsed["nextToken"].as_str().map(str::to_string),
        raw,
    }
}

/// Walk every page of the listing `query` addresses one entry at a time, returning each
/// page's answer in order.
///
/// Bounded by the number of pages a one-entry page size can possibly need for the
/// associations a case makes, so a token that failed to advance fails the test instead
/// of looping.
async fn page_walk(router: &Router, region: &str, query: &str, pages_max: usize, case: &str) -> Vec<Listing> {
    let mut pages: Vec<Listing> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let page_query = match token.as_deref() {
            Some(token) => format!("{query}&nextToken={token}"),
            None => query.to_string(),
        };
        let page = listing(router, region, &page_query, case).await;
        token = page.next_token.clone();
        pages.push(page);
        assert!(
            pages.len() <= pages_max,
            "case {case:?}: expected the page walk to end within {pages_max} pages, got {} pages",
            pages.len()
        );
        if token.is_none() {
            break;
        }
    }
    pages
}

/// Assert the identity rule on ONE serialized entry: its `associationId` is a plain
/// JSON string, equal to `minted_id` and to the id its own `arn` names, and the bytes
/// of the page carry both members in exactly that form.
fn assert_serialized_identity(raw: &str, entry: &Value, minted_id: &str, arn: &str, case: &str) {
    let listed = entry["associationId"].as_str().unwrap_or_else(|| {
        panic!("case {case:?}: expected associationId to be a JSON string, got {entry:?}")
    });
    assert_eq!(
        listed, minted_id,
        "case {case:?}: expected the entry to advertise the id the create minted {minted_id:?}, got {entry:?}"
    );
    let named = arn_association_id(
        entry["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an arn, got {entry:?}")),
    )
    .unwrap_or_else(|| panic!("case {case:?}: expected an arn naming an association, got {entry:?}"));
    assert_eq!(
        named, listed,
        "case {case:?}: expected the entry's arn to end in its own associationId {listed:?}, got {entry:?}"
    );
    // The serialized form, byte for byte: a quoted string with the id's own characters
    // and nothing around them — not a number, not a doubly-quoted string, not padded.
    let serialized = format!("\"associationId\":\"{minted_id}\"");
    assert!(
        raw.contains(&serialized),
        "case {case:?}: expected the body to carry {serialized} as serialized, got {raw:?}"
    );
    let serialized_arn = format!("\"arn\":\"{arn}\"");
    assert!(
        raw.contains(&serialized_arn),
        "case {case:?}: expected the body to carry {serialized_arn} as serialized, got {raw:?}"
    );
}

/// The `AssociationDetail` the read of `(rule_id, id)` reports, with the status it
/// answered.
async fn read_association(router: &Router, region: &str, rule_id: &str, id: &str) -> (u16, Value) {
    call_in(
        router,
        region,
        "GET",
        &format!("{RULES_PATH}/{rule_id}/association/{id}"),
        None,
    )
    .await
}

/// One association a case made: where it lives, what it binds, and the identity the
/// create reported for it.
struct Made {
    region: &'static str,
    rule_id: String,
    mode: &'static str,
    id: String,
    arn: String,
}

/// The listings that report an association, paired with the label a failure names: the
/// whole Region's set and each of the two filters the operation takes.
fn single_page_queries(rule_id: &str, mode: &str) -> Vec<(String, String)> {
    vec![
        (
            format!("?maxResults={MAX_RESULTS}"),
            "unfiltered listing".to_string(),
        ),
        (
            format!("?maxResults={MAX_RESULTS}&ruleId={rule_id}"),
            "listing filtered by rule".to_string(),
        ),
        (
            format!("?maxResults={MAX_RESULTS}&mode={mode}"),
            "listing filtered by mode".to_string(),
        ),
    ]
}

// ---------------------------------------------------------------------------
// Rule 1: the serialized handle and the serialized ARN name one resource.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_listed_entry_serializes_the_handle_its_own_arn_names() {
    let router = new_router();
    let rules = published_rule_ids(&router).await;

    // Each row: a published rule, a Region and the mode the association is made in, the
    // mode alternating with both indices so every rule is exercised in both modes
    // across the two Regions.
    let mut made: Vec<Made> = Vec::new();
    for (region_index, region) in REGIONS.iter().enumerate() {
        for (rule_index, rule_id) in rules.iter().enumerate() {
            let mode = MODES[(rule_index + region_index) % MODES.len()];
            let case = format!("{rule_id}/{mode} in {region}");
            let created = associate(&router, region, rule_id, mode).await;
            made.push(Made {
                region,
                rule_id: rule_id.clone(),
                mode,
                id: created["associationId"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: expected an associationId, got {created:?}")
                    })
                    .to_string(),
                arn: created["arn"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: expected an arn, got {created:?}"))
                    .to_string(),
            });
        }
    }

    for row in &made {
        for (query, label) in single_page_queries(&row.rule_id, row.mode) {
            let case = format!("{}/{} in {}: {label}", row.rule_id, row.mode, row.region);
            let page = listing(&router, row.region, &query, &case).await;
            let entry = entry_with_arn(&page.entries, &row.arn, &case);
            assert_serialized_identity(&page.raw, &entry, &row.id, &row.arn, &case);

            // The handle is taken out of the LISTING, as a list-then-act client takes
            // it, and must address the association the listing reported it for.
            let listed = entry["associationId"].as_str().unwrap_or_default();
            let (status, body) = read_association(&router, row.region, &row.rule_id, listed).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the listed handle must resolve, got {status} {body:?}"
            );
            assert_eq!(
                body["ruleAssociation"]["arn"].as_str(),
                Some(row.arn.as_str()),
                "case {case:?}: the listed handle must address its own association, got {body:?}"
            );
        }
    }

    // A page walk one entry at a time reports each association of the Region exactly
    // once, and every page holds its entry to the same serialized rule: a page token
    // changes WHICH entries a page carries, never the identity one advertises.
    for region in REGIONS {
        let expected: Vec<&Made> = made.iter().filter(|row| row.region == region).collect();
        let case = format!("page walk one entry at a time in {region}");
        let pages = page_walk(
            &router,
            region,
            "?maxResults=1",
            expected.len() + 1,
            &case,
        )
        .await;
        let mut seen: Vec<(String, String)> = Vec::new();
        for page in &pages {
            assert!(
                page.entries.len() <= 1,
                "case {case:?}: expected no page longer than the page size asked for, got {:?}",
                page.entries
            );
            for entry in &page.entries {
                let arn = entry["arn"].as_str().unwrap_or_default().to_string();
                let row = expected
                    .iter()
                    .find(|row| row.arn == arn)
                    .unwrap_or_else(|| panic!("case {case:?}: expected a page entry of this Region's set, got {entry:?}"));
                assert_serialized_identity(&page.raw, entry, &row.id, &row.arn, &case);
                seen.push((
                    arn,
                    entry["associationId"].as_str().unwrap_or_default().to_string(),
                ));
            }
        }
        assert_eq!(
            seen.len(),
            expected.len(),
            "case {case:?}: expected the walk to report each of this Region's {} associations once, got {seen:?}",
            expected.len()
        );
        for row in &expected {
            assert!(
                seen.iter().any(|(arn, id)| *arn == row.arn && *id == row.id),
                "case {case:?}: expected the walk to report {} under {}, got {seen:?}",
                row.arn,
                row.id
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the discovered handle is the one the update and the delete take.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_handle_discovered_in_a_listing_drives_the_update_and_the_delete() {
    let router = new_router();
    let rules = published_rule_ids(&router).await;
    let region = REGIONS[0];

    // Each row: a published rule, the mode it is associated in and the mode an update
    // then asks for — so the table spans a move in each direction as well as an update
    // asking for the mode the association already has.
    for (rule_index, rule_id) in rules.iter().enumerate() {
        let from = MODES[rule_index % MODES.len()];
        let to = MODES[(rule_index / MODES.len()) % MODES.len()];
        let case = format!("{rule_id}: associated {from}, updated to {to} from the listing");

        let created = associate(&router, region, rule_id, from).await;
        let arn = created["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an arn, got {created:?}"))
            .to_string();

        // Discovery: everything below uses the id the LISTING advertises.
        let page = listing(
            &router,
            region,
            &format!("?maxResults={MAX_RESULTS}&ruleId={rule_id}"),
            &case,
        )
        .await;
        let entry = entry_with_arn(&page.entries, &arn, &case);
        let discovered = entry["associationId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an associationId, got {entry:?}"))
            .to_string();
        assert_serialized_identity(&page.raw, &entry, &discovered, &arn, &case);

        let (status, body) = call_in(
            &router,
            region,
            "PUT",
            &format!("{RULES_PATH}/{rule_id}/association/{discovered}"),
            Some(json!({"mode": to})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the discovered handle must be accepted by the update, got {status} {body:?}"
        );
        // `UpdateCustomDetectionRuleAssociation` answers no payload
        // (`smithy_query(operation="UpdateCustomDetectionRuleAssociation")` reports an
        // empty output), so what it did is read back through the same discovered
        // handle: the mode moved, the identity did not.
        let (status, body) = read_association(&router, region, rule_id, &discovered).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the discovered handle must still resolve after the update, got {status} {body:?}"
        );
        assert_eq!(
            body["ruleAssociation"]["associationId"].as_str(),
            Some(discovered.as_str()),
            "case {case:?}: the read must answer under the handle it was addressed by, got {body:?}"
        );
        assert_eq!(
            body["ruleAssociation"]["mode"].as_str(),
            Some(to),
            "case {case:?}: the update must set the mode it asked for, got {body:?}"
        );

        // The listing keeps advertising that same handle after the update, now in the
        // mode the update set.
        let page = listing(
            &router,
            region,
            &format!("?maxResults={MAX_RESULTS}&mode={to}"),
            &case,
        )
        .await;
        let entry = entry_with_arn(&page.entries, &arn, &case);
        assert_serialized_identity(&page.raw, &entry, &discovered, &arn, &case);

        let (status, body) = call_in(
            &router,
            region,
            "DELETE",
            &format!("{RULES_PATH}/{rule_id}/association/{discovered}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the discovered handle must be accepted by the delete, got {status} {body:?}"
        );

        // The association the discovered handle addressed is the one that went: the
        // handle stops resolving and its ARN leaves the listing.
        let (status, body) = read_association(&router, region, rule_id, &discovered).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a deleted handle must stop resolving, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            NOT_FOUND,
            "case {case:?}: expected the {NOT_FOUND} class, got {body:?}"
        );
        let page = listing(
            &router,
            region,
            &format!("?maxResults={MAX_RESULTS}"),
            &case,
        )
        .await;
        assert!(
            !page.entries.iter().any(|entry| entry["arn"].as_str() == Some(arn.as_str())),
            "case {case:?}: a deleted association must leave the listing, got {:?}",
            page.entries
        );
    }
}
