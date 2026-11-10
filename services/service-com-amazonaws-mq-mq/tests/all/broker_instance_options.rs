// SPDX-License-Identifier: CC0-1.0

//! `DescribeBrokerInstanceOptions`, driven over the wire.
//!
//! RULE: the operation reports Amazon MQ's published instance-option catalog — one entry
//! per (engine type, host instance type, storage type) combination the service offers,
//! each carrying the deployment modes and engine versions that combination supports and
//! the Availability Zones of the Region the caller signed for — narrowed by every filter
//! member the request set and paged by `maxResults` with a round-trippable `nextToken`.
//! Because the catalog describes the SERVICE and not the account, the answer does not
//! depend on anything a caller created and a call changes nothing a later caller sees.
//!
//! The tables span what the rule generalises over rather than one recorded request: each
//! filter member crossed with every value the model enumerates for it, with the values
//! the catalog itself publishes for the free-form one and with a value it does not
//! publish; the published bounds of `maxResults` and one past each; and several Regions.
//! The expected answer of a filtered call is always derived from the UNFILTERED response
//! read over the same wire, so no entry is written out here.
//!
//! The value catalogs the tables span are the model's own enumerations —
//! `smithy_query(query="shape", shape="EngineType"|"BrokerStorageType"|"DeploymentMode")`,
//! which the crate re-exports as `ENGINE_TYPE_VALUES`, `BROKER_STORAGE_TYPE_VALUES` and
//! `DEPLOYMENT_MODE_VALUES`.

use serde_json::{json, Value};

use service_com_amazonaws_mq_mq::ops::types::{
    BROKER_STORAGE_TYPE_VALUES, DEPLOYMENT_MODE_VALUES, ENGINE_TYPE_VALUES,
};
use service_com_amazonaws_mq_mq::state::{
    availability_zones_in_region, invalid_page_token_message, published_engine_versions,
    DEFAULT_MAX_RESULTS, DEPLOYMENT_MODE_ACTIVE_STANDBY_MULTI_AZ,
    DEPLOYMENT_MODE_CLUSTER_MULTI_AZ, DEPLOYMENT_MODE_SINGLE_INSTANCE, ENGINE_TYPE_RABBITMQ,
    MAX_MAX_RESULTS, MIN_MAX_RESULTS, STORAGE_TYPE_EFS,
};

use crate::broker_lifecycle::{create_body, Mq};

/// A storage type as this catalog renders it: Amazon MQ publishes a `storageType` in
/// lower case, not in the upper-case spelling the `BrokerStorageType` enum declares for
/// the value a request sends.
fn rendered(storage_type: &str) -> String {
    storage_type.to_ascii_lowercase()
}

/// Where one entry sits in the order the catalog reports: by host instance type, then by
/// engine type as the model enumerates them, then by storage type.
fn order_key(case: &str, entry: &Value) -> (String, usize, usize) {
    let engine_type = member(case, entry, "engineType");
    let storage_type = member(case, entry, "storageType");
    (
        member(case, entry, "hostInstanceType"),
        ENGINE_TYPE_VALUES
            .iter()
            .position(|value| *value == engine_type)
            .unwrap_or(usize::MAX),
        BROKER_STORAGE_TYPE_VALUES
            .iter()
            .position(|value| rendered(value) == storage_type)
            .unwrap_or(usize::MAX),
    )
}

/// The operation's modelled path (`smithy_query(query="operation",
/// operation="DescribeBrokerInstanceOptions")`: `GET /v1/broker-instance-options`).
const PATH: &str = "/v1/broker-instance-options";

/// One `DescribeBrokerInstanceOptions` call, with `query` as its query string.
async fn describe(mq: &Mq, query: &str) -> (u16, Value) {
    let path = if query.is_empty() {
        PATH.to_string()
    } else {
        format!("{PATH}?{query}")
    };
    mq.call("GET", &path, None).await
}

/// The same call, also reporting the error code a restJson1 client branches on.
async fn describe_with_code(mq: &Mq, query: &str) -> (u16, String, Value) {
    let path = if query.is_empty() {
        PATH.to_string()
    } else {
        format!("{PATH}?{query}")
    };
    mq.call_with_code("GET", &path, None).await
}

