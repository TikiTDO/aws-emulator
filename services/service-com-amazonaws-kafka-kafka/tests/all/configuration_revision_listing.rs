// SPDX-License-Identifier: CC0-1.0

//! What `ListConfigurationRevisions` reports for a stored MSK configuration's
//! revision history, driven end to end over the wire through the same
//! `MultiServiceRouter` the emulator serves from (the shared `kafka_harness`).
//!
//! RULE under test: a revision listing reports the revisions the named
//! configuration actually holds — every one of them, in revision order, each
//! rendered as the `ConfigurationRevision` the revision reader reports for it —
//! bounded by `MaxResults` (1..100 per the model) and resumed by a `NextToken`
//! this same listing minted, with no token on the page that closes the list.
//!
//! The rows differ along what the rule generalises over: how many revisions the
//! configuration holds, whether each update named a description, the page size
//! asked for, and the signing region the endpoint serves. Where the rule is an
//! invariant — paging covering every revision exactly once, a read leaving the
//! history as it found it, a page size or a token outside what the operation
//! accepts being refused — the input space is walked exhaustively, which is
//! stronger than sampling it.

use serde_json::Value;

use super::kafka_harness::{pct, Emulator, Reply, REGION};

/// The page size `ListConfigurationRevisions` may be asked for, from the `range`
/// on the model's `MaxResults` shape (1..100,
/// `smithy_query(query="operation",operation="ListConfigurationRevisions")`).
/// Both bounds are exercised, as is the first value past each of them.
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 100;

/// The description the create below stores with revision 1.
const CREATED_DESCRIPTION: &str = "created";

/// `ListConfigurationRevisions`, which the model binds to
/// `GET /v1/configurations/{Arn}/revisions`.
async fn list(emulator: &Emulator, arn: &str, query: &str) -> Reply {
    list_in(emulator, REGION, arn, query).await
}

/// The same listing, as an endpoint for `region` serves it — only the region the
/// configuration lives in holds it.
async fn list_in(emulator: &Emulator, region: &str, arn: &str, query: &str) -> Reply {
    emulator
        .call_in(
            region,
            "GET",
            &format!("/v1/configurations/{}/revisions{query}", pct(arn)),
            None,
        )
        .await
}

/// The `revisions` array of a successful listing.
fn revisions_of(reply: &Reply, case: &str) -> Vec<Value> {
    assert_eq!(
        reply.status, 200,
        "case {case}: expected ListConfigurationRevisions to succeed, got {} {}",
        reply.status, reply.body
    );
    reply.body["revisions"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: a listing must carry revisions, got {}", reply.body))
}

/// One update applied after the create, as a row of a table: the description it
/// names (`None` where the request omits the member) and the body it stores.
#[derive(Debug, Clone)]
struct UpdateRow {
    case: &'static str,
    description: Option<&'static str>,
    server_properties: Vec<u8>,
}

impl UpdateRow {
    /// What `ConfigurationRevision$Description` must report: the description as
    /// given, and NOTHING at all when the request named none or named an empty
    /// one, since the member is optional and an empty description is no
    /// description.
    fn expected_description(&self) -> Option<&'static str> {
        self.description.filter(|description| !description.is_empty())
    }
}

