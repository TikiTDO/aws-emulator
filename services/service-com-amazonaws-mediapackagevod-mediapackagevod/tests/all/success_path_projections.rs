// SPDX-License-Identifier: CC0-1.0

//! What this service PROJECTS on its success paths, and what its writes persist.
//!
//! Every test drives real HTTP requests through the router the production binary
//! builds, so what it asserts is the wire behaviour a client sees. Each test owns its
//! own engine, so no test observes another's state. The request harness
//! ([`super::packaging_lifecycle::call`] and friends) is shared with the lifecycle
//! suite rather than rebuilt here.
//!
//! The rules asserted, each over a SET of inputs rather than one recorded call:
//!
//! * a create answers with the SAME resource projection the matching read answers
//!   with — every member the service materialises included, counted or derived at
//!   answer time, not a subset of what the request carried;
//! * a packaging group's `DomainName` is an egress hostname of this service in the
//!   request's region, a function of the group rather than of the moment;
//! * `CreatePackagingConfiguration` resolves the optional settings of the chosen
//!   packaging format to the values it will package with, stores THOSE, and answers
//!   202 — so a read-back of the configuration equals the create answer;
//! * `ConfigureLogs` is a mutation of the group: the configured subscription is what
//!   every later read of the group reports;
//! * `DeletePackagingGroup` removes the group, so the following read is a modelled
//!   404 `NotFoundException` carrying a message and the `x-amzn-errortype` header;
//! * `ListAssets` reports every stored asset, filtered by `packagingGroupId` when the
//!   request names one, each entry carrying the modelled `AssetShallow` members.

use serde_json::{json, Value};

use super::packaging_lifecycle::{
    account_id, assert_projection, call, call_raw, create_asset, create_config, create_group,
    router, REGION,
};

/// The members `CreatePackagingGroup` and `DescribePackagingGroup` both project for
/// the same group. The two answers must agree member for member: a client that seeds
/// its view of a group from the create reply and one that reads the group back see the
/// same resource.
const GROUP_PROJECTION: [&str; 7] = [
    "approximateAssetCount",
    "arn",
    "authorization",
    "createdAt",
    "domainName",
    "egressAccessLogs",
    "id",
];

/// The members the modelled `AssetShallow` of a `ListAssets` page carries for an
/// asset created without a `ResourceId`: everything the service itself materialises.
/// `ResourceId` is NOT among them — it is reported only where the caller stated it,
/// which [`list_assets_reports_resource_id_only_where_the_caller_stated_it`] pins.
const ASSET_SHALLOW_PROJECTION: [&str; 6] = [
    "arn",
    "createdAt",
    "id",
    "packagingGroupId",
    "sourceArn",
    "sourceRoleArn",
];

/// The host labels an egress origin of this service is built from, asserted as a
/// structure rather than as one recorded string: the TLS scheme, then
/// `<group>.egress.<service name>.<region>.amazonaws.com`.
///
/// Shared with `super::announced_egress_domain`, which asserts the same structure for the
/// one case that still announces this form — a caller on an AWS hostname talking to an
/// emulator served on no port at all.
pub(crate) fn assert_egress_domain(case: &str, domain: &str) {
    use service_com_amazonaws_mediapackagevod_mediapackagevod::state as vod;

    let Some(host) = domain.strip_prefix(vod::EGRESS_ORIGIN_PREFIX) else {
        panic!(
            "case {case:?}: expected an egress origin under {:?}, got {domain:?}",
            vod::EGRESS_ORIGIN_PREFIX
        );
    };
    let labels: Vec<&str> = host.split('.').collect();
    assert_eq!(
        labels.len(),
        6,
        "case {case:?}: expected a 6-label egress hostname, got {host:?}"
    );
    assert_eq!(
        &labels[1..],
        &["egress", "mediapackage-vod", REGION, "amazonaws", "com"],
        "case {case:?}: expected an egress hostname of this service in {REGION}, got {host:?}"
    );
    assert!(
        !labels[0].is_empty()
            && labels[0]
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit()),
        "case {case:?}: the first label of {host:?} must be a non-empty DNS label, got {:?}",
        labels[0]
    );
}

