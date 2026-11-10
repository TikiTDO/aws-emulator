// SPDX-License-Identifier: CC0-1.0

//! What a discovered-resource read reports, over the wire.
//!
//! The rule pinned here: `ListDiscoveredResources` reports, out of the resources Config
//! has discovered in this account and Region, the identifiers of the ONE requested
//! resource type, narrowed to the resource IDs or the resource name the request asked
//! for (never both), excluding deleted resources unless the request asks for them,
//! ordered by resource id and served in pages of at most `limit` identifiers (100 when
//! the request asks for no limit of its own) with a `nextToken` only while resources
//! remain; a read of an account with no configuration recorder is SERVED, and reports
//! the empty inventory such an account has discovered.
//!
//! Every case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the production router builds — using the
//! `Emulator` harness `config_setup_and_rule_loop` already defines. The discovered
//! resources themselves are seeded in the store the way a handler reaches it
//! (`service_state`, as the other suites of this crate seed state with), so the
//! reader's rule is pinned over inventories a table states, including deleted
//! resources. `PutResourceConfig`, which records a resource's configuration
//! item, is exercised against this reader in `config_discovered_resource_counts`, and
//! `DeleteResourceConfig`, which records a resource's deletion, in
//! `resource_config_deletion`.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::state::{
    service_state, DiscoveredResourceEntry,
};

use crate::config_setup_and_rule_loop::{recorder_request, Emulator, ROLE_ARN};

/// Resource types the rows use. Entries of `com.amazonaws.configservice#ResourceType`,
/// from `smithy_query(query="shape", shape="ResourceType")`, deliberately including
/// types no recording in the issue used.
const EC2_VOLUME: &str = "AWS::EC2::Volume";
/// See [`EC2_VOLUME`].
const DYNAMODB_TABLE: &str = "AWS::DynamoDB::Table";
/// See [`EC2_VOLUME`].
const S3_BUCKET: &str = "AWS::S3::Bucket";
/// See [`EC2_VOLUME`].
const LAMBDA_FUNCTION: &str = "AWS::Lambda::Function";
/// See [`EC2_VOLUME`].
const IAM_ROLE: &str = "AWS::IAM::Role";

/// The page size a request that states no limit of its own gets
/// (`ListDiscoveredResourcesRequest$limit`: "The default is 100").
const DEFAULT_LIMIT: usize = 100;

/// The published bounds of `limit` (`com.amazonaws.configservice#Limit`, range 0..100;
/// the member's doc: "You cannot specify a number greater than 100").
const LIMIT_MIN: i64 = 0;
/// See [`LIMIT_MIN`].
const LIMIT_MAX: i64 = 100;

/// The most resource IDs one request may narrow by
/// (`ListDiscoveredResourcesRequest$resourceIds`: "You can list a minimum of 1
/// resourceID and a maximum of 20 resourceIds").
const MAX_RESOURCE_IDS: usize = 20;

/// One seeded discovered resource: its type, its id, the custom name it carries if any,
/// and whether it has been deleted.
struct Discovered {
    resource_type: &'static str,
    resource_id: String,
    resource_name: Option<&'static str>,
    deleted: bool,
}

/// A live resource of `resource_type` with no custom name.
fn live(resource_type: &'static str, resource_id: &str) -> Discovered {
    Discovered {
        resource_type,
        resource_id: resource_id.to_string(),
        resource_name: None,
        deleted: false,
    }
}

/// A live resource carrying the custom name `resource_name`.
fn named(resource_type: &'static str, resource_id: &str, resource_name: &'static str) -> Discovered {
    Discovered {
        resource_name: Some(resource_name),
        ..live(resource_type, resource_id)
    }
}

/// A resource that has been deleted, which the inventory keeps.
fn deleted(resource_type: &'static str, resource_id: &str) -> Discovered {
    Discovered {
        deleted: true,
        ..live(resource_type, resource_id)
    }
}

/// The instant a seeded deleted resource went away: a fixed point in the past, so a
/// reported `resourceDeletionTime` can be checked against it.
fn deletion_time() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