/// The updates every suite below applies, in order. The rows differ along what
/// the rule generalises over: whether a description was given (and whether it is
/// empty or carries non-ASCII text) and the size and byte content of the
/// `server.properties` body.
fn update_rows() -> Vec<UpdateRow> {
    vec![
        UpdateRow {
            case: "ascii-properties",
            description: Some("second revision"),
            server_properties: b"auto.create.topics.enable=true\n".to_vec(),
        },
        UpdateRow {
            case: "no-description",
            description: None,
            server_properties: b"num.partitions=4\n".to_vec(),
        },
        UpdateRow {
            case: "empty-description",
            description: Some(""),
            server_properties: b"log.retention.hours=168\n".to_vec(),
        },
        UpdateRow {
            case: "multi-byte-utf8",
            description: Some("révision — настройка 構成"),
            server_properties: "log.message.format.version=2.8-ß\n".as_bytes().to_vec(),
        },
        UpdateRow {
            case: "not-utf8-at-all",
            description: Some("raw bytes"),
            // The body is a blob, so a revision may hold bytes that are not
            // UTF-8 anywhere; the listing must still report its revision.
            server_properties: [
                b"compression.type=".as_slice(),
                &[0x00, 0xFF, 0xFE, 0x80],
            ]
            .concat(),
        },
        UpdateRow {
            case: "empty-body",
            description: Some("no overrides"),
            server_properties: Vec::new(),
        },
    ]
}

/// Create a configuration in `region`, apply the first `updates` rows of the
/// table to it, and hand back its ARN plus the description expected of every
/// stored revision, in revision order.
async fn configuration_with_updates_in(
    emulator: &Emulator,
    region: &str,
    name: &str,
    updates: usize,
) -> (String, Vec<Option<&'static str>>) {
    let reply = emulator
        .call_in(
            region,
            "POST",
            "/v1/configurations",
            Some(serde_json::json!({
                "name": name,
                "description": CREATED_DESCRIPTION,
                "serverProperties": ::aws_smithy_types::base64::encode(b"num.partitions=1\n"),
            })),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case name={name:?}: expected CreateConfiguration in {region} to succeed, got {} {}",
        reply.status, reply.body
    );
    let arn = reply.body["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case name={name:?}: CreateConfiguration returned no arn"))
        .to_string();

    let mut descriptions = vec![Some(CREATED_DESCRIPTION)];
    for row in update_rows().iter().take(updates) {
        let updated = emulator
            .update_configuration_in(region, &arn, row.description, &row.server_properties)
            .await;
        assert_eq!(
            updated.status, 200,
            "case {}: expected UpdateConfiguration to succeed, got {} {}",
            row.case, updated.status, updated.body
        );
        descriptions.push(row.expected_description());
    }
    (arn, descriptions)
}

/// The same, under the suite's own signing region.
async fn configuration_with_updates(
    emulator: &Emulator,
    name: &str,
    updates: usize,
) -> (String, Vec<Option<&'static str>>) {
    configuration_with_updates_in(emulator, REGION, name, updates).await
}

/// RULE: the listing reports every revision the configuration holds, in revision
/// order, each carrying the revision number, the description that revision was
/// stored with and a creation time — the same record
/// `DescribeConfigurationRevision` reports for it.
#[tokio::test]
async fn the_listing_reports_every_stored_revision_in_revision_order() {
    // Walked over the whole revision count the table can produce, so the rule is
    // asserted on a configuration holding only its create's revision as well as
    // on one holding every update's.
    for updates in 0..=update_rows().len() {
        let emulator = Emulator::new();
        let (arn, descriptions) =
            configuration_with_updates(&emulator, &format!("listed-{updates}"), updates).await;
        let case = format!("updates={updates}");

        let reply = list(&emulator, &arn, "").await;
        let listed = revisions_of(&reply, &case);
        assert_eq!(
            listed.len(),
            descriptions.len(),
            "case {case}: expected {} revisions, got {}",
            descriptions.len(),
            reply.body
        );
        assert!(
            reply.body.get("nextToken").is_none(),
            "case {case}: a listing that holds every revision must close the list, got {}",
            reply.body
        );

        for (index, (entry, description)) in listed.iter().zip(&descriptions).enumerate() {
            let expected_revision = index as i64 + 1;
            assert_eq!(
                entry["revision"].as_i64(),
                Some(expected_revision),
                "case {case}: expected revision {expected_revision} at position {index}, got {entry}"
            );
            assert_eq!(
                entry["description"].as_str(),
                *description,
                "case {case} revision {expected_revision}: expected description \
                 {description:?}, got {entry}"
            );
            let creation_time = entry["creationTime"].clone();
            assert!(
                creation_time.as_f64().is_some_and(|epoch| epoch > 0.0)
                    || creation_time.as_str().is_some_and(|text| text.contains('T')),
                "case {case} revision {expected_revision}: expected a creationTime timestamp, \
                 got {entry}"
            );

            // The listing and the revision reader are two views of one stored
            // record, so they agree on every member they share.
            let described = emulator
                .describe_configuration_revision(&arn, expected_revision)
                .await;
            assert_eq!(
                described.status, 200,
                "case {case} revision {expected_revision}: expected the stored revision, got {} {}",
                described.status, described.body
            );
            assert_eq!(
                described.at("/description").as_str(),
                entry["description"].as_str(),
                "case {case} revision {expected_revision}: the listing and the reader disagree \
                 about the description: {entry} vs {}",
                described.body
            );
            assert_eq!(
                described.at("/creationTime"),
                &creation_time,
                "case {case} revision {expected_revision}: the listing and the reader disagree \
                 about the creation time: {entry} vs {}",
                described.body
            );
        }
    }
}