/// The entries of a successful answer.
fn entries(case: &str, body: &Value) -> Vec<Value> {
    let Some(entries) = body.get("brokerInstanceOptions").and_then(Value::as_array) else {
        panic!("case {case:?}: expected a brokerInstanceOptions array, got {body:?}");
    };
    entries.clone()
}

/// One entry's string member.
fn member(case: &str, entry: &Value, name: &str) -> String {
    let Some(value) = entry.get(name).and_then(Value::as_str) else {
        panic!("case {case:?}: expected a string {name}, got {entry:?}");
    };
    value.to_string()
}

/// One entry's list-of-string member.
fn members(case: &str, entry: &Value, name: &str) -> Vec<String> {
    let Some(values) = entry.get(name).and_then(Value::as_array) else {
        panic!("case {case:?}: expected a {name} array, got {entry:?}");
    };
    values
        .iter()
        .map(|value| {
            let Some(value) = value.as_str() else {
                panic!("case {case:?}: expected {name} to hold strings, got {value:?}");
            };
            value.to_string()
        })
        .collect()
}

/// The whole catalog of `region`, read over the wire in one page.
async fn whole_catalog(case: &str, mq: &Mq) -> Vec<Value> {
    let (status, body) = describe(mq, &format!("maxResults={MAX_MAX_RESULTS}")).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 for the unfiltered catalog, got {status} {body:?}"
    );
    assert!(
        body.get("nextToken").is_none(),
        "case {case:?}: expected the whole catalog to fit one page of {MAX_MAX_RESULTS}, \
         got a nextToken in {body:?}"
    );
    entries(case, &body)
}

