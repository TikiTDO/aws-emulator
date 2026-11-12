// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue integration resource property slice:
//! `CreateIntegrationResourceProperty`, the `GetIntegrationResourceProperty` that reads
//! back what it stored, and the `DeleteIntegrationResourceProperty` that removes it.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is the HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: a record is addressed by the ARN of the resource whose properties
//! it carries, and there is ONE record per resource — so a create for a resource that
//! already has one writes that record (each stated member replacing the stored one, each
//! unstated member left alone) under the one `ResourcePropertyArn` that resource derives,
//! rather than minting a second record or refusing the write. One call states ONE side of
//! the integration, so a create stating both sides and a create stating neither are both
//! refused on the request body alone. A DELETE of one record is answered from the record
//! store alone — it resolves no target resource and rejects no resource KIND — so every value
//! this service reads as an ARN of one of its own resources reaches the lookup, and one no
//! record is keyed under is the modelled `EntityNotFoundException` carrying the delete's own
//! record-miss sentence. The round-trip suite is a
//! table whose rows differ along which side of the integration the properties belong to,
//! which resource kind the ARN names, how many members the side carries, their length, and
//! whether the create carried tags.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::ops::types::CreateIntegrationResourcePropertyInput;
use service_com_amazonaws_glue_awsglue::state::{
    ComAmazonawsGlueAwsglueState, IntegrationResourcePropertyEntry,
    MAX_INTEGRATION_RESOURCE_PROPERTIES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{
    assert_echoed, call, emulator, emulator_on, error_code, error_message, ok, ACCOUNT_ID, REGION,
};

/// The longest `RoleArn` either side accepts: "len 1..128" on `String128`
/// (`smithy_query shape=SourceProcessingProperties,TargetProcessingProperties`).
const MAX_ROLE_ARN_LENGTH: usize = 128;

/// The longest `KmsArn`/`EventBusArn` a target carries: "len 1..2048" on `String2048`
/// (same query).
const MAX_KMS_ARN_LENGTH: usize = 2048;

/// The ARN of the Glue connection `name`, as this emulator mints a connection's ARN. Shared
/// with [`crate::integration_resource_property_delete_misses`], so both suites address a
/// resource the one way.
pub(crate) fn connection_arn(name: &str) -> String {
    format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:connection/{name}")
}

/// The ARN of the Glue database `name`, the same way.
fn database_arn(name: &str) -> String {
    format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:database/{name}")
}

/// Register a Glue connection, the resource a source's properties belong to.
async fn create_connection(router: &MultiServiceRouter, name: &str) {
    ok(
        router,
        "CreateConnection",
        &json!({
            "ConnectionInput": {
                "Name": name,
                "ConnectionType": "JDBC",
                "ConnectionProperties": {
                    "JDBC_CONNECTION_URL": "jdbc:postgresql://db.internal:5432/warehouse",
                    "USERNAME": "etl",
                },
            },
        }),
    )
    .await;
}

/// Register a Glue database, the resource a target's properties belong to.
async fn create_database(router: &MultiServiceRouter, name: &str) {
    ok(
        router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": name } }),
    )
    .await;
}

/// Write one integration resource property record and return the response body.
async fn create_property(router: &MultiServiceRouter, body: &Value) -> Value {
    ok(router, "CreateIntegrationResourceProperty", body).await
}

/// The `GetIntegrationResourceProperty` response for `resource_arn`.
async fn get_property(router: &MultiServiceRouter, resource_arn: &str) -> Value {
    ok(
        router,
        "GetIntegrationResourceProperty",
        &json!({ "ResourceArn": resource_arn }),
    )
    .await
}

/// The `ResourcePropertyArn` of a response, which every call that reports a record carries.
fn property_arn(case: &str, body: &Value) -> String {
    body.get("ResourcePropertyArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case}: the response must carry a ResourcePropertyArn, got {body}")
        })
        .to_string()
}

/// Assert that `arn` is the record ARN the resource `resource_arn` names derives: the
/// documented resource type ("The format is something like
/// arn:aws:glue:::integrationresourceproperty/*" —
/// `CreateIntegrationResourcePropertyResponse$ResourcePropertyArn`) under this endpoint's
/// Region and account, carrying the resource the record describes — the resource type and
/// name its own ARN carries — so a caller that holds only the ARN it submitted derives the
/// record's.
fn assert_record_arn(case: &str, resource_arn: &str, arn: &str) {
    let described = resource_arn
        .rsplit_once(':')
        .unwrap_or_else(|| panic!("case {case}: {resource_arn:?} carries no resource portion"))
        .1;
    let expected =
        format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:integrationresourceproperty/{described}");
    assert_eq!(
        arn, expected,
        "case {case}: the record ARN must be derived from the resource it describes; expected \
         {expected:?}, got {arn:?}"
    );
}

// ---------------------------------------------------------------------------
// Create, then read back
// ---------------------------------------------------------------------------

