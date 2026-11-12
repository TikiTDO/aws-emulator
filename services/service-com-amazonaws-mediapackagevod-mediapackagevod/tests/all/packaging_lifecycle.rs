// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the MediaPackage VOD packaging-group -> asset lifecycle.
//!
//! Every test drives real HTTP requests through the router the production binary
//! builds, so what it asserts is the wire behaviour a client sees: restJson1 routes,
//! status codes and JSON bodies. Each test owns its own engine, so no test observes
//! another's state.
//!
//! The tests assert RULES over a set of inputs rather than one recorded call:
//!
//! * a resource read back is the resource that was stored (same ARN, CreatedAt, tags);
//! * `ApproximateAssetCount` and `EgressEndpoints` are derived from state at read time;
//! * the playback URL's manifest file name follows the packaging format the
//!   configuration was created with;
//! * paging with any page size yields every asset exactly once, in one stable order;
//! * `MaxResults` is accepted on each modelled bound and refused one past it;
//! * a collection operation reports each stored resource exactly the record the
//!   matching describe returns, and a cursor one collection issued is refused by
//!   another;
//! * a packaging group is removed only while no packaging configuration references
//!   it, and a refused delete removes nothing.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_mediapackagevod_mediapackagevod::state as vod;

/// The region the test signs for. It appears in every ARN and in the group's egress
/// domain name, so it is asserted rather than assumed.
pub(crate) const REGION: &str = "us-west-2";

/// The account id every emulated ARN carries (`engine::emulator_account_id`).
pub(crate) fn account_id() -> &'static str {
    engine::emulator_account_id()
}

/// A router carrying exactly this service over one shared engine — the same
/// assembly `core/router` performs, minus the other services.
pub(crate) fn router() -> MultiServiceRouter {
    router_over(Arc::new(engine::InMemoryEngine::new()))
}

/// The same router over an engine the caller already holds, so a test can pre-fill
/// the store (to reach a capacity bound) without issuing one request per resource.
pub(crate) fn router_over(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_mediapackagevod_mediapackagevod::EmulatedService::new(engine),
    );
    router
}

/// Send one request and return the WHOLE response, headers included — the one request
/// seam both [`call`] and a test that asserts on a response header go through.
///
/// The `Authorization` header carries only a SigV4 credential SCOPE: that is what the
/// router routes on and what the handlers read the region from. Signature
/// verification is off for this router, so the signature itself is never checked.
pub(crate) async fn call_raw(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> http::Response<bytes::Bytes> {
    let host = format!("mediapackage-vod.{REGION}.amazonaws.com");
    call_raw_addressed(router, method, path, body, &host, Some(&host)).await
}

/// The same request, sent to an ADDRESS the caller chooses: the authority its URI names
/// and the `Host` header it carries (`None` sends none at all). The SigV4 scope still
/// routes it, so a test can drive these routes as a client that talks to the emulator's
/// own endpoint rather than to one of AWS's hostnames.
pub(crate) async fn call_raw_addressed(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
    authority: &str,
    host_header: Option<&str>,
) -> http::Response<bytes::Bytes> {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{authority}{path}"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}\
                 /mediapackage-vod/aws4_request, SignedHeaders=host, Signature=unverified"
            ),
        );
    if let Some(host) = host_header {
        builder = builder.header("host", host);
    }
    let request = builder
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed test request");

    router.dispatch(request).await
}

/// Send one request and return its status and parsed JSON body (`Null` when empty).
pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let response = call_raw(router, method, path, body).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Create a packaging group, asserting it succeeded.
pub(crate) async fn create_group(
    router: &MultiServiceRouter,
    id: &str,
    tags: Option<Value>,
) -> Value {
    let mut body = json!({ "id": id });
    if let Some(tags) = tags {
        body["tags"] = tags;
    }
    create_group_from(router, id, body).await
}

/// Create a packaging group from a whole request body, asserting it succeeded — for a
/// create that names optional members `create_group` does not carry.
async fn create_group_from(router: &MultiServiceRouter, id: &str, body: Value) -> Value {
    let (status, group) = call(router, "POST", "/packaging_groups", Some(body)).await;
    assert_eq!(status, 200, "CreatePackagingGroup {id}: body {group}");
    group
}

/// Create a packaging configuration from a package block, asserting it succeeded.
pub(crate) async fn create_config(
    router: &MultiServiceRouter,
    id: &str,
    group_id: &str,
    package: Value,
) -> Value {
    let mut body = json!({ "id": id, "packagingGroupId": group_id });
    let Value::Object(members) = package else {
        panic!("the package block must be a JSON object");
    };
    for (key, value) in members {
        body[key] = value;
    }
    let (status, config) = call(router, "POST", "/packaging_configurations", Some(body)).await;
    assert_eq!(status, 202, "CreatePackagingConfiguration {id}: body {config}");
    config
}

/// Create an asset, asserting it succeeded.
pub(crate) async fn create_asset(router: &MultiServiceRouter, id: &str, group_id: &str) -> Value {
    let body = json!({
        "id": id,
        "packagingGroupId": group_id,
        "sourceArn": format!("arn:aws:s3:::test-bucket/{id}.smil"),
        "sourceRoleArn": format!("arn:aws:iam::{}:role/MediaPackageVod", account_id()),
    });
    let (status, asset) = call(router, "POST", "/assets", Some(body)).await;
    assert_eq!(status, 200, "CreateAsset {id}: body {asset}");
    asset
}