/// RULE: paging enumerates every stored revision exactly once for any page size
/// the model allows, each page holds at most the size asked for, and only a page
/// that left something behind carries a `NextToken`.
#[tokio::test]
async fn paging_covers_every_revision_exactly_once() {
    for updates in 0..=update_rows().len() {
        let emulator = Emulator::new();
        let (arn, descriptions) =
            configuration_with_updates(&emulator, &format!("paged-{updates}"), updates).await;
        let stored: Vec<i64> = (1..=descriptions.len() as i64).collect();

        for page_size in 1..=(update_rows().len() + 2) {
            let case = format!("updates={updates} maxResults={page_size}");
            let mut seen: Vec<i64> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0usize;
            loop {
                pages += 1;
                assert!(
                    pages <= stored.len() + 1,
                    "case {case}: pagination did not end after {pages} pages"
                );
                let query = match &token {
                    Some(value) => format!("?maxResults={page_size}&nextToken={}", pct(value)),
                    None => format!("?maxResults={page_size}"),
                };
                let reply = list(&emulator, &arn, &query).await;
                let page = revisions_of(&reply, &case);
                assert!(
                    page.len() <= page_size,
                    "case {case}: page carried {} entries",
                    page.len()
                );
                for entry in &page {
                    seen.push(
                        entry["revision"]
                            .as_i64()
                            .unwrap_or_else(|| panic!("case {case}: a revision carries a number, got {entry}")),
                    );
                }
                match reply.body.get("nextToken").and_then(Value::as_str) {
                    Some(next) => {
                        assert!(
                            !page.is_empty(),
                            "case {case}: an empty page must close the list, got {}",
                            reply.body
                        );
                        token = Some(next.to_string());
                    }
                    None => break,
                }
            }
            assert_eq!(
                seen, stored,
                "case {case}: the pages did not cover every revision exactly once, got {seen:?}"
            );
        }
    }
}