/// What `CreateIntegrationResourceProperty` persists is what
/// `GetIntegrationResourceProperty` reports: both calls answer the record now stored for
/// the ARN the request named, member for member, under the one `ResourcePropertyArn` that
/// resource derives.
#[tokio::test]
async fn created_integration_resource_properties_are_read_back_from_the_store() {
    let router = emulator();

    let long_role = format!("arn:aws:iam::{ACCOUNT_ID}:role/")
        + &"r".repeat(MAX_ROLE_ARN_LENGTH - format!("arn:aws:iam::{ACCOUNT_ID}:role/").len());
    let long_kms = format!("arn:aws:kms:{REGION}:{ACCOUNT_ID}:key/")
        + &"k".repeat(
            MAX_KMS_ARN_LENGTH - format!("arn:aws:kms:{REGION}:{ACCOUNT_ID}:key/").len(),
        );

    // (case, whether the ARN names a connection, the properties the create carries)
    let cases: Vec<(&str, bool, Value)> = vec![
        (
            // The shape a client setting up a source sends: the connection's role alone.
            "a connection's source role",
            true,
            json!({
                "SourceProcessingProperties": {
                    "RoleArn": format!("arn:aws:iam::{ACCOUNT_ID}:role/glue-source"),
                },
            }),
        ),
        (
            "a database's target role",
            false,
            json!({
                "TargetProcessingProperties": {
                    "RoleArn": format!("arn:aws:iam::{ACCOUNT_ID}:role/glue-target"),
                },
            }),
        ),
        (
            "a target carrying every modelled member",
            false,
            json!({
                "TargetProcessingProperties": {
                    "RoleArn": format!("arn:aws:iam::{ACCOUNT_ID}:role/glue-target-full"),
                    "KmsArn": format!("arn:aws:kms:{REGION}:{ACCOUNT_ID}:key/11111111-2222-3333-4444-555555555555"),
                    "ConnectionName": "vpc-network-connection",
                    "EventBusArn": format!("arn:aws:events:{REGION}:{ACCOUNT_ID}:event-bus/integration-status"),
                },
            }),
        ),
        (
            // Each side is written by its own call, so a request stating BOTH is refused
            // ([`a_create_stating_neither_or_both_processing_sides_is_refused`]) and the
            // sides of one resource are built up by successive writes instead.
            "a target stating its network connection alone",
            false,
            json!({
                "TargetProcessingProperties": { "ConnectionName": "vpc-network-connection" },
            }),
        ),
        (
            "tags sent with the create",
            true,
            json!({
                "SourceProcessingProperties": {
                    "RoleArn": format!("arn:aws:iam::{ACCOUNT_ID}:role/glue-tagged"),
                },
                "Tags": [
                    { "key": "owner", "value": "data-platform" },
                    { "key": "tier", "value": "gold" },
                ],
            }),
        ),
        (
            "a role at the modelled length bound",
            true,
            json!({ "SourceProcessingProperties": { "RoleArn": long_role } }),
        ),
        (
            "a KMS ARN at the modelled length bound",
            false,
            json!({ "TargetProcessingProperties": { "KmsArn": long_kms } }),
        ),
        (
            "a role of the shortest length the model admits",
            false,
            json!({ "TargetProcessingProperties": { "RoleArn": "r" } }),
        ),
    ];

    let mut stored: Vec<(&str, String, Value)> = Vec::new();
    for (index, (case, is_connection, properties)) in cases.iter().enumerate() {
        let name = format!("integration-resource-{index}");
        let resource_arn = if *is_connection {
            create_connection(&router, &name).await;
            connection_arn(&name)
        } else {
            create_database(&router, &name).await;
            database_arn(&name)
        };

        let mut request = properties.clone();
        request["ResourceArn"] = json!(resource_arn);
        let created = create_property(&router, &request).await;
        // The two property sides are what a response carries; `Tags` has no member on it.
        let mut sides = properties.clone();
        if let Some(members) = sides.as_object_mut() {
            members.remove("Tags");
        }

        // "The connection ARN of the source, or the database ARN of the target": the
        // create reports the resource it wrote the record for.
        assert_eq!(
            created.get("ResourceArn").and_then(Value::as_str),
            Some(resource_arn.as_str()),
            "case {case}: the create must report the ResourceArn it stored, got {created}"
        );
        let record_arn = property_arn(case, &created);
        assert_record_arn(case, &resource_arn, &record_arn);
        // Every property member the request carried comes back from the store unchanged.
        assert_echoed(case, &sides, &created);
        // `CreateIntegrationResourcePropertyResponse` models no `Tags` member, so the tags
        // a create carried are not part of what it answers.
        assert!(
            created.get("Tags").is_none(),
            "case {case}: the create must report no Tags, got {created}"
        );

        let read = get_property(&router, &resource_arn).await;
        assert_eq!(
            read.get("ResourceArn").and_then(Value::as_str),
            Some(resource_arn.as_str()),
            "case {case}: the read must report the resource it was asked for, got {read}"
        );
        assert_eq!(
            property_arn(case, &read),
            record_arn,
            "case {case}: the read must report the same record ARN the create did, got {read}"
        );
        assert_echoed(case, &sides, &read);

        stored.push((case, resource_arn, sides));
    }

    // Each resource has its OWN record: reading one back after every create has landed
    // still reports that resource's properties and its own ARN, so no create overwrote
    // another resource's record.
    let mut record_arns: Vec<String> = Vec::new();
    for (case, resource_arn, properties) in &stored {
        let read = get_property(&router, resource_arn).await;
        assert_echoed(case, properties, &read);
        record_arns.push(property_arn(case, &read));
    }
    let distinct: std::collections::BTreeSet<&String> = record_arns.iter().collect();
    assert_eq!(
        distinct.len(),
        record_arns.len(),
        "case every record: each resource must carry its own ResourcePropertyArn, got \
         {record_arns:?}"
    );
}

