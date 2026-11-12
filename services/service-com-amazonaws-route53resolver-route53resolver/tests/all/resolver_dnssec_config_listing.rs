// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListResolverDnssecConfigs`.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared ones
//! in [`super::harness`].
//!
//! What these cases pin is the collection this operation reports — the
//! configurations for DNSSEC validation whose validation is not off — and the
//! paging and filtering contract over it.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_route53resolver_route53resolver::state::{
    DNSSEC_STATUS_ENABLED, DNSSEC_STATUS_USE_LOCAL_RESOURCE_SETTING, PROVISIONING_DELAY,
    VALIDATION_DISABLE, VALIDATION_ENABLE, VALIDATION_USE_LOCAL_RESOURCE_SETTING,
};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{call, emulator, error_type, ok, string_member, Emulator, ACCOUNT};

/// Wait out the span the clock owes a configuration whose validation status is
/// still in flight, so the next call sees the settled status.
async fn settle() {
    tokio::time::sleep(PROVISIONING_DELAY + Duration::from_millis(250)).await;
}

/// The `ResolverDnssecConfigs` of one page.
fn page_items(body: &Value) -> Vec<Value> {
    body.get("ResolverDnssecConfigs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!("the response carries a ResolverDnssecConfigs array, got {body}")
        })
}

/// Walk every page of `ListResolverDnssecConfigs` for `request`, returning the
/// items in the order they were reported.
async fn walk(router: &MultiServiceRouter, request: &Value, case: &str) -> Vec<Value> {
    let mut walked: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0;

    loop {
        let mut body = request.clone();
        if let Some(token) = &next_token {
            body["NextToken"] = json!(token);
        }
        let page = ok(router, "ListResolverDnssecConfigs", body).await;
        pages += 1;
        assert!(
            pages <= 200,
            "case {case:?}: paging did not terminate after {pages} pages"
        );

        let items = page_items(&page);
        if let Some(size) = request.get("MaxResults").and_then(Value::as_i64) {
            assert!(
                i64::try_from(items.len()).unwrap_or(i64::MAX) <= size,
                "case {case:?}: a page must hold at most MaxResults items, got {page}"
            );
        }
        walked.extend(items);

        next_token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next_token.is_none() {
            break;
        }
    }
    walked
}

/// Set one VPC's DNSSEC validation setting over the wire, returning the `Id` of
/// the configuration the call reports.
///
/// The VPC is registered in the account's own Amazon EC2 store first: a per-VPC
/// configuration belongs to a VPC of the account, which the write resolves
/// against that store.
async fn update(router: &Emulator, resource_id: &str, validation: &str) -> String {
    router.hold_vpcs([resource_id]);
    let body = ok(
        router,
        "UpdateResolverDnssecConfig",
        json!({ "ResourceId": resource_id, "Validation": validation }),
    )
    .await;
    let config = body
        .get("ResolverDNSSECConfig")
        .unwrap_or_else(|| panic!("the update response carries a ResolverDNSSECConfig, got {body}"));
    string_member(config, "Id")
}

/// The entry the listing reports for `resource_id`, if any.
fn entry_for<'a>(listed: &'a [Value], resource_id: &str) -> Option<&'a Value> {
    listed
        .iter()
        .find(|item| string_member(item, "ResourceId") == resource_id)
}

/// One row of the enabling table: the setting a VPC is moved to, and the status
/// its configuration must report in the listing once the move has settled.
#[derive(Debug)]
struct ListedCase {
    case: &'static str,
    resource_id: String,
    validation: &'static str,
    settled: &'static str,
}