/// A create answers with the same group projection the following describe answers
/// with — `approximateAssetCount` (0 for a new group) and a `DomainName` of this
/// service's egress form included, whatever the request did and did not carry.
#[tokio::test]
async fn create_packaging_group_projects_the_whole_group_representation() {
    // (case, the request's optional members beyond `id`)
    let cases: Vec<(&str, Value)> = vec![
        ("no optional members", json!({})),
        ("tags only", json!({ "tags": { "env": "test", "team": "video" } })),
        ("empty tag map", json!({ "tags": {} })),
        (
            "egress access logs only",
            json!({ "egressAccessLogs": { "logGroupName": "/aws/MediaPackage/vod-probe" } }),
        ),
        (
            "cdn authorization only",
            json!({
                "authorization": {
                    "cdnIdentifierSecret": format!(
                        "arn:aws:secretsmanager:{REGION}:{}:secret:cdn-AbCdEf",
                        account_id()
                    ),
                    "secretsRoleArn": format!(
                        "arn:aws:iam::{}:role/MediaPackageVodSecrets",
                        account_id()
                    ),
                },
            }),
        ),
        (
            "every optional member",
            json!({
                "tags": { "env": "prod" },
                "egressAccessLogs": { "logGroupName": "/aws/MediaPackage/all" },
                "authorization": {
                    "cdnIdentifierSecret": format!(
                        "arn:aws:secretsmanager:{REGION}:{}:secret:cdn-ZyXwVu",
                        account_id()
                    ),
                    "secretsRoleArn": format!(
                        "arn:aws:iam::{}:role/MediaPackageVodSecrets",
                        account_id()
                    ),
                },
            }),
        ),
    ];

    for (index, (case, optional)) in cases.into_iter().enumerate() {
        let router = router();
        let id = format!("group-{index}");
        let mut body = json!({ "id": id });
        let Value::Object(members) = optional else {
            panic!("case {case:?}: the optional members must be a JSON object");
        };
        for (key, value) in members {
            body[key] = value;
        }

        let (status, created) = call(&router, "POST", "/packaging_groups", Some(body)).await;
        assert_eq!(status, 200, "case {case:?}: body {created}");
        assert_eq!(
            created["approximateAssetCount"].as_i64(),
            Some(0),
            "case {case:?}: a brand-new group holds no assets, got {}",
            created["approximateAssetCount"]
        );
        assert_egress_domain(case, created["domainName"].as_str().unwrap_or_default());
        assert_eq!(
            created["arn"].as_str(),
            Some(
                format!(
                    "arn:aws:mediapackage-vod:{REGION}:{}:packaging-groups/{id}",
                    account_id()
                )
                .as_str()
            ),
            "case {case:?}: expected the computed ARN, got {}",
            created["arn"]
        );

        let (status, described) =
            call(&router, "GET", &format!("/packaging_groups/{id}"), None).await;
        assert_eq!(status, 200, "case {case:?}: describe body {described}");
        for member in GROUP_PROJECTION {
            assert_eq!(
                created[member], described[member],
                "case {case:?}: the create and the read must agree on {member}, create said \
                 {} and the read said {}",
                created[member], described[member]
            );
        }
        assert_eq!(
            created["tags"], described["tags"],
            "case {case:?}: the create and the read must agree on tags, create said {} and \
             the read said {}",
            created["tags"], described["tags"]
        );
    }
}

