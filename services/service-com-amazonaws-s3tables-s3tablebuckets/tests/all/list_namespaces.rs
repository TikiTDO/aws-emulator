// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `ListNamespaces`.
//!
//! The rule under test: the listing reports one summary per namespace the
//! addressed table bucket holds — the same record `GetNamespace` reports, built
//! from the create — in namespace-name order, narrowed to the namespaces whose
//! name begins with the `prefix` the request names, at most `maxNamespaces` of
//! them, with a `continuationToken` back only when a namespace the caller has
//! not seen remains. The listing writes nothing, it reports no namespace of
//! another bucket, and a bucket the store does not hold is refused with the
//! modeled not-found.
//!
//! The table spans what the rule generalises over: the number of namespaces, the
//! page size (including one past the number held), the prefix (one that matches
//! every, some and no namespace), a resumed page, and the region.

use serde_json::{json, Value};

use super::harness::Emulator;

/// The fixed sentence this service states a table bucket the store does not hold
/// with.
const NO_BUCKET: &str = "The specified bucket does not exist.";

/// One row of the listing table: the case, the query members the listing is
/// addressed with, the namespaces it must report and whether a continuation
/// token must come back.
type ListCase = (
    &'static str,
    Vec<(&'static str, String)>,
    Vec<&'static str>,
    bool,
);

/// Create a table bucket holding `namespaces`, and answer its ARN.
async fn create_bucket(
    emulator: &Emulator,
    region: &str,
    bucket: &str,
    namespaces: &[&str],
) -> String {
    let created = emulator
        .create_table_bucket(region, &json!({ "name": bucket }))
        .await;
    assert_eq!(
        created.status, 200,
        "creating the bucket {bucket:?} must succeed, got {status} {body}",
        status = created.status,
        body = created.body
    );
    let bucket_arn = created.text("arn");
    for namespace in namespaces {
        let reply = emulator
            .create_namespace(region, &bucket_arn, &json!({ "namespace": [namespace] }))
            .await;
        assert_eq!(
            reply.status, 200,
            "creating the namespace {namespace:?} must succeed, got {status} {body}",
            status = reply.status,
            body = reply.body
        );
    }
    bucket_arn
}

/// The single-element namespace name of every row of a listing.
fn listed(body: &Value, case: &str) -> Vec<String> {
    body.get("namespaces")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: the listing carries no namespaces: {body}"))
        .iter()
        .map(|summary| {
            summary
                .get("namespace")
                .and_then(Value::as_array)
                .and_then(|parts| parts.first())
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: a row names no namespace: {summary}"))
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn the_listing_reports_the_namespaces_the_bucket_holds() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    // Created out of order, so the listing's order is its own and not the order
    // the creates arrived in.
    let held = ["sales", "analytics", "ops_2026", "analytics_raw"];
    let bucket_arn = create_bucket(&emulator, region, "list-namespaces", &held).await;

    // A second bucket, so the listing is graded on reporting the namespaces of
    // the bucket it addressed and no other.
    let other_arn = create_bucket(&emulator, region, "list-namespaces-other", &["elsewhere"]).await;

    // (the case, the query the listing is addressed with, the namespaces it must
    // report, whether a continuation token must come back)
    let rows: Vec<ListCase> = vec![
        (
            "every namespace the bucket holds, in name order",
            vec![],
            vec!["analytics", "analytics_raw", "ops_2026", "sales"],
            false,
        ),
        (
            "one page of two",
            vec![("maxNamespaces", "2".to_string())],
            vec!["analytics", "analytics_raw"],
            true,
        ),
        (
            "a page as large as the bucket holds",
            vec![("maxNamespaces", "4".to_string())],
            vec!["analytics", "analytics_raw", "ops_2026", "sales"],
            false,
        ),
        (
            "a page larger than the bucket holds",
            vec![("maxNamespaces", "5".to_string())],
            vec!["analytics", "analytics_raw", "ops_2026", "sales"],
            false,
        ),
        (
            "a prefix every namespace of the page shares",
            vec![("prefix", "analytics".to_string())],
            vec!["analytics", "analytics_raw"],
            false,
        ),
        (
            "a prefix one namespace has",
            vec![("prefix", "ops".to_string())],
            vec!["ops_2026"],
            false,
        ),
        (
            "a prefix no namespace has",
            vec![("prefix", "zzz".to_string())],
            vec![],
            false,
        ),
    ];

    for (case, query, expected, expect_token) in rows {
        let reply = emulator.list_namespaces(region, &bucket_arn, &query).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: the listing must succeed, got {status} {body}",
            status = reply.status,
            body = reply.body
        );
        let actual = listed(&reply.body, case);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
        assert_eq!(
            reply.optional_text("continuationToken").is_some(),
            expect_token,
            "case {case:?}: expected a continuation token: {expect_token}, got {actual:?}",
            actual = reply.optional_text("continuationToken")
        );
    }

    // The page a token resumes continues where the first one stopped, and the
    // two pages together are the whole listing.
    let first = emulator
        .list_namespaces(region, &bucket_arn, &[("maxNamespaces", "3".to_string())])
        .await;
    let token = first
        .optional_text("continuationToken")
        .unwrap_or_else(|| panic!("the first page must carry a token: {}", first.body));
    let second = emulator
        .list_namespaces(region, &bucket_arn, &[("continuationToken", token)])
        .await;
    let mut both = listed(&first.body, "the first page");
    both.extend(listed(&second.body, "the resumed page"));
    assert_eq!(
        both,
        vec!["analytics", "analytics_raw", "ops_2026", "sales"],
        "the resumed page must continue where the first stopped, got {both:?}"
    );
    assert_eq!(
        second.optional_text("continuationToken"),
        None,
        "the last page must carry no token, got {actual:?}",
        actual = second.optional_text("continuationToken")
    );

    // Each row carries the record `GetNamespace` reports for the same namespace.
    let whole = emulator.list_namespaces(region, &bucket_arn, &[]).await;
    let rows = whole
        .body
        .get("namespaces")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for row in rows {
        let name = row
            .get("namespace")
            .and_then(Value::as_array)
            .and_then(|parts| parts.first())
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let read = emulator.get_namespace(region, &bucket_arn, &name).await;
        assert_eq!(
            read.status, 200,
            "the namespace {name:?} must read back, got {status} {body}",
            status = read.status,
            body = read.body
        );
        for member in [
            "namespace",
            "createdAt",
            "createdBy",
            "ownerAccountId",
            "namespaceId",
            "tableBucketId",
        ] {
            assert_eq!(
                row.get(member),
                read.body.get(member),
                "the listing row for {name:?} must carry the {member} GetNamespace reports, got \
                 {actual:?} against {expected:?}",
                actual = row.get(member),
                expected = read.body.get(member)
            );
        }
    }

    // The other bucket still lists its own namespace alone, so the listing is
    // scoped to the bucket it addressed.
    let elsewhere = emulator.list_namespaces(region, &other_arn, &[]).await;
    assert_eq!(
        listed(&elsewhere.body, "the other bucket"),
        vec!["elsewhere"],
        "the other bucket must list its own namespace alone, got {body}",
        body = elsewhere.body
    );
}

#[tokio::test]
async fn a_listing_of_a_bucket_the_store_does_not_hold_is_refused() {
    let emulator = Emulator::new();
    let region = "eu-west-1";
    let bucket_arn = create_bucket(&emulator, region, "list-namespaces-absent", &["present"]).await;

    for (case, arn) in [
        (
            "a table bucket that was never created",
            "arn:aws:s3tables:eu-west-1:000000000000:bucket/list-namespaces-missing".to_string(),
        ),
        (
            "the resident bucket's name in another account",
            bucket_arn.replace("000000000000", "123456789012"),
        ),
    ] {
        let reply = emulator.list_namespaces(region, &arn, &[]).await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected 404, got {status} {body}",
            status = reply.status,
            body = reply.body
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some("NotFoundException"),
            "case {case:?}: expected NotFoundException, got {actual:?}",
            actual = reply.error_code
        );
        assert_eq!(
            reply.message(),
            NO_BUCKET,
            "case {case:?}: expected {NO_BUCKET:?}, got {actual:?}",
            actual = reply.message()
        );
    }

    // A token this service never minted names no place in the listing.
    let refused = emulator
        .list_namespaces(
            region,
            &bucket_arn,
            &[("continuationToken", "not-a-token".to_string())],
        )
        .await;
    assert_eq!(
        refused.status, 400,
        "a token this service never minted must be refused, got {status} {body}",
        status = refused.status,
        body = refused.body
    );
    assert_eq!(
        refused.error_code.as_deref(),
        Some("BadRequestException"),
        "expected BadRequestException, got {actual:?}",
        actual = refused.error_code
    );

    // And the resident bucket still lists, so the refusals above are about what
    // each named.
    let reply = emulator.list_namespaces(region, &bucket_arn, &[]).await;
    assert_eq!(
        listed(&reply.body, "the resident bucket"),
        vec!["present"],
        "the resident bucket must still list its namespace, got {body}",
        body = reply.body
    );
}
