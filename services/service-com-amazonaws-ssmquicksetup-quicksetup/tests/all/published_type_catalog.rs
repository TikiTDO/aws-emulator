// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the TYPE CATALOG this service publishes.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization and the handlers — so what is asserted is what a signed AWS SDK request
//! observes.
//!
//! The rule under test: `ListQuickSetupTypes` is the discovery call, so the catalog it
//! publishes is the whole set of configuration types this service offers, ordered by
//! `Type`, each entry reporting the newest version of that type's own published history —
//! and what it publishes is what the write paths accept, so every type it lists can be
//! created at the `LatestVersion` it advertises. A type the catalog does not list cannot
//! be created at all; a version it does not publish for a type cannot be deployed.
//!
//! The published rows are written out in [`PUBLISHED_CATALOG`] (which names its source),
//! so this suite pins the catalog DATA as well as the RELATIONSHIP between the catalog and
//! the operations that read it: membership and version strings are catalog data, versioned
//! out of band from the model, which constrains neither. Entries the discovery call is
//! usually exercised with are not treated specially: every published type is driven
//! through the create and read back.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_ssmquicksetup_quicksetup::state::{
    quick_setup_type_entry, type_version_order, QUICK_SETUP_TYPE_CATALOG,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for.
const REGION: &str = "us-west-2";

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ssmquicksetup_quicksetup::EmulatedService::new(engine),
    );
    router
}

/// A request signed for `ssm-quicksetup` in [`REGION`].
fn request(method: &str, uri: &str, body: Option<&Value>) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("ssm-quicksetup.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDQUICKSETUP/20260101/{REGION}/ssm-quicksetup/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(method, uri, body)).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    (status, parsed)
}

/// Percent-encode a value for use as one `@httpLabel` path segment.
fn label(value: &str) -> String {
    value.replace('/', "%2F")
}

/// The `(Type, LatestVersion)` pairs `GET /listQuickSetupTypes` publishes, in the order
/// the response carries them.
async fn published(router: &MultiServiceRouter) -> Vec<(String, String)> {
    let (status, answer) = call(router, "GET", "/listQuickSetupTypes", None).await;
    assert_eq!(
        status, 200,
        "ListQuickSetupTypes must answer 200, got {status} / {answer:?}"
    );
    answer
        .get("QuickSetupTypeList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a QuickSetupTypeList, got {answer:?}"))
        .iter()
        .map(|entry| {
            let type_ = entry
                .get("Type")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("an entry must report its Type, got {entry:?}"));
            let version = entry
                .get("LatestVersion")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("an entry must report its LatestVersion, got {entry:?}")
                });
            (type_.to_string(), version.to_string())
        })
        .collect()
}

/// The whole catalog this service publishes, as `(Type, LatestVersion)` pairs in the
/// order the discovery call must serve them.
///
/// Written out here rather than read off `crate::state::QUICK_SETUP_TYPE_CATALOG`, so the
/// published DATA is under test and not only the handler's agreement with the table it
/// maps: a table that lost a type or fell behind on a version fails here. Source: the
/// catalog `ListQuickSetupTypes` answers for this service — membership and version
/// strings are catalog data the model constrains in no way (`smithy_query(query="shape",
/// shape="QuickSetupTypeOutput")` declares neither an enum nor a pattern), so the
/// published response is their only authority.
const PUBLISHED_CATALOG: &[(&str, &str)] = &[
    ("AWSQuickSetupType-CFGCPacks", "3.0"),
    ("AWSQuickSetupType-CFGRecording", "3.1"),
    ("AWSQuickSetupType-CWASetup", "1.3"),
    ("AWSQuickSetupType-DHMC", "2.1"),
    ("AWSQuickSetupType-DevOpsGuru", "3.0"),
    ("AWSQuickSetupType-Distributor", "4.1"),
    ("AWSQuickSetupType-JITNA", "1.1"),
    ("AWSQuickSetupType-PatchPolicy", "2.6"),
    ("AWSQuickSetupType-ResourceExplorer", "2.2"),
    ("AWSQuickSetupType-SSM", "3.0"),
    ("AWSQuickSetupType-SSMChangeMgr", "2.2"),
    ("AWSQuickSetupType-SSMHostMgmt", "5.1"),
    ("AWSQuickSetupType-SSMOpsCenter", "1.6"),
    ("AWSQuickSetupType-Scheduler", "4.0"),
];