/// `CreatePackagingConfiguration` answers 202 and resolves the package block's
/// optional settings to the values the service packages with: an absent member comes
/// back as the service's default, a member the request supplied comes back as sent,
/// and a read of the configuration reports the identical block.
#[tokio::test]
async fn create_packaging_configuration_materialises_the_server_side_defaults() {
    use service_com_amazonaws_mediapackagevod_mediapackagevod::state as vod;

    // The defaults the service materialises, as JSON values, by the member path they
    // appear at inside the package block of the format under test.
    let hls_manifest_defaults = json!({
        "adMarkers": vod::DEFAULT_AD_MARKERS,
        "includeIframeOnlyStream": false,
        "programDateTimeIntervalSeconds": vod::DEFAULT_PROGRAM_DATE_TIME_INTERVAL_SECONDS,
        "repeatExtXKey": false,
    });

    // (case, the package block as sent, the members the answer must carry)
    //
    // `expected` is a JSON object mirroring the package block's own shape; every leaf
    // in it is compared against the same path in the answer, so a row states both the
    // defaults it expects materialised and the values it expects echoed back.
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "hls, nothing optional supplied",
            json!({ "hlsPackage": { "hlsManifests": [{}] } }),
            json!({
                "hlsPackage": {
                    "includeDvbSubtitles": false,
                    "useAudioRenditionGroup": false,
                    "hlsManifests": [hls_manifest_defaults.clone()],
                },
            }),
        ),
        (
            "hls, two manifests, one of them configured",
            json!({
                "hlsPackage": {
                    "segmentDurationSeconds": 6,
                    "useAudioRenditionGroup": true,
                    "hlsManifests": [
                        {},
                        {
                            "manifestName": "iframe",
                            "adMarkers": "PASSTHROUGH",
                            "includeIframeOnlyStream": true,
                            "programDateTimeIntervalSeconds": 60,
                            "repeatExtXKey": true,
                        },
                    ],
                },
            }),
            json!({
                "hlsPackage": {
                    "includeDvbSubtitles": false,
                    "useAudioRenditionGroup": true,
                    "segmentDurationSeconds": 6,
                    "hlsManifests": [
                        hls_manifest_defaults.clone(),
                        {
                            "manifestName": "iframe",
                            "adMarkers": "PASSTHROUGH",
                            "includeIframeOnlyStream": true,
                            "programDateTimeIntervalSeconds": 60,
                            "repeatExtXKey": true,
                        },
                    ],
                },
            }),
        ),
        (
            "dash, nothing optional supplied",
            json!({ "dashPackage": { "dashManifests": [{}] } }),
            json!({
                "dashPackage": {
                    "includeEncoderConfigurationInSegments": false,
                    "includeIframeOnlyStream": false,
                    "periodTriggers": [],
                    "segmentTemplateFormat": vod::DEFAULT_SEGMENT_TEMPLATE_FORMAT,
                    "dashManifests": [{
                        "minBufferTimeSeconds": vod::DEFAULT_MIN_BUFFER_TIME_SECONDS,
                        "profile": vod::DEFAULT_DASH_PROFILE,
                        "scteMarkersSource": vod::DEFAULT_SCTE_MARKERS_SOURCE,
                    }],
                },
            }),
        ),
        (
            "dash, every defaulted member supplied with a non-default value",
            json!({
                "dashPackage": {
                    "includeEncoderConfigurationInSegments": true,
                    "includeIframeOnlyStream": true,
                    "periodTriggers": ["ADS"],
                    "segmentTemplateFormat": "TIME_WITH_TIMELINE",
                    "dashManifests": [{
                        "minBufferTimeSeconds": 4,
                        "profile": "HBBTV_1_5",
                        "scteMarkersSource": "MANIFEST",
                    }],
                },
            }),
            json!({
                "dashPackage": {
                    "includeEncoderConfigurationInSegments": true,
                    "includeIframeOnlyStream": true,
                    "periodTriggers": ["ADS"],
                    "segmentTemplateFormat": "TIME_WITH_TIMELINE",
                    "dashManifests": [{
                        "minBufferTimeSeconds": 4,
                        "profile": "HBBTV_1_5",
                        "scteMarkersSource": "MANIFEST",
                    }],
                },
            }),
        ),
        (
            "cmaf, nothing optional supplied",
            json!({ "cmafPackage": { "hlsManifests": [{}] } }),
            json!({
                "cmafPackage": {
                    "includeEncoderConfigurationInSegments": false,
                    "hlsManifests": [hls_manifest_defaults.clone()],
                },
            }),
        ),
        (
            "cmaf, encoder configuration asked for",
            json!({
                "cmafPackage": {
                    "includeEncoderConfigurationInSegments": true,
                    "hlsManifests": [{ "manifestName": "primary" }],
                },
            }),
            json!({
                "cmafPackage": {
                    "includeEncoderConfigurationInSegments": true,
                    "hlsManifests": [{
                        "manifestName": "primary",
                        "adMarkers": vod::DEFAULT_AD_MARKERS,
                        "includeIframeOnlyStream": false,
                        "programDateTimeIntervalSeconds":
                            vod::DEFAULT_PROGRAM_DATE_TIME_INTERVAL_SECONDS,
                        "repeatExtXKey": false,
                    }],
                },
            }),
        ),
        (
            // Microsoft Smooth carries no member the service materialises a default
            // for, so its block comes back exactly as sent.
            "mss, nothing optional supplied",
            json!({ "mssPackage": { "mssManifests": [{}] } }),
            json!({ "mssPackage": { "mssManifests": [{}] } }),
        ),
    ];

    for (index, (case, package, expected)) in cases.into_iter().enumerate() {
        let router = router();
        create_group(&router, "g", None).await;
        let id = format!("cfg-{index}");
        let mut body = json!({ "id": id, "packagingGroupId": "g" });
        let Value::Object(members) = package else {
            panic!("case {case:?}: the package block must be a JSON object");
        };
        for (key, value) in members {
            body[key] = value;
        }

        let (status, created) =
            call(&router, "POST", "/packaging_configurations", Some(body)).await;
        assert_eq!(
            status, 202,
            "case {case:?}: a successful create is accepted (202), got {status}, body {created}"
        );
        assert_projection(case, "the create answer", &expected, &created);

        // The resolved block is what was STORED, so the read reports it too.
        let (status, described) =
            call(&router, "GET", &format!("/packaging_configurations/{id}"), None).await;
        assert_eq!(status, 200, "case {case:?}: describe body {described}");
        assert_projection(case, "the read-back", &expected, &described);
        for member in ["arn", "createdAt", "id", "packagingGroupId", "tags"] {
            assert_eq!(
                created[member], described[member],
                "case {case:?}: the create and the read must agree on {member}, create said \
                 {} and the read said {}",
                created[member], described[member]
            );
        }
    }
}

