// SPDX-License-Identifier: CC0-1.0

//! WHERE a packaging group's egress is announced — the address `DomainName` names on
//! every projection of a group.
//!
//! RULE under test: an address this service hands a caller to DIAL later names a host
//! this emulator answers on — so the `DomainName` of a packaging group is the endpoint
//! the creating request reached, and only a caller that resolves AWS's own hostnames,
//! talking to an emulator served on no port at all, is told AWS's own egress hostname.
//! `DomainName` is "The fully qualified domain name for Assets in the PackagingGroup"
//! (`smithy_query(query="doc",shape="com.amazonaws.mediapackagevod#PackagingGroup",member="DomainName",model_path="services/service-com-amazonaws-mediapackagevod-mediapackagevod/smithy-model/com-amazonaws-mediapackagevod-mediapackagevod.json")`),
//! which a player, a CDN origin configuration or a health probe joins an egress path
//! onto; a hostname this process neither resolves to nor listens on makes nothing the
//! emulator created retrievable through the address it published.
//!
//! The rows differ along what the rule generalises over: the shape of the emulator's own
//! address a client reaches it at (an IPv4 literal with and without a port, a loopback
//! name, a bracketed IPv6 literal), where the request carries that address (the `Host`
//! header or an absolute-form URI), and whether the emulator is served on a port at all.
//! The resource identity is varied too — one group per row — because the announced
//! address is a function of the endpoint rather than of the group.
//!
//! The five projections that report the member are each asserted, since each reaches the
//! wire on its own: `CreatePackagingGroup`, `DescribePackagingGroup`,
//! `ListPackagingGroups` (per element), `UpdatePackagingGroup` and `ConfigureLogs`. The
//! playback URLs beneath a group are asserted too: they are rooted on the same announced
//! origin, so the whole data plane of a group is announced at one address.

use serde_json::{json, Value};
use smithy_server_runtime::{EndpointUrl, MultiServiceRouter};

use service_com_amazonaws_mediapackagevod_mediapackagevod::state as vod;

use super::packaging_lifecycle::{call, call_raw_addressed, create_asset, create_config, router};
use super::success_path_projections::assert_egress_domain;

/// One address a client reaches this emulator at: the authority its request URI names and
/// the `Host` header it carries (`None` sends none at all, as an absolute-form request
/// does).
struct Addressing {
    case: &'static str,
    authority: &'static str,
    host_header: Option<&'static str>,
    /// The authority the announced origin must name, which is the one the request reached.
    announced: &'static str,
}

/// The addresses a client on the machine this emulator runs on reaches it at. Each is an
/// address rather than a service hostname, so each is the emulator's own endpoint.
const OWN_ENDPOINTS: &[Addressing] = &[
    Addressing {
        case: "an IPv4 literal with the data port",
        authority: "127.0.0.1:9297",
        host_header: Some("127.0.0.1:9297"),
        announced: "127.0.0.1:9297",
    },
    Addressing {
        case: "an IPv4 literal on the default port",
        authority: "127.0.0.1",
        host_header: Some("127.0.0.1"),
        announced: "127.0.0.1",
    },
    Addressing {
        case: "a loopback name",
        authority: "localhost:9297",
        host_header: Some("localhost:9297"),
        announced: "localhost:9297",
    },
    Addressing {
        case: "a bracketed IPv6 literal",
        authority: "[::1]:9297",
        host_header: Some("[::1]:9297"),
        announced: "[::1]:9297",
    },
    Addressing {
        case: "no Host header, the endpoint on an absolute-form URI",
        authority: "127.0.0.1:39355",
        host_header: None,
        announced: "127.0.0.1:39355",
    },
];

/// The scheme every origin this service announces carries
/// ([`vod::EGRESS_ORIGIN_PREFIX`]), so a row states the authority and not the whole URL.
fn origin(authority: &str) -> String {
    format!("{}{authority}", vod::EGRESS_ORIGIN_PREFIX)
}