/// Every configuration whose validation is not off is reported, with the members
/// of the record the update wrote, and in the order the configurations were
/// first moved off their default. An account that has configured nothing is
/// reported as an empty list, and reading a VPC's configuration adds nothing to
/// the collection.
///
/// The table spans what the rule generalises over: the two settings that move a
/// VPC off validation-off (`ENABLE`, `USE_LOCAL_RESOURCE_SETTING`) and the form
/// of the VPC id, at both ends of `ResourceId`'s modelled 1..64 length.
#[tokio::test]
async fn every_configuration_with_validation_on_is_reported() {
    let cases = vec![
        ListedCase {
            case: "ENABLE on the 17-hex-digit id form",
            resource_id: "vpc-0123456789abcdef0".to_string(),
            validation: VALIDATION_ENABLE,
            settled: DNSSEC_STATUS_ENABLED,
        },
        ListedCase {
            case: "USE_LOCAL_RESOURCE_SETTING on the 8-hex-digit id form",
            resource_id: "vpc-1a2b3c4d".to_string(),
            validation: VALIDATION_USE_LOCAL_RESOURCE_SETTING,
            settled: DNSSEC_STATUS_USE_LOCAL_RESOURCE_SETTING,
        },
        ListedCase {
            case: "a one-character ResourceId, the modelled minimum",
            resource_id: "v".to_string(),
            validation: VALIDATION_ENABLE,
            settled: DNSSEC_STATUS_ENABLED,
        },
        ListedCase {
            case: "a 64-character ResourceId, the modelled maximum",
            resource_id: format!("vpc-{}", "a".repeat(60)),
            validation: VALIDATION_ENABLE,
            settled: DNSSEC_STATUS_ENABLED,
        },
    ];

    let router = emulator();

    // An account that has configured nothing: the collection is empty, and a
    // read of a VPC's configuration — which every VPC has — adds nothing to it.
    let empty = walk(&router, &json!({}), "an account that configured nothing").await;
    assert!(
        empty.is_empty(),
        "case \"an account that configured nothing\": the listing must be empty, got {empty:?}"
    );
    router.hold_vpcs(["vpc-only-read"]);
    ok(
        &router,
        "GetResolverDnssecConfig",
        json!({ "ResourceId": "vpc-only-read" }),
    )
    .await;
    let after_read = walk(&router, &json!({}), "after a read").await;
    assert!(
        after_read.is_empty(),
        "case \"after a read\": a read must not add a configuration to the listing, got {after_read:?}"
    );

    let mut ids: Vec<(&str, String)> = Vec::with_capacity(cases.len());
    for case in &cases {
        ids.push((
            case.case,
            update(&router, &case.resource_id, case.validation).await,
        ));
    }
    settle().await;

    let listed = walk(&router, &json!({}), "every configured VPC").await;
    assert_eq!(
        listed.len(),
        cases.len(),
        "case \"every configured VPC\": the listing must report each configuration once, got {listed:?}"
    );

    // The order is the order the configurations were first moved off their
    // default, so paging a growing collection only ever appends.
    for (index, case) in cases.iter().enumerate() {
        let item = &listed[index];
        assert_eq!(
            string_member(item, "ResourceId"),
            case.resource_id,
            "case {:?}: the listing must report the configurations in the order they were configured, got {listed:?}",
            case.case
        );
        assert_eq!(
            string_member(item, "Id"),
            ids[index].1,
            "case {:?}: the listed Id must be the one the update reported, got {item}",
            case.case
        );
        assert_eq!(
            string_member(item, "OwnerId"),
            ACCOUNT,
            "case {:?}: OwnerId must be the account the emulator serves, got {item}",
            case.case
        );
        assert_eq!(
            string_member(item, "ValidationStatus"),
            case.settled,
            "case {:?}: the listed ValidationStatus must be the settled one, got {item}",
            case.case
        );

        // The entry resolves to the same configuration through the reader, so
        // one id names one resource across the service.
        let read = ok(
            &router,
            "GetResolverDnssecConfig",
            json!({ "ResourceId": case.resource_id.clone() }),
        )
        .await;
        let read = read
            .get("ResolverDNSSECConfig")
            .unwrap_or_else(|| panic!("case {:?}: the read carries a config", case.case));
        for member in ["Id", "OwnerId", "ResourceId", "ValidationStatus"] {
            assert_eq!(
                read.get(member),
                item.get(member),
                "case {:?}: the read must agree with the listing on {member}, got {read}",
                case.case
            );
        }
    }

    // A listing is a pure read: walking it again reports the very same entries.
    let again = walk(&router, &json!({}), "a second walk").await;
    assert_eq!(
        again, listed,
        "case \"a second walk\": listing twice must report the same entries"
    );
}