#[tokio::test]
async fn the_catalog_is_well_formed_and_covers_every_modelled_engine_type() {
    let region = "us-east-1";
    let mq = Mq::new(region);
    let case = "unfiltered";
    let catalog = whole_catalog(case, &mq).await;

    assert!(
        !catalog.is_empty(),
        "case {case:?}: expected the published catalog to hold entries, got {catalog:?}"
    );

    let expected_zones = availability_zones_in_region(region);
    let mut keys: Vec<(String, String, String)> = Vec::new();
    for entry in &catalog {
        let engine_type = member(case, entry, "engineType");
        let host_instance_type = member(case, entry, "hostInstanceType");
        let storage_type = member(case, entry, "storageType");
        let case = &format!("{engine_type}/{host_instance_type}/{storage_type}");

        assert!(
            ENGINE_TYPE_VALUES.contains(&engine_type.as_str()),
            "case {case:?}: engineType must be one of {ENGINE_TYPE_VALUES:?}, got \
             {engine_type:?}"
        );
        // The catalog renders `storageType` in the lower-case spelling Amazon MQ
        // publishes for it, not the upper-case `BrokerStorageType` enum name a broker's
        // own storage type carries.
        let rendered_storage_types: Vec<String> =
            BROKER_STORAGE_TYPE_VALUES.iter().map(|v| rendered(v)).collect();
        assert!(
            rendered_storage_types.contains(&storage_type),
            "case {case:?}: storageType must be one of {rendered_storage_types:?}, got \
             {storage_type:?}"
        );
        // `BrokerStorageType`'s own documentation: "EFS is not supported for RabbitMQ
        // engine type".
        assert!(
            engine_type != ENGINE_TYPE_RABBITMQ || storage_type != rendered(STORAGE_TYPE_EFS),
            "case {case:?}: {ENGINE_TYPE_RABBITMQ} is not offered on {STORAGE_TYPE_EFS}, got \
             {entry:?}"
        );

        let modes = members(case, entry, "supportedDeploymentModes");
        assert_eq!(
            modes.first().map(String::as_str),
            Some(DEPLOYMENT_MODE_SINGLE_INSTANCE),
            "case {case:?}: every option offers {DEPLOYMENT_MODE_SINGLE_INSTANCE} first, got \
             {modes:?}"
        );
        for mode in &modes {
            assert!(
                DEPLOYMENT_MODE_VALUES.contains(&mode.as_str()),
                "case {case:?}: supportedDeploymentModes must hold modes from \
                 {DEPLOYMENT_MODE_VALUES:?}, got {mode:?}"
            );
        }
        // Beyond the single-instance mode, an entry carries its ENGINE's highly
        // available mode: ActiveMQ's active/standby pair wherever the entry's storage is
        // the shared file system, RabbitMQ's cluster on every size but the burstable one.
        let highly_available: Vec<&str> = modes
            .iter()
            .map(String::as_str)
            .filter(|mode| *mode != DEPLOYMENT_MODE_SINGLE_INSTANCE)
            .collect();
        let expected_highly_available: Vec<&str> = if engine_type == ENGINE_TYPE_RABBITMQ {
            (!host_instance_type.ends_with(".micro"))
                .then_some(DEPLOYMENT_MODE_CLUSTER_MULTI_AZ)
                .into_iter()
                .collect()
        } else {
            (storage_type == rendered(STORAGE_TYPE_EFS))
                .then_some(DEPLOYMENT_MODE_ACTIVE_STANDBY_MULTI_AZ)
                .into_iter()
                .collect()
        };
        assert_eq!(
            highly_available, expected_highly_available,
            "case {case:?}: unexpected highly available deployment mode in {modes:?}"
        );

        // The versions are the ones the service offers for creation on that engine,
        // newest first and at MAJOR.MINOR granularity.
        let versions = members(case, entry, "supportedEngineVersions");
        let mut offered: Vec<String> = published_engine_versions(&engine_type)
            .iter()
            .map(|version| (*version).to_string())
            .collect();
        offered.reverse();
        assert_eq!(
            versions, offered,
            "case {case:?}: expected the offered versions newest first, got {versions:?}"
        );
        for version in &versions {
            assert_eq!(
                version.split('.').count(),
                2,
                "case {case:?}: a version a broker can be created with is MAJOR.MINOR, got \
                 {version:?}"
            );
        }

        let Some(zones) = entry.get("availabilityZones").and_then(Value::as_array) else {
            panic!("case {case:?}: expected an availabilityZones array, got {entry:?}");
        };
        let zone_names: Vec<String> = zones
            .iter()
            .map(|zone| member(case, zone, "name"))
            .collect();
        assert_eq!(
            zone_names, expected_zones,
            "case {case:?}: the zones must be the signed Region's, expected \
             {expected_zones:?}, got {zone_names:?}"
        );

        keys.push((engine_type, host_instance_type, storage_type));
    }

    let mut distinct = keys.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        keys.len(),
        "case {case:?}: every (engineType, hostInstanceType, storageType) appears once, \
         got {keys:?}"
    );

    for engine_type in ENGINE_TYPE_VALUES {
        assert!(
            keys.iter().any(|(engine, _, _)| engine == engine_type),
            "case {engine_type:?}: the catalog must offer options for every modelled engine \
             type, got {keys:?}"
        );
    }

    // The catalog is grouped by host instance type and only then by engine type and
    // storage type, which is what makes a `nextToken` offset name the same entry on a
    // later call.
    let order: Vec<(String, usize, usize)> = catalog
        .iter()
        .map(|entry| order_key(case, entry))
        .collect();
    let mut ascending = order.clone();
    ascending.sort();
    assert_eq!(
        order, ascending,
        "case {case:?}: the catalog is reported by (hostInstanceType, engineType, \
         storageType) ascending"
    );
}

