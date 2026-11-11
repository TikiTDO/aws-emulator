// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker Feature Store's control plane:
//! `CreateFeatureGroup` and the `DescribeFeatureGroup` that reads it back.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, on
//! the harness this crate's `inference_deployment_lifecycle` suite already owns, so what
//! these tests exercise is what a real SDK client gets.
//!
//! The tests assert RULES over TABLES of inputs rather than one recorded call:
//!
//! * a create/describe round trip for every combination of stores and optional members,
//!   including the empty-vs-absent distinction for a member that accepts the empty
//!   string;
//! * the ARN form, for every region and name shape, reachable by name and by ARN;
//! * the `Creating` -> `Created` transition the clock owes;
//! * one row per schema rule the model states, each refused with 400
//!   `ValidationException` and persisting nothing;
//! * every value of every enum the input carries accepted, and one value past each
//!   catalogue refused — the catalogues being the model's own generated value lists.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_sagemaker_sagemaker::ops::types::{
    COLLECTION_TYPE_VALUES, FEATURE_TYPE_VALUES, STORAGE_TYPE_VALUES, TABLE_FORMAT_VALUES,
    THROUGHPUT_MODE_VALUES, TTL_DURATION_UNIT_VALUES,
};
use service_com_amazonaws_sagemaker_sagemaker::state::RESERVED_FEATURE_NAMES;

use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// Long enough to pass `state::FEATURE_GROUP_CREATED_AFTER` (300 ms).
const PAST_CREATED_DUE: Duration = Duration::from_millis(600);

/// A role ARN matching `RoleArn`'s pattern
/// (`^arn:aws[a-z\-]*:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/FeatureStoreRole";

/// The members `DescribeFeatureGroup` replays verbatim from the create: the whole input
/// schema except the group's own name (asserted separately, beside its ARN), `Tags`
/// (which the describe response does not carry), `ThroughputConfig` (which the service
/// resolves for a create that stated none, asserted separately) and `OfflineStoreConfig`
/// (whose `ResolvedOutputS3Uri` the service resolves, also asserted separately).
const ECHOED_MEMBERS: &[&str] = &[
    "RecordIdentifierFeatureName",
    "EventTimeFeatureName",
    "FeatureDefinitions",
    "OnlineStoreConfig",
    "RoleArn",
    "Description",
];

/// A valid `CreateFeatureGroup` body for `name`, with `extra`'s members laid over it.
///
/// A member of `extra` whose value is `null` is REMOVED from the body, which is how a
/// row drops an otherwise-present member (`OnlineStoreConfig`, say) rather than sending
/// a JSON null.
fn feature_group_body(name: &str, extra: Value) -> Value {
    let mut body = json!({
        "FeatureGroupName": name,
        "RecordIdentifierFeatureName": "customer-id",
        "EventTimeFeatureName": "event-time",
        "FeatureDefinitions": [
            { "FeatureName": "customer-id", "FeatureType": "String" },
            { "FeatureName": "event-time", "FeatureType": "String" },
        ],
        "OnlineStoreConfig": { "EnableOnlineStore": true },
    });
    let object = body.as_object_mut().expect("a JSON object body");
    for (key, value) in extra.as_object().expect("a JSON object of extra members") {
        if value.is_null() {
            object.remove(key);
        } else {
            object.insert(key.clone(), value.clone());
        }
    }
    body
}

/// An `OfflineStoreConfig` naming `s3_uri`.
fn offline_store(s3_uri: &str) -> Value {
    json!({ "S3StorageConfig": { "S3Uri": s3_uri } })
}