/// RULE: a `MaxResults` inside the modelled 1..100 range is served; one outside
/// it is refused with HTTP 400 and the operation's modelled client-input error,
/// never clamped to the nearest legal size.
#[tokio::test]
async fn a_max_results_outside_the_modelled_range_is_refused() {
    let emulator = Emulator::new();
    let (arn, descriptions) = configuration_with_updates(&emulator, "bounded-page", 3).await;

    // Both published bounds, and the first value past each of them.
    for accepted in [MAX_RESULTS_MIN, MAX_RESULTS_MIN + 1, MAX_RESULTS_MAX] {
        let case = format!("maxResults={accepted}");
        let reply = list(&emulator, &arn, &format!("?maxResults={accepted}")).await;
        let page = revisions_of(&reply, &case);
        assert!(
            page.len() <= usize::try_from(accepted).unwrap_or(usize::MAX),
            "case {case}: page carried {} entries",
            page.len()
        );
    }

    for refused in [
        MAX_RESULTS_MIN - 1,
        -1,
        MAX_RESULTS_MAX + 1,
        1_000,
        i64::from(i32::MAX),
    ] {
        let reply = list(&emulator, &arn, &format!("?maxResults={refused}")).await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (400, "BadRequestException"),
            "case maxResults={refused}: a value outside 1..100 must be refused, got {} {}",
            reply.status,
            reply.body
        );
    }

    // None of those refusals disturbed the history.
    let reply = list(&emulator, &arn, "").await;
    assert_eq!(
        revisions_of(&reply, "after-refusals").len(),
        descriptions.len(),
        "case after-refusals: a refused page size must leave the history alone, got {}",
        reply.body
    );
}

/// RULE: a `NextToken` this listing did not mint for this configuration is
/// refused with HTTP 400 and the modelled `BadRequestException` — including one
/// minted by the same listing for another configuration.
#[tokio::test]
async fn a_next_token_this_listing_did_not_mint_is_refused() {
    let emulator = Emulator::new();
    let (arn, _) = configuration_with_updates(&emulator, "tokened", 3).await;
    let (other, _) = configuration_with_updates(&emulator, "tokened-other", 3).await;

    // A real token, minted by this listing for ANOTHER configuration: a token
    // carries the list it came from, so it cannot resume this one.
    let foreign_page = list(&emulator, &other, "?maxResults=1").await;
    let foreign_token = foreign_page
        .body
        .get("nextToken")
        .and_then(Value::as_str)
        .expect("a first page of a longer history carries a nextToken")
        .to_string();

    // Rows spanning the classes: not base64 at all, valid base64 of unrelated
    // bytes, the empty token, base64 of another list's marked plaintext, this
    // listing's own plaintext with a revision that is not a number, and a token
    // minted for another configuration.
    let other_list = ::aws_smithy_types::base64::encode("kafka:configurations:arn:aws:kafka:x");
    let not_a_number = ::aws_smithy_types::base64::encode(format!(
        "kafka:configuration-revisions:{arn}#not-a-number"
    ));
    for token in [
        "!!!not-base64!!!",
        "aGVsbG8gd29ybGQ=",
        "%20",
        other_list.as_str(),
        not_a_number.as_str(),
        foreign_token.as_str(),
    ] {
        let reply = list(&emulator, &arn, &format!("?nextToken={}", pct(token))).await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (400, "BadRequestException"),
            "case token={token:?}: expected 400 BadRequestException, got {} {}",
            reply.status,
            reply.body
        );
    }
}

