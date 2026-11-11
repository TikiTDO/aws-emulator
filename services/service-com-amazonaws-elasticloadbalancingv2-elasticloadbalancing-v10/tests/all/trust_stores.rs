// SPDX-License-Identifier: CC0-1.0

//! `DescribeTrustStores` over the wire: what the store holds is what the describe
//! reports.
//!
//! Every request goes through the router exactly as a client's would — awsQuery form
//! body in, `<DescribeTrustStoresResponse>` XML out — so these tests exercise the wire
//! protocol as well as the handler.
//!
//! The records are SEEDED through the shared engine, which is how a suite puts a record
//! at an exact INSTANT: the harness that does it lives beside the other shared helpers in
//! `provisioning_loop`, so reads and writes go through
//! `service_..._v10::state::service_state`'s store, exactly as a handler's do, and a
//! seeded record is the same record the handler serves. Seeding is what lets the
//! clock-owed `CREATING -> ACTIVE` move be asserted exactly — by placing the instant it
//! is owed at either side of now — instead of sleeping for it. The create that mints such
//! a record, bundle read and all, is driven end to end in `trust_store_lifecycle`.
//!
//! The rule under test is not one trust store: the describe reports every trust store
//! of the account, narrowed to the ARNs or names asked for, paginated in one stable
//! order, with every member read off the stored record; an identifier no stored trust
//! store carries is the modelled `TrustStoreNotFound`. So each suite below spans a set
//! of records, identifier lists and page sizes rather than asserting one answer.

use crate::provisioning_loop::{
    action, call, emulator, first_text, members, p, seed_trust_store, Reply, TrustStoreSeed as Seed,
    REGION,
};
use service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state::{
    TRUST_STORE_ACTIVE, TRUST_STORE_CREATING,
};
use smithy_server_runtime::MultiServiceRouter;

/// `DescribeTrustStores` with the extra form fields `fields`.
async fn describe(router: &MultiServiceRouter, fields: &[(String, String)]) -> Reply {
    let mut form = action("DescribeTrustStores");
    form.extend_from_slice(fields);
    call(router, &form).await
}

/// The one trust store the reply reports, or a panic naming the case.
fn only_trust_store(reply: &Reply, case: &str) -> String {
    let found = members(&reply.body, "TrustStores");
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly 1 trust store, got {} in {}",
        found.len(),
        reply.body
    );
    found[0].clone()
}

/// The ARNs the reply reports, in the order it reported them.
fn reported_arns(reply: &Reply) -> Vec<String> {
    members(&reply.body, "TrustStores")
        .iter()
        .filter_map(|store| first_text(store, "TrustStoreArn"))
        .collect()
}