/// RULE: `DescribeFeatureGroup` replays every member `CreateFeatureGroup` stored,
/// present members and absent ones alike, and reports the group's own name, ARN,
/// creation time and status beside them.
#[tokio::test]
async fn describe_feature_group_replays_exactly_what_create_feature_group_stored() {
    // Rows differ along the axes the rule generalises over: which stores the group has,
    // the online tier and its TTL, the offline table format and Glue handling, the
    // throughput mode, the feature count and types, a collection-typed feature, the
    // description (present, EMPTY and absent) and the tag count.
    let cases: Vec<(&str, Value)> = vec![
        ("online-store-only", json!({})),
        (
            "online-store-off",
            json!({ "OnlineStoreConfig": { "EnableOnlineStore": false } }),
        ),
        (
            "offline-store-only",
            json!({
                "OnlineStoreConfig": null,
                "OfflineStoreConfig": offline_store("s3://feature-store/offline/"),
                "RoleArn": ROLE_ARN,
            }),
        ),
        (
            "both-stores-with-glue-catalog",
            json!({
                "OnlineStoreConfig": {
                    "EnableOnlineStore": true,
                    "StorageType": "Standard",
                    "SecurityConfig": { "KmsKeyId": "alias/feature-store" },
                    "TtlDuration": { "Unit": "Days", "Value": 7 },
                },
                "OfflineStoreConfig": {
                    "S3StorageConfig": {
                        "S3Uri": "https://feature-store.s3.amazonaws.com/offline",
                        "KmsKeyId": "alias/offline-store",
                    },
                    "DisableGlueTableCreation": false,
                    "TableFormat": "Iceberg",
                    "DataCatalogConfig": {
                        "TableName": "customers",
                        "Catalog": "AwsDataCatalog",
                        "Database": "sagemaker_featurestore",
                    },
                },
                "RoleArn": ROLE_ARN,
                "Description": "customer features, both stores",
                "Tags": [{ "Key": "team", "Value": "ml" }, { "Key": "env", "Value": "test" }],
            }),
        ),
        (
            "provisioned-throughput",
            json!({
                "ThroughputConfig": {
                    "ThroughputMode": "Provisioned",
                    "ProvisionedReadCapacityUnits": 10,
                    "ProvisionedWriteCapacityUnits": 5,
                },
                "Description": "",
            }),
        ),
        (
            "on-demand-throughput-and-a-wide-schema",
            json!({
                "ThroughputConfig": { "ThroughputMode": "OnDemand" },
                "FeatureDefinitions": [
                    { "FeatureName": "customer-id", "FeatureType": "String" },
                    { "FeatureName": "event-time", "FeatureType": "Fractional" },
                    { "FeatureName": "age", "FeatureType": "Integral" },
                    { "FeatureName": "score", "FeatureType": "Fractional" },
                    {
                        "FeatureName": "embedding",
                        "FeatureType": "Fractional",
                        "CollectionType": "Vector",
                        "CollectionConfig": { "VectorConfig": { "Dimension": 4 } },
                    },
                ],
            }),
        ),
    ];

    for (case, extra) in cases {
        let emulator = Emulator::new();
        let request = feature_group_body(case, extra);

        let created = emulator.ok("CreateFeatureGroup", &request).await;
        assert_eq!(
            str_field(&created, "FeatureGroupArn"),
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:feature-group/{case}"),
            "case {case}: FeatureGroupArn, got {created:?}"
        );

        let described = emulator
            .ok("DescribeFeatureGroup", &json!({ "FeatureGroupName": case }))
            .await;

        for member in ECHOED_MEMBERS {
            assert_eq!(
                described.get(member),
                request.get(member),
                "case {case}: {member} must read back exactly as created, got {described:?}"
            );
        }
        assert_eq!(
            str_field(&described, "FeatureGroupName"),
            case,
            "case {case}: FeatureGroupName, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "FeatureGroupArn"),
            str_field(&created, "FeatureGroupArn"),
            "case {case}: the described ARN must be the minted one, got {described:?}"
        );
        // This row is about what the describe REPLAYS, so it asserts only that the
        // status is one of the two the create can have reached: `Creating`, or `Created`
        // if this loop's own round trip outlasted the owed span. The transition itself is
        // pinned at both ends by
        // `a_feature_group_advances_from_creating_to_created_once_the_clock_owes_it`.
        let status = str_field(&described, "FeatureGroupStatus");
        assert!(
            status == "Creating" || status == "Created",
            "case {case}: a freshly created group is Creating, or Created once the clock \
             owes it, got {status:?} in {described:?}"
        );
        assert_eq!(
            described.get("NextToken").and_then(Value::as_str),
            Some(""),
            "case {case}: one response carries every FeatureDefinition, so no page \
             remains to resume, got {described:?}"
        );
        assert!(
            described.get("CreationTime").and_then(Value::as_f64).is_some(),
            "case {case}: CreationTime must be reported, got {described:?}"
        );
        // `OfflineStoreConfig` replays as created with ONE member added: the output path
        // the service RESOLVED for this group — "The S3 path where offline records are
        // written" (`S3StorageConfig$ResolvedOutputS3Uri`), the published layout under the
        // configured `S3Uri`, which a create cannot state.
        match (
            request.get("OfflineStoreConfig"),
            described.get("OfflineStoreConfig"),
        ) {
            (None, None) => {}
            (Some(stated), Some(replayed)) => {
                let resolved = str_field(&replayed["S3StorageConfig"], "ResolvedOutputS3Uri");
                // The published layout under the configured location, in either spelling
                // the member admits: the account, `sagemaker`, the Region, this group, and
                // the `data` directory the record objects go under. The exact path is
                // pinned by `service_common::feature_store_offline_store`'s own suite.
                for expected in [
                    "s3://".to_string(),
                    format!("/{ACCOUNT}/sagemaker/"),
                    format!("/offline-store/{case}-"),
                    "/data".to_string(),
                ] {
                    assert!(
                        resolved.contains(&expected),
                        "case {case}: the resolved output path must carry {expected:?}, got \
                         {resolved:?}"
                    );
                }
                let mut without_resolved = replayed.clone();
                without_resolved["S3StorageConfig"]
                    .as_object_mut()
                    .expect("S3StorageConfig is an object")
                    .remove("ResolvedOutputS3Uri");
                assert_eq!(
                    &without_resolved, stated,
                    "case {case}: every other OfflineStoreConfig member must read back \
                     exactly as created, got {replayed:?}"
                );
            }
            (stated, replayed) => panic!(
                "case {case}: OfflineStoreConfig is replayed exactly when the create stated \
                 one, stated {stated:?} and got {replayed:?}"
            ),
        }
        // The offline store's replication status exists only for a group that has one.
        let offline_status = described
            .get("OfflineStoreStatus")
            .and_then(|status| status.get("Status"))
            .and_then(Value::as_str);
        assert_eq!(
            offline_status,
            request.get("OfflineStoreConfig").map(|_| "Active"),
            "case {case}: OfflineStoreStatus follows whether the group has an offline \
             store, got {described:?}"
        );
        // The throughput configuration the group RUNS under: the one the create stated,
        // or the on-demand mode the service resolves for a create that stated none.
        assert_eq!(
            described.get("ThroughputConfig"),
            Some(
                request
                    .get("ThroughputConfig")
                    .unwrap_or(&json!({ "ThroughputMode": "OnDemand" }))
            ),
            "case {case}: ThroughputConfig, got {described:?}"
        );
        // "The size of the OnlineStore in bytes": reported, and zero, for a group that
        // keeps an online store; absent for one that keeps none.
        let online_store_enabled = request
            .get("OnlineStoreConfig")
            .and_then(|config| config.get("EnableOnlineStore"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        assert_eq!(
            described.get("OnlineStoreTotalSizeBytes").and_then(Value::as_i64),
            online_store_enabled.then_some(0),
            "case {case}: OnlineStoreTotalSizeBytes follows whether the group keeps an \
             online store, got {described:?}"
        );
        for absent in ["LastUpdateStatus", "FailureReason"] {
            assert_eq!(
                described.get(absent),
                None,
                "case {case}: {absent} must be absent on a freshly created group, \
                 got {described:?}"
            );
        }
    }
}

/// RULE: a feature group's ARN is
/// `arn:aws:sagemaker:<signed region>:<account>:feature-group/<name>`, and because
/// `FeatureGroupNameOrArn` makes the ARN prefix optional, that ARN and the bare name
/// describe the one group.
#[tokio::test]
async fn a_feature_group_is_reachable_by_the_name_and_by_the_arn_its_region_minted() {
    // Rows differ along the name's shape and the region the caller signed for.
    // `FeatureGroupName` is `len 1..64` with pattern
    // `^[a-zA-Z0-9]([_-]*[a-zA-Z0-9]){0,63}$` (smithy_query operation=CreateFeatureGroup).
    let names = [
        "f",
        "group1",
        "customers_with_underscores",
        "customers-with-hyphens",
        "a234567890123456789012345678901234567890123456789012345678901234",
    ];
    let regions = ["us-east-1", "eu-west-1", "ap-southeast-2"];

    for region in regions {
        for name in names {
            let emulator = Emulator::new();
            let case = format!("region={region} name={name}");
            let expected_arn = format!("arn:aws:sagemaker:{region}:{ACCOUNT}:feature-group/{name}");

            let created = emulator
                .call_in(region, "CreateFeatureGroup", &feature_group_body(name, json!({})))
                .await;
            assert_eq!(
                created.0, 200,
                "case {case}: CreateFeatureGroup, got {} {}",
                created.0, created.1
            );
            assert_eq!(
                str_field(&created.1, "FeatureGroupArn"),
                expected_arn,
                "case {case}: FeatureGroupArn, got {:?}",
                created.1
            );

            // Both forms of the identifier reach the same stored group.
            for identifier in [name.to_string(), expected_arn.clone()] {
                let described = emulator
                    .call_in(
                        region,
                        "DescribeFeatureGroup",
                        &json!({ "FeatureGroupName": identifier }),
                    )
                    .await;
                assert_eq!(
                    described.0, 200,
                    "case {case} identifier={identifier}: DescribeFeatureGroup, got {} {}",
                    described.0, described.1
                );
                assert_eq!(
                    str_field(&described.1, "FeatureGroupArn"),
                    expected_arn,
                    "case {case} identifier={identifier}: FeatureGroupArn, got {:?}",
                    described.1
                );
            }
        }
    }
}

/// RULE: a newly created feature group is reported `Creating`, and the `Created`
/// transition the clock owes becomes visible once its span has elapsed — without any
/// call having to make it happen, and stably thereafter.
#[tokio::test]
async fn a_feature_group_advances_from_creating_to_created_once_the_clock_owes_it() {
    let emulator = Emulator::new();
    emulator
        .ok("CreateFeatureGroup", &feature_group_body("waiting", json!({})))
        .await;

    let creating = emulator
        .ok(
            "DescribeFeatureGroup",
            &json!({ "FeatureGroupName": "waiting" }),
        )
        .await;
    assert_eq!(
        str_field(&creating, "FeatureGroupStatus"),
        "Creating",
        "case immediately-after-create: FeatureGroupStatus, got {creating:?}"
    );

    tokio::time::sleep(PAST_CREATED_DUE).await;

    let settled = emulator
        .ok(
            "DescribeFeatureGroup",
            &json!({ "FeatureGroupName": "waiting" }),
        )
        .await;
    assert_eq!(
        str_field(&settled, "FeatureGroupStatus"),
        "Created",
        "case after-the-due-span: FeatureGroupStatus, got {settled:?}"
    );
    let created_at = settled
        .get("CreationTime")
        .and_then(Value::as_f64)
        .expect("CreationTime");
    let modified_at = settled
        .get("LastModifiedTime")
        .and_then(Value::as_f64)
        .expect("LastModifiedTime");
    assert!(
        modified_at > created_at,
        "case after-the-due-span: LastModifiedTime {modified_at} must move past \
         CreationTime {created_at} when the status changes"
    );
    assert_eq!(
        settled.get("CreationTime"),
        creating.get("CreationTime"),
        "case after-the-due-span: CreationTime must not move, got {settled:?}"
    );

    let again = emulator
        .ok(
            "DescribeFeatureGroup",
            &json!({ "FeatureGroupName": "waiting" }),
        )
        .await;
    assert_eq!(
        again, settled,
        "case repeated-read: a settled feature group must read back identically"
    );
}

/// RULE: a request whose schema cannot hold together is refused with HTTP 400
/// `ValidationException` and persists nothing.
#[tokio::test]
async fn a_schema_that_cannot_hold_together_is_refused_and_persists_nothing() {
    // One row per rule the model states, plus one value past each enum catalogue.
    let mut cases: Vec<(String, Value)> = vec![
        // "You must include at least one of OnlineStoreConfig and OfflineStoreConfig
        // to create a FeatureGroup."
        ("no-store-at-all".to_string(), json!({ "OnlineStoreConfig": null })),
        // `RoleArn` is the role that persists data into the OfflineStore.
        (
            "offline-store-without-a-role".to_string(),
            json!({
                "OnlineStoreConfig": null,
                "OfflineStoreConfig": offline_store("s3://feature-store/offline/"),
            }),
        ),
        // "RecordIdentifierFeatureName must be one of feature definitions' names."
        (
            "record-identifier-not-declared".to_string(),
            json!({ "RecordIdentifierFeatureName": "absent-feature" }),
        ),
        (
            "event-time-not-declared".to_string(),
            json!({ "EventTimeFeatureName": "absent-feature" }),
        ),
        // "An EventTime can be a String or Fractional."
        (
            "event-time-is-integral".to_string(),
            json!({
                "FeatureDefinitions": [
                    { "FeatureName": "customer-id", "FeatureType": "String" },
                    { "FeatureName": "event-time", "FeatureType": "Integral" },
                ],
            }),
        ),
        (
            "a-feature-name-declared-twice".to_string(),
            json!({
                "FeatureDefinitions": [
                    { "FeatureName": "customer-id", "FeatureType": "String" },
                    { "FeatureName": "event-time", "FeatureType": "String" },
                    { "FeatureName": "customer-id", "FeatureType": "Integral" },
                ],
            }),
        ),
        // "With provisioned throughput mode, you specify the read and write capacity."
        (
            "provisioned-without-any-capacity".to_string(),
            json!({ "ThroughputConfig": { "ThroughputMode": "Provisioned" } }),
        ),
        (
            "provisioned-with-only-read-capacity".to_string(),
            json!({
                "ThroughputConfig": {
                    "ThroughputMode": "Provisioned",
                    "ProvisionedReadCapacityUnits": 10,
                },
            }),
        ),
        // One value past each enum catalogue. `ON_DEMAND` is the spelling the prose
        // uses; the model's `ThroughputMode` value is `OnDemand`, so the prose spelling
        // is not an accepted value.
        (
            "throughput-mode-outside-the-enum".to_string(),
            json!({ "ThroughputConfig": { "ThroughputMode": "ON_DEMAND" } }),
        ),
        (
            "storage-type-outside-the-enum".to_string(),
            json!({ "OnlineStoreConfig": { "EnableOnlineStore": true, "StorageType": "Turbo" } }),
        ),
        (
            "ttl-unit-outside-the-enum".to_string(),
            json!({
                "OnlineStoreConfig": {
                    "EnableOnlineStore": true,
                    "TtlDuration": { "Unit": "Fortnights", "Value": 2 },
                },
            }),
        ),
        (
            "table-format-outside-the-enum".to_string(),
            json!({
                "OfflineStoreConfig": {
                    "S3StorageConfig": { "S3Uri": "s3://feature-store/offline/" },
                    "TableFormat": "Parquet",
                },
                "RoleArn": ROLE_ARN,
            }),
        ),
        (
            "feature-type-outside-the-enum".to_string(),
            json!({
                "FeatureDefinitions": [
                    { "FeatureName": "customer-id", "FeatureType": "Decimal" },
                    { "FeatureName": "event-time", "FeatureType": "String" },
                ],
            }),
        ),
        (
            "collection-type-outside-the-enum".to_string(),
            json!({
                "FeatureDefinitions": [
                    { "FeatureName": "customer-id", "FeatureType": "String" },
                    { "FeatureName": "event-time", "FeatureType": "String" },
                    {
                        "FeatureName": "embedding",
                        "FeatureType": "Fractional",
                        "CollectionType": "Bag",
                    },
                ],
            }),
        ),
    ];
    // The three names `FeatureDefinitions`' own doc reserves: "FeatureNames cannot be
    // any of the following: is_deleted, write_time, api_invocation_time". Spelled out
    // here rather than read from the implementation's `state::RESERVED_FEATURE_NAMES`,
    // so a name dropped from that constant loses a case instead of silently losing a row;
    // the assertion below pins the constant to this list as well.
    const RESERVED: &[&str] = &["is_deleted", "write_time", "api_invocation_time"];
    assert_eq!(
        RESERVED_FEATURE_NAMES, RESERVED,
        "the service's reserved-name list must be the three names the model documents"
    );
    for reserved in RESERVED {
        cases.push((
            format!("reserved-feature-name-{reserved}"),
            json!({
                "FeatureDefinitions": [
                    { "FeatureName": "customer-id", "FeatureType": "String" },
                    { "FeatureName": "event-time", "FeatureType": "String" },
                    { "FeatureName": reserved, "FeatureType": "String" },
                ],
            }),
        ));
    }

    for (case, extra) in cases {
        let emulator = Emulator::new();
        let (status, body) = emulator
            .call("CreateFeatureGroup", &feature_group_body("refused", extra))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case}: the create must be refused 400 ValidationException, \
             got {status} {body}"
        );
        assert!(
            !message(&body).is_empty(),
            "case {case}: the refusal must say what was wrong, got {body}"
        );

        let (status, body) = emulator
            .call(
                "DescribeFeatureGroup",
                &json!({ "FeatureGroupName": "refused" }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFound"),
            "case {case}: a refused create must persist nothing, got {status} {body}"
        );
    }
}

/// RULE: every value the model declares for an enum-typed input member is accepted —
/// the mirror of the refusals above, so the gate is the model's value list and not a
/// hand-picked subset.
#[tokio::test]
async fn every_value_the_model_declares_for_an_enum_member_is_accepted() {
    let emulator = Emulator::new();
    let mut accepted = 0;

    // Catalogue source: the generated value lists in `ops::types`, i.e. the model's own
    // enums (`StorageType`, `TtlDurationUnit`, `TableFormat`, `ThroughputMode`,
    // `FeatureType`, `CollectionType`).
    for (index, storage_type) in STORAGE_TYPE_VALUES.iter().enumerate() {
        let extra = json!({
            "OnlineStoreConfig": { "EnableOnlineStore": true, "StorageType": storage_type },
        });
        accepted += accept(&emulator, &format!("storage-type-{index}"), extra).await;
    }
    for (index, unit) in TTL_DURATION_UNIT_VALUES.iter().enumerate() {
        let extra = json!({
            "OnlineStoreConfig": {
                "EnableOnlineStore": true,
                "TtlDuration": { "Unit": unit, "Value": 1 },
            },
        });
        accepted += accept(&emulator, &format!("ttl-unit-{index}"), extra).await;
    }
    for (index, table_format) in TABLE_FORMAT_VALUES.iter().enumerate() {
        let extra = json!({
            "OfflineStoreConfig": {
                "S3StorageConfig": { "S3Uri": "s3://feature-store/offline/" },
                "TableFormat": table_format,
            },
            "RoleArn": ROLE_ARN,
        });
        accepted += accept(&emulator, &format!("table-format-{index}"), extra).await;
    }
    for (index, throughput_mode) in THROUGHPUT_MODE_VALUES.iter().enumerate() {
        let extra = json!({
            "ThroughputConfig": {
                "ThroughputMode": throughput_mode,
                "ProvisionedReadCapacityUnits": 1,
                "ProvisionedWriteCapacityUnits": 1,
            },
        });
        accepted += accept(&emulator, &format!("throughput-mode-{index}"), extra).await;
    }
    // A `FeatureType` is tested on a feature that is neither the record identifier nor
    // the event time, so every type is reachable — including `Integral`, which an
    // EventTime feature may not have.
    for (index, feature_type) in FEATURE_TYPE_VALUES.iter().enumerate() {
        let extra = json!({
            "FeatureDefinitions": [
                { "FeatureName": "customer-id", "FeatureType": "String" },
                { "FeatureName": "event-time", "FeatureType": "String" },
                { "FeatureName": "payload", "FeatureType": feature_type },
            ],
        });
        accepted += accept(&emulator, &format!("feature-type-{index}"), extra).await;
    }
    for (index, collection_type) in COLLECTION_TYPE_VALUES.iter().enumerate() {
        let extra = json!({
            "FeatureDefinitions": [
                { "FeatureName": "customer-id", "FeatureType": "String" },
                { "FeatureName": "event-time", "FeatureType": "String" },
                {
                    "FeatureName": "payload",
                    "FeatureType": "Fractional",
                    "CollectionType": collection_type,
                    "CollectionConfig": { "VectorConfig": { "Dimension": 2 } },
                },
            ],
        });
        accepted += accept(&emulator, &format!("collection-type-{index}"), extra).await;
    }

    let catalogue_size = STORAGE_TYPE_VALUES.len()
        + TTL_DURATION_UNIT_VALUES.len()
        + TABLE_FORMAT_VALUES.len()
        + THROUGHPUT_MODE_VALUES.len()
        + FEATURE_TYPE_VALUES.len()
        + COLLECTION_TYPE_VALUES.len();
    assert_eq!(
        accepted, catalogue_size,
        "every declared enum value must be accepted: {accepted} of {catalogue_size} were"
    );
}

/// Create a feature group named after `case` and require a 200, reading it back to prove
/// it was stored. Answers 1, so the caller can count the rows that passed.
async fn accept(emulator: &Emulator, case: &str, extra: Value) -> usize {
    let (status, body) = emulator
        .call("CreateFeatureGroup", &feature_group_body(case, extra))
        .await;
    assert_eq!(
        status, 200,
        "case {case}: a declared enum value must be accepted, got {status} {body}"
    );
    let (status, body) = emulator
        .call("DescribeFeatureGroup", &json!({ "FeatureGroupName": case }))
        .await;
    assert_eq!(
        status, 200,
        "case {case}: the accepted group must be stored, got {status} {body}"
    );
    1
}

/// RULE: a `FeatureGroupName` already in use is refused with the modelled
/// `ResourceInUse`, and the stored group is left exactly as the first create wrote it.
#[tokio::test]
async fn a_feature_group_name_already_in_use_is_refused_and_the_stored_group_survives() {
    // Rows differ in how far the second request differs from the first: same schema,
    // a different schema, and a different set of stores.
    let seconds: Vec<(&str, Value)> = vec![
        ("identical-request", json!({})),
        (
            "different-schema",
            json!({
                "FeatureDefinitions": [
                    { "FeatureName": "customer-id", "FeatureType": "String" },
                    { "FeatureName": "event-time", "FeatureType": "String" },
                    { "FeatureName": "extra", "FeatureType": "Integral" },
                ],
            }),
        ),
        (
            "different-stores",
            json!({
                "OfflineStoreConfig": offline_store("s3://feature-store/second/"),
                "RoleArn": ROLE_ARN,
                "Description": "the second request",
            }),
        ),
    ];

    for (case, extra) in seconds {
        let emulator = Emulator::new();
        let first = feature_group_body("taken", json!({}));
        emulator.ok("CreateFeatureGroup", &first).await;

        let (status, body) = emulator
            .call("CreateFeatureGroup", &feature_group_body("taken", extra))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceInUse"),
            "case {case}: a name already in use must be refused 400 ResourceInUse, \
             got {status} {body}"
        );

        let described = emulator
            .ok(
                "DescribeFeatureGroup",
                &json!({ "FeatureGroupName": "taken" }),
            )
            .await;
        for member in ECHOED_MEMBERS {
            assert_eq!(
                described.get(member),
                first.get(member),
                "case {case}: {member} must still be the first create's, got {described:?}"
            );
        }
    }
}