/// Create a packaging group through one addressing, asserting it succeeded, and report
/// the projection the create answered with.
async fn create_group_at(
    router: &MultiServiceRouter,
    addressing: &Addressing,
    id: &str,
) -> Value {
    let response = call_raw_addressed(
        router,
        "POST",
        "/packaging_groups",
        Some(json!({ "id": id })),
        addressing.authority,
        addressing.host_header,
    )
    .await;
    let status = response.status().as_u16();
    let body = response.into_body();
    let parsed: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    assert_eq!(
        status, 200,
        "case {:?}: CreatePackagingGroup expected 200, got {status} {parsed}",
        addressing.case
    );
    parsed
}

/// The `domainName` a projection reports, or `"<unset>"` when it reports none.
fn domain_of(body: &Value) -> String {
    body.get("domainName")
        .and_then(Value::as_str)
        .unwrap_or("<unset>")
        .to_string()
}

/// Every projection of `id` that carries `domainName`, as `(the operation, the value)`:
/// the four READS and writes beyond the create that each put the member on the wire.
async fn projected_domains(router: &MultiServiceRouter, id: &str) -> Vec<(&'static str, String)> {
    let path = format!("/packaging_groups/{id}");

    let (status, described) = call(router, "GET", &path, None).await;
    assert_eq!(status, 200, "DescribePackagingGroup {id}: body {described}");

    let (status, listed) = call(router, "GET", "/packaging_groups", None).await;
    assert_eq!(status, 200, "ListPackagingGroups: body {listed}");
    let element = listed
        .get("packagingGroups")
        .and_then(Value::as_array)
        .and_then(|groups| {
            groups
                .iter()
                .find(|group| group.get("id").and_then(Value::as_str) == Some(id))
                .cloned()
        })
        .unwrap_or_else(|| panic!("ListPackagingGroups must report {id}, got {listed}"));

    let (status, updated) = call(router, "PUT", &path, Some(json!({}))).await;
    assert_eq!(status, 200, "UpdatePackagingGroup {id}: body {updated}");

    let (status, configured) = call(
        router,
        "PUT",
        &format!("{path}/configure_logs"),
        Some(json!({ "egressAccessLogs": { "logGroupName": "/aws/MediaPackage/vod" } })),
    )
    .await;
    assert_eq!(status, 200, "ConfigureLogs {id}: body {configured}");

    vec![
        ("DescribePackagingGroup", domain_of(&described)),
        ("ListPackagingGroups", domain_of(&element)),
        ("UpdatePackagingGroup", domain_of(&updated)),
        ("ConfigureLogs", domain_of(&configured)),
    ]
}

/// RULE — every projection of a group announces the endpoint the creating request
/// reached, whatever shape that address took and wherever the request carried it.
#[tokio::test]
async fn every_projection_announces_the_endpoint_the_group_was_created_through() {
    for (index, addressing) in OWN_ENDPOINTS.iter().enumerate() {
        let case = addressing.case;
        let router = router();
        let id = format!("announced-{index}");
        let expected = origin(addressing.announced);

        let created = create_group_at(&router, addressing, &id).await;
        let announced = domain_of(&created);
        assert_eq!(
            announced, expected,
            "case {case:?}: CreatePackagingGroup must announce the endpoint the request \
             reached, expected {expected:?}, got {announced:?}"
        );
        // The point of the rule: the announced address is not some host off this process.
        assert!(
            !announced.contains("amazonaws.com"),
            "case {case:?}: the announced origin must not name an AWS host, got {announced:?}"
        );

        for (operation, projected) in projected_domains(&router, &id).await {
            assert_eq!(
                projected, expected,
                "case {case:?}: {operation} must announce the same endpoint, expected \
                 {expected:?}, got {projected:?}"
            );
        }
    }
}

