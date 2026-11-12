// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListTransformers`, driven over the wire (awsJson1_0,
//! `x-amz-target: B2BI.<Operation>`) through the same router the emulator boots, so every
//! assertion is about what a real SDK client observes.
//!
//! Each test states a rule and asserts it over a TABLE of inputs rather than one recorded
//! request: the rows differ along whatever the rule generalises over (configuration style,
//! which optional members are present, status, page size, token, store size).
//!
//! The wire harness and the transformer fixtures are reused from the `onboarding_loop` and
//! `transformer_lifecycle` suites rather than copied.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::onboarding_loop::{
    b2bi_router, call, call_signed_for, error_code, given_transformer, text, SIGNING_REGION,
};
use crate::transformer_lifecycle::{current_members, legacy_members};
use crate::wire_timestamps_and_discriminators::{as_wire_reports, date_time_seconds};

/// Every member `TransformerSummary` declares.
///
/// Source: `smithy_query(query="shape",shape="TransformerSummary")` — the four required
/// members (`transformerId`, `name`, `status`, `createdAt`), `modifiedAt`, and the eight
/// configuration members.
const SUMMARY_MEMBERS: &[&str] = &[
    "transformerId",
    "name",
    "status",
    "createdAt",
    "modifiedAt",
    "ediType",
    "fileFormat",
    "mappingTemplate",
    "sampleDocument",
    "inputConversion",
    "mapping",
    "outputConversion",
    "sampleDocuments",
];

/// Walk every page of `ListTransformers` and return the summaries in the order they were
/// reported. `max_results` is sent only when `Some`.
async fn walk(router: &MultiServiceRouter, max_results: Option<i32>) -> Vec<Value> {
    let mut walked: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0usize;

    loop {
        let mut request = json!({});
        if let Some(size) = max_results {
            request["maxResults"] = json!(size);
        }
        if let Some(token) = &next_token {
            request["nextToken"] = json!(token);
        }

        let (status, body) = call(router, "ListTransformers", request).await;
        assert_eq!(
            status, 200,
            "case maxResults={max_results:?}: ListTransformers must succeed, got {status} {body}"
        );
        let page = body["transformers"]
            .as_array()
            .unwrap_or_else(|| panic!("transformers must be a list, got {body}"));
        if let Some(size) = max_results {
            assert!(
                page.len() <= size as usize,
                "case maxResults={max_results:?}: a page must not exceed maxResults, got {} \
                 items in {body}",
                page.len()
            );
        }
        walked.extend(page.iter().cloned());

        pages += 1;
        assert!(
            pages <= walked.len() + 2,
            "case maxResults={max_results:?}: paging did not terminate after {pages} pages"
        );

        match body["nextToken"].as_str() {
            Some(token) => next_token = Some(token.to_string()),
            None => break,
        }
    }

    walked
}

/// The `transformerId` of each reported summary.
fn ids(summaries: &[Value]) -> Vec<String> {
    summaries
        .iter()
        .map(|summary| text(summary, "transformerId"))
        .collect()
}

