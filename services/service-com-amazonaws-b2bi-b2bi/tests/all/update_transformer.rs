// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateTransformer`, driven over the wire (awsJson1_0,
//! `x-amz-target: B2BI.<Operation>`) through the same router the emulator boots, so every
//! assertion is about what a real SDK client observes.
//!
//! Each test states a rule and asserts it over a TABLE of inputs rather than one recorded
//! request: the rows differ along what the rule generalises over (which members the update
//! names, how many, which configuration style they belong to, the enum value, the status
//! the record rests in, the signing region).
//!
//! The wire harness (`b2bi_router`, `call`, `error_code`, `text`) and the two
//! configuration-member tables are reused from the sibling suites rather than copied.

use serde_json::{json, Value};
use service_com_amazonaws_b2bi_b2bi::scaffolded::NOT_FOUND_STATUS;
use smithy_server_runtime::MultiServiceRouter;

use crate::onboarding_loop::{
    b2bi_router, call, call_signed_for, error_code, text, ACCOUNT, SIGNING_REGION,
};
use crate::transformer_lifecycle::{current_members, legacy_members, CONFIGURATION_MEMBERS};
use crate::wire_timestamps_and_discriminators::{as_wire_reports, date_time_seconds};

/// The members `UpdateTransformer` can write: the eight configuration members plus the two
/// the model lets a caller set directly.
///
/// Source: the input members of `com.amazonaws.b2bi#UpdateTransformer`
/// (`smithy_query(query="operation",operation="UpdateTransformer")`) minus the required
/// `transformerId`.
fn updatable_members() -> Vec<&'static str> {
    let mut members = vec!["name", "status"];
    members.extend_from_slice(CONFIGURATION_MEMBERS);
    members
}

/// The two values of the modelled `TransformerStatus` enum
/// (`smithy_query(query="shape",shape="TransformerStatus")`).
const TRANSFORMER_STATUS_VALUES: &[&str] = &["active", "inactive"];

/// The fixed text this service refuses an update of an `active` transformer with: the
/// whole message, interpolating nothing.
pub(crate) const ACTIVE_IS_IMMUTABLE: &str = "An active Transformer cannot be updated";

/// Create a transformer from `request` and return its create response.
async fn created_transformer(
    router: &MultiServiceRouter,
    region: &str,
    request: Value,
) -> (String, Value) {
    let (status, created) = call_signed_for(router, region, "CreateTransformer", request).await;
    assert_eq!(
        status, 200,
        "the transformer under test must be created, got {status} {created}"
    );
    (text(&created, "transformerId"), created)
}

/// The epoch seconds of a timestamp member, which this service renders in the `date-time`
/// form its model declares (`crate::wire_timestamps_and_discriminators`, which owns that
/// rule, is what asserts the form).
fn seconds(body: &Value, member: &str) -> f64 {
    date_time_seconds(body, member)
}