/// `ResolverDnssecConfigs`' doc — "It doesn't contain disabled DNSSEC
/// configurations for the resource" — bounds the collection: a VPC whose
/// validation is off is not reported, whether it was never configured or was
/// configured and then disabled again, while one still moving away from
/// validation-on stays reported until the move has settled.
#[tokio::test]
async fn a_configuration_with_validation_off_is_not_reported() {
    let router = emulator();

    let enabled = update(&router, "vpc-stays-enabled", VALIDATION_ENABLE).await;
    let disabled_again = update(&router, "vpc-disabled-again", VALIDATION_ENABLE).await;
    settle().await;

    let before = walk(&router, &json!({}), "both enabled").await;
    assert_eq!(
        before.len(),
        2,
        "case \"both enabled\": both configurations must be reported, got {before:?}"
    );

    // The move away from validation-on is in flight: the configuration is not
    // disabled yet, so it is still part of the collection.
    update(&router, "vpc-disabled-again", VALIDATION_DISABLE).await;
    let in_flight = walk(&router, &json!({}), "a disable in flight").await;
    assert!(
        entry_for(&in_flight, "vpc-disabled-again").is_some(),
        "case \"a disable in flight\": a configuration still moving away from validation-on stays reported, got {in_flight:?}"
    );

    settle().await;
    let after = walk(&router, &json!({}), "after the disable settled").await;
    let ids: Vec<String> = after.iter().map(|item| string_member(item, "Id")).collect();
    assert_eq!(
        ids,
        vec![enabled.clone()],
        "case \"after the disable settled\": only the configuration with validation on is reported, got {after:?}"
    );
    assert!(
        !ids.contains(&disabled_again),
        "case \"after the disable settled\": a disabled configuration must not be reported, got {after:?}"
    );

    // The disabled VPC still HAS a configuration, with the same identity: the
    // listing bound is about what it reports, not about the resource going away.
    let read = ok(
        &router,
        "GetResolverDnssecConfig",
        json!({ "ResourceId": "vpc-disabled-again" }),
    )
    .await;
    let read = read
        .get("ResolverDNSSECConfig")
        .expect("the read carries a ResolverDNSSECConfig");
    assert_eq!(
        string_member(read, "Id"),
        disabled_again,
        "case \"after the disable settled\": the disabled VPC keeps its configuration's Id, got {read}"
    );
}

/// Paging walks every configuration exactly once, whatever the page size: the
/// `NextToken` resumes where the previous page stopped, and the last page
/// carries none.
#[tokio::test]
async fn paging_walks_every_configuration_exactly_once() {
    let router = emulator();
    for index in 0..5 {
        update(&router, &format!("vpc-paged-{index}"), VALIDATION_ENABLE).await;
    }
    settle().await;

    let expected: Vec<String> = walk(&router, &json!({}), "MaxResults omitted")
        .await
        .iter()
        .map(|item| string_member(item, "Id"))
        .collect();
    assert_eq!(
        expected.len(),
        5,
        "the collection must be the five configured VPCs, got {expected:?}"
    );

    // Page sizes spanning the modelled MaxResults range (1..100): both published
    // bounds, sizes that do and do not divide the collection, exactly the
    // collection size, and MaxResults omitted altogether.
    for max_results in [Some(1), Some(2), Some(3), Some(5), Some(100), None] {
        let case = format!("MaxResults={max_results:?}");
        let mut request = json!({});
        if let Some(size) = max_results {
            request["MaxResults"] = json!(size);
        }
        let walked: Vec<String> = walk(&router, &request, &case)
            .await
            .iter()
            .map(|item| string_member(item, "Id"))
            .collect();
        assert_eq!(
            walked, expected,
            "case {case:?}: paging must walk every configuration exactly once, in the same order"
        );
    }

    // A page that ends the collection carries no cursor, and one that does not
    // carries one — so a caller knows when to stop without a second empty call.
    let first = ok(
        &router,
        "ListResolverDnssecConfigs",
        json!({ "MaxResults": 1 }),
    )
    .await;
    assert!(
        first.get("NextToken").is_some(),
        "case \"a page with items left over\": a cursor is owed, got {first}"
    );
    let whole = ok(
        &router,
        "ListResolverDnssecConfigs",
        json!({ "MaxResults": 100 }),
    )
    .await;
    assert_eq!(
        whole.get("NextToken"),
        None,
        "case \"a page that ends the collection\": no cursor is owed, got {whole}"
    );
}