/// RULE: `ListTransformers` enumerates the transformers this account's store holds, in
/// creation order, and reports each one's STORED record — every member
/// `TransformerSummary` models, and only those — so a summary cannot disagree with what
/// `GetTransformer` reports for the same transformer, and a member the create omitted is
/// absent from both.
///
/// The rows span both configuration styles (the current `inputConversion` / `mapping` /
/// `outputConversion` / `sampleDocuments` members and the deprecated `ediType` /
/// `fileFormat` / `mappingTemplate` / `sampleDocument` ones), several subsets of each, a
/// create with no configuration member at all, tags present and absent, two signing
/// regions, and both values of the modelled `TransformerStatus` (a record left `inactive`,
/// whose `modifiedAt` is still the instant it was created, and one activated, whose
/// `modifiedAt` the update moved).
#[tokio::test]
async fn listing_reports_every_stored_transformer_in_creation_order() {
    let router = b2bi_router();

    // (case, signing region, create request, whether this row is activated afterwards)
    let cases: Vec<(&str, &str, Value, bool)> = vec![
        (
            "current style, inbound X12 with a JSONATA mapping and sample documents",
            SIGNING_REGION,
            json!({
                "name": "inbound-850",
                "inputConversion": current_members()[0].1,
                "mapping": current_members()[1].1,
                "sampleDocuments": current_members()[3].1,
            }),
            false,
        ),
        (
            "current style, outputConversion only, activated",
            SIGNING_REGION,
            json!({
                "name": "outbound-850",
                "outputConversion": current_members()[2].1,
            }),
            true,
        ),
        (
            "current style, inputConversion only, another signing region",
            "eu-central-1",
            json!({
                "name": "inbound_minimal",
                "inputConversion": current_members()[0].1,
            }),
            false,
        ),
        (
            "legacy style, every deprecated member, with tags",
            SIGNING_REGION,
            json!({
                "name": "legacy-json",
                "ediType": legacy_members()[0].1,
                "fileFormat": "JSON",
                "mappingTemplate": legacy_members()[2].1,
                "sampleDocument": legacy_members()[3].1,
                "tags": [{ "Key": "team", "Value": "edi" }],
            }),
            false,
        ),
        (
            "legacy style, fileFormat XML only, activated",
            "ap-southeast-2",
            json!({ "name": "legacy-xml", "fileFormat": "XML" }),
            true,
        ),
        (
            "no configuration member at all",
            SIGNING_REGION,
            json!({ "name": "bare" }),
            false,
        ),
    ];

    let mut created_in_order: Vec<String> = Vec::new();
    for (case, region, request, activate) in &cases {
        let (status, created) =
            call_signed_for(&router, region, "CreateTransformer", request.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateTransformer must succeed, got {status} {created}"
        );
        let transformer_id = text(&created, "transformerId");
        if *activate {
            let (status, activated) = call_signed_for(
                &router,
                region,
                "UpdateTransformer",
                json!({ "transformerId": transformer_id, "status": "active" }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: activating the transformer must succeed, got {status} {activated}"
            );
        }
        created_in_order.push(transformer_id);
    }

    let listed = walk(&router, None).await;
    assert_eq!(
        ids(&listed),
        created_in_order,
        "the enumeration must report every created transformer once, in creation order, got {:?}",
        ids(&listed)
    );

    for ((case, region, request, activate), summary) in cases.iter().zip(listed.iter()) {
        let transformer_id = text(summary, "transformerId");
        let (status, fetched) = call_signed_for(
            &router,
            region,
            "GetTransformer",
            json!({ "transformerId": transformer_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetTransformer must find the listed transformer, got {status} {fetched}"
        );

        for member in SUMMARY_MEMBERS {
            assert_eq!(
                summary[*member], fetched[*member],
                "case {case:?}: the summary disagrees with GetTransformer on {member}: {:?} vs \
                 {:?}",
                summary[*member], fetched[*member]
            );
        }
        // A configuration member the create omitted is absent from the summary too, and a
        // nested structure is reported with the `__type` this service's wire puts on its
        // shape, which `as_wire_reports` adds to the value the request carried.
        for member in crate::transformer_lifecycle::CONFIGURATION_MEMBERS {
            assert_eq!(
                summary[*member],
                as_wire_reports(member, &request[*member]),
                "case {case:?}: {member} must be reported exactly as supplied (absent when \
                 omitted): got {:?}, sent {:?}",
                summary[*member], request[*member]
            );
        }
        assert_eq!(
            text(summary, "status"),
            if *activate { "active" } else { "inactive" },
            "case {case:?}: expected the stored status, got {:?}",
            summary["status"]
        );
        // Every stored transformer carries a `modifiedAt`: the instant it was created,
        // which an update (this row's activation) moves forward.
        let created_at = date_time_seconds(summary, "createdAt");
        let modified_at = date_time_seconds(summary, "modifiedAt");
        assert!(
            modified_at >= created_at,
            "case {case:?}: modifiedAt {modified_at} must be at or after createdAt \
             {created_at}"
        );
        // `TransformerSummary` models no ARN and no tags, so neither is reported.
        for absent in ["transformerArn", "tags"] {
            assert!(
                summary.get(absent).is_none(),
                "case {case:?}: TransformerSummary models no {absent}, got {:?}",
                summary[absent]
            );
        }
    }
}

/// RULE: `maxResults` is a real page size and `nextToken` resumes the enumeration, so
/// walking the pages yields every transformer exactly once, in creation order, and the
/// last page carries no token.
///
/// Rows span page sizes 1, 2, 3, 6, 7 and the modelled `MaxResults` maximum (100) over a
/// 7-element store, plus `maxResults` omitted entirely.
#[tokio::test]
async fn paging_the_transformer_list_yields_every_transformer_once_in_creation_order() {
    const TOTAL: usize = 7;

    let router = b2bi_router();
    let mut created_in_order = Vec::new();
    for index in 0..TOTAL {
        created_in_order.push(given_transformer(&router, &format!("transformer-{index}")).await);
    }

    // `None` means "maxResults omitted"; 1 and 100 are the published bounds of the
    // modelled `MaxResults` range (`range 1..100`).
    let page_sizes: &[Option<i32>] = &[
        Some(1),
        Some(2),
        Some(3),
        Some(6),
        Some(7),
        Some(100),
        None,
    ];

    for page_size in page_sizes {
        let walked = walk(&router, *page_size).await;
        assert_eq!(
            ids(&walked),
            created_in_order,
            "case maxResults={page_size:?}: paging must yield every transformer once, in \
             creation order, got {:?}",
            ids(&walked)
        );
    }
}

/// RULE: a `maxResults` outside the modelled `range 1..100` is refused with HTTP 400
/// before the handler runs — one value past each published bound — and a `nextToken` this
/// service never issued is refused with the modelled `ValidationException` (HTTP 400).
#[tokio::test]
async fn a_bad_page_request_is_refused_rather_than_answered() {
    let router = b2bi_router();
    given_transformer(&router, "refusal-fixture").await;

    for max_results in [0, 101] {
        let case = format!("maxResults={max_results}");
        let (status, body) = call(
            &router,
            "ListTransformers",
            json!({ "maxResults": max_results }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for a maxResults outside 1..100, got {status} {body}"
        );
    }

    for token in ["not-a-token", "zz", "abc"] {
        let case = format!("nextToken={token:?}");
        let (status, body) = call(&router, "ListTransformers", json!({ "nextToken": token })).await;
        assert_eq!(status, 400, "case {case:?}: expected 400, got {status} {body}");
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
    }
}

/// RULE: the enumeration is a live view of the store — an empty store reports an empty
/// list and no token, a delete removes that transformer from every later page while the
/// rest keep their creation order, and listing is a READ: repeating it reports the same
/// enumeration.
///
/// Rows delete the first, a middle and the last of the remaining transformers, so the
/// rule is asserted at each end of the order as well as inside it.
#[tokio::test]
async fn the_enumeration_follows_the_store_and_listing_changes_nothing() {
    let router = b2bi_router();

    let empty = walk(&router, None).await;
    assert!(
        empty.is_empty(),
        "an empty store must report an empty list, got {empty:?}"
    );

    let mut remaining: Vec<String> = Vec::new();
    for index in 0..5 {
        remaining.push(given_transformer(&router, &format!("live-{index}")).await);
    }

    // (case, the index into the REMAINING order this row deletes)
    let cases: Vec<(&str, usize)> = vec![
        ("the first transformer", 0),
        ("the last transformer", 3),
        ("a transformer in the middle", 1),
    ];

    for (case, index) in cases {
        let doomed = remaining.remove(index);
        let (status, deleted) = call(
            &router,
            "DeleteTransformer",
            json!({ "transformerId": doomed }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteTransformer must succeed, got {status} {deleted}"
        );

        let walked = walk(&router, Some(2)).await;
        assert_eq!(
            ids(&walked),
            remaining,
            "case {case:?}: the deleted transformer must leave the enumeration and the rest \
             keep their order, got {:?}",
            ids(&walked)
        );

        // Reading twice reports the same enumeration: the list writes nothing.
        let again = walk(&router, None).await;
        assert_eq!(
            ids(&again),
            remaining,
            "case {case:?}: a second list must report the same enumeration, got {:?}",
            ids(&again)
        );
    }
}