#[tokio::test]
async fn the_zones_of_an_entry_are_the_signed_regions_whole_zone_set() {
    // Each row: a Region and the Availability Zones AWS publishes for it, written out
    // (AWS's "Regions and Availability Zones" list). The names are literals so that a
    // change to the emulator's own Region table is caught, and the rows span a Region
    // with six zones, Regions with four, a Region whose zone letters are not
    // consecutive, and Regions with three — including Regions the recorded call never
    // signed for.
    let rows: [(&str, &[&str]); 5] = [
        (
            "us-east-1",
            &[
                "us-east-1a",
                "us-east-1b",
                "us-east-1c",
                "us-east-1d",
                "us-east-1e",
                "us-east-1f",
            ],
        ),
        (
            "us-west-2",
            &["us-west-2a", "us-west-2b", "us-west-2c", "us-west-2d"],
        ),
        (
            "ap-northeast-1",
            &["ap-northeast-1a", "ap-northeast-1c", "ap-northeast-1d"],
        ),
        ("eu-west-1", &["eu-west-1a", "eu-west-1b", "eu-west-1c"]),
        ("sa-east-1", &["sa-east-1a", "sa-east-1b", "sa-east-1c"]),
    ];

    for (region, zones) in rows {
        let case = &format!("{region} ({} zones)", zones.len());
        let mq = Mq::new(region);
        let catalog = whole_catalog(case, &mq).await;
        let expected: Vec<Value> = zones.iter().map(|name| json!({"name": name})).collect();
        assert_eq!(
            availability_zones_in_region(region),
            zones.iter().map(|z| (*z).to_string()).collect::<Vec<String>>(),
            "case {case:?}: expected the Region's published zone set {zones:?}, got {:?}",
            availability_zones_in_region(region)
        );
        assert!(
            !catalog.is_empty(),
            "case {case:?}: expected the catalog to hold entries"
        );
        for entry in &catalog {
            assert_eq!(
                entry.get("availabilityZones"),
                Some(&Value::Array(expected.clone())),
                "case {case:?}: expected every entry to carry the Region's whole zone set \
                 {zones:?}, got {entry:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_storage_type_filter_selects_the_same_entries_in_either_case() {
    let mq = Mq::new("us-west-2");
    let catalog = whole_catalog("unfiltered", &mq).await;

    // Each row: a storage type the model enumerates, spelled as the catalog renders it
    // (the lower-case spelling the recorded catalog serves) and as the model spells it
    // (an emulator choice: the recording does not show whether the filter takes the enum
    // name, and refusing the value a client read from its own model would make the
    // filter unusable). Both name the same storage type, so both must select the entries
    // the catalog reports for it.
    for storage_type in BROKER_STORAGE_TYPE_VALUES {
        let expected: Vec<Value> = catalog
            .iter()
            .filter(|entry| member("unfiltered", entry, "storageType") == rendered(storage_type))
            .cloned()
            .collect();
        assert!(
            !expected.is_empty(),
            "case {storage_type:?}: the catalog must report entries for every modelled \
             storage type"
        );
        for spelling in [rendered(storage_type), storage_type.to_string()] {
            let query = format!("maxResults={MAX_MAX_RESULTS}&storageType={spelling}");
            let case = &query;
            let (status, body) = describe(&mq, &query).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {body:?}"
            );
            let got = entries(case, &body);
            assert_eq!(
                got, expected,
                "case {case:?}: expected the {} entries stored with {storage_type}, got {}",
                expected.len(),
                got.len()
            );
        }
    }

    // A value outside the modelled set is still refused, in either case.
    for spelling in ["sdd", "SDD"] {
        let query = format!("storageType={spelling}");
        let case = &query;
        let (status, code, body) = describe_with_code(&mq, &query).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            code, "BadRequestException",
            "case {case:?}: expected the modelled error code, got {code:?} {body:?}"
        );
    }
}

#[tokio::test]
async fn each_filter_selects_exactly_the_matching_catalog_entries() {
    let mq = Mq::new("us-east-1");
    let catalog = whole_catalog("unfiltered", &mq).await;

    // The free-form filter's values are the ones the catalog itself publishes, plus a
    // host instance type it does not publish (an `mq.m5` size outside the offered set),
    // which must select nothing rather than everything.
    let mut host_instance_types: Vec<String> = catalog
        .iter()
        .map(|entry| member("unfiltered", entry, "hostInstanceType"))
        .collect();
    host_instance_types.sort();
    host_instance_types.dedup();
    host_instance_types.push("mq.m5.24xlarge".to_string());

    let engine_filters: Vec<Option<String>> = std::iter::once(None)
        .chain(ENGINE_TYPE_VALUES.iter().map(|v| Some((*v).to_string())))
        .collect();
    let storage_filters: Vec<Option<String>> = std::iter::once(None)
        .chain(BROKER_STORAGE_TYPE_VALUES.iter().map(|v| Some(rendered(v))))
        .collect();
    let host_filters: Vec<Option<String>> = std::iter::once(None)
        .chain(host_instance_types.into_iter().map(Some))
        .collect();

    for engine_filter in &engine_filters {
        for storage_filter in &storage_filters {
            for host_filter in &host_filters {
                let mut query = vec![format!("maxResults={MAX_MAX_RESULTS}")];
                if let Some(engine) = engine_filter {
                    query.push(format!("engineType={engine}"));
                }
                if let Some(storage) = storage_filter {
                    query.push(format!("storageType={storage}"));
                }
                if let Some(host) = host_filter {
                    query.push(format!("hostInstanceType={host}"));
                }
                let query = query.join("&");
                let case = &query;

                let expected: Vec<Value> = catalog
                    .iter()
                    .filter(|entry| {
                        [
                            ("engineType", engine_filter),
                            ("storageType", storage_filter),
                            ("hostInstanceType", host_filter),
                        ]
                        .iter()
                        .all(|(name, filter)| match filter {
                            None => true,
                            Some(filter) => &member(case, entry, name) == filter,
                        })
                    })
                    .cloned()
                    .collect();

                let (status, body) = describe(&mq, &query).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200, got {status} {body:?}"
                );
                let got = entries(case, &body);
                assert_eq!(
                    got, expected,
                    "case {case:?}: expected the {} matching entries, got {}",
                    expected.len(),
                    got.len()
                );
            }
        }
    }
}