/// A create for a resource that already has a record writes THAT record: it answers 200
/// rather than refusing a duplicate, reports the same `ResourcePropertyArn` as the first
/// create, replaces every member it states and leaves every member it does not state as
/// stored — and the store still holds one record per resource.
#[tokio::test]
async fn a_second_create_for_one_resource_writes_the_record_it_already_has() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());

    let source = |role: &str| json!({ "SourceProcessingProperties": { "RoleArn": role } });
    let target = |role: &str| json!({ "TargetProcessingProperties": { "RoleArn": role } });

    // (case, the first create's properties, the second create's, what must then be stored)
    let cases: Vec<(&str, Value, Value, Value)> = vec![
        (
            "the same side with another role replaces it",
            source("arn:aws:iam::000000000000:role/first"),
            source("arn:aws:iam::000000000000:role/second"),
            source("arn:aws:iam::000000000000:role/second"),
        ),
        (
            "the other side added leaves the first side stored",
            source("arn:aws:iam::000000000000:role/kept-source"),
            target("arn:aws:iam::000000000000:role/added-target"),
            json!({
                "SourceProcessingProperties": { "RoleArn": "arn:aws:iam::000000000000:role/kept-source" },
                "TargetProcessingProperties": { "RoleArn": "arn:aws:iam::000000000000:role/added-target" },
            }),
        ),
        (
            "a member of a stated side replaces only that side",
            json!({
                "TargetProcessingProperties": {
                    "RoleArn": "arn:aws:iam::000000000000:role/tgt",
                    "ConnectionName": "first-network",
                },
            }),
            json!({
                "TargetProcessingProperties": {
                    "RoleArn": "arn:aws:iam::000000000000:role/tgt",
                    "ConnectionName": "second-network",
                },
            }),
            json!({
                "TargetProcessingProperties": {
                    "RoleArn": "arn:aws:iam::000000000000:role/tgt",
                    "ConnectionName": "second-network",
                },
            }),
        ),
        (
            "tags sent beside a write of the stored side leave both sides as stored",
            json!({
                "SourceProcessingProperties": { "RoleArn": "arn:aws:iam::000000000000:role/tags-source" },
            }),
            json!({
                "SourceProcessingProperties": { "RoleArn": "arn:aws:iam::000000000000:role/tags-source" },
                "Tags": [{ "key": "owner", "value": "data-platform" }],
            }),
            json!({
                "SourceProcessingProperties": { "RoleArn": "arn:aws:iam::000000000000:role/tags-source" },
            }),
        ),
        (
            "the identical create repeated changes nothing",
            source("arn:aws:iam::000000000000:role/idempotent"),
            source("arn:aws:iam::000000000000:role/idempotent"),
            source("arn:aws:iam::000000000000:role/idempotent"),
        ),
    ];

    for (index, (case, first, second, expected)) in cases.iter().enumerate() {
        let name = format!("upserted-{index}");
        create_connection(&router, &name).await;
        let resource_arn = connection_arn(&name);

        let mut request = first.clone();
        request["ResourceArn"] = json!(resource_arn);
        let created = create_property(&router, &request).await;
        let record_arn = property_arn(case, &created);

        let mut again = second.clone();
        again["ResourceArn"] = json!(resource_arn);
        let (status, body) = call(&router, "CreateIntegrationResourceProperty", &again).await;
        assert_eq!(
            status, 200,
            "case {case}: a second create for one resource must answer 200, got {status} with \
             {body}"
        );
        assert_eq!(
            property_arn(case, &body),
            record_arn,
            "case {case}: the record keeps the ResourcePropertyArn the first create \
             reported, expected {record_arn}, got {body}"
        );
        assert_echoed(case, expected, &body);

        let read = get_property(&router, &resource_arn).await;
        assert_echoed(case, expected, &read);
        assert_eq!(
            property_arn(case, &read),
            record_arn,
            "case {case}: the read must report the same record ARN, expected {record_arn}, got \
             {read}"
        );
    }

    // One record per resource: the second create of each row wrote the record its row's
    // resource already had rather than adding one.
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let held = state.lock().integration_resource_properties.len();
    assert_eq!(
        held,
        cases.len(),
        "case every row: expected one record per resource ({}), got {held}",
        cases.len()
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// The sentence a create that states neither processing side is refused with: fixed text
/// naming the two sides, carrying no value read off the request or the store.
const NEITHER_SIDE_MESSAGE: &str = "Either source processing properties or target \
     processing properties must be provided in the request.";

/// The sentence a create that states BOTH processing sides is refused with — the two sides
/// belong to two different resources and are written by two calls — again fixed text naming
/// the two sides in prose.
const BOTH_SIDES_MESSAGE: &str = "Both source processing properties and target processing \
     properties cannot be provided in the same request.";

/// The sentence a read answers for a resource this account holds no record for, which this
/// surface words by the LEG the ARN names: a Glue connection is the integration's SOURCE and
/// its miss names the record the lookup did not find, while every other value is read as the
/// integration's TARGET and its miss names that target resource.
fn read_miss_message(resource_arn: &str, names_the_source: bool) -> String {
    if names_the_source {
        record_miss_message(resource_arn)
    } else {
        format!("The target resource {resource_arn} is not found.")
    }
}

/// The sentence a miss of the RECORD carries: the lookup this surface makes in the
/// per-resource namespace it keeps, named as the service names it, around its one variable
/// part — the ARN the request addressed the record by. It is the whole of what a DELETE
/// answers for a `ResourceArn` this account holds no record under (the delete resolves no
/// target to report instead), and the sentence a READ answers for the SOURCE leg.
/// Written here independently of the handler, so the suites pin the wording and not the code,
/// and shared with [`crate::integration_resource_property_delete_misses`] so they cannot
/// disagree about it.
pub(crate) fn record_miss_message(resource_arn: &str) -> String {
    format!("Failed to find the resource property with arn {resource_arn} in the table")
}

/// One call states ONE side of the integration, so a request stating NEITHER
/// `SourceProcessingProperties` nor `TargetProcessingProperties` and a request stating BOTH
/// are each refused with the modelled `ValidationException` and persist nothing — whichever
/// resource kind they address, whatever else they carry, and whether or not that resource
/// already has a record — while a request stating exactly one side is served. The rows differ
/// along which sides the request states, how many members those sides carry, and what the
/// request carries beside them.
#[tokio::test]
async fn a_create_stating_neither_or_both_processing_sides_is_refused() {
    let router = emulator();

    // (case, whether the ARN names a connection, the members beside `ResourceArn`, the
    // sentence the refusal must carry, or `None` when the create must be served)
    let cases: Vec<(&str, bool, Value, Option<&str>)> = vec![
        (
            // The recorded case: a database ARN and nothing else.
            "neither side, on a database",
            false,
            json!({}),
            Some(NEITHER_SIDE_MESSAGE),
        ),
        (
            "neither side, on a connection",
            true,
            json!({}),
            Some(NEITHER_SIDE_MESSAGE),
        ),
        (
            "neither side, with tags alone",
            false,
            json!({ "Tags": [{ "key": "owner", "value": "data-platform" }] }),
            Some(NEITHER_SIDE_MESSAGE),
        ),
        (
            // The recorded case: both sides in one request, on the connection the source
            // leg is addressed by.
            "both sides at once",
            true,
            json!({
                "SourceProcessingProperties": {
                    "RoleArn": format!("arn:aws:iam::{ACCOUNT_ID}:role/glue-both-source"),
                },
                "TargetProcessingProperties": {
                    "RoleArn": format!("arn:aws:iam::{ACCOUNT_ID}:role/glue-both-target"),
                    "ConnectionName": "both-sides-connection",
                },
            }),
            Some(BOTH_SIDES_MESSAGE),
        ),
        (
            "both sides at once, each carrying nothing",
            false,
            json!({
                "SourceProcessingProperties": {},
                "TargetProcessingProperties": {},
            }),
            Some(BOTH_SIDES_MESSAGE),
        ),
        (
            "both sides at once, beside tags",
            false,
            json!({
                "SourceProcessingProperties": { "RoleArn": "arn:aws:iam::000000000000:role/s" },
                "TargetProcessingProperties": { "KmsArn": "arn:aws:kms:us-east-1:000000000000:key/k" },
                "Tags": [{ "key": "tier", "value": "gold" }],
            }),
            Some(BOTH_SIDES_MESSAGE),
        ),
        (
            "the source side alone",
            true,
            json!({
                "SourceProcessingProperties": {
                    "RoleArn": format!("arn:aws:iam::{ACCOUNT_ID}:role/glue-source"),
                },
            }),
            None,
        ),
        (
            "the target side alone",
            false,
            json!({
                "TargetProcessingProperties": {
                    "RoleArn": format!("arn:aws:iam::{ACCOUNT_ID}:role/glue-target"),
                },
            }),
            None,
        ),
        (
            "a side stated beside tags",
            false,
            json!({
                "TargetProcessingProperties": {
                    "RoleArn": format!("arn:aws:iam::{ACCOUNT_ID}:role/glue-tagged-target"),
                },
                "Tags": [{ "key": "tier", "value": "gold" }],
            }),
            None,
        ),
    ];

    for (index, (case, is_connection, members, refusal)) in cases.iter().enumerate() {
        let name = format!("side-required-{index}");
        let resource_arn = if *is_connection {
            create_connection(&router, &name).await;
            connection_arn(&name)
        } else {
            create_database(&router, &name).await;
            database_arn(&name)
        };

        let mut request = members.clone();
        request["ResourceArn"] = json!(resource_arn);
        let (status, body) = call(&router, "CreateIntegrationResourceProperty", &request).await;
        let Some(expected) = refusal else {
            assert_eq!(
                status, 200,
                "case {case:?}: a create stating one side must be served, got {status} with {body}"
            );
            continue;
        };

        assert_eq!(
            status, 400,
            "case {case:?}: a create stating neither side or both sides is refused, got {status} \
             with {body}"
        );
        assert!(
            error_code(&body).ends_with("ValidationException"),
            "case {case:?}: expected ValidationException, got {}",
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            *expected,
            "case {case:?}: expected the fixed refusal sentence, got {:?}",
            error_message(&body)
        );

        // The refusal persisted nothing, so the resource still has no record.
        let (read_status, read_body) = call(
            &router,
            "GetIntegrationResourceProperty",
            &json!({ "ResourceArn": resource_arn }),
        )
        .await;
        assert_eq!(
            read_status, 400,
            "case {case:?}: the refused create must have stored nothing, got {read_status} with \
             {read_body}"
        );
        assert!(
            error_code(&read_body).ends_with("EntityNotFoundException"),
            "case {case:?}: expected EntityNotFoundException from the read, got {}",
            error_code(&read_body)
        );
        assert_eq!(
            error_message(&read_body),
            read_miss_message(&resource_arn, *is_connection),
            "case {case:?}: expected the miss this resource's leg earns, got {:?}",
            error_message(&read_body)
        );
    }

    // The requirement is on the REQUEST, not on what the store already holds: a create
    // stating neither side is refused for a resource that already has a record too, and
    // leaves that record exactly as it was.
    create_connection(&router, "already-recorded").await;
    let resource_arn = connection_arn("already-recorded");
    let stored_role = format!("arn:aws:iam::{ACCOUNT_ID}:role/glue-stored");
    let created = create_property(
        &router,
        &json!({
            "ResourceArn": resource_arn,
            "SourceProcessingProperties": { "RoleArn": &stored_role },
        }),
    )
    .await;
    let record_arn = property_arn("a resource that already has a record", &created);

    let (status, body) = call(
        &router,
        "CreateIntegrationResourceProperty",
        &json!({ "ResourceArn": resource_arn }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {:?}: a create stating neither side is refused, got {status} with {body}",
        "a resource that already has a record"
    );
    assert_eq!(
        error_message(&body),
        NEITHER_SIDE_MESSAGE,
        "case {:?}: expected the fixed refusal sentence, got {:?}",
        "a resource that already has a record",
        error_message(&body)
    );
    let read = get_property(&router, &resource_arn).await;
    assert_echoed(
        "a resource that already has a record",
        &json!({ "SourceProcessingProperties": { "RoleArn": &stored_role } }),
        &read,
    );
    assert_eq!(
        property_arn("a resource that already has a record", &read),
        record_arn,
        "case {:?}: the record keeps the ResourcePropertyArn the first create reported, expected \
         {record_arn}, got {read}",
        "a resource that already has a record"
    );
}

/// A `ResourceArn` that names no Glue connection and no Glue database is refused by every
/// operation of this family, and each one classifies it by WHAT the value is rather than by
/// what the operation is doing: a value outside Glue's own namespace is a validation failure
/// naming that value, while a Glue ARN of another kind reaches each operation's own lookup and
/// is the modelled `EntityNotFoundException`. The three operations WORD the validation failure
/// differently — the create states the target it writes against, the read the resource it was
/// asked for, the delete this service's general reading of the member — so each sentence is
/// asserted per operation. The rows differ along how the value misses: no ARN at all, another
/// service's namespace, three Glue ARNs of other resource kinds, a Glue ARN with no resource
/// type, and a Glue ARN carrying no resource name.
#[tokio::test]
async fn a_resource_arn_naming_no_connection_or_database_is_refused() {
    let router = emulator();

    // (case, the value sent in `ResourceArn`, whether it is an ARN in Glue's own namespace)
    for (case, resource_arn, in_glue_namespace) in [
        ("not an ARN at all", "my-connection".to_string(), false),
        (
            "another service's namespace",
            "arn:aws:s3:::bucket/key".to_string(),
            false,
        ),
        (
            // The recorded case: the zero-ETL target lives in another service.
            "a Redshift Serverless namespace ARN",
            format!("arn:aws:redshift-serverless:{REGION}:{ACCOUNT_ID}:namespace/warehouse"),
            false,
        ),
        (
            "a Glue crawler ARN",
            format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:crawler/nightly"),
            true,
        ),
        (
            "a Glue job ARN",
            format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:job/etl"),
            true,
        ),
        (
            // The recorded case: a catalog table is neither leg of an integration.
            "a Glue table ARN",
            format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:table/sales/orders"),
            true,
        ),
        (
            "the Data Catalog's own ARN",
            format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:catalog"),
            true,
        ),
        (
            "a connection ARN carrying no name",
            format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:connection/"),
            true,
        ),
    ] {
        // (the operation, the error the model names, the sentence it carries)
        let expected: [(&str, &str, String); 3] = [
            (
                "CreateIntegrationResourceProperty",
                if in_glue_namespace {
                    "EntityNotFoundException"
                } else {
                    "ValidationException"
                },
                if in_glue_namespace {
                    format!("The target resource {resource_arn} is not found.")
                } else {
                    format!("The glue target ARN {resource_arn} is invalid.")
                },
            ),
            (
                "GetIntegrationResourceProperty",
                if in_glue_namespace {
                    "EntityNotFoundException"
                } else {
                    "ValidationException"
                },
                if in_glue_namespace {
                    // None of these values names the source's connection, so each is read
                    // as the integration's target.
                    read_miss_message(&resource_arn, false)
                } else {
                    format!("The glue resource ARN {resource_arn} is invalid.")
                },
            ),
            (
                "DeleteIntegrationResourceProperty",
                if in_glue_namespace {
                    "EntityNotFoundException"
                } else {
                    "ValidationException"
                },
                if in_glue_namespace {
                    record_miss_message(&resource_arn)
                } else {
                    format!("Encountered invalid resource arn {resource_arn}")
                },
            ),
        ];

        for (operation, expected_code, expected_message) in expected {
            let mut request = json!({ "ResourceArn": &resource_arn });
            if operation.starts_with("Create") {
                request["SourceProcessingProperties"] =
                    json!({ "RoleArn": "arn:aws:iam::000000000000:role/r" });
            }
            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                status, 400,
                "case {case} ({operation}): expected 400 for {resource_arn:?}, got {status} with \
                 {body}"
            );
            assert!(
                error_code(&body).ends_with(expected_code),
                "case {case} ({operation}): expected {expected_code} for {resource_arn:?}, got {}",
                error_code(&body)
            );
            assert_eq!(
                error_message(&body),
                expected_message,
                "case {case} ({operation}): expected {expected_message:?}, got {:?}",
                error_message(&body)
            );
        }
    }
}

/// The properties are the named resource's own, so a `ResourceArn` naming a connection or
/// database this account does not hold earns the modelled `EntityNotFoundException`, and
/// nothing is stored for it. The rows differ along how the resource misses: a connection
/// never created, a database never created, a stored name in another account, and a stored
/// name in the other resource's kind.
#[tokio::test]
async fn a_resource_arn_naming_an_absent_resource_is_refused() {
    let router = emulator();
    create_connection(&router, "stored-connection").await;
    create_database(&router, "stored-database").await;

    for (case, resource_arn) in [
        ("a connection never created", connection_arn("absent")),
        ("a database never created", database_arn("absent")),
        (
            "a stored connection's name in another account",
            format!("arn:aws:glue:{REGION}:111122223333:connection/stored-connection"),
        ),
        (
            "a stored connection's name as a database",
            database_arn("stored-connection"),
        ),
        (
            "a stored database's name as a connection",
            connection_arn("stored-database"),
        ),
    ] {
        let (status, body) = call(
            &router,
            "CreateIntegrationResourceProperty",
            &json!({
                "ResourceArn": resource_arn,
                "SourceProcessingProperties": { "RoleArn": "arn:aws:iam::000000000000:role/r" },
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {resource_arn:?}, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException for {resource_arn:?}, got {body}"
        );

        // A refused create stores nothing, so the read misses too.
        let (status, body) = call(
            &router,
            "GetIntegrationResourceProperty",
            &json!({ "ResourceArn": resource_arn }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: the refused create must have stored nothing, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException from the read, got {body}"
        );
    }

    // The resources that DO exist still take a record, so the misses above are misses and
    // not a broken resolution.
    for (case, resource_arn) in [
        ("the stored connection", connection_arn("stored-connection")),
        ("the stored database", database_arn("stored-database")),
    ] {
        let created = create_property(
            &router,
            &json!({
                "ResourceArn": resource_arn,
                "SourceProcessingProperties": { "RoleArn": "arn:aws:iam::000000000000:role/r" },
            }),
        )
        .await;
        assert_record_arn(case, &resource_arn, &property_arn(case, &created));
    }
}

/// A read for a resource this account holds no record for is refused with the modelled
/// `EntityNotFoundException` — the resource existing is not the record existing. The rows
/// differ along the resource kind, and the resource that DOES have a record still reads
/// back.
#[tokio::test]
async fn reading_a_resource_with_no_record_is_refused() {
    let router = emulator();
    create_connection(&router, "with-record").await;
    create_connection(&router, "without-record").await;
    create_database(&router, "database-without-record").await;
    create_property(
        &router,
        &json!({
            "ResourceArn": connection_arn("with-record"),
            "SourceProcessingProperties": { "RoleArn": "arn:aws:iam::000000000000:role/kept" },
        }),
    )
    .await;

    for (case, resource_arn) in [
        (
            "a connection with no record",
            connection_arn("without-record"),
        ),
        (
            "a database with no record",
            database_arn("database-without-record"),
        ),
    ] {
        let (status, body) = call(
            &router,
            "GetIntegrationResourceProperty",
            &json!({ "ResourceArn": resource_arn }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {resource_arn:?}, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException for {resource_arn:?}, got {body}"
        );
    }

    let read = get_property(&router, &connection_arn("with-record")).await;
    assert_eq!(
        read.get("SourceProcessingProperties")
            .and_then(|side| side.get("RoleArn"))
            .and_then(Value::as_str),
        Some("arn:aws:iam::000000000000:role/kept"),
        "case the resource with a record: expected the stored role, got {read}"
    );
}

/// The store holds a bounded number of records: the create at the ceiling succeeds, the one
/// past it is refused with the closest client error this operation models, a create for a
/// resource that already HAS a record still succeeds at the ceiling (the bound is on
/// records, not on writes), and freeing a slot lets a new resource take one again.
#[tokio::test]
async fn a_create_past_the_integration_resource_property_ceiling_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());

    // Reaching the ceiling over the wire would take a thousand requests and would
    // exercise the transport rather than the bound.
    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        let mut store = state.lock();
        for index in 0..MAX_INTEGRATION_RESOURCE_PROPERTIES - 1 {
            let resource_arn = connection_arn(&format!("seeded-{index}"));
            let input = CreateIntegrationResourcePropertyInput {
                resource_arn: resource_arn.clone(),
                ..CreateIntegrationResourcePropertyInput::default()
            };
            store.integration_resource_properties.insert(
                resource_arn,
                IntegrationResourcePropertyEntry::from_create_input(&input),
            );
        }
    }

    create_connection(&router, "last-resource").await;
    create_connection(&router, "one-too-many").await;
    let last = json!({
        "ResourceArn": connection_arn("last-resource"),
        "SourceProcessingProperties": { "RoleArn": "arn:aws:iam::000000000000:role/last" },
    });
    let past = json!({
        "ResourceArn": connection_arn("one-too-many"),
        "SourceProcessingProperties": { "RoleArn": "arn:aws:iam::000000000000:role/past" },
    });

    create_property(&router, &last).await;
    let (status, body) = call(&router, "CreateIntegrationResourceProperty", &past).await;
    assert_eq!(
        status, 400,
        "case record {} of {MAX_INTEGRATION_RESOURCE_PROPERTIES}: expected 400, got {status} \
         with {body}",
        MAX_INTEGRATION_RESOURCE_PROPERTIES + 1
    );
    assert!(
        error_code(&body).contains("InvalidInputException"),
        "case one record too many: expected InvalidInputException, got {body}"
    );

    // A write to a record the store already holds adds nothing, so it is served at the
    // ceiling.
    let again = create_property(
        &router,
        &json!({
            "ResourceArn": connection_arn("last-resource"),
            "SourceProcessingProperties": { "RoleArn": "arn:aws:iam::000000000000:role/again" },
        }),
    )
    .await;
    assert_eq!(
        again
            .get("SourceProcessingProperties")
            .and_then(|side| side.get("RoleArn"))
            .and_then(Value::as_str),
        Some("arn:aws:iam::000000000000:role/again"),
        "case a write at the ceiling: expected the role the write stated, got {again}"
    );

    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        state
            .lock()
            .integration_resource_properties
            .remove(&connection_arn("seeded-0"));
    }
    create_property(&router, &past).await;
}

// ---------------------------------------------------------------------------
// Delete
// ---------------------------------------------------------------------------

/// `DeleteIntegrationResourceProperty` removes the record of the resource its `ResourceArn`
/// names and nothing else: the read of that resource then misses, a second delete is that
/// same miss, every other resource's record still reads back, and a create after the delete
/// stores the record afresh under the ARN its resource derives. A `ResourceArn` this family
/// can address no record under is refused as this delete words that refusal.
///
/// The rows differ along which resource kind the record belongs to and which side of the
/// integration its properties state.
#[tokio::test]
async fn deleting_an_integration_resource_property_removes_only_the_record_it_names() {
    let router = emulator();

    // (case, the ARN of the resource, the side the create states)
    let mut rows: Vec<(&str, String, Value)> = Vec::new();
    create_connection(&router, "deleted-connection").await;
    rows.push((
        "a connection's source properties",
        connection_arn("deleted-connection"),
        json!({ "SourceProcessingProperties": { "RoleArn": "arn:aws:iam::000000000000:role/s" } }),
    ));
    create_database(&router, "deleted_database").await;
    rows.push((
        "a database's target properties",
        database_arn("deleted_database"),
        json!({ "TargetProcessingProperties": { "RoleArn": "arn:aws:iam::000000000000:role/t" } }),
    ));

    // A bystander, to prove each delete is scoped to the record it names.
    create_connection(&router, "kept-connection").await;
    let bystander = connection_arn("kept-connection");
    create_property(
        &router,
        &json!({
            "ResourceArn": &bystander,
            "SourceProcessingProperties": { "RoleArn": "arn:aws:iam::000000000000:role/kept" },
        }),
    )
    .await;

    for (case, resource_arn, properties) in &rows {
        let mut request = properties.clone();
        request["ResourceArn"] = json!(resource_arn);
        let created = create_property(&router, &request).await;
        let record_arn = property_arn(case, &created);
        assert_record_arn(case, resource_arn, &record_arn);

        ok(
            &router,
            "DeleteIntegrationResourceProperty",
            &json!({ "ResourceArn": resource_arn }),
        )
        .await;

        // The record is gone: the read of that resource misses, which is what makes the
        // delete observable.
        for (attempt, operation) in [
            ("the read after the delete", "GetIntegrationResourceProperty"),
            ("a second delete", "DeleteIntegrationResourceProperty"),
        ] {
            let (status, body) =
                call(&router, operation, &json!({ "ResourceArn": resource_arn })).await;
            assert_eq!(
                status, 400,
                "case {case}: {attempt} must be refused, got {status} with {body}"
            );
            assert!(
                error_code(&body).contains("EntityNotFoundException"),
                "case {case}: {attempt} expected EntityNotFoundException, got {body}"
            );
        }

        // A create after the delete stores the record afresh, under the ARN the resource
        // derives — the identity is the resource's, not the write's.
        let reborn = create_property(&router, &request).await;
        assert_eq!(
            property_arn(case, &reborn),
            record_arn,
            "case {case}: the re-created record must carry the same ARN, got {reborn}"
        );
    }

    // A value this family can address no record under is refused as each operation words it:
    // a Glue ARN of another kind reaches the delete's own lookup and misses, while a value
    // outside Glue's namespace is the validation failure this delete words.
    for (case, resource_arn, expected_code, expected_message) in [
        (
            "a catalog table's ARN",
            format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:table/deleted_database/orders"),
            "EntityNotFoundException",
            None,
        ),
        (
            "a value that is no ARN at all",
            "deleted-connection".to_string(),
            "ValidationException",
            Some("Encountered invalid resource arn"),
        ),
    ] {
        let (status, body) = call(
            &router,
            "DeleteIntegrationResourceProperty",
            &json!({ "ResourceArn": resource_arn }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {resource_arn:?}, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with(expected_code),
            "case {case}: expected {expected_code} for {resource_arn:?}, got {}",
            error_code(&body)
        );
        let expected = match expected_message {
            Some(prefix) => format!("{prefix} {resource_arn}"),
            None => record_miss_message(&resource_arn),
        };
        assert_eq!(
            error_message(&body),
            expected,
            "case {case}: expected {expected:?}, got {:?}",
            error_message(&body)
        );
    }

    // Nothing any delete did reached the record it did not name.
    let kept = get_property(&router, &bystander).await;
    assert_eq!(
        kept.get("SourceProcessingProperties")
            .and_then(|side| side.get("RoleArn"))
            .and_then(Value::as_str),
        Some("arn:aws:iam::000000000000:role/kept"),
        "case the bystander: a delete must leave every other record in place, got {kept}"
    );
}

/// An ARN in Glue's own namespace, in this endpoint's Region and account, whose resource
/// portion is `resource`. The two kinds a record belongs to have their own helpers
/// ([`connection_arn`], [`database_arn`]); this one addresses every other kind and every
/// other form the member can carry, through the crate's one local-ARN builder.
fn local_glue_arn(resource: &str) -> String {
    crate::resource_tagging::glue_arn(REGION, ACCOUNT_ID, resource)
}

/// A delete of ONE record is answered from the record store alone: it resolves no target
/// resource and rejects no resource KIND, so every value this service reads as an ARN of one
/// of its own resources reaches the lookup, and one no record is keyed under is the modelled
/// `EntityNotFoundException` carrying the delete's own record-miss sentence — whatever
/// resource kind the ARN names, and whether or not this account holds that resource.
///
/// The rows differ along which Glue resource kind the ARN names, whether that resource is
/// held, which account, Region and partition the ARN carries, and the form of its resource
/// portion.
#[tokio::test]
async fn a_delete_of_any_glue_arn_with_no_record_is_the_record_miss() {
    let router = emulator();

    // Resources this account really holds, so the rows naming them are a missing RECORD and
    // not a missing resource. The table is seeded through the sibling suite's helper, the one
    // copy both integration-property suites use.
    create_connection(&router, "held-connection").await;
    create_database(&router, "held_database").await;
    crate::integration_table_properties::create_table(&router, "held_database", "orders").await;

    // (case, the value sent in `ResourceArn`). The resource portions are the ARN forms of
    // Glue's own resource types; only the two legs of an integration carry these properties
    // — "The connection ARN of the source, or the database ARN of the target"
    // (`smithy_query shape=CreateIntegrationResourcePropertyRequest member=ResourceArn`) — so
    // every other kind here is a resource that could never hold one.
    let rows: Vec<(&str, String)> = vec![
        // The source leg: the connection is held, its record is not.
        (
            "a held connection with no record",
            connection_arn("held-connection"),
        ),
        (
            "a connection this account does not hold",
            connection_arn("absent-connection"),
        ),
        // The target leg.
        (
            "a held database with no record",
            database_arn("held_database"),
        ),
        (
            "a database this account does not hold",
            database_arn("absent_database"),
        ),
        // Kinds that hold no integration resource property at all. The values the suite's
        // `a_resource_arn_naming_no_connection_or_database_is_refused` table already sends
        // for this operation are not repeated here; these are the rest of the kind axis,
        // plus a table this account really holds.
        (
            "a held catalog table",
            local_glue_arn("table/held_database/orders"),
        ),
        ("a trigger", local_glue_arn("trigger/on-demand")),
        ("a workflow", local_glue_arn("workflow/ingest")),
        ("a development endpoint", local_glue_arn("devEndpoint/notebook")),
        ("a schema registry", local_glue_arn("registry/default-registry")),
        ("a schema set", local_glue_arn("schema/default-registry/payments")),
        ("a machine learning transform", local_glue_arn("mlTransform/tfm-1")),
        ("a session", local_glue_arn("session/interactive-1")),
        ("a blueprint", local_glue_arn("blueprint/starter")),
        (
            "a record's own ARN rather than its resource's",
            local_glue_arn("integrationresourceproperty/connection/held-connection"),
        ),
        // The ARN's identity fields are part of the value a record is keyed under, so an ARN
        // of another estate is one this store holds nothing for either.
        (
            "another account's connection",
            format!("arn:aws:glue:{REGION}:111122223333:connection/elsewhere"),
        ),
        (
            "another Region's database",
            format!("arn:aws:glue:eu-west-1:{ACCOUNT_ID}:database/elsewhere"),
        ),
        (
            "another partition's connection",
            format!("arn:aws-cn:glue:cn-north-1:{ACCOUNT_ID}:connection/elsewhere"),
        ),
        // Forms of the resource portion a reader might treat specially. The last three are
        // generalisation rows the recorded corpus does not cover: they follow from reading
        // the member as a Glue ARN and nothing more, so a future reading of one of those
        // forms is a change of rule rather than a regression of a recorded answer.
        (
            "a resource portion with no type segment",
            local_glue_arn("not-a-type"),
        ),
        (
            "a colon-separated resource portion",
            local_glue_arn("connection:held-connection"),
        ),
        (
            "a resource portion of several segments",
            local_glue_arn("connection/team/held-connection"),
        ),
        ("an empty resource portion", local_glue_arn("")),
    ];

    for (case, resource_arn) in &rows {
        let (status, body) = call(
            &router,
            "DeleteIntegrationResourceProperty",
            &json!({ "ResourceArn": resource_arn }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: a delete of a record nothing stored must be refused, got {status} \
             with {body}"
        );
        assert!(
            error_code(&body).ends_with("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException for {resource_arn:?}, got {}",
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            record_miss_message(resource_arn),
            "case {case}: expected {:?}, got {:?}",
            record_miss_message(resource_arn),
            error_message(&body)
        );
    }

    // The refusals changed nothing: the resources those rows named are still read back by
    // their own readers, and a record written afterwards for one of them still reads back —
    // so the misses are misses and not a lookup that refuses everything.
    let read = ok(
        &router,
        "GetConnection",
        &json!({ "Name": "held-connection" }),
    )
    .await;
    assert_eq!(
        read["Connection"]["Name"].as_str(),
        Some("held-connection"),
        "case the held connection: the refused deletes must leave it as it was, got {read}"
    );
    create_property(
        &router,
        &json!({
            "ResourceArn": connection_arn("held-connection"),
            "SourceProcessingProperties": { "RoleArn": "arn:aws:iam::000000000000:role/after" },
        }),
    )
    .await;
    let read = get_property(&router, &connection_arn("held-connection")).await;
    assert_eq!(
        read.get("SourceProcessingProperties")
            .and_then(|side| side.get("RoleArn"))
            .and_then(Value::as_str),
        Some("arn:aws:iam::000000000000:role/after"),
        "case the held connection: a record written after the misses must read back, got \
         {read}"
    );
}
