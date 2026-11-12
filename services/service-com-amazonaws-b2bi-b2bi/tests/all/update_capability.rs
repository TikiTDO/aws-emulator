// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateCapability`, driven over the wire (awsJson1_0,
//! `x-amz-target: B2BI.<Operation>`) through the same router the emulator boots, so every
//! assertion is about what a real SDK client observes.
//!
//! Each test states a rule and asserts it over a TABLE of inputs rather than one recorded
//! request: the rows differ along what the rule generalises over (which members the update
//! names, the size of the document list it submits, the configuration it writes, the form
//! of the id that resolves to nothing, which of the two locations names an unowned
//! bucket).
//!
//! The wire harness (`b2bi_router`, `call`, `error_code`, `text`) and the `given_*`
//! fixtures are reused from the sibling suites rather than copied.

use serde_json::{json, Value};
use service_com_amazonaws_b2bi_b2bi::scaffolded::NOT_FOUND_STATUS;
use smithy_server_runtime::MultiServiceRouter;

use crate::onboarding_loop::{
    b2bi_router, call, capability_configuration, error_code, given_capability,
    given_capability_buckets, given_profile, given_transformer, text, ACCOUNT,
    CAPABILITY_INPUT_BUCKET, CAPABILITY_OUTPUT_BUCKET, SIGNING_REGION,
};

/// The HTTP status this service puts on the wire for a modelled client-error refusal —
/// the `@httpError 400` of `ValidationException`.
const VALIDATION_STATUS: u16 = 400;

/// The `ResourceNotFoundException` message AWS reports when `UpdateCapability` addresses a
/// `capabilityId` that resolves to no capability: fixed text naming the member the id
/// arrived in, around the id taken from the request.
fn capability_not_found(capability_id: &str) -> String {
    format!("Capability with capabilityId {capability_id} not found.")
}

/// The `ResourceNotFoundException` message AWS reports for a `transformerId` another
/// resource's configuration references and this account holds no transformer for.
fn transformer_reference_not_found(transformer_id: &str) -> String {
    format!("Transformer with transformerId {transformer_id} not found")
}

/// The `ValidationException` message AWS reports for a capability configuration whose
/// input and/or output location names an Amazon S3 bucket this account does not own.
const BUCKETS_NOT_OWNED: &str = "Customer does not own input and/or output buckets.";

/// `count` well-formed `InstructionsDocuments` entries — members of
/// `com.amazonaws.b2bi#S3Location` (`bucketName`, `key`), with the bucket name inside the
/// modelled `len 3..63`.
fn documents(count: usize) -> Value {
    json!((0..count)
        .map(|index| json!({
            "bucketName": "instructions-bucket",
            "key": format!("doc-{index}.txt")
        }))
        .collect::<Vec<_>>())
}

/// A `CapabilityConfiguration` whose locations name `input` and `output` instead of the
/// two buckets this account owns, so a row can point either end at a bucket nobody holds.
fn configuration_over(transformer_id: &str, input: &str, output: &str) -> Value {
    let mut configuration = capability_configuration(transformer_id, "X12_850", "VERSION_4010");
    configuration["edi"]["inputLocation"]["bucketName"] = json!(input);
    configuration["edi"]["outputLocation"]["bucketName"] = json!(output);
    configuration
}