#[tokio::test]
async fn pagination_walks_every_entry_exactly_once() {
    let mq = Mq::new("eu-west-1");
    let catalog = whole_catalog("unfiltered", &mq).await;

    // The published bounds of `maxResults` (1 and 100) and sizes between them, so the
    // walk is tested where a page is one entry, several entries and the whole catalog.
    let sizes = [MIN_MAX_RESULTS, 2, 5, MAX_MAX_RESULTS];
    for size in sizes {
        let case = &format!("maxResults={size}");
        let mut seen: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                None => format!("maxResults={size}"),
                Some(token) => format!("maxResults={size}&nextToken={token}"),
            };
            let (status, body) = describe(&mq, &query).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 on page {pages}, got {status} {body:?}"
            );
            assert_eq!(
                body.get("maxResults").and_then(Value::as_i64),
                Some(i64::from(size)),
                "case {case:?}: the answer reports the page size it used, got {body:?}"
            );
            let page = entries(case, &body);
            assert!(
                page.len() <= usize::try_from(size).unwrap_or_default(),
                "case {case:?}: a page holds at most {size} entries, got {}",
                page.len()
            );
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= catalog.len() + 1,
                "case {case:?}: the walk must terminate, got {pages} pages for {} entries",
                catalog.len()
            );
            match body.get("nextToken").and_then(Value::as_str) {
                None => break,
                Some(next) => token = Some(next.to_string()),
            }
        }
        assert_eq!(
            seen, catalog,
            "case {case:?}: paging must report every entry exactly once, in the same order"
        );
    }

    // The default page size, from the output member's own documentation ("20 by
    // default").
    let case = "no maxResults";
    let (status, body) = describe(&mq, "").await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} {body:?}"
    );
    assert_eq!(
        body.get("maxResults").and_then(Value::as_i64),
        Some(i64::try_from(DEFAULT_MAX_RESULTS).unwrap_or_default()),
        "case {case:?}: the answer reports the documented default page size, got {body:?}"
    );
    let page = entries(case, &body);
    assert_eq!(
        page,
        catalog[..catalog.len().min(DEFAULT_MAX_RESULTS)].to_vec(),
        "case {case:?}: the first page is the catalog's first {DEFAULT_MAX_RESULTS} entries"
    );
}