/// The discovery call publishes exactly the catalog the service offers: every type it
/// supports, each with the version it publishes as that type's latest, in `Type` order —
/// and the same list on a second read, because the catalog is service data rather than
/// per-account state.
#[tokio::test]
async fn the_whole_published_catalog_is_served_in_type_order() {
    let router = emulator();
    let listed = published(&router).await;

    let expected: Vec<(String, String)> = PUBLISHED_CATALOG
        .iter()
        .map(|(type_, version)| ((*type_).to_string(), (*version).to_string()))
        .collect();
    assert_eq!(
        listed, expected,
        "the published catalog must be served in full, in Type order: expected \
         {expected:?}, got {listed:?}"
    );
    // The order the rows above are written in is `Type` order, so the table itself cannot
    // pin a wrong order: it is checked rather than assumed.
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for (index, (type_, _)) in PUBLISHED_CATALOG.iter().enumerate() {
        assert!(
            seen.insert(type_),
            "entry {index}: {type_:?} must be published once"
        );
        if let Some((previous, _)) = PUBLISHED_CATALOG.get(index.wrapping_sub(1)) {
            assert!(
                previous < type_,
                "entry {index}: expected Type order, but {previous:?} precedes {type_:?}"
            );
        }
    }

    // Every published version is one that type's own history offers, and the newest of
    // it: the catalog cannot advertise a version the write paths would refuse.
    for (type_, version) in PUBLISHED_CATALOG {
        let entry = quick_setup_type_entry(type_)
            .unwrap_or_else(|| panic!("case {type_}: the type must be one the service offers"));
        let newest = entry
            .versions
            .iter()
            .copied()
            .max_by_key(|published| {
                type_version_order(published)
                    .unwrap_or_else(|| panic!("case {type_}: {published:?} must be a numbered version"))
            })
            .unwrap_or_else(|| panic!("case {type_}: a type must publish at least one version"));
        assert_eq!(
            &newest, version,
            "case {type_}: expected the newest version of its own history {:?}, got {version:?}",
            entry.versions
        );
    }

    let again = published(&router).await;
    assert_eq!(
        again, listed,
        "two reads of the catalog must publish the same list, got {again:?} then {listed:?}"
    );
}

/// Every type the catalog publishes can be created at the `LatestVersion` it advertises,
/// and the created manager reports back that type and that version: the discovery call
/// and the write path read ONE catalog, so a client that pins what it discovered is never
/// refused for it.
#[tokio::test]
async fn every_published_type_can_be_created_at_the_version_the_catalog_advertises() {
    let router = emulator();
    let listed = published(&router).await;
    assert!(
        !listed.is_empty(),
        "the catalog must publish at least one type"
    );

    for (type_, version) in &listed {
        let case = format!("{type_} at {version}");
        let (status, answer) = call(
            &router,
            "POST",
            "/configurationManager",
            Some(&json!({
                "Name": "catalog",
                "ConfigurationDefinitions": [{
                    "Type": type_,
                    "TypeVersion": version,
                    // Every parameter that type's own documented table requires, plus
                    // somewhere to deploy to and the Region to deploy into: no case here
                    // is about those requirements.
                    "Parameters": super::type_parameter_fixtures::with_required_parameters(
                        type_,
                        json!({
                            "TargetOrganizationalUnits": "ou-1",
                            "TargetRegions": REGION,
                        }),
                    ),
                }],
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: a definition of a published type at its published LatestVersion \
             must be accepted, got {status} / {answer:?}"
        );
        let manager_arn = answer
            .get("ManagerArn")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case}: expected a ManagerArn, got {answer:?}"))
            .to_string();

        let (status, read) = call(
            &router,
            "GET",
            &format!("/configurationManager/{}", label(&manager_arn)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: GetConfigurationManager must succeed, got {status} / {read:?}"
        );
        let stored = read
            .get("ConfigurationDefinitions")
            .and_then(Value::as_array)
            .and_then(|definitions| definitions.first())
            .cloned()
            .unwrap_or_else(|| panic!("case {case}: expected one stored definition, got {read:?}"));
        assert_eq!(
            stored.get("Type").and_then(Value::as_str),
            Some(type_.as_str()),
            "case {case}: expected the type the create named, got {stored:?}"
        );
        assert_eq!(
            stored.get("TypeVersion").and_then(Value::as_str),
            Some(version.as_str()),
            "case {case}: expected the version the catalog advertises, got {stored:?}"
        );
    }
}

/// A type the catalog does not publish names no service-owned template, so a definition
/// naming one is refused — including a value that is only a near-miss of a published
/// type, which is what tells the membership check apart from a shape check.
#[tokio::test]
async fn a_type_the_catalog_does_not_publish_cannot_be_created() {
    let router = emulator();
    let published_type = QUICK_SETUP_TYPE_CATALOG
        .last()
        .expect("the catalog publishes at least one type")
        .type_;

    let rows: Vec<(&str, String)> = vec![
        (
            "a type the catalog does not list",
            "AWSQuickSetupType-NoSuchTemplate".to_string(),
        ),
        (
            "a published type in another case",
            published_type.to_ascii_uppercase(),
        ),
        (
            "a published type with a trailing character",
            format!("{published_type}X"),
        ),
    ];

    for (case, type_) in rows {
        let (status, answer) = call(
            &router,
            "POST",
            "/configurationManager",
            Some(&json!({
                "Name": "catalog",
                "ConfigurationDefinitions": [{
                    "Type": type_,
                    "Parameters": { "TargetOrganizationalUnits": "ou-1", "TargetRegions": REGION },
                }],
            })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: {type_:?} names no published type and must be refused, got \
             {status} / {answer:?}"
        );
    }
}