/// RULE: a feature group the account does not hold is refused with the modelled
/// `ResourceNotFound`, whether it is named by name or by ARN.
#[tokio::test]
async fn describing_a_feature_group_that_was_never_created_is_refused_with_not_found() {
    let emulator = Emulator::new();
    emulator
        .ok("CreateFeatureGroup", &feature_group_body("present", json!({})))
        .await;

    let identifiers = [
        "absent".to_string(),
        "present-but-not-quite".to_string(),
        format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:feature-group/absent"),
    ];
    for identifier in identifiers {
        let (status, body) = emulator
            .call(
                "DescribeFeatureGroup",
                &json!({ "FeatureGroupName": identifier }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFound"),
            "case {identifier}: an unknown feature group must be refused 400 \
             ResourceNotFound, got {status} {body}"
        );
    }
}

/// RULE: `DescribeFeatureGroup` returns every `FeatureDefinition` of a group in one
/// response, so the only pagination token it issues — and the only one it accepts — is
/// the empty one: a token it never minted is refused 400 `ValidationException`.
#[tokio::test]
async fn a_pagination_token_describe_feature_group_never_issued_is_refused() {
    let emulator = Emulator::new();
    emulator
        .ok("CreateFeatureGroup", &feature_group_body("paged", json!({})))
        .await;

    // Rows differ along the token's shape: a plausible opaque cursor, an echo of the
    // group's own name, hex, and the modelled maximum length (`NextToken` is
    // `len 0..8192`, pattern `.*` — smithy_query operation=DescribeFeatureGroup).
    let refused = [
        "AAAAAUkZ".to_string(),
        "paged".to_string(),
        "deadbeef".to_string(),
        "t".repeat(8192),
    ];
    for token in refused {
        let (status, body) = emulator
            .call(
                "DescribeFeatureGroup",
                &json!({ "FeatureGroupName": "paged", "NextToken": token }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case token-len-{}: a token this operation never issued must be refused 400 \
             ValidationException, got {status} {body}",
            token.len()
        );
    }

    // The two tokens that ARE this operation's own: absent, and the empty token it
    // reports. Both describe the group.
    for token in [None, Some(String::new())] {
        let mut request = json!({ "FeatureGroupName": "paged" });
        if let Some(token) = &token {
            request["NextToken"] = json!(token);
        }
        let (status, body) = emulator.call("DescribeFeatureGroup", &request).await;
        assert_eq!(
            status, 200,
            "case token={token:?}: the operation's own token must be accepted, \
             got {status} {body}"
        );
        assert_eq!(
            body.get("NextToken").and_then(Value::as_str),
            Some(""),
            "case token={token:?}: no page remains to resume, got {body:?}"
        );
    }
}