// ---------------------------------------------------------------------------
// Every member is read off the stored record, by ARN and by name.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_stored_trust_store_is_reported_by_arn_and_by_name() {
    let (router, engine) = emulator();

    // The rule generalizes over what the record HOLDS, so the rows differ in the
    // counts they carry and in whether the clock has settled them. One active store
    // with a single CA certificate and no revocations is the shape of the recorded
    // call; the rest are the same rule.
    let cases = [
        (
            "one ca certificate, settled",
            Seed {
                name: "ts-one",
                certificates: 1,
                revoked: 0,
                settled: true,
                tags: &[],
            },
            TRUST_STORE_ACTIVE,
        ),
        (
            "two ca certificates, still creating",
            Seed {
                name: "ts-two",
                certificates: 2,
                revoked: 0,
                settled: false,
                tags: &[],
            },
            TRUST_STORE_CREATING,
        ),
        (
            "three ca certificates and revocations",
            Seed {
                name: "ts-three",
                certificates: 3,
                revoked: 7,
                settled: true,
                tags: &[],
            },
            TRUST_STORE_ACTIVE,
        ),
        (
            "many of both",
            Seed {
                name: "ts-many",
                certificates: 42,
                revoked: 123_456,
                settled: true,
                tags: &[],
            },
            TRUST_STORE_ACTIVE,
        ),
    ];

    for (index, (case, seed, expected_status)) in cases.iter().enumerate() {
        let arn = seed_trust_store(&engine, index, seed);

        let by_arn = describe(&router, &[p("TrustStoreArns.member.1", &arn)]).await;
        assert_eq!(
            by_arn.status, 200,
            "case {case:?}: expected 200 describing by ARN, got {} body {}",
            by_arn.status, by_arn.body
        );
        let store = only_trust_store(&by_arn, case);
        assert_eq!(
            first_text(&store, "Name").as_deref(),
            Some(seed.name),
            "case {case:?}: expected the stored name, got {:?}",
            first_text(&store, "Name")
        );
        assert_eq!(
            first_text(&store, "NumberOfCaCertificates"),
            Some(seed.certificates.to_string()),
            "case {case:?}: expected the stored certificate count, got {:?}",
            first_text(&store, "NumberOfCaCertificates")
        );
        assert_eq!(
            first_text(&store, "TotalRevokedEntries"),
            Some(seed.revoked.to_string()),
            "case {case:?}: expected the stored revoked count, got {:?}",
            first_text(&store, "TotalRevokedEntries")
        );
        assert_eq!(
            first_text(&store, "Status").as_deref(),
            Some(*expected_status),
            "case {case:?}: expected {expected_status}, got {:?}",
            first_text(&store, "Status")
        );

        // By name: the same record, so the name is an identifier of the same resource.
        let by_name = describe(&router, &[p("Names.member.1", seed.name)]).await;
        let same = only_trust_store(&by_name, case);
        assert_eq!(
            first_text(&same, "TrustStoreArn").as_deref(),
            Some(arn.as_str()),
            "case {case:?}: expected the same ARN by name, got {:?}",
            first_text(&same, "TrustStoreArn")
        );
    }
}

// ---------------------------------------------------------------------------
// The whole collection, and paging over it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_account_with_no_trust_store_is_reported_as_an_empty_collection() {
    let (router, _engine) = emulator();
    let empty = describe(&router, &[]).await;
    assert_eq!(
        empty.status, 200,
        "case empty account: expected 200, got {} body {}",
        empty.status, empty.body
    );
    assert_eq!(
        reported_arns(&empty),
        Vec::<String>::new(),
        "case empty account: expected no trust store, got {}",
        empty.body
    );
    assert_eq!(
        empty.text("NextMarker"),
        None,
        "case empty account: nothing follows, got {:?}",
        empty.text("NextMarker")
    );
}

#[tokio::test]
async fn every_trust_store_is_reported_and_paged_in_one_stable_order() {
    let (router, engine) = emulator();

    let mut created: Vec<String> = Vec::new();
    for index in 0..5usize {
        let name = format!("paged-{index}");
        created.push(seed_trust_store(
            &engine,
            index,
            &Seed {
                name: &name,
                // The records differ in size so no two are the same value.
                certificates: index as i32 + 1,
                revoked: index as i64,
                settled: true,
                tags: &[],
            },
        ));
    }

    let whole = describe(&router, &[]).await;
    assert_eq!(
        whole.status, 200,
        "case whole collection: expected 200, got {} body {}",
        whole.status, whole.body
    );
    let listed = reported_arns(&whole);
    assert_eq!(
        listed, created,
        "case whole collection: expected every stored trust store in creation order, got {listed:?}"
    );
    assert_eq!(
        whole.text("NextMarker"),
        None,
        "case whole collection: nothing follows the last page, got {:?}",
        whole.text("NextMarker")
    );

    // Paging is the SAME sequence in any page size: each marker resumes where the
    // previous page ended, so the pages concatenate to the unpaged order.
    for page_size in [1usize, 2, 3, 5, 9] {
        let case = format!("page size {page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut fields = vec![p("PageSize", &page_size.to_string())];
            if let Some(marker) = marker.as_deref() {
                fields.push(p("Marker", marker));
            }
            let page = describe(&router, &fields).await;
            assert_eq!(
                page.status, 200,
                "case {case:?}: expected 200, got {} body {}",
                page.status, page.body
            );
            let arns = reported_arns(&page);
            assert!(
                arns.len() <= page_size,
                "case {case:?}: a page holds at most {page_size} trust stores, got {}",
                arns.len()
            );
            walked.extend(arns);
            pages += 1;
            assert!(
                pages <= created.len() + 1,
                "case {case:?}: paging did not terminate after {pages} pages"
            );
            match page.text("NextMarker") {
                Some(next) => marker = Some(next),
                None => break,
            }
        }
        assert_eq!(
            walked, listed,
            "case {case:?}: expected the unpaged order, got {walked:?}"
        );
    }

    // A marker that is not a position in this collection is refused rather than
    // silently treated as the first page.
    for (case, marker) in [
        ("marker that is not a number", "not-a-marker"),
        ("marker past the end", "500"),
    ] {
        let refused = describe(&router, &[p("Marker", marker)]).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {} {}",
            refused.status,
            refused.body
        );
    }
}