/// An egress-endpoint list with each entry's volatile `status` removed, so two reads
/// can be compared on the parts that must not change.
fn endpoints_without_status(value: &Value) -> Vec<(String, String)> {
    value
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    (
                        entry["packagingConfigurationId"].as_str().unwrap_or_default().to_string(),
                        entry["url"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The ids `ListAssets` reports for `path`, following `nextToken` to the end.
async fn list_all_asset_ids(router: &MultiServiceRouter, query: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let mut token: Option<String> = None;
    // One more iteration than there can be pages of one asset each, so a cursor that
    // fails to advance fails the test instead of looping forever.
    for _ in 0..64 {
        let mut path = format!("/assets?{query}");
        if let Some(token) = &token {
            path.push_str(&format!("&nextToken={token}"));
        }
        let (status, page) = call(router, "GET", &path, None).await;
        assert_eq!(status, 200, "ListAssets {path}: body {page}");
        for asset in page["assets"].as_array().into_iter().flatten() {
            ids.push(asset["id"].as_str().unwrap_or_default().to_string());
        }
        match page["nextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return ids,
        }
    }
    panic!("ListAssets never stopped returning a nextToken for query {query:?}");
}

/// Every leaf of `expected` must appear, with that value, at the same path of
/// `actual`: a SUBSET comparison, so a response that also carries members the
/// expectation does not state still passes.
///
/// `pub(crate)` and shared with this crate's other suites
/// (`tests/all/success_path_projections.rs`, `tests/all/describe_packaging_configuration.rs`)
/// rather than copied into each: both compare a request's or an expectation's members
/// against a response that carries the service's resolved defaults besides them, and
/// one walk means the two cannot drift. `which` names the body being compared and
/// `case` the row, so a failure says which assertion of which row failed and where.
pub(crate) fn assert_projection(case: &str, which: &str, expected: &Value, actual: &Value) {
    assert_projection_at(case, which, "$", expected, actual);
}

/// See [`assert_projection`]; `path` is where in the body this comparison is.
fn assert_projection_at(case: &str, which: &str, path: &str, expected: &Value, actual: &Value) {
    match expected {
        Value::Object(members) => {
            for (key, value) in members {
                assert_projection_at(
                    case,
                    which,
                    &format!("{path}.{key}"),
                    value,
                    &actual[key.as_str()],
                );
            }
        }
        Value::Array(entries) => {
            assert_eq!(
                actual.as_array().map(Vec::len),
                Some(entries.len()),
                "case {case:?}: {which} at {path} must hold {} entries, got {actual}",
                entries.len()
            );
            for (index, value) in entries.iter().enumerate() {
                assert_projection_at(
                    case,
                    which,
                    &format!("{path}[{index}]"),
                    value,
                    &actual[index],
                );
            }
        }
        _ => assert_eq!(
            actual, expected,
            "case {case:?}: {which} at {path} must be {expected}, got {actual}"
        ),
    }
}

/// The issue's happy path, end to end: every read answers from what the writes
/// persisted, and a delete is visible to the following read.
#[tokio::test]
async fn the_packaging_group_asset_lifecycle_round_trips_through_state() {
    let router = router();

    let group = create_group(&router, "vod-group", Some(json!({ "env": "test" }))).await;
    let expected_group_arn =
        format!("arn:aws:mediapackage-vod:{REGION}:{}:packaging-groups/vod-group", account_id());
    assert_eq!(
        group["arn"].as_str(),
        Some(expected_group_arn.as_str()),
        "CreatePackagingGroup: expected the computed ARN, got {}",
        group["arn"]
    );
    let domain_name = group["domainName"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        domain_name.ends_with(&format!(".egress.mediapackage-vod.{REGION}.amazonaws.com")),
        "CreatePackagingGroup: expected an egress origin in this region, got {domain_name:?}"
    );
    assert!(
        !group["createdAt"].as_str().unwrap_or_default().is_empty(),
        "CreatePackagingGroup: expected a CreatedAt, got {}",
        group["createdAt"]
    );

    // DescribePackagingGroup returns the SAME stored record, with a counted asset
    // count rather than a stored one.
    let (status, described) = call(&router, "GET", "/packaging_groups/vod-group", None).await;
    assert_eq!(status, 200, "DescribePackagingGroup: body {described}");
    for member in ["arn", "domainName", "createdAt", "id", "tags"] {
        assert_eq!(
            described[member], group[member],
            "DescribePackagingGroup: {member} must be what CreatePackagingGroup stored, got {}",
            described[member]
        );
    }
    assert_eq!(
        described["approximateAssetCount"].as_i64(),
        Some(0),
        "DescribePackagingGroup with no assets: got {}",
        described["approximateAssetCount"]
    );

    // The packaging configuration's package block is read back from state verbatim.
    let config = create_config(
        &router,
        "hls-cfg",
        "vod-group",
        json!({ "hlsPackage": { "hlsManifests": [{ "manifestName": "index" }] } }),
    )
    .await;
    assert_eq!(
        config["hlsPackage"]["hlsManifests"][0]["manifestName"].as_str(),
        Some("index"),
        "CreatePackagingConfiguration: the package block must round-trip, got {}",
        config["hlsPackage"]
    );
    assert_eq!(
        config["arn"].as_str(),
        Some(
            format!(
                "arn:aws:mediapackage-vod:{REGION}:{}:packaging-configurations/hls-cfg",
                account_id()
            )
            .as_str()
        ),
        "CreatePackagingConfiguration: expected the computed ARN, got {}",
        config["arn"]
    );

    // CreateAsset derives one egress endpoint per configuration on the group.
    let asset = create_asset(&router, "asset-1", "vod-group").await;
    assert_eq!(
        asset["resourceId"],
        Value::Null,
        "CreateAsset without ResourceId: the service states none of its own, got {}",
        asset["resourceId"]
    );
    assert_eq!(
        endpoints_without_status(&asset["egressEndpoints"])
            .iter()
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>(),
        vec!["hls-cfg".to_string()],
        "CreateAsset: expected one endpoint per configuration on the group, got {}",
        asset["egressEndpoints"]
    );

    // DescribeAsset returns the stored record, endpoints included.
    let (status, described_asset) = call(&router, "GET", "/assets/asset-1", None).await;
    assert_eq!(status, 200, "DescribeAsset: body {described_asset}");
    for member in ["arn", "createdAt", "sourceArn", "sourceRoleArn", "tags", "resourceId"] {
        assert_eq!(
            described_asset[member], asset[member],
            "DescribeAsset: {member} must be what CreateAsset stored, got {}",
            described_asset[member]
        );
    }
    assert_eq!(
        endpoints_without_status(&described_asset["egressEndpoints"]),
        endpoints_without_status(&asset["egressEndpoints"]),
        "DescribeAsset: the egress endpoints must be the ones CreateAsset reported, got {}",
        described_asset["egressEndpoints"]
    );

    // A second asset, and the group filter plus the counted asset count follow.
    create_asset(&router, "asset-2", "vod-group").await;
    assert_eq!(
        list_all_asset_ids(&router, "packagingGroupId=vod-group").await,
        vec!["asset-1".to_string(), "asset-2".to_string()],
        "ListAssets filtered by the group must report both stored assets"
    );
    let (_, described) = call(&router, "GET", "/packaging_groups/vod-group", None).await;
    assert_eq!(
        described["approximateAssetCount"].as_i64(),
        Some(2),
        "DescribePackagingGroup after two creates: got {}",
        described["approximateAssetCount"]
    );

    // DeleteAsset removes it from state: 204, then 404, and gone from the listing.
    let (status, body) = call(&router, "DELETE", "/assets/asset-1", None).await;
    assert_eq!(
        status, 204,
        "DeleteAsset: expected the recorded 204 No Content, body {body}"
    );
    let (status, body) = call(&router, "GET", "/assets/asset-1", None).await;
    assert_eq!(status, 404, "DescribeAsset after delete: body {body}");
    assert_eq!(
        list_all_asset_ids(&router, "packagingGroupId=vod-group").await,
        vec!["asset-2".to_string()],
        "ListAssets after delete must not report the deleted asset"
    );
    let (_, described) = call(&router, "GET", "/packaging_groups/vod-group", None).await;
    assert_eq!(
        described["approximateAssetCount"].as_i64(),
        Some(1),
        "DescribePackagingGroup after a delete: got {}",
        described["approximateAssetCount"]
    );
}

/// The playback URL's file name follows the packaging format the configuration was
/// created with, and its name follows that format's manifest block.
///
/// The suffix per format is the parent manifest each packaging format publishes: HLS
/// and CMAF an HLS playlist, DASH an MPD, Microsoft Smooth a server manifest. The
/// name defaults to `index` when the manifest block names none
/// (`HlsManifest$ManifestName`: "An optional string to include in the name of the
/// manifest").
#[tokio::test]
async fn the_playback_url_manifest_follows_the_packaging_format() {
    // (case, package block, expected manifest file name)
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "hls, default manifest name",
            json!({ "hlsPackage": { "hlsManifests": [{}] } }),
            "index.m3u8",
        ),
        (
            "hls, named manifest",
            json!({ "hlsPackage": { "hlsManifests": [{ "manifestName": "primary" }] } }),
            "primary.m3u8",
        ),
        (
            "dash, default manifest name",
            json!({ "dashPackage": { "dashManifests": [{}] } }),
            "index.mpd",
        ),
        (
            "dash, named manifest",
            json!({ "dashPackage": { "dashManifests": [{ "manifestName": "wide" }] } }),
            "wide.mpd",
        ),
        (
            "cmaf, default manifest name",
            json!({ "cmafPackage": { "hlsManifests": [{}] } }),
            "index.m3u8",
        ),
        (
            "mss, default manifest name",
            json!({ "mssPackage": { "mssManifests": [{}] } }),
            "index.ism/Manifest",
        ),
        (
            "mss, named manifest",
            json!({ "mssPackage": { "mssManifests": [{ "manifestName": "smooth" }] } }),
            "smooth.ism/Manifest",
        ),
    ];

    for (case, package, expected_file) in cases {
        let router = router();
        let group = create_group(&router, "g", None).await;
        let domain_name = group["domainName"].as_str().unwrap_or_default().to_string();
        create_config(&router, "cfg", "g", package).await;
        let asset = create_asset(&router, "a", "g").await;

        let endpoints = endpoints_without_status(&asset["egressEndpoints"]);
        let [(config_id, url)] = endpoints.as_slice() else {
            panic!("case {case:?}: expected exactly one endpoint, got {endpoints:?}");
        };
        assert_eq!(
            config_id, "cfg",
            "case {case:?}: expected the configuration's id, got {config_id:?}"
        );
        assert!(
            url.starts_with(&format!("{domain_name}/out/v1/")),
            "case {case:?}: expected a URL on the group's egress origin, got {url:?}"
        );
        assert!(
            url.ends_with(&format!("/{expected_file}")),
            "case {case:?}: expected a URL ending in {expected_file:?}, got {url:?}"
        );
        // The URL is addressable state, not a per-call value: reading again returns it.
        let (_, described) = call(&router, "GET", "/assets/a", None).await;
        assert_eq!(
            endpoints_without_status(&described["egressEndpoints"]),
            endpoints,
            "case {case:?}: the playback URL must be stable across reads, got {}",
            described["egressEndpoints"]
        );
    }
}

/// An egress endpoint reports the non-terminal `QUEUED` until the packaging job the
/// clock owes falls due, then `PLAYABLE` — and a read never advances it early.
#[tokio::test]
async fn an_egress_endpoint_reports_queued_until_the_packaging_job_is_due() {
    use service_com_amazonaws_mediapackagevod_mediapackagevod::state::{
        EGRESS_STATUS_PLAYABLE, EGRESS_STATUS_QUEUED, INGEST_PLAYABLE_AFTER,
    };

    let router = router();
    create_group(&router, "g", None).await;
    create_config(
        &router,
        "cfg",
        "g",
        json!({ "hlsPackage": { "hlsManifests": [{}] } }),
    )
    .await;
    let asset = create_asset(&router, "a", "g").await;
    assert_eq!(
        asset["egressEndpoints"][0]["status"].as_str(),
        Some(EGRESS_STATUS_QUEUED),
        "straight after CreateAsset the packaging job is not due yet, got {}",
        asset["egressEndpoints"][0]["status"]
    );

    // Past the due instant the clock has made the transition owed, so the next read
    // materialises it. Nothing was written in between.
    tokio::time::sleep(INGEST_PLAYABLE_AFTER + std::time::Duration::from_millis(100)).await;
    let (_, described) = call(&router, "GET", "/assets/a", None).await;
    assert_eq!(
        described["egressEndpoints"][0]["status"].as_str(),
        Some(EGRESS_STATUS_PLAYABLE),
        "past the due instant the endpoint must be playable, got {}",
        described["egressEndpoints"][0]["status"]
    );
}

/// `ApproximateAssetCount` is the number of assets stored in THAT group, counted at
/// read time — so it tracks creates and deletes and never counts another group's.
#[tokio::test]
async fn approximate_asset_count_is_counted_per_group() {
    // (assets in group "a", assets in group "b")
    for (in_a, in_b) in [(0_usize, 0_usize), (1, 0), (0, 3), (2, 2), (5, 1)] {
        let router = router();
        create_group(&router, "a", None).await;
        create_group(&router, "b", None).await;
        for index in 0..in_a {
            create_asset(&router, &format!("a-{index}"), "a").await;
        }
        for index in 0..in_b {
            create_asset(&router, &format!("b-{index}"), "b").await;
        }

        for (group, expected) in [("a", in_a), ("b", in_b)] {
            let (status, described) =
                call(&router, "GET", &format!("/packaging_groups/{group}"), None).await;
            assert_eq!(
                status, 200,
                "case ({in_a}, {in_b}) group {group}: body {described}"
            );
            assert_eq!(
                described["approximateAssetCount"].as_i64(),
                Some(expected as i64),
                "case ({in_a}, {in_b}) group {group}: got {}",
                described["approximateAssetCount"]
            );
        }
    }
}

/// Paging reports every stored asset exactly once, in one stable order, whatever the
/// page size — and a cursor resumes strictly after the id the previous page ended on.
#[tokio::test]
async fn list_assets_pages_every_asset_exactly_once() {
    // (total assets, maxResults per page)
    for (total, page_size) in [(1_usize, 1_usize), (3, 1), (3, 2), (5, 4), (4, 1000), (7, 3)] {
        let router = router();
        create_group(&router, "g", None).await;
        let mut expected: Vec<String> = Vec::new();
        for index in 0..total {
            // Zero-padded so the id order the service pages in is unambiguous.
            let id = format!("asset-{index:02}");
            create_asset(&router, &id, "g").await;
            expected.push(id);
        }
        expected.sort();

        let seen = list_all_asset_ids(&router, &format!("maxResults={page_size}")).await;
        assert_eq!(
            seen, expected,
            "case total={total} maxResults={page_size}: every asset exactly once, in id order"
        );
        assert_eq!(
            seen.iter().collect::<BTreeSet<_>>().len(),
            seen.len(),
            "case total={total} maxResults={page_size}: a page repeated an asset: {seen:?}"
        );

        // The first page is no longer than the page size the caller asked for.
        let (status, first) = call(
            &router,
            "GET",
            &format!("/assets?maxResults={page_size}"),
            None,
        )
        .await;
        assert_eq!(status, 200, "case maxResults={page_size}: body {first}");
        let first_len = first["assets"].as_array().map_or(0, Vec::len);
        assert!(
            first_len <= page_size,
            "case total={total} maxResults={page_size}: first page held {first_len} assets"
        );
    }
}

/// `MaxResults` is accepted on each bound the model publishes (`range(min: 1, max:
/// 1000)`) and refused one past either, and a `NextToken` this service never issued
/// is refused as an invalid parameter.
#[tokio::test]
async fn max_results_and_next_token_follow_their_modelled_constraints() {
    let router = router();
    create_group(&router, "g", None).await;
    create_asset(&router, "a-1", "g").await;
    create_asset(&router, "a-2", "g").await;

    for accepted in [1, 2, 1000] {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/assets?maxResults={accepted}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "maxResults={accepted} is within the modelled range: body {body}"
        );
        let returned = body["assets"].as_array().map_or(0, Vec::len);
        assert_eq!(
            returned,
            accepted.min(2),
            "maxResults={accepted}: expected at most that many assets, got {returned}"
        );
    }

    for refused in [0, 1001] {
        let (status, body) =
            call(&router, "GET", &format!("/assets?maxResults={refused}"), None).await;
        assert!(
            (400..500).contains(&status),
            "maxResults={refused} is outside the modelled range and must be refused, \
             got {status}: body {body}"
        );
    }

    // (case, nextToken) — every cursor this service did not issue is refused.
    let forged: Vec<(&str, String)> = vec![
        ("not hex at all", "not-a-cursor".to_string()),
        ("hex, but no checksum prefix", hex::encode("a-1")),
        (
            "hex with a wrong checksum prefix",
            format!("00000000{}", hex::encode("a-1")),
        ),
        ("shorter than the checksum prefix", "00ff".to_string()),
        ("non-ascii", "🙈🙈🙈🙈🙈".to_string()),
    ];
    for (case, token) in forged {
        let (status, body) =
            call(&router, "GET", &format!("/assets?nextToken={token}"), None).await;
        assert_eq!(
            status, 422,
            "case {case:?}: a NextToken this service never issued must be refused as \
             unprocessable, got {status}: body {body}"
        );
    }

    // ... and the cursor it DID issue resumes the listing.
    let (status, first) = call(&router, "GET", "/assets?maxResults=1", None).await;
    assert_eq!(status, 200, "the first page: body {first}");
    let issued = first["nextToken"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !issued.is_empty(),
        "with two assets and maxResults=1 the first page must carry a cursor: {first}"
    );
    let (status, second) = call(
        &router,
        "GET",
        &format!("/assets?maxResults=1&nextToken={issued}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "the issued cursor must be accepted: body {second}");
    assert_eq!(
        second["assets"][0]["id"].as_str(),
        Some("a-2"),
        "the issued cursor must resume after the first page, got {}",
        second["assets"]
    );
}

/// A PackagingGroupRecord to pre-fill the store with, so a capacity bound is reached
/// without one request per resource.
fn filler_group(id: &str) -> vod::PackagingGroupRecord {
    vod::PackagingGroupRecord {
        id: id.to_string(),
        region: REGION.to_string(),
        arn: vod::resource_arn("packaging-groups", REGION, id),
        domain_name: vod::packaging_group_domain_name(id, REGION),
        created_at: "2026-01-01T00:00:00Z".to_string(),
        authorization: None,
        egress_access_logs: None,
    }
}

/// A PackagingConfigurationRecord to pre-fill the store with. See [`filler_group`].
fn filler_config(id: &str, group_id: &str) -> vod::PackagingConfigurationRecord {
    vod::PackagingConfigurationRecord {
        id: id.to_string(),
        region: REGION.to_string(),
        arn: vod::resource_arn("packaging-configurations", REGION, id),
        created_at: "2026-01-01T00:00:00Z".to_string(),
        packaging_group_id: group_id.to_string(),
        hls_package: None,
        dash_package: None,
        cmaf_package: None,
        mss_package: None,
        created_instant: std::time::Instant::now(),
    }
}

/// An AssetRecord to pre-fill the store with. See [`filler_group`].
fn filler_asset(id: &str, group_id: &str) -> vod::AssetRecord {
    vod::AssetRecord {
        id: id.to_string(),
        region: REGION.to_string(),
        arn: vod::resource_arn("assets", REGION, id),
        created_at: "2026-01-01T00:00:00Z".to_string(),
        packaging_group_id: group_id.to_string(),
        resource_id: Some(id.to_string()),
        source_arn: format!("arn:aws:s3:::test-bucket/{id}.smil"),
        source_role_arn: format!("arn:aws:iam::{}:role/MediaPackageVod", account_id()),
        // Pre-filled records stand in for assets whose ingest read their source; what
        // decides this for a record a CreateAsset wrote is asserted in
        // `tests/all/ingest_source_status.rs`.
        source_readability: vod::SourceReadability::Readable,
        created_instant: std::time::Instant::now(),
    }
}

/// Every store a create grows carries a capacity bound: one below it a create still
/// succeeds, and at it the create is refused with the modelled
/// `TooManyRequestsException` (429) — "The client has exceeded their resource or
/// throttling limits" — rather than growing the process without limit.
#[tokio::test]
async fn creates_are_refused_at_each_capacity_bound() {
    // (case, how to fill the store to `held` resources, the create to then attempt)
    // Each row is run twice: once one short of the bound (must be accepted) and once
    // at the bound (must be refused).
    for short_of_bound in [true, false] {
        // --- PackagingGroups ---
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let held = vod::MAX_PACKAGING_GROUPS - usize::from(short_of_bound);
        {
            let store = engine.state::<vod::ComAmazonawsMediapackagevodMediapackagevodState>();
            let mut state = store.lock();
            for index in 0..held {
                let id = format!("filler-{index:05}");
                state.insert_packaging_group(filler_group(&id));
            }
        }
        let router = router_over(engine);
        let (status, body) = call(
            &router,
            "POST",
            "/packaging_groups",
            Some(json!({ "id": "one-more-group" })),
        )
        .await;
        let expected = if short_of_bound { 200 } else { 429 };
        // A successful `CreatePackagingConfiguration` answers 202, not 200.
        let expected_config = if short_of_bound { 202 } else { 429 };
        assert_eq!(
            status, expected,
            "CreatePackagingGroup holding {held} of {}: got {status}, body {body}",
            vod::MAX_PACKAGING_GROUPS
        );

        // --- PackagingConfigurations, service-wide ---
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let held = vod::MAX_PACKAGING_CONFIGURATIONS - usize::from(short_of_bound);
        {
            let store = engine.state::<vod::ComAmazonawsMediapackagevodMediapackagevodState>();
            let mut state = store.lock();
            state.insert_packaging_group(filler_group("g"));
            for index in 0..held {
                // Spread over synthetic groups so the per-group bound is not what
                // refuses this create.
                let id = format!("filler-{index:05}");
                let group = format!("group-{}", index / 10);
                state.insert_packaging_configuration(filler_config(&id, &group));
            }
        }
        let router = router_over(engine);
        let (status, body) = call(
            &router,
            "POST",
            "/packaging_configurations",
            Some(json!({
                "id": "one-more-config",
                "packagingGroupId": "g",
                "hlsPackage": { "hlsManifests": [{}] },
            })),
        )
        .await;
        assert_eq!(
            status, expected_config,
            "CreatePackagingConfiguration holding {held} of {}: got {status}, body {body}",
            vod::MAX_PACKAGING_CONFIGURATIONS
        );

        // --- PackagingConfigurations in ONE group (this bounds the derived
        // EgressEndpoints list too) ---
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let held = vod::MAX_PACKAGING_CONFIGURATIONS_PER_GROUP - usize::from(short_of_bound);
        {
            let store = engine.state::<vod::ComAmazonawsMediapackagevodMediapackagevodState>();
            let mut state = store.lock();
            state.insert_packaging_group(filler_group("g"));
            for index in 0..held {
                let id = format!("filler-{index:05}");
                state.insert_packaging_configuration(filler_config(&id, "g"));
            }
        }
        let router = router_over(engine);
        let (status, body) = call(
            &router,
            "POST",
            "/packaging_configurations",
            Some(json!({
                "id": "one-more-config",
                "packagingGroupId": "g",
                "hlsPackage": { "hlsManifests": [{}] },
            })),
        )
        .await;
        assert_eq!(
            status, expected_config,
            "CreatePackagingConfiguration holding {held} of {} in one group: got {status}, \
             body {body}",
            vod::MAX_PACKAGING_CONFIGURATIONS_PER_GROUP
        );

        // --- Assets ---
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let held = vod::MAX_ASSETS - usize::from(short_of_bound);
        {
            let store = engine.state::<vod::ComAmazonawsMediapackagevodMediapackagevodState>();
            let mut state = store.lock();
            state.insert_packaging_group(filler_group("g"));
            for index in 0..held {
                let id = format!("filler-{index:05}");
                state.insert_asset(filler_asset(&id, "g"));
            }
        }
        let router = router_over(engine);
        let (status, body) = call(
            &router,
            "POST",
            "/assets",
            Some(json!({
                "id": "one-more-asset",
                "packagingGroupId": "g",
                "sourceArn": "arn:aws:s3:::test-bucket/one-more.smil",
                "sourceRoleArn": format!("arn:aws:iam::{}:role/MediaPackageVod", account_id()),
            })),
        )
        .await;
        assert_eq!(
            status, expected,
            "CreateAsset holding {held} of {}: got {status}, body {body}",
            vod::MAX_ASSETS
        );
    }
}

/// Every operation that ADDRESSES a resource by id answers `NotFoundException` (404)
/// for an id nothing is stored under, while a create that reuses a stored id — or
/// names a `packagingGroupId` in its body that resolves to nothing — is refused as
/// unprocessable (422) rather than overwriting or inventing anything.
#[tokio::test]
async fn missing_and_duplicate_resources_are_refused_with_the_modelled_errors() {
    let router = router();
    create_group(&router, "g", None).await;
    create_config(
        &router,
        "cfg",
        "g",
        json!({ "hlsPackage": { "hlsManifests": [{}] } }),
    )
    .await;
    create_asset(&router, "a", "g").await;

    // (case, method, path, body) that must answer 404.
    let not_found: Vec<(&str, &str, String, Option<Value>)> = vec![
        ("DescribeAsset, unknown id", "GET", "/assets/nope".into(), None),
        ("DeleteAsset, unknown id", "DELETE", "/assets/nope".into(), None),
        (
            "DescribePackagingGroup, unknown id",
            "GET",
            "/packaging_groups/nope".into(),
            None,
        ),
    ];
    for (case, method, path, body) in not_found {
        let (status, response) = call(&router, method, &path, body).await;
        assert_eq!(status, 404, "case {case:?}: got {status}, body {response}");
    }

    // (case, path, body) that must be refused as unprocessable. A `packagingGroupId`
    // that resolves to nothing belongs here rather than with the 404s: it arrives in
    // the request BODY, so it is a violation of that PARAMETER, while the 404 above
    // belongs to the resource a request ADDRESSES. `tests/all/error_responses.rs`
    // pins the body each refusal carries.
    let unprocessable: Vec<(&str, &str, Value)> = vec![
        (
            "CreateAsset, unknown group",
            "/assets",
            json!({
                "id": "orphan",
                "packagingGroupId": "nope",
                "sourceArn": "arn:aws:s3:::test-bucket/orphan.smil",
                "sourceRoleArn": "arn:aws:iam::000000000000:role/MediaPackageVod",
            }),
        ),
        (
            "CreatePackagingConfiguration, unknown group",
            "/packaging_configurations",
            json!({
                "id": "orphan-cfg",
                "packagingGroupId": "nope",
                "hlsPackage": { "hlsManifests": [{}] },
            }),
        ),
        (
            "CreatePackagingGroup, id already stored",
            "/packaging_groups",
            json!({ "id": "g" }),
        ),
        (
            "CreatePackagingConfiguration, id already stored",
            "/packaging_configurations",
            json!({
                "id": "cfg",
                "packagingGroupId": "g",
                "hlsPackage": { "hlsManifests": [{}] },
            }),
        ),
        (
            "CreateAsset, id already stored",
            "/assets",
            json!({
                "id": "a",
                "packagingGroupId": "g",
                "sourceArn": "arn:aws:s3:::test-bucket/a.smil",
                "sourceRoleArn": "arn:aws:iam::000000000000:role/MediaPackageVod",
            }),
        ),
        (
            "CreatePackagingConfiguration, no package block",
            "/packaging_configurations",
            json!({ "id": "empty-cfg", "packagingGroupId": "g" }),
        ),
        (
            "CreatePackagingConfiguration, two package blocks",
            "/packaging_configurations",
            json!({
                "id": "two-cfg",
                "packagingGroupId": "g",
                "hlsPackage": { "hlsManifests": [{}] },
                "dashPackage": { "dashManifests": [{}] },
            }),
        ),
    ];
    for (case, path, body) in unprocessable {
        let (status, response) = call(&router, "POST", path, Some(body)).await;
        assert_eq!(status, 422, "case {case:?}: got {status}, body {response}");
    }

    // A refused duplicate must not have disturbed what was already stored.
    let (status, described) = call(&router, "GET", "/assets/a", None).await;
    assert_eq!(status, 200, "the stored asset survived the refused creates: {described}");
    assert_eq!(
        described["id"].as_str(),
        Some("a"),
        "the stored asset survived the refused creates, got {}",
        described["id"]
    );
}

/// A packaging group's egress domain name is stable across reads and distinct per
/// group, so two groups never hand out the same playback host.
#[tokio::test]
async fn egress_domain_names_are_stable_and_distinct_per_group() {
    let router = router();
    let ids = ["alpha", "beta", "gamma", "a-very-long-packaging-group-id-0123456789"];
    let mut domains = BTreeSet::new();
    for id in ids {
        let created = create_group(&router, id, None).await;
        let domain = created["domainName"].as_str().unwrap_or_default().to_string();
        let (_, described) = call(&router, "GET", &format!("/packaging_groups/{id}"), None).await;
        assert_eq!(
            described["domainName"].as_str(),
            Some(domain.as_str()),
            "group {id:?}: the domain name must be stable across reads, got {}",
            described["domainName"]
        );
        assert!(
            domains.insert(domain.clone()),
            "group {id:?}: the domain name {domain:?} collided with another group's"
        );
    }
}

/// Every member `PackagingGroup` models, so a listed entry is compared on all of
/// them rather than on the few a single recorded call happened to carry
/// (`smithy_query(query="shape",shape="PackagingGroup")`).
const PACKAGING_GROUP_MEMBERS: [&str; 8] = [
    "approximateAssetCount",
    "arn",
    "authorization",
    "createdAt",
    "domainName",
    "egressAccessLogs",
    "id",
    "tags",
];

/// The ids `ListPackagingGroups` reports for `query`, following `nextToken` to the end.
async fn list_all_group_ids(router: &MultiServiceRouter, query: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let mut token: Option<String> = None;
    // One more iteration than there can be pages of one group each, so a cursor that
    // fails to advance fails the test instead of looping forever.
    for _ in 0..64 {
        let mut path = format!("/packaging_groups?{query}");
        if let Some(token) = &token {
            path.push_str(&format!("&nextToken={token}"));
        }
        let (status, page) = call(router, "GET", &path, None).await;
        assert_eq!(status, 200, "ListPackagingGroups {path}: body {page}");
        for group in page["packagingGroups"].as_array().into_iter().flatten() {
            ids.push(group["id"].as_str().unwrap_or_default().to_string());
        }
        match page["nextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return ids,
        }
    }
    panic!("ListPackagingGroups never stopped returning a nextToken for query {query:?}");
}

/// A listed PackagingGroup is the stored record — the same one `DescribePackagingGroup`
/// returns, on every member the shape models, whichever optional members the create
/// carried and however many assets the group holds.
#[tokio::test]
async fn list_packaging_groups_reports_the_stored_record() {
    // (case, extra create members, assets to ingest into the group)
    let cases: Vec<(&str, Value, usize)> = vec![
        ("no optional members", json!({}), 0),
        (
            "tags only",
            json!({ "tags": { "env": "test", "team": "vod" } }),
            0,
        ),
        (
            "cdn authorization",
            json!({
                "authorization": {
                    "cdnIdentifierSecret":
                        format!("arn:aws:secretsmanager:{REGION}:{}:secret:cdn", account_id()),
                    "secretsRoleArn":
                        format!("arn:aws:iam::{}:role/MediaPackageVodSecrets", account_id()),
                },
            }),
            0,
        ),
        (
            "egress access logs",
            json!({ "egressAccessLogs": { "logGroupName": "/aws/MediaPackage/vod" } }),
            2,
        ),
        (
            "every optional member, with assets stored",
            json!({
                "authorization": {
                    "cdnIdentifierSecret":
                        format!("arn:aws:secretsmanager:{REGION}:{}:secret:cdn2", account_id()),
                    "secretsRoleArn":
                        format!("arn:aws:iam::{}:role/MediaPackageVodSecrets", account_id()),
                },
                "egressAccessLogs": { "logGroupName": "/aws/MediaPackage/vod-2" },
                "tags": { "env": "prod" },
            }),
            3,
        ),
    ];

    for (case, extra, assets) in cases {
        let router = router();
        let mut body = json!({ "id": "listed-group" });
        let Value::Object(members) = extra else {
            panic!("case {case:?}: the extra create members must be a JSON object");
        };
        for (key, value) in members {
            body[key] = value;
        }
        create_group_from(&router, "listed-group", body).await;
        for index in 0..assets {
            create_asset(&router, &format!("asset-{index}"), "listed-group").await;
        }

        let (status, page) = call(&router, "GET", "/packaging_groups", None).await;
        assert_eq!(status, 200, "case {case:?}: body {page}");
        let listed = page["packagingGroups"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        let [listed] = listed else {
            panic!("case {case:?}: expected exactly one listed group, got {page}");
        };
        let (status, described) =
            call(&router, "GET", "/packaging_groups/listed-group", None).await;
        assert_eq!(status, 200, "case {case:?}: body {described}");
        for member in PACKAGING_GROUP_MEMBERS {
            assert_eq!(
                listed[member], described[member],
                "case {case:?}: listed {member} must be the stored record's, got {}",
                listed[member]
            );
        }
        // Counted from the asset map at read time, like every other read of the group.
        assert_eq!(
            listed["approximateAssetCount"].as_i64(),
            Some(assets as i64),
            "case {case:?}: expected {assets} assets counted, got {}",
            listed["approximateAssetCount"]
        );
        // Nothing the listing read may change what the next caller sees.
        let (_, again) = call(&router, "GET", "/packaging_groups", None).await;
        assert_eq!(
            again["packagingGroups"], page["packagingGroups"],
            "case {case:?}: a second listing must report the same groups, got {}",
            again["packagingGroups"]
        );
    }
}

/// Paging reports every stored packaging group exactly once, in one stable order,
/// whatever the page size — and a cursor resumes strictly after the id the previous
/// page ended on. An empty store reports an empty collection and no cursor.
#[tokio::test]
async fn list_packaging_groups_pages_every_group_exactly_once() {
    // (total groups, maxResults per page)
    for (total, page_size) in [
        (0_usize, 1_usize),
        (1, 1),
        (3, 1),
        (3, 2),
        (5, 4),
        (4, 1000),
        (7, 3),
    ] {
        let router = router();
        let mut expected: Vec<String> = Vec::new();
        for index in 0..total {
            // Zero-padded so the id order the service pages in is unambiguous.
            let id = format!("group-{index:02}");
            create_group(&router, &id, None).await;
            expected.push(id);
        }
        expected.sort();

        let seen = list_all_group_ids(&router, &format!("maxResults={page_size}")).await;
        assert_eq!(
            seen, expected,
            "case total={total} maxResults={page_size}: every group exactly once, in id order"
        );
        assert_eq!(
            seen.iter().collect::<BTreeSet<_>>().len(),
            seen.len(),
            "case total={total} maxResults={page_size}: a page repeated a group: {seen:?}"
        );

        let (status, first) = call(
            &router,
            "GET",
            &format!("/packaging_groups?maxResults={page_size}"),
            None,
        )
        .await;
        assert_eq!(status, 200, "case maxResults={page_size}: body {first}");
        // The first page is no longer than the page size the caller asked for, and it
        // carries a cursor only when something is left to read.
        let first_len = first["packagingGroups"].as_array().map_or(0, Vec::len);
        assert!(
            first_len <= page_size,
            "case total={total} maxResults={page_size}: first page held {first_len} groups"
        );
        assert_eq!(
            first["nextToken"].is_null(),
            total <= page_size,
            "case total={total} maxResults={page_size}: a cursor only when a page is left, \
             got {}",
            first["nextToken"]
        );
        if total == 0 {
            assert_eq!(
                first["packagingGroups"],
                json!([]),
                "with nothing stored the collection is empty, got {first}"
            );
        }
    }
}

/// `ListPackagingGroups`'s `MaxResults` is accepted on each bound the model publishes
/// (`range(min: 1, max: 1000)`) and refused one past either, and a `NextToken` this
/// listing never issued — including one another listing issued — is refused as
/// unprocessable.
#[tokio::test]
async fn list_packaging_groups_max_results_and_next_token_follow_their_modelled_constraints() {
    let router = router();
    create_group(&router, "g-1", None).await;
    create_group(&router, "g-2", None).await;
    create_asset(&router, "a-1", "g-1").await;
    create_asset(&router, "a-2", "g-1").await;

    for accepted in [1, 2, 1000] {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/packaging_groups?maxResults={accepted}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "maxResults={accepted} is within the modelled range: body {body}"
        );
        let returned = body["packagingGroups"].as_array().map_or(0, Vec::len);
        assert_eq!(
            returned,
            accepted.min(2),
            "maxResults={accepted}: expected at most that many groups, got {returned}"
        );
    }

    for refused in [0, 1001] {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/packaging_groups?maxResults={refused}"),
            None,
        )
        .await;
        assert!(
            (400..500).contains(&status),
            "maxResults={refused} is outside the modelled range and must be refused, \
             got {status}: body {body}"
        );
    }

    // A cursor the ASSETS collection issued names an asset id, so it is not a resume
    // point in this collection: each listing's cursors are its own.
    let (status, assets_page) = call(&router, "GET", "/assets?maxResults=1", None).await;
    assert_eq!(status, 200, "the first assets page: body {assets_page}");
    let assets_cursor = assets_page["nextToken"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !assets_cursor.is_empty(),
        "with two assets and maxResults=1 the first assets page must carry a cursor: \
         {assets_page}"
    );

    // (case, nextToken) — every cursor this listing did not issue is refused.
    let forged: Vec<(&str, String)> = vec![
        ("not hex at all", "not-a-cursor".to_string()),
        ("hex, but no checksum prefix", hex::encode("g-1")),
        (
            "hex with a wrong checksum prefix",
            format!("00000000{}", hex::encode("g-1")),
        ),
        ("shorter than the checksum prefix", "00ff".to_string()),
        ("non-ascii", "🙈🙈🙈🙈🙈".to_string()),
        ("a cursor the assets collection issued", assets_cursor),
    ];
    for (case, token) in forged {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/packaging_groups?nextToken={token}"),
            None,
        )
        .await;
        assert_eq!(
            status, 422,
            "case {case:?}: a NextToken this listing never issued must be refused as \
             unprocessable, got {status}: body {body}"
        );
    }

    // ... and the cursor it DID issue resumes the listing.
    let (status, first) = call(&router, "GET", "/packaging_groups?maxResults=1", None).await;
    assert_eq!(status, 200, "the first page: body {first}");
    let issued = first["nextToken"].as_str().unwrap_or_default().to_string();
    assert!(
        !issued.is_empty(),
        "with two groups and maxResults=1 the first page must carry a cursor: {first}"
    );
    let (status, second) = call(
        &router,
        "GET",
        &format!("/packaging_groups?maxResults=1&nextToken={issued}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "the issued cursor must be accepted: body {second}"
    );
    assert_eq!(
        second["packagingGroups"][0]["id"].as_str(),
        Some("g-2"),
        "the issued cursor must resume after the first page, got {}",
        second["packagingGroups"]
    );
}

/// A packaging group is removed only while no child resource references it, and its
/// tags go with it.
///
/// A PackagingConfiguration and an Asset each name the group they belong to by
/// `PackagingGroupId` (both creates refuse one that resolves to no stored group), and
/// the service protects that reference: while any stored child still names the group,
/// `DeletePackagingGroup` is refused with the modelled
/// `UnprocessableEntityException` (422) and the group is left exactly as it was —
/// recorded as behavioural units
/// `mediapackage-vod.delete-packaging-group-with-dependent-configs-rejected` and
/// `mediapackage-vod.delete-packaging-group-with-attached-asset-rejected`. Delete the
/// dependents first and the identical delete succeeds, answering 204 (the status
/// `src/ops/delete_packaging_group.rs` documents against the model's 202), after which
/// the group reads back the modelled `NotFoundException` (404) and its tags are gone
/// with it. Nothing is cascaded: each child is removed by its own delete.
///
/// The rows span the dependent class (configurations, assets, both, neither), the
/// dependent count (none, one, several) and the packaging format each configuration was
/// created with. The full dependency table — the refusal's effect-freedom, its message,
/// the id forms, and a child of ANOTHER group protecting nothing — is
/// `tests/all/packaging_group_deletion.rs`.
#[tokio::test]
async fn a_packaging_group_is_deletable_only_while_nothing_references_it() {
    // (case, the package blocks of the configurations created in the group, how many
    // assets the group holds)
    let cases: Vec<(&str, Vec<Value>, usize)> = vec![
        ("no dependents, no assets", vec![], 0),
        ("no dependents, two assets", vec![], 2),
        (
            "one hls dependent",
            vec![json!({ "hlsPackage": { "hlsManifests": [{}] } })],
            0,
        ),
        (
            "one dash dependent, one asset",
            vec![json!({ "dashPackage": { "dashManifests": [{}] } })],
            1,
        ),
        (
            "three dependents, mixed formats",
            vec![
                json!({ "hlsPackage": { "hlsManifests": [{ "manifestName": "primary" }] } }),
                json!({ "cmafPackage": { "hlsManifests": [{}] } }),
                json!({ "mssPackage": { "mssManifests": [{}] } }),
            ],
            0,
        ),
    ];

    for (case, packages, assets) in cases {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = router_over(engine.clone());
        create_group(&router, "del-group", Some(json!({ "case": case }))).await;
        for (index, package) in packages.iter().enumerate() {
            create_config(&router, &format!("cfg-{index}"), "del-group", package.clone()).await;
        }
        for index in 0..assets {
            create_asset(&router, &format!("asset-{index}"), "del-group").await;
        }

        // While a child is stored the delete is refused and removes nothing.
        if !packages.is_empty() || assets > 0 {
            let (status, body) = call(&router, "DELETE", "/packaging_groups/del-group", None).await;
            assert_eq!(
                status, 422,
                "case {case:?}: DeletePackagingGroup with {} configuration(s) and {assets} \
                 asset(s) still stored must be refused as unprocessable, got {status} and \
                 body {body}",
                packages.len()
            );
            let (status, described) =
                call(&router, "GET", "/packaging_groups/del-group", None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the refused delete must leave the group readable, got \
                 {status} and body {described}"
            );
            // The dependents are deleted through their own operation, which is what
            // lifts the protection.
            for index in 0..assets {
                let (status, body) =
                    call(&router, "DELETE", &format!("/assets/asset-{index}"), None).await;
                assert_eq!(
                    status, 204,
                    "case {case:?}: DeleteAsset asset-{index}, got {status} and body {body}"
                );
            }
            for index in 0..packages.len() {
                let (status, body) = call(
                    &router,
                    "DELETE",
                    &format!("/packaging_configurations/cfg-{index}"),
                    None,
                )
                .await;
                assert_eq!(
                    status, 204,
                    "case {case:?}: DeletePackagingConfiguration cfg-{index}, got {status} \
                     and body {body}"
                );
            }
        }

        let (status, body) = call(&router, "DELETE", "/packaging_groups/del-group", None).await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected the recorded 204 No Content for a group holding no \
             child (the model declares 202 — see \
             `src/ops/delete_packaging_group.rs`), got {status} and body {body}"
        );
        // Nothing is cascaded: every child was removed by its own delete before the
        // group's, and the state the engine holds says so.
        {
            let store = engine.state::<vod::ComAmazonawsMediapackagevodMediapackagevodState>();
            let state = store.lock();
            let held_configs = state.packaging_configurations.len();
            let held_assets = state.assets.len();
            assert_eq!(
                held_configs, 0,
                "case {case:?}: every dependent was deleted before the group, \
                 {held_configs} configuration(s) left"
            );
            assert_eq!(
                held_assets, 0,
                "case {case:?}: every dependent was deleted before the group, \
                 {held_assets} asset(s) left"
            );
        }

        // Deleted by now in every row: the group is gone from state, so a read answers
        // the modelled 404 and so does deleting the same id again.
        let (status, body) = call(&router, "GET", "/packaging_groups/del-group", None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: DescribePackagingGroup after the delete, got {status} and body {body}"
        );
        let (status, body) = call(&router, "DELETE", "/packaging_groups/del-group", None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a second delete of the same id, got {status} and body {body}"
        );

        // The tag store is keyed by ARN, so the deleted group must leave no tags behind
        // for a group later created with the same id.
        let recreated = create_group(&router, "del-group", None).await;
        assert_eq!(
            recreated["tags"],
            json!({}),
            "case {case:?}: a re-created group must not inherit the deleted group's tags, got {}",
            recreated["tags"]
        );
    }
}