/// RULE: `UpdateTransformer` writes exactly the members the request names onto the stored
/// transformer, leaves every member it omits as it was, stamps `modifiedAt`, keeps the
/// system-assigned `transformerId` / `transformerArn` / `createdAt`, and answers with that
/// stored record — which `GetTransformer` then reports member for member.
///
/// The rows span updating only `name`, only `status`, one configuration member, every
/// member of each configuration style at once, a legacy-style record updated through its
/// legacy members, a mixed set of non-configuration and configuration members, and an
/// update that names no member at all, over two signing regions.
#[tokio::test]
async fn an_update_writes_the_named_members_and_leaves_the_rest() {
    let router = b2bi_router();

    let current = current_members();
    let legacy = legacy_members();

    // (case, signing region, the create request, the update request)
    let cases: Vec<(&str, &str, Value, Value)> = vec![
        (
            "name only",
            SIGNING_REGION,
            json!({ "name": "inbound-850", "inputConversion": current[0].1 }),
            json!({ "name": "inbound_850_v2" }),
        ),
        (
            "status only, inactive -> active",
            SIGNING_REGION,
            json!({ "name": "activated", "inputConversion": current[0].1 }),
            json!({ "status": "active" }),
        ),
        (
            "one current configuration member replaced",
            "eu-central-1",
            json!({
                "name": "remapped",
                "inputConversion": current[0].1,
                "mapping": current[1].1,
            }),
            json!({
                "mapping": { "templateLanguage": "XSLT", "template": "<xsl:stylesheet version=\"1.0\"/>" }
            }),
        ),
        (
            "every current configuration member at once",
            SIGNING_REGION,
            json!({ "name": "fully-rewritten", "inputConversion": current[0].1 }),
            json!({
                "inputConversion": current[0].1,
                "mapping": current[1].1,
                "outputConversion": current[2].1,
                "sampleDocuments": current[3].1,
            }),
        ),
        (
            "legacy record, one legacy member replaced",
            SIGNING_REGION,
            json!({ "name": "legacy-xml", "fileFormat": "XML", "mappingTemplate": "{}" }),
            json!({ "fileFormat": "JSON" }),
        ),
        (
            "legacy record, every legacy member at once",
            "ap-southeast-2",
            json!({ "name": "legacy-full", "fileFormat": "NOT_USED" }),
            json!({
                "ediType": legacy[0].1,
                "fileFormat": legacy[1].1,
                "mappingTemplate": legacy[2].1,
                "sampleDocument": legacy[3].1,
            }),
        ),
        (
            "name, status and a configuration member together",
            SIGNING_REGION,
            json!({ "name": "renamed-and-activated", "inputConversion": current[0].1 }),
            json!({
                "name": "renamed_and_activated",
                "status": "active",
                "sampleDocuments": current[3].1,
            }),
        ),
        (
            // Nothing but the required id: a well-formed request, so it is served, and the
            // only thing it changes is `modifiedAt`.
            "no member but the required transformerId",
            SIGNING_REGION,
            json!({ "name": "untouched", "inputConversion": current[0].1 }),
            json!({}),
        ),
    ];

    for (case, region, create, update) in &cases {
        let (transformer_id, created) = created_transformer(&router, region, create.clone()).await;
        let mut request = update.clone();
        request["transformerId"] = json!(transformer_id);

        let (status, updated) =
            call_signed_for(&router, region, "UpdateTransformer", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateTransformer must succeed, got {status} {updated}"
        );

        // The system-assigned identity is the create's: an update never re-mints it.
        assert_eq!(
            text(&updated, "transformerId"),
            transformer_id,
            "case {case:?}: the id must be the one the create minted"
        );
        assert_eq!(
            text(&updated, "transformerArn"),
            format!("arn:aws:b2bi:{region}:{ACCOUNT}:transformer/{transformer_id}"),
            "case {case:?}: the ARN must stay the create's, got {:?}",
            updated["transformerArn"]
        );
        assert_eq!(
            updated["createdAt"], created["createdAt"],
            "case {case:?}: createdAt must not move: {:?} vs {:?}",
            updated["createdAt"], created["createdAt"]
        );
        assert!(
            seconds(&updated, "modifiedAt") >= seconds(&created, "createdAt"),
            "case {case:?}: modifiedAt {:?} must be at or after createdAt {:?}",
            updated["modifiedAt"],
            created["createdAt"]
        );

        for member in updatable_members() {
            let expected = update.get(member).cloned().unwrap_or_else(|| {
                // `status` is the only member the create assigns rather than echoes.
                created.get(member).cloned().unwrap_or(Value::Null)
            });
            // A nested structure is reported with the `__type` this service's wire puts on
            // its shape, so that is part of the expectation — `as_wire_reports` adds it to
            // the value the request carried.
            let expected = as_wire_reports(member, &expected);
            assert_eq!(
                updated[member], expected,
                "case {case:?}: {member} must be {expected:?} (the update's value, or the \
                 stored one when the update omits it), got {:?}",
                updated[member]
            );
        }

        // Round trip: the reader reports the very record the update wrote.
        let (status, fetched) = call_signed_for(
            &router,
            region,
            "GetTransformer",
            json!({ "transformerId": transformer_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetTransformer must find the updated transformer, got \
             {status} {fetched}"
        );
        for member in ["transformerId", "transformerArn", "createdAt", "modifiedAt"]
            .iter()
            .copied()
            .chain(updatable_members())
        {
            assert_eq!(
                fetched[member], updated[member],
                "case {case:?}: GetTransformer disagrees with UpdateTransformer on {member}: \
                 {:?} vs {:?}",
                fetched[member], updated[member]
            );
        }
    }
}

/// RULE: a stored transformer moves from `inactive` to `active`, and that is the only
/// transition a caller can drive — the model documents no other
/// (`UpdateTransformerRequest$status`: "You can update the state of the transformer from
/// inactive to active"), so once the record is `active` every further update is refused
/// with the modelled `ValidationException` (HTTP 400, member `Message`) and the stored
/// status stays `active`.
///
/// The rows are every value of the modelled `TransformerStatus` enum sent against an
/// active record — including the value it already holds — plus the `inactive` flip the
/// recorded refusal was taken from.
#[tokio::test]
async fn the_status_moves_to_active_once_and_no_further_update_is_taken() {
    let router = b2bi_router();
    let current = current_members();
    let (transformer_id, created) = created_transformer(
        &router,
        SIGNING_REGION,
        json!({ "name": "toggled", "inputConversion": current[0].1 }),
    )
    .await;
    assert_eq!(
        text(&created, "status"),
        "inactive",
        "a new transformer rests inactive, got {created}"
    );

    // The one documented transition, served.
    let (status, activated) = call(
        &router,
        "UpdateTransformer",
        json!({ "transformerId": transformer_id, "status": "active" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the inactive -> active move must succeed, got {status} {activated}"
    );
    assert_eq!(
        text(&activated, "status"),
        "active",
        "the move must be reported, got {activated}"
    );

    // Every status a caller can ask for next, refused.
    for target in TRANSFORMER_STATUS_VALUES {
        let case = format!("status -> {target}");
        let (status, body) = call(
            &router,
            "UpdateTransformer",
            json!({ "transformerId": transformer_id, "status": target }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: an active transformer takes no further update, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
        assert_eq!(
            text(&body, "Message"),
            ACTIVE_IS_IMMUTABLE,
            "case {case:?}: unexpected refusal text, got {body}"
        );

        // Nothing was written: the record still reports `active` and its configuration.
        let (status, fetched) = call(
            &router,
            "GetTransformer",
            json!({ "transformerId": transformer_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetTransformer must succeed, got {status} {fetched}"
        );
        assert_eq!(
            text(&fetched, "status"),
            "active",
            "case {case:?}: the refused update must leave the status active, got {:?}",
            fetched["status"]
        );
        assert_eq!(
            fetched["inputConversion"], created["inputConversion"],
            "case {case:?}: the refused update must not touch the configuration, got {:?}",
            fetched["inputConversion"]
        );
    }
}

/// RULE: once a transformer's stored status is `active` it is immutable — EVERY
/// `UpdateTransformer` against it is refused with the modelled `ValidationException`
/// (HTTP 400, member `Message`) carrying the service's fixed text, and nothing is written —
/// while the identical update against the same transformer while it is still `inactive` is
/// served.
///
/// The rows are every member `UpdateTransformer` can write (the eight configuration
/// members in both styles, plus `name` and `status`), each asserted twice: served on an
/// inactive transformer, refused on an active one.
#[tokio::test]
async fn an_active_transformer_takes_no_update_at_all() {
    let router = b2bi_router();
    let current = current_members();

    // (member, a value its own shape accepts)
    let mut members: Vec<(&'static str, Value)> = vec![
        ("name", json!("renamed_while_inactive")),
        ("status", json!("active")),
    ];
    members.extend(legacy_members());
    members.extend(current_members());

    for (member, value) in &members {
        // The positive control: the same write, on a transformer that is still inactive.
        let (inactive_id, _) = created_transformer(
            &router,
            SIGNING_REGION,
            json!({ "name": "mutable-while-inactive", "fileFormat": "JSON" }),
        )
        .await;
        let (status, served) = call(
            &router,
            "UpdateTransformer",
            json!({ "transformerId": inactive_id, (*member): value }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {member:?}: an inactive transformer must take the update, got {status} {served}"
        );
        assert_eq!(
            served[*member],
            as_wire_reports(member, value),
            "case {member:?}: the update must be stored, got {:?}",
            served[*member]
        );

        // The same write against an ACTIVE transformer, refused.
        let (active_id, active_created) = created_transformer(
            &router,
            SIGNING_REGION,
            json!({ "name": "locked", "inputConversion": current[0].1 }),
        )
        .await;
        let (status, activated) = call(
            &router,
            "UpdateTransformer",
            json!({ "transformerId": active_id, "status": "active" }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {member:?}: activating the transformer must succeed, got {status} {activated}"
        );

        let (status, body) = call(
            &router,
            "UpdateTransformer",
            json!({ "transformerId": active_id, (*member): value }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {member:?}: updating an active transformer must be refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {member:?}: expected ValidationException, got {body}"
        );
        // `ValidationException`'s member is `Message` (capital M) in this service's model —
        // `smithy_query(query="shape",shape="ValidationException")`.
        assert_eq!(
            text(&body, "Message"),
            ACTIVE_IS_IMMUTABLE,
            "case {member:?}: unexpected refusal text, got {body}"
        );

        // Nothing was written: the active record still holds what it was activated with.
        let (status, fetched) = call(
            &router,
            "GetTransformer",
            json!({ "transformerId": active_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {member:?}: GetTransformer must succeed, got {status} {fetched}"
        );
        assert_eq!(
            text(&fetched, "name"),
            "locked",
            "case {member:?}: the refused update must not have renamed the transformer, got \
             {fetched}"
        );
        assert_eq!(
            fetched["status"],
            json!("active"),
            "case {member:?}: the refused update must leave the status active, got {fetched}"
        );
        for configuration_member in CONFIGURATION_MEMBERS {
            assert_eq!(
                fetched[*configuration_member], active_created[*configuration_member],
                "case {member:?}: the refused update must not have written \
                 {configuration_member}: got {:?}, created with {:?}",
                fetched[*configuration_member], active_created[*configuration_member]
            );
        }
    }
}

/// RULE: an update naming BOTH a member the model marks `@deprecated` and a member that
/// replaces it is refused with the modelled `ValidationException` (HTTP 400) and writes
/// nothing — the same rule `CreateTransformer` enforces on a create.
///
/// The rows are every (deprecated member, replacement member) pair: 4 x 4 = 16.
#[tokio::test]
async fn mixing_the_deprecated_and_current_configuration_members_is_refused() {
    use service_com_amazonaws_b2bi_b2bi::ops::create_transformer::MIXED_CONFIGURATION_STYLES;

    let router = b2bi_router();
    let current = current_members();
    let (transformer_id, created) = created_transformer(
        &router,
        SIGNING_REGION,
        json!({ "name": "single-style", "inputConversion": current[0].1 }),
    )
    .await;

    for (legacy_member, legacy_value) in legacy_members() {
        for (current_member, current_value) in current_members() {
            let case = format!("{legacy_member} + {current_member}");
            let mut request = json!({ "transformerId": transformer_id });
            request[legacy_member] = legacy_value.clone();
            request[current_member] = current_value.clone();

            let (status, body) = call(&router, "UpdateTransformer", request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: a mixed-style update must be refused, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body}"
            );
            // `ValidationException`'s member is `Message` (capital M) in this service's
            // model — `smithy_query(query="shape",shape="ValidationException")`.
            let message = text(&body, "Message");
            assert_eq!(
                message, MIXED_CONFIGURATION_STYLES,
                "case {case:?}: the refusal states the rule in fixed text, got {message:?} \
                 in {body}"
            );
        }
    }

    // None of the 16 refusals touched the record: it is still exactly what the create
    // stored, and its `modifiedAt` is still the instant the create stamped.
    let (status, fetched) = call(
        &router,
        "GetTransformer",
        json!({ "transformerId": transformer_id }),
    )
    .await;
    assert_eq!(status, 200, "GetTransformer must succeed, got {status} {fetched}");
    assert_eq!(
        fetched["modifiedAt"], fetched["createdAt"],
        "a refused update must not stamp modifiedAt, got {:?} against a createdAt of {:?}",
        fetched["modifiedAt"], fetched["createdAt"]
    );
    for member in updatable_members() {
        assert_eq!(
            fetched[member], created[member],
            "the refused updates must not have written {member}: got {:?}, created with {:?}",
            fetched[member], created[member]
        );
    }
}

/// RULE: `UpdateTransformer` for an id this account's store does not hold is the modelled
/// `ResourceNotFoundException` at this service's not-found status, naming that id — never
/// an empty success and never a minted transformer — whatever the id looks like, including
/// an id another b2bi resource kind really does hold, and whatever else the request gets
/// wrong: the resource is resolved before the request's own members are judged.
#[tokio::test]
async fn updating_an_unknown_transformer_is_refused_as_not_found() {
    let router = b2bi_router();
    let current = current_members();
    let legacy = legacy_members();
    let profile_id =
        crate::onboarding_loop::given_profile(&router, "update-transformer-unknown").await;

    // (case, a transformerId satisfying the modelled len 1..64 / ^[a-zA-Z0-9_-]+$, the
    // members the update names beside it)
    let cases: Vec<(&str, String, Value)> = vec![
        (
            "a never-minted transformer id",
            "t-0123456789abcdef0".to_string(),
            json!({ "status": "active" }),
        ),
        ("a one-character id", "t".to_string(), json!({ "status": "inactive" })),
        (
            "an id of the maximum modelled length",
            "t".repeat(64),
            json!({ "name": "renamed" }),
        ),
        (
            "an id with every permitted character class",
            "T-9_z".to_string(),
            json!({ "mapping": current[1].1 }),
        ),
        (
            // The missing resource is answered BEFORE the request's members are judged,
            // so a mixed-style update against an unknown id is still not-found.
            "an unknown id with a mixed-style configuration",
            "t-fedcba98765432100".to_string(),
            json!({ "fileFormat": legacy[1].1, "inputConversion": current[0].1 }),
        ),
        (
            "a profile id that really exists",
            profile_id,
            json!({ "status": "active" }),
        ),
    ];

    for (case, transformer_id, members) in &cases {
        let mut request = members.clone();
        request["transformerId"] = json!(transformer_id);
        let (status, body) = call(&router, "UpdateTransformer", request).await;
        assert_eq!(
            status,
            NOT_FOUND_STATUS,
            "case {case:?}: expected {NOT_FOUND_STATUS} for {transformer_id:?}, got {status} \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
        assert!(
            text(&body, "message").contains(transformer_id),
            "case {case:?}: the refusal must name the id that was not found, got {:?}",
            text(&body, "message")
        );

        // The refusal minted nothing: the reader still does not know that id.
        let (status, fetched) = call(
            &router,
            "GetTransformer",
            json!({ "transformerId": transformer_id }),
        )
        .await;
        assert_eq!(
            status,
            NOT_FOUND_STATUS,
            "case {case:?}: the refused update must not have created a transformer, got \
             {status} {fetched}"
        );
    }
}