// ---------------------------------------------------------------------------
// An identifier no stored trust store carries.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_identifier_no_trust_store_carries_is_refused() {
    let (router, engine) = emulator();
    let known = seed_trust_store(
        &engine,
        0,
        &Seed {
            name: "known",
            certificates: 2,
            revoked: 0,
            settled: true,
            tags: &[],
        },
    );

    let unknown_arn = format!(
        "arn:aws:elasticloadbalancing:{REGION}:{}:truststore/absent/0123456789abcdef",
        emulator_server::emulator_account_id()
    );
    let cases: Vec<(&str, Vec<(String, String)>)> = vec![
        (
            "an ARN no trust store carries",
            vec![p("TrustStoreArns.member.1", &unknown_arn)],
        ),
        (
            "a name no trust store carries",
            vec![p("Names.member.1", "absent")],
        ),
        (
            "a known ARN beside an unknown one",
            vec![
                p("TrustStoreArns.member.1", &known),
                p("TrustStoreArns.member.2", &unknown_arn),
            ],
        ),
        (
            "a known name beside an unknown one",
            vec![p("Names.member.1", "known"), p("Names.member.2", "absent")],
        ),
        (
            "an ARN that is not an ARN at all",
            vec![p("TrustStoreArns.member.1", "not-an-arn")],
        ),
    ];

    for (case, fields) in cases {
        let refused = describe(&router, &fields).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, "TrustStoreNotFound"),
            "case {case:?}: expected 400 TrustStoreNotFound, got {} {}",
            refused.status,
            refused.body
        );
    }

    // The known one is still reported, so a refusal changed nothing.
    let still = describe(&router, &[p("Names.member.1", "known")]).await;
    let store = only_trust_store(&still, "the known one survives");
    assert_eq!(
        first_text(&store, "TrustStoreArn").as_deref(),
        Some(known.as_str()),
        "case {:?}: expected the known ARN, got {:?}",
        "the known one survives",
        first_text(&store, "TrustStoreArn")
    );
}

// ---------------------------------------------------------------------------
// The move the clock owes a trust store that is still creating.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_clock_moves_a_trust_store_to_active_when_the_span_has_passed() {
    let (router, engine) = emulator();
    // Two records that differ only in WHEN their move to `ACTIVE` is owed: one past,
    // one ahead. No sleeping: the instant is what the move depends on.
    let owed = seed_trust_store(
        &engine,
        0,
        &Seed {
            name: "span-passed",
            certificates: 1,
            revoked: 0,
            settled: true,
            tags: &[],
        },
    );
    let pending = seed_trust_store(
        &engine,
        1,
        &Seed {
            name: "span-ahead",
            certificates: 1,
            revoked: 0,
            settled: false,
            tags: &[],
        },
    );

    for (case, arn, expected) in [
        ("the span has passed", &owed, TRUST_STORE_ACTIVE),
        ("the span is still ahead", &pending, TRUST_STORE_CREATING),
    ] {
        // Twice: the move is materialized once and then stays, and a read never moves
        // a trust store whose span has not passed.
        for read in ["first read", "second read"] {
            let reply = describe(&router, &[p("TrustStoreArns.member.1", arn)]).await;
            let store = only_trust_store(&reply, case);
            assert_eq!(
                first_text(&store, "Status").as_deref(),
                Some(expected),
                "case {case:?} ({read}): expected {expected}, got {:?}",
                first_text(&store, "Status")
            );
        }
    }
}