/// `ConfigureLogs` states the group's egress-access-log subscription: the group it
/// answers with, and every later read of that group, report what was configured —
/// including the configuration that switches logging off again.
#[tokio::test]
async fn configure_logs_is_persisted_and_reported_by_every_later_read() {
    let router = router();
    create_group(&router, "g", Some(json!({ "env": "test" }))).await;

    // (case, the request body, the egressAccessLogs the group must then report)
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "a log group under the service's own path",
            json!({ "egressAccessLogs": { "logGroupName": "/aws/MediaPackage/vod-probe" } }),
            json!({ "logGroupName": "/aws/MediaPackage/vod-probe" }),
        ),
        (
            "re-pointed at another log group",
            json!({ "egressAccessLogs": { "logGroupName": "/custom/egress-logs" } }),
            json!({ "logGroupName": "/custom/egress-logs" }),
        ),
        (
            "a log group name that is not a path",
            json!({ "egressAccessLogs": { "logGroupName": "egress-logs" } }),
            json!({ "logGroupName": "egress-logs" }),
        ),
        (
            "an empty block subscribes to no log group",
            json!({ "egressAccessLogs": {} }),
            json!({}),
        ),
        (
            "no block at all switches the subscription off",
            json!({}),
            Value::Null,
        ),
    ];

    for (case, body, expected) in cases {
        let (status, configured) = call(
            &router,
            "PUT",
            "/packaging_groups/g/configure_logs",
            Some(body),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: body {configured}");
        assert_eq!(
            configured["egressAccessLogs"], expected,
            "case {case:?}: ConfigureLogs must answer with the configured subscription, got {}",
            configured["egressAccessLogs"]
        );

        let (status, described) = call(&router, "GET", "/packaging_groups/g", None).await;
        assert_eq!(status, 200, "case {case:?}: describe body {described}");
        assert_eq!(
            described["egressAccessLogs"], expected,
            "case {case:?}: the group must report the configured subscription on every read, \
             got {}",
            described["egressAccessLogs"]
        );
        // Configuring logs changes the subscription and nothing else about the group.
        for member in ["approximateAssetCount", "arn", "createdAt", "domainName", "id", "tags"] {
            assert_eq!(
                configured[member], described[member],
                "case {case:?}: ConfigureLogs must not have changed {member}, it answered {} \
                 and the read said {}",
                configured[member], described[member]
            );
        }
    }

    // A group that does not exist cannot have its logs configured.
    let (status, body) = call(
        &router,
        "PUT",
        "/packaging_groups/nope/configure_logs",
        Some(json!({ "egressAccessLogs": { "logGroupName": "/aws/MediaPackage/x" } })),
    )
    .await;
    assert_eq!(status, 404, "ConfigureLogs on an unknown group: body {body}");
    assert!(
        body["message"].as_str().is_some_and(|m| m.contains("nope")),
        "ConfigureLogs on an unknown group must say which group, got {}",
        body["message"]
    );
}