/// RULE: the configuration is named by ARN, so an `Arn` that resolves to no
/// configuration of this endpoint's region is refused HTTP 400
/// `BadRequestException` — whether it is not an MSK configuration ARN at all or
/// a well-formed one nothing holds — never an empty page.
#[tokio::test]
async fn an_unknown_or_malformed_arn_is_refused() {
    let emulator = Emulator::new();
    let (arn, descriptions) = configuration_with_updates(&emulator, "present", 2).await;

    // Rows spanning the refusal classes: not an ARN at all, an ARN of another
    // service, an MSK ARN of another resource type, a configuration ARN missing
    // its id segment, and a well-formed configuration ARN nothing holds.
    let candidates: Vec<(&str, u16, &str)> = vec![
        ("not-an-arn", 400, "BadRequestException"),
        (
            "arn:aws:s3:::configuration/present/abc-1",
            400,
            "BadRequestException",
        ),
        (
            "arn:aws:kafka:us-west-2:000000000000:cluster/present/abc-1",
            400,
            "BadRequestException",
        ),
        (
            "arn:aws:kafka:us-west-2:000000000000:configuration/present",
            400,
            "BadRequestException",
        ),
        (
            "arn:aws:kafka:us-west-2:000000000000:configuration/absent/0e7b1f9a-1",
            400,
            "BadRequestException",
        ),
    ];
    for (candidate, status, code) in &candidates {
        let reply = list(&emulator, candidate, "").await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (*status, *code),
            "case arn={candidate:?}: expected {status} {code}, got {} {}",
            reply.status,
            reply.body
        );
    }

    // A configuration lives in one region, so another region's endpoint holds
    // neither the configuration nor its revisions.
    let elsewhere = list_in(&emulator, "eu-central-1", &arn, "").await;
    assert_eq!(
        (elsewhere.status, elsewhere.error_code()),
        (400, "BadRequestException"),
        "case region=eu-central-1: another region's endpoint must not list its revisions, \
         got {} {}",
        elsewhere.status,
        elsewhere.body
    );

    // A configuration created in that other region has its own history there,
    // and the listing serves it from that endpoint — so the rule is about which
    // region holds the record, not about one favoured region.
    let (there, there_descriptions) =
        configuration_with_updates_in(&emulator, "eu-central-1", "present", 1).await;
    let served = list_in(&emulator, "eu-central-1", &there, "").await;
    assert_eq!(
        revisions_of(&served, "region=eu-central-1").len(),
        there_descriptions.len(),
        "case region=eu-central-1: expected that region's own history, got {}",
        served.body
    );
    let here = list(&emulator, &arn, "").await;
    assert_eq!(
        revisions_of(&here, "region=us-west-2").len(),
        descriptions.len(),
        "case region=us-west-2: each region reports its own history, got {}",
        here.body
    );
}

/// RULE: listing the revisions reports them without changing anything, so
/// repeated listings answer identically and the configuration's latest revision
/// does not move.
#[tokio::test]
async fn listing_the_revisions_leaves_the_history_as_it_found_it() {
    let emulator = Emulator::new();
    let updates = update_rows().len();
    let (arn, descriptions) = configuration_with_updates(&emulator, "stable-listing", updates).await;
    let latest = descriptions.len() as i64;

    // Page sizes that split the history and one that holds all of it: a listing
    // repeated at any of them answers the same body.
    for page_size in [1usize, 2, update_rows().len() + 1] {
        let query = format!("?maxResults={page_size}");
        let first = list(&emulator, &arn, &query).await;
        let again = list(&emulator, &arn, &query).await;
        assert_eq!(
            first.body, again.body,
            "case maxResults={page_size}: a listing must not change what a later one sees: \
             {} vs {}",
            first.body, again.body
        );
    }

    let described = emulator.describe_configuration(&arn).await;
    assert_eq!(
        described.at("/latestRevision/revision").as_i64(),
        Some(latest),
        "case reads-only: expected revision {latest} to remain the latest, got {}",
        described.body
    );

    // A revision appended after a listing shows up in the next one, which is how
    // the listing stays a report of the live history rather than a snapshot.
    let appended = emulator
        .update_configuration(&arn, Some("after the listing"), b"num.partitions=9\n")
        .await;
    assert_eq!(
        appended.status, 200,
        "case appended: expected UpdateConfiguration to succeed, got {} {}",
        appended.status, appended.body
    );
    let reply = list(&emulator, &arn, "").await;
    let listed = revisions_of(&reply, "appended");
    assert_eq!(
        listed.len(),
        descriptions.len() + 1,
        "case appended: expected the appended revision to be listed, got {}",
        reply.body
    );
    assert_eq!(
        listed.last().and_then(|entry| entry["revision"].as_i64()),
        Some(latest + 1),
        "case appended: expected revision {} last, got {}",
        latest + 1,
        reply.body
    );
    assert_eq!(
        listed
            .last()
            .and_then(|entry| entry["description"].as_str()),
        Some("after the listing"),
        "case appended: expected the appended revision's own description, got {}",
        reply.body
    );
}