/// `MaxResults` is modelled `range 1..100`, so a request outside it is refused
/// rather than clamped into a page the caller did not ask for.
///
/// The refusal is the boundary validator's, so only its HTTP status is asserted:
/// a handler-returned error and a boundary one differ in the error code they
/// render.
#[tokio::test]
async fn a_max_results_outside_the_modelled_range_is_refused() {
    let router = emulator();
    update(&router, "vpc-bounded", VALIDATION_ENABLE).await;

    for (case, max_results, accepted) in [
        ("one below the published minimum", 0, false),
        ("far below the published minimum", -1, false),
        ("the published minimum", 1, true),
        ("the published maximum", 100, true),
        ("one above the published maximum", 101, false),
        ("far above the published maximum", 10_000, false),
    ] {
        let (status, body) = call(
            &router,
            "ListResolverDnssecConfigs",
            json!({ "MaxResults": max_results }),
        )
        .await;
        let expected = if accepted { 200 } else { 400 };
        assert_eq!(
            status, expected,
            "case {case:?}: MaxResults={max_results} must answer HTTP {expected}, got {status}: {body}"
        );
    }
}

/// A `NextToken` this service did not mint for this query is refused with the
/// `InvalidNextTokenException` the operation models, rather than silently read as
/// "start over" and answering page one again.
#[tokio::test]
async fn a_next_token_this_query_did_not_mint_is_refused() {
    let router = emulator();
    for index in 0..4 {
        update(&router, &format!("vpc-cursor-{index}"), VALIDATION_ENABLE).await;
    }
    settle().await;

    let page = ok(
        &router,
        "ListResolverDnssecConfigs",
        json!({ "MaxResults": 1 }),
    )
    .await;
    let minted = page
        .get("NextToken")
        .and_then(Value::as_str)
        .expect("a page with items left over carries a cursor")
        .to_string();

    // Rows spanning the ways a cursor can fail to be this query's: nothing like
    // a cursor, a cursor whose payload was edited, a truncated one, a plausible
    // plaintext offset nobody minted, and a cursor minted for another query of
    // the same collection (a different filter set).
    let tampered = {
        let mut bytes = minted.clone().into_bytes();
        let last = bytes.len() - 1;
        bytes[last] = if bytes[last] == b'0' { b'1' } else { b'0' };
        String::from_utf8(bytes).expect("still ASCII")
    };
    let filtered = ok(
        &router,
        "ListResolverDnssecConfigs",
        json!({
            "MaxResults": 1,
            "Filters": [{ "Name": "ValidationStatus", "Values": [DNSSEC_STATUS_ENABLED] }],
        }),
    )
    .await;
    let other_query = filtered
        .get("NextToken")
        .and_then(Value::as_str)
        .expect("the filtered query also leaves items over")
        .to_string();

    for (case, token) in [
        ("not a cursor at all", "not-a-cursor".to_string()),
        ("an empty string", String::new()),
        ("a tampered cursor", tampered),
        (
            "the minted cursor with its tail cut off",
            minted[..minted.len() - 4].to_string(),
        ),
        (
            "a plaintext offset",
            hex::encode("ListResolverDnssecConfigs/all:2"),
        ),
        ("a cursor minted for another filter set", other_query),
    ] {
        let (status, body) = call(
            &router,
            "ListResolverDnssecConfigs",
            json!({ "NextToken": token }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a cursor this query did not mint must be refused, got {status}: {body}"
        );
        assert!(
            error_type(&body).contains("InvalidNextTokenException"),
            "case {case:?}: the refusal must name InvalidNextTokenException, got {body}"
        );
    }

    // The cursor this query did mint still resumes the walk, so the refusals
    // above are about the token and not about refusing to resume.
    let resumed = ok(
        &router,
        "ListResolverDnssecConfigs",
        json!({ "MaxResults": 1, "NextToken": minted }),
    )
    .await;
    assert_eq!(
        page_items(&resumed).len(),
        1,
        "case \"the minted cursor\": the walk must resume, got {resumed}"
    );
}

/// A filter selects the subset of the collection whose member holds one of the
/// filter's values, filters combine with AND, and a name outside the catalog is
/// refused rather than silently ignored.
///
/// The rows span the catalog — the members `ResolverDnssecConfig` declares — and
/// both the documented spelling of a name and the upper-snake spelling
/// `Filter$Name`'s doc says is "still supported".
#[tokio::test]
async fn a_filter_selects_the_subset_it_names() {
    let router = emulator();
    let enabled = update(&router, "vpc-filter-enabled", VALIDATION_ENABLE).await;
    let local = update(
        &router,
        "vpc-filter-local",
        VALIDATION_USE_LOCAL_RESOURCE_SETTING,
    )
    .await;
    update(&router, "vpc-filter-second-enabled", VALIDATION_ENABLE).await;
    settle().await;

    let all = walk(&router, &json!({}), "unfiltered").await;
    assert_eq!(
        all.len(),
        3,
        "case \"unfiltered\": the three configured VPCs must be reported, got {all:?}"
    );

    for (case, filters, expected) in [
        (
            "ResourceId selects one VPC",
            json!([{ "Name": "ResourceId", "Values": ["vpc-filter-enabled"] }]),
            vec![enabled.clone()],
        ),
        (
            "RESOURCE_ID, the upper-snake spelling, selects the same VPC",
            json!([{ "Name": "RESOURCE_ID", "Values": ["vpc-filter-enabled"] }]),
            vec![enabled.clone()],
        ),
        (
            "Id selects the configuration it names",
            json!([{ "Name": "Id", "Values": [local.clone()] }]),
            vec![local.clone()],
        ),
        (
            "ValidationStatus selects every VPC in that status",
            json!([{ "Name": "ValidationStatus", "Values": [DNSSEC_STATUS_USE_LOCAL_RESOURCE_SETTING] }]),
            vec![local.clone()],
        ),
        (
            "OwnerId selects every configuration of this account",
            json!([{ "Name": "OwnerId", "Values": [ACCOUNT] }]),
            all.iter().map(|item| string_member(item, "Id")).collect(),
        ),
        (
            "two values of one filter select both",
            json!([{ "Name": "ResourceId", "Values": ["vpc-filter-enabled", "vpc-filter-local"] }]),
            vec![enabled.clone(), local.clone()],
        ),
        (
            "two filters combine with AND",
            json!([
                { "Name": "ResourceId", "Values": ["vpc-filter-enabled", "vpc-filter-local"] },
                { "Name": "ValidationStatus", "Values": [DNSSEC_STATUS_ENABLED] },
            ]),
            vec![enabled.clone()],
        ),
        (
            "a filter no configuration satisfies selects none",
            json!([{ "Name": "ResourceId", "Values": ["vpc-not-configured"] }]),
            Vec::new(),
        ),
    ] {
        let walked: Vec<String> = walk(&router, &json!({ "Filters": filters }), case)
            .await
            .iter()
            .map(|item| string_member(item, "Id"))
            .collect();
        assert_eq!(
            walked, expected,
            "case {case:?}: the filter must select exactly the configurations it names"
        );
    }

    // A name outside the catalog is refused, with the parameter it names. (The
    // empty string is not among the rows: `Filter$Name`'s modelled length
    // excludes it, so it never reaches the catalog.)
    for case in ["Status", "ResourceID ", "Name", "SomethingElse"] {
        let (status, body) = call(
            &router,
            "ListResolverDnssecConfigs",
            json!({ "Filters": [{ "Name": case, "Values": ["vpc-filter-enabled"] }] }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a filter name outside the catalog must be refused, got {status}: {body}"
        );
        assert!(
            error_type(&body).contains("InvalidParameterException"),
            "case {case:?}: the refusal must name InvalidParameterException, got {body}"
        );
    }
}