/// `DeletePackagingGroup` removes the group: the delete answers 204 No Content (the
/// status the wire recordings show, not the 202 the model declares), and
/// the following read is a 404 `NotFoundException` with a message and the
/// `x-amzn-errortype` header — whatever the group held when it was deleted.
///
/// A PackagingConfiguration or an Asset still naming the group PROTECTS it (the delete
/// is then refused with 422 — see `tests/all/packaging_group_deletion.rs`), so the rows
/// that create one delete it first; this test is about what the accepted delete removes.
#[tokio::test]
async fn delete_packaging_group_makes_every_later_read_not_found() {
    // (case, what the group holds when it is deleted)
    let cases: [&str; 4] = [
        "an empty group",
        "a group with a packaging configuration",
        "a group with an asset",
        "a group with both",
    ];

    for case in cases {
        let router = router();
        // A distinctive id per row, so the 404's message assertion below pins the id
        // the request asked for rather than any word that happens to share a letter
        // with it.
        let group = format!("vod-group-{}", case.replace(' ', "-"));
        create_group(&router, &group, Some(json!({ "env": "test" }))).await;
        if case.contains("configuration") || case.contains("both") {
            create_config(
                &router,
                "cfg",
                &group,
                json!({ "hlsPackage": { "hlsManifests": [{}] } }),
            )
            .await;
        }
        if case.contains("asset") || case.contains("both") {
            create_asset(&router, "a", &group).await;
        }

        // Each dependent is removed through its own operation, which is what lets the
        // group's delete be accepted at all.
        if case.contains("configuration") || case.contains("both") {
            let (status, body) = call(&router, "DELETE", "/packaging_configurations/cfg", None).await;
            assert_eq!(
                status, 204,
                "case {case:?}: DeletePackagingConfiguration cfg, got {status}, body {body}"
            );
        }
        if case.contains("asset") || case.contains("both") {
            let (status, body) = call(&router, "DELETE", "/assets/a", None).await;
            assert_eq!(
                status, 204,
                "case {case:?}: DeleteAsset a, got {status}, body {body}"
            );
        }

        let path = format!("/packaging_groups/{group}");
        let (status, body) = call(&router, "DELETE", &path, None).await;
        assert_eq!(
            status, 204,
            "case {case:?}: DeletePackagingGroup answers 204 No Content, got {status}, \
             body {body}"
        );
        // The operation models no output members and answers 204 No Content, which has
        // no body to carry the framework's rendering of an empty output struct — see
        // `src/service.rs` and `tests/all/empty_response_framing.rs`.
        assert_eq!(
            body,
            Value::Null,
            "case {case:?}: a 204 DeletePackagingGroup carries no body, got {body}"
        );

        // The read after the delete: 404, the modelled error type on the wire, and a
        // message naming what was asked for.
        let response = call_raw(&router, "GET", &path, None).await;
        assert_eq!(
            response.status().as_u16(),
            404,
            "case {case:?}: the group must be gone from every later read"
        );
        assert_eq!(
            response
                .headers()
                .get("x-amzn-errortype")
                .and_then(|value| value.to_str().ok()),
            Some("NotFoundException"),
            "case {case:?}: expected the modelled error type on the wire, got {:?}",
            response.headers().get("x-amzn-errortype")
        );
        let body: Value = serde_json::from_slice(&response.into_body()).unwrap_or(Value::Null);
        assert!(
            body["message"]
                .as_str()
                .is_some_and(|message| message.contains(&group)),
            "case {case:?}: the 404 must carry a message naming {group:?}, got {body}"
        );

        // And the delete is not idempotent in state terms: the second one 404s too.
        let (status, body) = call(&router, "DELETE", &path, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: deleting the group twice must report it gone, body {body}"
        );
    }
}

/// One row of [`list_assets_projects_every_stored_asset`]: the case name, how many
/// assets each group holds, the `packagingGroupId` filter to list with (`None` for an
/// unfiltered listing), and the asset ids the listing must then report.
type ListAssetsCase<'a> = (
    &'a str,
    Vec<(&'a str, usize)>,
    Option<&'a str>,
    Vec<&'a str>,
);