/// Read a capability back through `GetCapability`.
async fn get_capability(router: &MultiServiceRouter, capability_id: &str) -> Value {
    let (status, fetched) = call(
        router,
        "GetCapability",
        json!({ "capabilityId": capability_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "GetCapability({capability_id}) must succeed, got {status} {fetched}"
    );
    fetched
}

/// One row of the update table: the case name, the `name` the update submits, the
/// `configuration` it submits and the `instructionsDocuments` it submits.
type UpdateCase = (&'static str, Option<String>, Option<Value>, Option<Value>);

/// RULE: an update writes exactly the members the request names onto the stored record —
/// each submitted member REPLACING the stored one wholesale — leaves every member it omits
/// as it was, keeps the system-assigned identity (`capabilityId`, `capabilityArn`, `type`,
/// `createdAt`), stamps `modifiedAt`, and answers with that stored record, which
/// `GetCapability` then reports member for member.
///
/// The rows span an update naming only `name`, only `configuration`, only
/// `instructionsDocuments`, every combination of the three, and none of them at all;
/// document lists of 0, 1, 2, 3 and 5 entries (the bounds of the modelled
/// `InstructionsDocuments`, `len 0..5`); and configurations differing in X12 transaction
/// set, X12 version and the transformer they reference.
#[tokio::test]
async fn an_update_writes_the_named_members_and_leaves_the_rest() {
    let router = b2bi_router();
    let first_transformer = given_transformer(&router, "edi-850-to-json").await;
    let second_transformer = given_transformer(&router, "edi-810-to-xml").await;
    given_capability_buckets(&router).await;

    let initial_configuration =
        capability_configuration(&first_transformer, "X12_850", "VERSION_4010");
    let (status, created) = call(
        &router,
        "CreateCapability",
        json!({
            "name": "capability-under-test",
            "type": "edi",
            "configuration": initial_configuration.clone()
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateCapability must succeed, got {status} {created}"
    );
    let capability_id = text(&created, "capabilityId");

    // Values from the modelled `X12TransactionSet` / `X12Version` enums
    // (`smithy_query(query="shape",shape="X12Details")`).
    let x12_810 = capability_configuration(&second_transformer, "X12_810", "VERSION_5010");
    let x12_997 = capability_configuration(&first_transformer, "X12_997", "VERSION_4030");
    let x12_214 = capability_configuration(&second_transformer, "X12_214", "VERSION_4060");

    let cases: Vec<UpdateCase> = vec![
        ("name only", Some("renamed-once".to_string()), None, None),
        (
            "configuration only: another transaction set, version and transformer",
            None,
            Some(x12_810.clone()),
            None,
        ),
        (
            "instructionsDocuments only: one document",
            None,
            None,
            Some(documents(1)),
        ),
        (
            "instructionsDocuments only: five documents, the modelled maximum",
            None,
            None,
            Some(documents(5)),
        ),
        (
            "instructionsDocuments only: the empty list, the modelled minimum",
            None,
            None,
            Some(documents(0)),
        ),
        (
            "name and configuration together",
            Some("renamed-with-configuration".to_string()),
            Some(x12_997.clone()),
            None,
        ),
        (
            "configuration and documents together",
            None,
            Some(x12_214.clone()),
            Some(documents(2)),
        ),
        (
            "all three members at once",
            Some("renamed-with-everything".to_string()),
            Some(initial_configuration.clone()),
            Some(documents(3)),
        ),
        ("no optional member at all: only the id", None, None, None),
    ];

    // What the store holds as the rows are applied one after another.
    let mut expected_name = text(&created, "name");
    let mut expected_configuration = initial_configuration.clone();
    let mut expected_documents = Value::Null;

    for (case, name, configuration, instructions) in &cases {
        let mut request = json!({ "capabilityId": capability_id.clone() });
        if let Some(name) = name {
            request["name"] = json!(name);
            expected_name = name.clone();
        }
        if let Some(configuration) = configuration {
            request["configuration"] = configuration.clone();
            expected_configuration = configuration.clone();
        }
        if let Some(instructions) = instructions {
            request["instructionsDocuments"] = instructions.clone();
            expected_documents = instructions.clone();
        }

        let (status, updated) = call(&router, "UpdateCapability", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateCapability must succeed, got {status} {updated}"
        );
        assert_eq!(
            text(&updated, "name"),
            expected_name,
            "case {case:?}: expected the name {expected_name:?}, got {:?}",
            updated["name"]
        );
        assert_eq!(
            updated["configuration"], expected_configuration,
            "case {case:?}: expected the configuration last written, got {:?}",
            updated["configuration"]
        );
        assert_eq!(
            updated["instructionsDocuments"], expected_documents,
            "case {case:?}: expected the document list last written, got {:?}",
            updated["instructionsDocuments"]
        );

        // The system-assigned identity and the members this operation cannot write are
        // the create's: an update never re-mints or re-points them.
        for member in ["capabilityId", "capabilityArn", "type", "createdAt"] {
            assert_eq!(
                updated[member], created[member],
                "case {case:?}: {member} must stay the created value, got {:?} instead of {:?}",
                updated[member], created[member]
            );
        }
        assert_eq!(
            text(&updated, "capabilityArn"),
            format!("arn:aws:b2bi:{SIGNING_REGION}:{ACCOUNT}:capability/{capability_id}"),
            "case {case:?}: the ARN must name the request's region and the emulator account"
        );
        assert!(
            !updated["modifiedAt"].is_null(),
            "case {case:?}: an applied update must stamp modifiedAt, got {:?}",
            updated["modifiedAt"]
        );

        // Round trip: the independent reader reports the very record the update wrote.
        let fetched = get_capability(&router, &capability_id).await;
        for member in [
            "capabilityId",
            "capabilityArn",
            "name",
            "type",
            "configuration",
            "instructionsDocuments",
            "createdAt",
            "modifiedAt",
        ] {
            assert_eq!(
                fetched[member], updated[member],
                "case {case:?}: GetCapability disagrees with UpdateCapability on {member}: \
                 {:?} vs {:?}",
                fetched[member], updated[member]
            );
        }
    }

    // The enumeration reports the same record too: `ListCapabilities` summarises the
    // stored capability, so the last name written is the one it lists.
    let (status, listed) = call(&router, "ListCapabilities", json!({})).await;
    assert_eq!(status, 200, "ListCapabilities failed: {status} {listed}");
    let summary = listed["capabilities"]
        .as_array()
        .expect("capabilities is a list")
        .iter()
        .find(|summary| text(summary, "capabilityId") == capability_id)
        .expect("the updated capability is listed")
        .clone();
    assert_eq!(
        text(&summary, "name"),
        expected_name,
        "ListCapabilities must report the updated name, got {:?}",
        summary["name"]
    );
    assert!(
        !summary["modifiedAt"].is_null(),
        "ListCapabilities must report the update's modifiedAt, got {:?}",
        summary["modifiedAt"]
    );
}

/// RULE: the resource an update addresses is resolved before anything else the request
/// carries, so an update for a `capabilityId` this account's store does not hold is the
/// modelled `ResourceNotFoundException` at this service's not-found status, naming that id
/// — never a minted capability, and never a refusal about the configuration that request
/// also carried.
///
/// The rows span a never-minted id, ids at both ends of the modelled `CapabilityId` length
/// (`len 1..64`, `^[a-zA-Z0-9_-]+$`), ids of other b2bi resource kinds that really do
/// exist (a profile, a transformer), a capability that existed and was deleted, and an
/// unknown capability whose request also names an unresolvable transformer.
#[tokio::test]
async fn updating_an_unknown_capability_is_refused_as_not_found() {
    let router = b2bi_router();
    let profile_id = given_profile(&router, "acme").await;
    let transformer_id = given_transformer(&router, "update-capability-404").await;
    given_capability_buckets(&router).await;

    // A capability that really existed and was then deleted: its id resolves to nothing
    // from the delete on, so an update naming it is refused exactly like a never-minted id.
    let deleted = given_capability(&router, "edi-860").await;
    let (status, body) = call(
        &router,
        "DeleteCapability",
        json!({ "capabilityId": deleted }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the capability under test must be deletable, got {status} {body}"
    );

    let valid_configuration = capability_configuration(&transformer_id, "X12_850", "VERSION_4010");

    // (case, the addressed capabilityId, the members the update names beside it)
    let cases: Vec<(&str, String, Value)> = vec![
        (
            "a never-minted capability id",
            "ca-0123456789abcdef0".to_string(),
            json!({ "name": "renamed" }),
        ),
        (
            "a one-character id, the modelled minimum length",
            "c".to_string(),
            json!({ "configuration": valid_configuration.clone() }),
        ),
        (
            "an id of the modelled maximum length",
            "c".repeat(64),
            json!({ "instructionsDocuments": documents(1) }),
        ),
        (
            "underscores and dashes",
            "no_such-CAPABILITY-42".to_string(),
            json!({}),
        ),
        (
            "a profile id that really exists",
            profile_id.clone(),
            json!({ "name": "renamed" }),
        ),
        (
            "a transformer id that really exists",
            transformer_id.clone(),
            json!({ "configuration": valid_configuration.clone() }),
        ),
        (
            "a capability that was deleted",
            deleted.clone(),
            json!({ "name": "renamed" }),
        ),
        (
            "an unknown capability whose configuration also names an unknown transformer",
            "ca-fedcba98765432100".to_string(),
            json!({ "configuration": configuration_over(
                "t-9999999999999999d",
                CAPABILITY_INPUT_BUCKET,
                CAPABILITY_OUTPUT_BUCKET
            ) }),
        ),
    ];

    for (case, addressed, members) in &cases {
        let mut request = members.clone();
        request["capabilityId"] = json!(addressed);
        let (status, body) = call(&router, "UpdateCapability", request).await;
        assert_eq!(
            status, NOT_FOUND_STATUS,
            "case {case:?}: expected {NOT_FOUND_STATUS} for {addressed:?}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
        // `ResourceNotFoundException`'s member is `message` —
        // `smithy_query(query="shape",shape="ResourceNotFoundException")`.
        assert_eq!(
            text(&body, "message"),
            capability_not_found(addressed),
            "case {case:?}: expected the refusal to name {addressed:?}, got {:?}",
            text(&body, "message")
        );

        // The refusal minted nothing: the reader still does not know that id.
        let (status, fetched) = call(
            &router,
            "GetCapability",
            json!({ "capabilityId": addressed }),
        )
        .await;
        assert_eq!(
            status, NOT_FOUND_STATUS,
            "case {case:?}: the refused update must not have created a capability, got \
             {status} {fetched}"
        );
    }
}

/// One row of the refused-configuration table: the case name, the configuration the update
/// submits, the error code it must be refused with, the member that class carries its
/// sentence in, and the message that refusal carries.
type RefusedConfigurationCase = (&'static str, Value, &'static str, &'static str, String);

/// RULE: a configuration an update writes onto a capability is resolved exactly as a
/// create's is, before anything is written — its `transformerId` against this service's
/// stored transformers (the modelled `ResourceNotFoundException`) and each of its input and
/// output locations against the buckets this account owns (the modelled
/// `ValidationException`) — so an update carrying an unresolvable configuration is refused
/// whole and the stored record is left exactly as it was, with no partial write and no
/// `modifiedAt` stamp.
///
/// The rows span the form of the unresolvable transformer id (a plausible generated id, a
/// single character, underscores and dashes, the longest id the model allows, and a
/// transformer that existed and was deleted) and which location names a bucket nobody
/// holds (the input, the output, both).
#[tokio::test]
async fn an_update_whose_configuration_does_not_resolve_is_refused_and_writes_nothing() {
    let router = b2bi_router();
    let transformer_id = given_transformer(&router, "edi-850-to-json").await;
    let capability_id = given_capability(&router, "edi-850").await;
    given_capability_buckets(&router).await;

    // A transformer that really existed and was then deleted: a configuration naming it is
    // refused exactly like one naming an id nobody ever minted.
    let deleted_transformer = given_transformer(&router, "deleted-transformer").await;
    let (status, body) = call(
        &router,
        "DeleteTransformer",
        json!({ "transformerId": deleted_transformer }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the transformer under test must be deletable, got {status} {body}"
    );

    // Well-formed per the modelled `^[a-zA-Z0-9_-]+$` / `len 1..64` on `TransformerId`,
    // so what refuses them is the existence check and not input validation.
    let unknown_transformers: &[(&str, String)] = &[
        ("a plausible generated id", "t-0123456789abcdef0".to_string()),
        ("a single character", "x".to_string()),
        ("underscores and dashes", "no_such-TRANSFORMER-42".to_string()),
        ("the longest id the model allows", "t".repeat(64)),
        ("a transformer that was deleted", deleted_transformer.clone()),
    ];

    // Well-formed per the modelled `BucketName` (`len 3..63`), but never created, so this
    // account owns no such bucket.
    let unowned = "a-bucket-this-account-does-not-own";

    let mut cases: Vec<RefusedConfigurationCase> = unknown_transformers
        .iter()
        .map(|(case, unknown)| {
            (
                *case,
                configuration_over(unknown, CAPABILITY_INPUT_BUCKET, CAPABILITY_OUTPUT_BUCKET),
                "ResourceNotFoundException",
                // `ResourceNotFoundException`'s member is `message` —
                // `smithy_query(query="shape",shape="ResourceNotFoundException")`.
                "message",
                transformer_reference_not_found(unknown),
            )
        })
        .collect();
    for (case, input, output) in [
        ("the input location names an unowned bucket", unowned, CAPABILITY_OUTPUT_BUCKET),
        ("the output location names an unowned bucket", CAPABILITY_INPUT_BUCKET, unowned),
        ("both locations name an unowned bucket", unowned, unowned),
    ] {
        cases.push((
            case,
            configuration_over(&transformer_id, input, output),
            "ValidationException",
            // `ValidationException` spells its member `Message` —
            // `smithy_query(query="shape",shape="ValidationException")`.
            "Message",
            BUCKETS_NOT_OWNED.to_string(),
        ));
    }

    let created = get_capability(&router, &capability_id).await;

    for (case, configuration, code, member, message) in &cases {
        let (status, body) = call(
            &router,
            "UpdateCapability",
            json!({
                "capabilityId": capability_id.clone(),
                "name": format!("renamed for {case}"),
                "configuration": configuration.clone(),
            }),
        )
        .await;
        let expected_status = if *code == "ResourceNotFoundException" {
            NOT_FOUND_STATUS
        } else {
            VALIDATION_STATUS
        };
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            *code,
            "case {case:?}: expected {code}, got {body}"
        );
        assert_eq!(
            &text(&body, member),
            message,
            "case {case:?}: expected the refusal {message:?} in {member:?}, got {body}"
        );

        // Nothing was written: every member, including the `name` the same request asked
        // to change, is still the created one, and `modifiedAt` is still the instant the
        // create stamped.
        let fetched = get_capability(&router, &capability_id).await;
        assert_eq!(
            fetched, created,
            "case {case:?}: the refused update must leave the capability untouched, got \
             {fetched} instead of {created}"
        );
        assert_eq!(
            fetched["modifiedAt"], fetched["createdAt"],
            "case {case:?}: a refused update must not stamp modifiedAt, got {:?} against a \
             createdAt of {:?}",
            fetched["modifiedAt"], fetched["createdAt"]
        );
    }

    // The SAME request whose configuration does resolve is served, so what refused the
    // rows above is the resolution and nothing else.
    let resolvable = capability_configuration(&transformer_id, "X12_997", "VERSION_4030");
    let (status, updated) = call(
        &router,
        "UpdateCapability",
        json!({
            "capabilityId": capability_id.clone(),
            "name": "renamed for the configuration that resolves",
            "configuration": resolvable.clone(),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "an update whose configuration resolves must be served, got {status} {updated}"
    );
    assert_eq!(
        updated["configuration"], resolvable,
        "the resolvable configuration must be the one stored, got {:?}",
        updated["configuration"]
    );
    let fetched = get_capability(&router, &capability_id).await;
    assert_eq!(
        fetched["configuration"], resolvable,
        "GetCapability must report the configuration the update wrote, got {:?}",
        fetched["configuration"]
    );
    assert!(
        !fetched["modifiedAt"].is_null(),
        "an applied update must stamp modifiedAt, got {:?}",
        fetched["modifiedAt"]
    );
}