#[tokio::test]
async fn values_outside_the_published_bounds_and_sets_are_refused() {
    let mq = Mq::new("us-east-1");

    // Each row: the query, the member the refusal must name, and the sentence it must
    // carry. The enum values are built FROM the model's sets rather than copied from a
    // request, and the bounds are the published ones and one past each.
    let engine_outside = format!("{}_NOT_AN_ENGINE", ENGINE_TYPE_VALUES[0]);
    let storage_outside = format!("{}_NOT_A_STORAGE", BROKER_STORAGE_TYPE_VALUES[0]);
    let bogus_token = "not-a-token-this-service-issued";
    let rows: Vec<(String, &str, Option<String>)> = vec![
        (
            format!("maxResults={}", MIN_MAX_RESULTS - 1),
            "maxResults",
            None,
        ),
        (
            format!("maxResults={}", MAX_MAX_RESULTS + 1),
            "maxResults",
            None,
        ),
        (
            format!("engineType={engine_outside}"),
            "engineType",
            Some(format!(
                "Broker engine type [{engine_outside}] is invalid. Valid values are: [{}]",
                ENGINE_TYPE_VALUES.join(", ")
            )),
        ),
        (
            format!("storageType={storage_outside}"),
            "storageType",
            Some(format!(
                "Broker storage type [{storage_outside}] is invalid. Valid values are: [{}]",
                BROKER_STORAGE_TYPE_VALUES.join(", ")
            )),
        ),
        (
            format!("nextToken={bogus_token}"),
            "nextToken",
            Some(invalid_page_token_message(bogus_token)),
        ),
    ];

    for (query, attribute, message) in &rows {
        let case = query;
        let (status, code, body) = describe_with_code(&mq, query).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            code, "BadRequestException",
            "case {case:?}: expected the modelled error code, got {code:?} {body:?}"
        );
        assert_eq!(
            body.get("errorAttribute").and_then(Value::as_str),
            Some(*attribute),
            "case {case:?}: the refusal must name the member at fault, got {body:?}"
        );
        if let Some(message) = message {
            assert_eq!(
                body.get("message").and_then(Value::as_str),
                Some(message.as_str()),
                "case {case:?}: unexpected sentence in {body:?}"
            );
        }
    }

    // The published bounds themselves are accepted.
    for size in [MIN_MAX_RESULTS, MAX_MAX_RESULTS] {
        let query = format!("maxResults={size}");
        let (status, body) = describe(&mq, &query).await;
        assert_eq!(
            status, 200,
            "case {query:?}: a value on the published bound is accepted, got {status} {body:?}"
        );
    }
}

#[tokio::test]
async fn the_catalog_follows_the_region_and_not_the_account() {
    // Every Region offers the same options; only the zone names differ, because a zone
    // name is its Region's code plus a letter.
    let mut without_zones: Option<Vec<Value>> = None;
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let mq = Mq::new(region);
        let catalog = whole_catalog(region, &mq).await;
        let expected_zones: Vec<Value> = availability_zones_in_region(region)
            .into_iter()
            .map(|name| json!({"name": name}))
            .collect();
        for entry in &catalog {
            assert_eq!(
                entry.get("availabilityZones"),
                Some(&Value::Array(expected_zones.clone())),
                "case {region:?}: the zones must be named for the signed Region, got {entry:?}"
            );
        }
        let stripped: Vec<Value> = catalog
            .iter()
            .map(|entry| {
                let mut entry = entry.clone();
                entry
                    .as_object_mut()
                    .expect("an entry is a JSON object")
                    .remove("availabilityZones");
                entry
            })
            .collect();
        match &without_zones {
            None => without_zones = Some(stripped),
            Some(first) => assert_eq!(
                &stripped, first,
                "case {region:?}: the offered options do not differ by Region"
            ),
        }
    }

    // The catalog describes the service, so what the account holds cannot change it,
    // and reading it changes nothing a later caller sees.
    let mq = Mq::new("us-east-1");
    let before = whole_catalog("before a broker exists", &mq).await;
    let (status, created) = mq.create_broker(create_body("catalog-witness")).await;
    assert_eq!(
        status, 200,
        "case \"create a broker\": expected 200, got {status} {created:?}"
    );
    let after = whole_catalog("after a broker exists", &mq).await;
    assert_eq!(
        after, before,
        "case \"after a broker exists\": the catalog must not depend on the account's brokers"
    );
    let again = whole_catalog("read twice", &mq).await;
    assert_eq!(
        again, after,
        "case \"read twice\": a read must not change what a later caller sees"
    );
}