/// An emulator with a configuration recorder (which a discovered-resource read needs),
/// holding `resources` in its discovered-resource inventory.
async fn emulator_discovering(resources: &[Discovered]) -> Emulator {
    let emulator = Emulator::new();
    emulator
        .ok(
            "PutConfigurationRecorder",
            &recorder_request(json!({ "roleARN": ROLE_ARN })),
        )
        .await;
    {
        let state = service_state(&*emulator.engine).expect("the test engine is the shared engine");
        let mut store = state.lock();
        for resource in resources {
            store.discovered_resources.insert(
                (
                    resource.resource_type.to_string(),
                    resource.resource_id.clone(),
                ),
                DiscoveredResourceEntry {
                    resource_type: resource.resource_type.to_string(),
                    resource_id: resource.resource_id.clone(),
                    resource_name: resource.resource_name.map(str::to_string),
                    deleted_at: resource.deleted.then(deletion_time),
                    // A resource seeded here carries no recorded configuration item:
                    // what a listing reports is its identity, not its configuration.
                    ..DiscoveredResourceEntry::default()
                },
            );
        }
    }
    emulator
}

/// The `resourceId` of every identifier in a response, in the order reported.
fn reported_ids(case: &str, body: &Value) -> Vec<String> {
    body["resourceIdentifiers"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected resourceIdentifiers, got {body}"))
        .iter()
        .map(|identifier| {
            identifier["resourceId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every identifier reports a resourceId, got {body}")
                })
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn a_listing_reports_the_resources_of_the_requested_type_the_request_narrows_to() {
    let inventory = [
        named(EC2_VOLUME, "vol-0001", "data-volume"),
        live(EC2_VOLUME, "vol-0002"),
        deleted(EC2_VOLUME, "vol-0003"),
        named(DYNAMODB_TABLE, "orders", "orders"),
        live(S3_BUCKET, "config-bucket"),
        deleted(LAMBDA_FUNCTION, "evaluate"),
    ];

    // Rows differ along what the rule generalises over: the type asked for (including
    // one nothing was discovered for), whether deleted resources are asked for, and
    // whether the request narrows by resource ids (one, several, one that matches
    // nothing) or by resource name.
    let cases: [(&str, Value, &[&str]); 13] = [
        (
            "every live resource of a type",
            json!({ "resourceType": EC2_VOLUME }),
            &["vol-0001", "vol-0002"],
        ),
        (
            "the deleted resources of a type as well",
            json!({ "resourceType": EC2_VOLUME, "includeDeletedResources": true }),
            &["vol-0001", "vol-0002", "vol-0003"],
        ),
        (
            "another type entirely",
            json!({ "resourceType": DYNAMODB_TABLE }),
            &["orders"],
        ),
        (
            "a type nothing was discovered for",
            json!({ "resourceType": IAM_ROLE }),
            &[],
        ),
        (
            "a type whose only resource is deleted",
            json!({ "resourceType": LAMBDA_FUNCTION }),
            &[],
        ),
        (
            "that same type, with deleted resources included",
            json!({ "resourceType": LAMBDA_FUNCTION, "includeDeletedResources": true }),
            &["evaluate"],
        ),
        (
            "one resource id",
            json!({ "resourceType": EC2_VOLUME, "resourceIds": ["vol-0002"] }),
            &["vol-0002"],
        ),
        (
            "several resource ids, one of which was never discovered",
            json!({ "resourceType": EC2_VOLUME, "resourceIds": ["vol-0001", "vol-0404"] }),
            &["vol-0001"],
        ),
        (
            "a deleted resource named by id, with deleted resources excluded",
            json!({ "resourceType": EC2_VOLUME, "resourceIds": ["vol-0003"] }),
            &[],
        ),
        (
            "a deleted resource named by id, with deleted resources included",
            json!({
                "resourceType": EC2_VOLUME,
                "resourceIds": ["vol-0003"],
                "includeDeletedResources": true,
            }),
            &["vol-0003"],
        ),
        (
            "a resource name one resource carries",
            json!({ "resourceType": EC2_VOLUME, "resourceName": "data-volume" }),
            &["vol-0001"],
        ),
        (
            "a resource name nothing carries",
            json!({ "resourceType": EC2_VOLUME, "resourceName": "no-such-name" }),
            &[],
        ),
        (
            "an empty resource name, which no resource carries either",
            json!({ "resourceType": EC2_VOLUME, "resourceName": "" }),
            &[],
        ),
    ];

    for (case, request, expected) in cases {
        let emulator = emulator_discovering(&inventory).await;
        let body = emulator.ok("ListDiscoveredResources", &request).await;

        assert_eq!(
            reported_ids(case, &body),
            expected,
            "case {case:?}: expected the resource ids {expected:?}, got {body}"
        );
        assert!(
            body.get("nextToken").is_none(),
            "case {case:?}: a listing that fits in one page carries no nextToken, got {body}"
        );

        // Every identifier reports the type and, for the resources that carry one, the
        // custom name and the deletion time out of the inventory.
        for identifier in body["resourceIdentifiers"].as_array().unwrap_or(&Vec::new()) {
            let reported_id = identifier["resourceId"].as_str().unwrap_or_default();
            let seeded = inventory
                .iter()
                .find(|resource| resource.resource_id == reported_id)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: {reported_id:?} is not in the inventory, got {body}")
                });
            assert_eq!(
                identifier["resourceType"],
                json!(seeded.resource_type),
                "case {case:?}: expected resourceType {:?} for {reported_id:?}, got {identifier}",
                seeded.resource_type
            );
            assert_eq!(
                identifier["resourceName"].as_str(),
                seeded.resource_name,
                "case {case:?}: expected resourceName {:?} for {reported_id:?}, got {identifier}",
                seeded.resource_name
            );
            assert_eq!(
                identifier["resourceDeletionTime"].is_null(),
                !seeded.deleted,
                "case {case:?}: a resourceDeletionTime is reported for a deleted resource \
                 and only for one, got {identifier}"
            );
            if seeded.deleted {
                let expected_seconds = deletion_time()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .expect("the seeded deletion time is after the epoch")
                    .as_secs_f64();
                assert_eq!(
                    identifier["resourceDeletionTime"].as_f64(),
                    Some(expected_seconds),
                    "case {case:?}: expected the deletion time the inventory holds, got \
                     {identifier}"
                );
            }
        }

        // The read is a report: it must not change what the next read sees.
        let again = emulator.ok("ListDiscoveredResources", &request).await;
        assert_eq!(
            again, body,
            "case {case:?}: a second listing must report the same resources, got {again}"
        );
    }
}