/// RULE — a caller that addressed one of AWS's own hostnames is announced the port this
/// emulator is served on, which is the one address this process answers on; only an
/// emulator served on no port at all announces AWS's own egress hostname, because it has
/// no own address to announce in its place.
#[tokio::test]
async fn a_caller_on_an_aws_hostname_is_announced_the_port_the_emulator_is_served_on() {
    // (case, the base URL `OperationContext::endpoint` reports, the expected origin)
    let served = "127.0.0.1:39355";
    let cases: Vec<(&str, Option<String>, Option<String>)> = vec![
        (
            "served on a data port",
            Some(origin(served)),
            Some(origin(served)),
        ),
        (
            "served on another port",
            Some(origin("[::1]:443")),
            Some(origin("[::1]:443")),
        ),
        // No own address to announce: the hostname AWS itself publishes stands.
        ("served on no port at all", None, None),
    ];

    for (index, (case, base, expected)) in cases.into_iter().enumerate() {
        let mut router = router();
        if let Some(base) = &base {
            router.set_url_base(
                EndpointUrl::parse(base).unwrap_or_else(|| panic!("case {case:?}: {base} parses")),
            );
        }
        let id = format!("aws-host-{index}");
        // `call` addresses this service's own regional hostname, as an SDK client that
        // resolves AWS's names does.
        let (status, created) = call(
            &router,
            "POST",
            "/packaging_groups",
            Some(json!({ "id": id })),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: create body {created}");

        let announced = domain_of(&created);
        match &expected {
            // An own address to announce: the announced origin is exactly it.
            Some(expected) => assert_eq!(
                &announced, expected,
                "case {case:?}: expected {expected:?}, got {announced:?}"
            ),
            // None: the announced origin is AWS's own egress hostname, asserted as the
            // STRUCTURE that form has rather than against the helper that builds it —
            // the TLS scheme, then `<per-group label>.egress.<service>.<region>.
            // amazonaws.com`.
            None => assert_egress_domain(case, &announced),
        }
        for (operation, projected) in projected_domains(&router, &id).await {
            assert_eq!(
                projected, announced,
                "case {case:?}: {operation} must announce the same origin, expected \
                 {announced:?}, got {projected:?}"
            );
        }
    }
}

/// RULE — every URL beneath a group is rooted on the group's announced origin, so a
/// client that follows an egress endpoint of an asset stays on the address the group was
/// announced at.
///
/// The rows differ in the packaging format, which is what decides the manifest the URL
/// ends in, so the assertion holds for every endpoint the group exposes rather than for
/// one.
#[tokio::test]
async fn the_playback_urls_beneath_a_group_are_rooted_on_the_announced_origin() {
    let addressing = &OWN_ENDPOINTS[0];
    let router = router();
    let group = "rooted";
    let created = create_group_at(&router, addressing, group).await;
    let announced = domain_of(&created);
    assert_eq!(
        announced,
        origin(addressing.announced),
        "the group must be announced at the endpoint the create reached, got {announced:?}"
    );

    // (configuration id, the package block it is created with)
    let packages: Vec<(&str, Value)> = vec![
        ("hls-cfg", json!({ "hlsPackage": { "hlsManifests": [{}] } })),
        ("dash-cfg", json!({ "dashPackage": { "dashManifests": [{}] } })),
        ("mss-cfg", json!({ "mssPackage": { "mssManifests": [{}] } })),
    ];
    for (id, package) in packages {
        create_config(&router, id, group, package).await;
    }
    let asset = create_asset(&router, "rooted-asset", group).await;

    let endpoints = asset
        .get("egressEndpoints")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("an asset must report its egress endpoints, got {asset}"));
    assert_eq!(
        endpoints.len(),
        3,
        "one endpoint per configuration of the group, got {asset}"
    );
    for endpoint in endpoints {
        let url = endpoint
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("an egress endpoint must report its URL, got {endpoint}"));
        let case = endpoint
            .get("packagingConfigurationId")
            .and_then(Value::as_str)
            .unwrap_or("<unset>");
        assert!(
            url.starts_with(&format!("{announced}/")),
            "case {case:?}: a playback URL must be rooted on the announced origin \
             {announced:?}, got {url:?}"
        );
    }
}