/// `ListAssets` reports every stored asset with the modelled `AssetShallow` members,
/// filtered by `packagingGroupId` when the request names one.
#[tokio::test]
async fn list_assets_projects_every_stored_asset() {
    // (case, assets per group, the filter to list with, the ids expected)
    let cases: Vec<ListAssetsCase<'_>> = vec![
        ("one asset, filtered by its group", vec![("g1", 1)], Some("g1"), vec!["g1-0"]),
        ("one asset, no filter", vec![("g1", 1)], None, vec!["g1-0"]),
        (
            "two groups, filtered by the first",
            vec![("g1", 2), ("g2", 3)],
            Some("g1"),
            vec!["g1-0", "g1-1"],
        ),
        (
            "two groups, filtered by the second",
            vec![("g1", 2), ("g2", 3)],
            Some("g2"),
            vec!["g2-0", "g2-1", "g2-2"],
        ),
        (
            "two groups, no filter",
            vec![("g1", 2), ("g2", 1)],
            None,
            vec!["g1-0", "g1-1", "g2-0"],
        ),
        (
            "a group holding none",
            vec![("g1", 1), ("g2", 0)],
            Some("g2"),
            vec![],
        ),
        ("a filter naming no group at all", vec![("g1", 1)], Some("absent"), vec![]),
    ];

    for (case, groups, filter, expected_ids) in cases {
        let router = router();
        for (group, count) in &groups {
            create_group(&router, group, None).await;
            for index in 0..*count {
                create_asset(&router, &format!("{group}-{index}"), group).await;
            }
        }

        let path = match filter {
            Some(group) => format!("/assets?packagingGroupId={group}"),
            None => "/assets".to_string(),
        };
        let (status, page) = call(&router, "GET", &path, None).await;
        assert_eq!(status, 200, "case {case:?}: body {page}");
        let listed = page["assets"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            listed
                .iter()
                .map(|asset| asset["id"].as_str().unwrap_or_default().to_string())
                .collect::<Vec<_>>(),
            expected_ids
                .iter()
                .map(|id| (*id).to_string())
                .collect::<Vec<_>>(),
            "case {case:?}: expected exactly the stored assets, got {}",
            page["assets"]
        );

        for asset in &listed {
            for member in ASSET_SHALLOW_PROJECTION {
                assert!(
                    asset[member].as_str().is_some_and(|value| !value.is_empty()),
                    "case {case:?}: the listed asset must carry {member}, got {asset}"
                );
            }
            assert!(
                asset["tags"].is_object(),
                "case {case:?}: the listed asset must carry its tags, got {asset}"
            );
            // The shallow projection is the same resource the single read reports.
            let id = asset["id"].as_str().unwrap_or_default();
            let (status, described) = call(&router, "GET", &format!("/assets/{id}"), None).await;
            assert_eq!(status, 200, "case {case:?}: DescribeAsset body {described}");
            for member in ASSET_SHALLOW_PROJECTION {
                assert_eq!(
                    asset[member], described[member],
                    "case {case:?}: the listing and the read must agree on {member} of {id}, \
                     the listing said {} and the read said {}",
                    asset[member], described[member]
                );
            }
        }
    }
}

/// `ResourceId` is the caller's own id for the content ("The resource ID to include in
/// SPEKE key requests"), so every projection of an asset reports it exactly where the
/// request stated it and nowhere else — the service never mints one.
#[tokio::test]
async fn list_assets_reports_resource_id_only_where_the_caller_stated_it() {
    // (case, the ResourceId the create request carries, what every read must report)
    let cases: Vec<(&str, Option<&str>, Value)> = vec![
        ("no ResourceId at all", None, Value::Null),
        ("a ResourceId of its own", Some("speke-resource-7"), json!("speke-resource-7")),
        (
            "a ResourceId that repeats the asset's id",
            Some("asset-2"),
            json!("asset-2"),
        ),
    ];

    for (index, (case, resource_id, expected)) in cases.into_iter().enumerate() {
        let router = router();
        create_group(&router, "g", None).await;
        let id = format!("asset-{index}");
        let mut body = json!({
            "id": id,
            "packagingGroupId": "g",
            "sourceArn": format!("arn:aws:s3:::test-bucket/{id}.smil"),
            "sourceRoleArn": format!("arn:aws:iam::{}:role/MediaPackageVod", account_id()),
        });
        if let Some(resource_id) = resource_id {
            body["resourceId"] = json!(resource_id);
        }

        let (status, created) = call(&router, "POST", "/assets", Some(body)).await;
        assert_eq!(status, 200, "case {case:?}: body {created}");
        assert_eq!(
            created["resourceId"], expected,
            "case {case:?}: CreateAsset must report {expected}, got {}",
            created["resourceId"]
        );

        let (_, described) = call(&router, "GET", &format!("/assets/{id}"), None).await;
        assert_eq!(
            described["resourceId"], expected,
            "case {case:?}: DescribeAsset must report {expected}, got {}",
            described["resourceId"]
        );
        let (_, page) = call(&router, "GET", "/assets?packagingGroupId=g", None).await;
        assert_eq!(
            page["assets"][0]["resourceId"], expected,
            "case {case:?}: the listing must report {expected}, got {}",
            page["assets"][0]["resourceId"]
        );
    }
}