#[tokio::test]
async fn a_page_carries_at_most_the_limit_and_its_token_resumes_the_listing() {
    // Rows differ along how many resources of the type are in the inventory and how
    // many identifiers one page may carry: no limit at all (the default page size),
    // each published bound of `limit`, and sizes under, at and over the page size.
    let cases: [(&str, usize, Option<i64>); 8] = [
        ("nothing discovered, no limit", 0, None),
        ("one resource, no limit", 1, None),
        ("seven resources, pages of one", 7, Some(1)),
        ("seven resources, pages of two", 7, Some(2)),
        ("seven resources, pages of seven", 7, Some(7)),
        ("seven resources, limit at the published maximum", 7, Some(LIMIT_MAX)),
        ("seven resources, limit 0 means the default", 7, Some(LIMIT_MIN)),
        ("one resource past the default page size, no limit", DEFAULT_LIMIT + 1, None),
    ];

    for (case, size, limit) in cases {
        let inventory: Vec<Discovered> = (0..size)
            .map(|index| live(EC2_VOLUME, &format!("vol-{index:04}")))
            .collect();
        let expected: Vec<String> = inventory
            .iter()
            .map(|resource| resource.resource_id.clone())
            .collect();
        let page_size = match limit {
            None | Some(0) => DEFAULT_LIMIT,
            Some(stated) => stated as usize,
        };
        let emulator = emulator_discovering(&inventory).await;

        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({ "resourceType": EC2_VOLUME });
            if let Some(stated) = limit {
                request["limit"] = json!(stated);
            }
            if let Some(resume_at) = token.as_deref() {
                request["nextToken"] = json!(resume_at);
            }
            let body = emulator.ok("ListDiscoveredResources", &request).await;
            let page = reported_ids(case, &body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page carries at most {page_size} identifiers, got {body}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= size + 1,
                "case {case:?}: paging must terminate, walked {pages} pages of {body}"
            );
            match body["nextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }

        assert_eq!(
            walked, expected,
            "case {case:?}: walking every page must report each discovered resource once, \
             in resource-id order"
        );
        let expected_pages = size.max(1).div_ceil(page_size);
        assert_eq!(
            pages,
            expected_pages.max(1),
            "case {case:?}: expected {} pages of at most {page_size} of {size} resources, \
             walked {pages}",
            expected_pages.max(1)
        );
    }
}

#[tokio::test]
async fn a_discovered_resource_read_without_a_configuration_recorder_reports_an_empty_page() {
    // Rows differ along the request shape, so what is served is the empty inventory
    // rather than something about what the request asked for.
    let requests: [(&str, Value); 4] = [
        ("a bare listing", json!({ "resourceType": EC2_VOLUME })),
        (
            "a listing narrowed by id",
            json!({ "resourceType": EC2_VOLUME, "resourceIds": ["vol-0001"] }),
        ),
        (
            "a listing narrowed by name",
            json!({ "resourceType": S3_BUCKET, "resourceName": "config-bucket" }),
        ),
        (
            "a listing of a type nothing was discovered for",
            json!({ "resourceType": IAM_ROLE, "includeDeletedResources": true }),
        ),
    ];

    for (case, request) in requests {
        let emulator = Emulator::new();
        // An account that has recorded nothing has discovered nothing, so the read is
        // served an empty page rather than refused for the recorder it does not have.
        let body = emulator.ok("ListDiscoveredResources", &request).await;
        assert_eq!(
            reported_ids(case, &body),
            Vec::<String>::new(),
            "case {case:?}: with no configuration recorder the read reports no \
             identifiers, got {body}"
        );

        // With a recorder the same read reports the same empty inventory.
        emulator
            .ok(
                "PutConfigurationRecorder",
                &recorder_request(json!({ "roleARN": ROLE_ARN })),
            )
            .await;
        let body = emulator.ok("ListDiscoveredResources", &request).await;
        assert_eq!(
            reported_ids(case, &body),
            Vec::<String>::new(),
            "case {case:?}: an account that has discovered nothing reports no \
             identifiers, got {body}"
        );
    }
}

#[tokio::test]
async fn a_request_that_cannot_hold_together_is_refused() {
    let inventory = [
        named(EC2_VOLUME, "vol-0001", "data-volume"),
        live(EC2_VOLUME, "vol-0002"),
    ];
    let many_ids: Vec<String> = (0..MAX_RESOURCE_IDS)
        .map(|index| format!("vol-{index:04}"))
        .collect();
    let one_too_many: Vec<String> = (0..=MAX_RESOURCE_IDS)
        .map(|index| format!("vol-{index:04}"))
        .collect();

    // Rows differ along which rule the request breaks: narrowing by ids AND by name,
    // more resource ids than the maximum, and a token this operation never issued.
    let cases: [(&str, Value, &str); 3] = [
        (
            "both resourceIds and resourceName",
            json!({
                "resourceType": EC2_VOLUME,
                "resourceIds": ["vol-0001"],
                "resourceName": "data-volume",
            }),
            "ValidationException",
        ),
        (
            "one resource id past the maximum",
            json!({ "resourceType": EC2_VOLUME, "resourceIds": one_too_many }),
            "ValidationException",
        ),
        (
            "a nextToken this operation never issued",
            json!({ "resourceType": EC2_VOLUME, "nextToken": "!!! not a token !!!" }),
            "InvalidNextTokenException",
        ),
    ];

    for (case, request, expected) in cases {
        let emulator = emulator_discovering(&inventory).await;
        let (status, code) = emulator.refused("ListDiscoveredResources", &request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, expected),
            "case {case:?}: expected 400 {expected}, got {status} {code}"
        );
    }

    // At the maximum the same narrowing is served, and reports the resources among
    // those ids that the inventory holds.
    let emulator = emulator_discovering(&inventory).await;
    let body = emulator
        .ok(
            "ListDiscoveredResources",
            &json!({ "resourceType": EC2_VOLUME, "resourceIds": many_ids }),
        )
        .await;
    assert_eq!(
        reported_ids("resource ids at the maximum", &body),
        ["vol-0001", "vol-0002"],
        "narrowing by the maximum of {MAX_RESOURCE_IDS} resource ids is served, got {body}"
    );

    // A limit outside the published range is refused — one past each bound.
    for (case, limit) in [("below the minimum", LIMIT_MIN - 1), ("above the maximum", LIMIT_MAX + 1)]
    {
        let (status, code) = emulator
            .refused(
                "ListDiscoveredResources",
                &json!({ "resourceType": EC2_VOLUME, "limit": limit }),
            )
            .await;
        // The modelled range is refused at the request boundary (`ValidationException`)
        // before the handler's own `InvalidLimitException` can answer for it; either
        // way the caller gets a 400 naming the limit.
        assert_eq!(
            status, 400,
            "case {case:?}: limit {limit} is outside {LIMIT_MIN}..{LIMIT_MAX} and must be \
             refused 400, got {status} {code}"
        );
        assert!(
            ["ValidationException", "InvalidLimitException"].contains(&code.as_str()),
            "case {case:?}: expected a modelled 400 for limit {limit}, got {code}"
        );
    }
}
