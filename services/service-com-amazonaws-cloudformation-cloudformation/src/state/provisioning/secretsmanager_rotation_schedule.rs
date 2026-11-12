// SPDX-License-Identifier: CC0-1.0

//! `AWS::SecretsManager::RotationSchedule`: the rotation a stack binds to a secret, put in
//! force in AWS Secrets Manager itself.
//!
//! The type is no entity of Secrets Manager's own: it is the ROTATION CONFIGURATION of the
//! secret it names, which `RotateSecret` writes ("Configures and starts the asynchronous
//! process of rotating the secret") and `CancelRotateSecret` turns off ("Turns off automatic
//! rotation, and if a rotation is currently in progress, cancels the rotation"). Its
//! properties are that call's own members under the names the type publishes, so the
//! declaration reaches the owning service as written.
//!
//! ## Why the rotation is really configured
//!
//! Credential rotation is what the pair of resources exists for: the stack declares a
//! rotation function and this resource binds it to the secret, and from then on every
//! rotation of that secret — the scheduled one and a client's own `RotateSecret` — drives
//! that function through the four documented steps. A stack that reported this resource
//! created while nothing configured the secret leaves the secret with no rotation function
//! at all, so a later `RotateSecret` is refused for the very reason the stack was supposed to
//! have removed ("You tried to enable rotation on a secret that doesn't already have a Lambda
//! function ARN configured") and the credential silently never rotates.
//!
//! ## What the create does, and when the resource is settled
//!
//! `RotateSecret` configures the rotation AND starts one: "If you include the configuration
//! parameters, the operation sets the values for the secret and then immediately starts a
//! rotation", and the type's own `RotateImmediatelyOnUpdate` is what asks for that start to be
//! held back ("Specifies whether to rotate the secret immediately or wait until the next
//! scheduled rotation window"; AWS documents the default as rotating immediately). The call
//! ANSWERS while that rotation is in flight, so the resource is not settled when it returns:
//! it is settled once the owning service reports the secret no longer mid-rotation
//! ([`readiness`]), which is also what keeps a client's own `RotateSecret` right after the
//! stack completed from meeting "a previous rotation request is still in progress".
//!
//! It lives beside [`super::secretsmanager_secret`] and is reached through the same seam
//! ([`super::Owner`]), for the reason that module records.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::{call_secretsmanager, Created, Readiness, Refusal, Removal};
use crate::state::StoredResource;

/// The resource type this module provisions, as [`crate::state::template`] names it: the
/// identifier of a resource of this type is the one the owning service answered
/// (`identity_is_service_assigned`), and its `SecretId` is create-only
/// (`replacement_properties`), so both halves of this module's contract are read from there.
pub(super) use crate::state::template::SECRETSMANAGER_ROTATION_SCHEDULE;

/// The error code Secrets Manager answers for a secret it does not hold
/// (`ResourceNotFoundException`, the modelled refusal of its reads and of
/// `CancelRotateSecret`), which is what says a removal has nothing left to turn off and a
/// read nothing to report.
const SECRET_NOT_FOUND: &str = "ResourceNotFoundException";

/// The code a refusal of this module's own carries for a declaration that describes no
/// configuration this emulator can put in force: the same one the catalogue's arms use.
const NOT_IMPLEMENTED: &str = "NotImplemented";

/// The code a refusal carries for a declaration that leaves out the one property the type
/// requires.
const INVALID_REQUEST: &str = "InvalidRequest";

/// The code a refusal carries where the owning service answered without the one value this
/// resource's identity is.
const INTERNAL_FAILURE: &str = "InternalFailure";

/// The property naming the secret whose rotation this resource configures, which is
/// `RotateSecret`'s own `SecretId` member and the resource's identity.
const SECRET_ID: &str = "SecretId";

/// The property asking for the start of the rotation to be held back, which is
/// `RotateSecret`'s own `RotateImmediately` member under the name the type publishes.
const ROTATE_IMMEDIATELY_ON_UPDATE: &str = "RotateImmediatelyOnUpdate";
/// See [`ROTATE_IMMEDIATELY_ON_UPDATE`].
const ROTATE_IMMEDIATELY: &str = "RotateImmediately";

/// The property declaring a rotation function Secrets Manager itself would HOST, which this
/// emulator creates nothing for ([`refuse_unapplied_properties`]).
const HOSTED_ROTATION_LAMBDA: &str = "HostedRotationLambda";

/// Every property `AWS::SecretsManager::RotationSchedule` publishes. A declared property
/// this module does not apply is REFUSED rather than dropped, so a rotation is never
/// reported configured with less than its declaration asked for.
///
/// Source: the type's own property set; `SecretId`, `RotationLambdaARN` and `RotationRules`
/// are `RotateSecret`'s own members of the same name, and `RotateImmediatelyOnUpdate` is its
/// `RotateImmediately` (`smithy_query(query="operation",operation="RotateSecret")`).
const SCHEDULE_PROPERTIES: &[&str] = &[
    HOSTED_ROTATION_LAMBDA,
    ROTATE_IMMEDIATELY_ON_UPDATE,
    "RotationLambdaARN",
    "RotationRules",
    SECRET_ID,
];

/// The properties that reach `RotateSecret` under their own names, each being that call's
/// member of the same spelling.
const ROTATE_MEMBERS: &[&str] = &["RotationLambdaARN", "RotationRules"];

/// The member every write of a secret's new version is staked under, and how many characters
/// this provider mints one with: the modelled `ClientRequestTokenType`'s own shortest length
/// (32..64, `smithy_query(query="operation",operation="RotateSecret")`).
const CLIENT_REQUEST_TOKEN: &str = "ClientRequestToken";
/// See [`CLIENT_REQUEST_TOKEN`].
const CLIENT_REQUEST_TOKEN_CHARACTERS: usize = 32;

/// The `DescribeSecret` member reporting whether a rotation schedule is in force on the
/// secret.
const ROTATION_ENABLED: &str = "RotationEnabled";

/// The `DescribeSecret` member reporting which stages each of the secret's versions carries.
const VERSION_IDS_TO_STAGES: &str = "VersionIdsToStages";

/// The staging label a rotation stakes on the version it is creating, and the one naming the
/// version in use: "if the AWSPENDING staging label is present but not attached to the same
/// version as AWSCURRENT, then any later invocation of RotateSecret assumes that a previous
/// rotation request is still in progress" (`RotateSecret`, this service's Smithy model) —
/// which is the one reading of "a rotation is in flight" this module waits out.
const AWSPENDING: &str = "AWSPENDING";
/// See [`AWSPENDING`].
const AWSCURRENT: &str = "AWSCURRENT";

/// The attribute the type publishes, which is the ARN of the secret the rotation was
/// configured on — the same value `Ref` returns for the resource.
const ID_ATTRIBUTE: &str = "Id";

/// Configure the rotation an `AWS::SecretsManager::RotationSchedule` declares, in AWS Secrets
/// Manager itself — so the secret the stack published really rotates through the function the
/// stack deployed.
///
/// `Err` is that service's own refusal of the call: a secret it does not hold, a schedule
/// that cannot hold together, a rotation function whose own policy does not let Secrets
/// Manager invoke it.
pub(super) async fn create(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let answered = rotate(ctx, region, &resource.properties, resource).await?;
    // The ARN that service answered is the resource's identity: "Ref returns the ARN of the
    // secret", which is also the one attribute the type publishes and the handle every later
    // call on the rotation takes.
    let arn = answered
        .get("ARN")
        .and_then(Value::as_str)
        .filter(|arn| !arn.is_empty())
        .ok_or_else(|| Refusal {
            code: INTERNAL_FAILURE.to_string(),
            message: "AWS Secrets Manager answered RotateSecret without an ARN".to_string(),
        })?
        .to_string();
    let mut attributes = BTreeMap::new();
    attributes.insert(ID_ATTRIBUTE.to_string(), arn.clone());
    Ok(Created {
        physical_resource_id: Some(arn),
        attributes,
    })
}

/// Apply a CHANGED declaration to the rotation the stack already configured, through the same
/// `RotateSecret` — the one call that writes a secret's rotation configuration, and the one
/// the type's own `RotateImmediatelyOnUpdate` is documented against.
///
/// A changed `SecretId` is not one of them: the secret IS this resource's identity, so a
/// declaration naming another secret is a different resource, which the algorithm above
/// replaces rather than bringing here — that property is one of the type's create-only ones
/// (`crate::state::template::replacement_properties`).
pub(super) async fn update(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    rotate(ctx, region, declared, resource).await?;
    // The rotation stays on the same secret, so the handle the stack published — and the one
    // attribute read off it — is unchanged.
    Ok(Some(Created::default()))
}

/// Whether the rotation this resource configured has SETTLED, which is what the resource's
/// own creation waits on.
///
/// The configuration is in force once `DescribeSecret` reports rotation enabled on the
/// secret, and the rotation the create started is over once no version carries `AWSPENDING`
/// apart from the one `AWSCURRENT` names — the owning service's own reading of "a previous
/// rotation request is still in progress" ([`AWSPENDING`]). Nothing here guesses at a span:
/// the answer is that service's, read per poll.
pub(super) async fn readiness(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Readiness, Refusal> {
    let described = match describe(ctx, region, physical_resource_id).await {
        Ok(Some(described)) => described,
        // The secret is gone, so nothing is going to bring this resource up.
        Ok(None) => {
            return Ok(Readiness::Failed(format!(
                "AWS Secrets Manager no longer holds the secret {physical_resource_id}"
            )))
        }
        Err(refusal) => return Err(refusal),
    };
    if described.get(ROTATION_ENABLED).and_then(Value::as_bool) != Some(true) {
        return Ok(Readiness::Pending(
            "the secret does not report rotation enabled yet".to_string(),
        ));
    }
    if rotation_in_flight(&described) {
        return Ok(Readiness::Pending(
            "a rotation of the secret is still in progress".to_string(),
        ));
    }
    Ok(Readiness::Ready)
}

/// `CancelRotateSecret` turns the schedule off as it answers, so there is nothing for a later
/// read to wait on.
pub(super) fn removal() -> Removal {
    Removal::Gone
}

/// Turn off the rotation a stack configured, addressed by the ARN of the secret it was
/// configured on.
///
/// `CancelRotateSecret` is what CloudFormation makes of this type's removal: it "turns off
/// automatic rotation, and if a rotation is currently in progress, cancels the rotation",
/// which is exactly the resource ceasing to exist while the SECRET — a resource of its own,
/// with its own deletion policy — stays. A secret that service no longer holds is already in
/// the state the removal asks for, judged on the CODE and never on a message.
pub(super) async fn delete(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ SECRET_ID: resource.physical_resource_id });
    match call_secretsmanager(ctx, region, "CancelRotateSecret", &request).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == SECRET_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The rotation's configuration as the owning service reports it, for drift detection:
/// `DescribeSecret`'s own record, read by the ARN the delete uses.
///
/// Only the properties that read answers under the same names are reported — the rotation
/// function and the rotation rules — and a property the read does not report is not evidence
/// of anything ([`super::super::drift::differs`]). A secret that service no longer holds
/// carries no rotation at all.
pub(super) async fn actual_configuration(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let Some(described) = describe(ctx, region, &resource.physical_resource_id).await? else {
        return Ok(None);
    };
    let mut configuration = Map::new();
    for property in ROTATE_MEMBERS {
        if let Some(value) = described.get(*property).filter(|value| !value.is_null()) {
            configuration.insert((*property).to_string(), value.clone());
        }
    }
    Ok(Some(Value::Object(configuration)))
}

/// Write `properties` as the secret's rotation configuration, through `RotateSecret`.
async fn rotate(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    properties: &Value,
    resource: &StoredResource,
) -> Result<Value, Refusal> {
    refuse_unapplied_properties(properties, &resource.resource_type)?;
    let mut request = Map::new();
    request.insert(SECRET_ID.to_string(), Value::String(secret_of(properties)?));
    // A raw HTTP request to this service carries its own `ClientRequestToken` — "If you
    // generate a raw HTTP request to the Secrets Manager service endpoint, then you must
    // generate a ClientRequestToken and include it in the request", which the SDKs do for a
    // caller — and it names the version THIS rotation creates, so it is minted per call
    // rather than derived from the resource: a token already naming a stored version could
    // not be the id of a new one.
    request.insert(
        CLIENT_REQUEST_TOKEN.to_string(),
        Value::String(crate::state::new_identifier_digits(
            &format!("{SECRETSMANAGER_ROTATION_SCHEDULE}/{}", resource.logical_id),
            CLIENT_REQUEST_TOKEN_CHARACTERS,
        )),
    );
    for member in ROTATE_MEMBERS {
        if let Some(value) = stated(properties, member) {
            request.insert((*member).to_string(), value.clone());
        }
    }
    if let Some(value) = stated(properties, ROTATE_IMMEDIATELY_ON_UPDATE) {
        request.insert(ROTATE_IMMEDIATELY.to_string(), value.clone());
    }
    call_secretsmanager(ctx, region, "RotateSecret", &Value::Object(request)).await
}

/// The secret the declaration names, which the type requires: "SecretId — the ARN or name of
/// the secret to rotate" (`RotateSecret`, this service's Smithy model).
fn secret_of(properties: &Value) -> Result<String, Refusal> {
    match stated(properties, SECRET_ID).map(::service_common::json_wire_values::value_text) {
        Some(secret) if !secret.is_empty() => Ok(secret),
        _ => Err(Refusal {
            code: INVALID_REQUEST.to_string(),
            message: format!(
                "{SECRETSMANAGER_ROTATION_SCHEDULE} requires the {SECRET_ID} property"
            ),
        }),
    }
}

/// Whether the secret `described` reports is mid-rotation: `AWSPENDING` is staked on a version
/// other than the one `AWSCURRENT` names ([`AWSPENDING`]).
fn rotation_in_flight(described: &Value) -> bool {
    let Some(stages) = described.get(VERSION_IDS_TO_STAGES).and_then(Value::as_object) else {
        return false;
    };
    let carries = |version: &Value, label: &str| {
        version
            .as_array()
            .is_some_and(|labels| labels.iter().any(|held| held.as_str() == Some(label)))
    };
    let current = stages
        .iter()
        .find(|(_, labels)| carries(labels, AWSCURRENT))
        .map(|(version, _)| version.clone());
    stages
        .iter()
        .any(|(version, labels)| carries(labels, AWSPENDING) && current.as_ref() != Some(version))
}

/// What `DescribeSecret` reports for the secret `secret_id`, or `None` for one that service no
/// longer holds.
async fn describe(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    secret_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ SECRET_ID: secret_id });
    match call_secretsmanager(ctx, region, "DescribeSecret", &request).await {
        Ok(described) => Ok(Some(described)),
        Err(refusal) if refusal.code == SECRET_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The value a declaration STATES for `property`, or `None` for one it leaves out or states
/// as null.
fn stated<'a>(properties: &'a Value, property: &str) -> Option<&'a Value> {
    properties.get(property).filter(|value| !value.is_null())
}

/// The refusal for a declared property this module does not apply, naming which property it
/// was: refusing is what keeps a rotation from being reported as configured with less than
/// its declaration asked for. A `HostedRotationLambda` is the one that matters — it asks
/// Secrets Manager to CREATE a rotation function of its own, which nothing here creates, so a
/// declaration stating one is refused rather than deployed with no rotation function at all.
fn refuse_unapplied_properties(properties: &Value, resource_type: &str) -> Result<(), Refusal> {
    let Value::Object(properties) = properties else {
        return Ok(());
    };
    for property in properties.keys() {
        if SCHEDULE_PROPERTIES.contains(&property.as_str()) && property != HOSTED_ROTATION_LAMBDA {
            continue;
        }
        return Err(Refusal {
            code: NOT_IMPLEMENTED.to_string(),
            message: format!(
                "this emulator does not apply the {property} property of {resource_type}"
            ),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every member this arm sends is a published property of the type, and the ones that are
    /// not body members under their own name are the id, the start flag and the hosted
    /// function this emulator creates nothing for.
    #[test]
    fn every_sent_member_is_a_published_property() {
        for member in ROTATE_MEMBERS {
            assert!(
                SCHEDULE_PROPERTIES.contains(member),
                "case {member:?}: expected the sent member to be a published property of \
                 {SECRETSMANAGER_ROTATION_SCHEDULE}, got none"
            );
        }
        for property in SCHEDULE_PROPERTIES {
            let placed = ROTATE_MEMBERS.contains(property)
                || [SECRET_ID, ROTATE_IMMEDIATELY_ON_UPDATE, HOSTED_ROTATION_LAMBDA]
                    .contains(property);
            assert!(
                placed,
                "case {property:?}: expected the property to be a body member, the secret it \
                 addresses, the start flag or the refused hosted function, got none"
            );
        }
    }

    /// RULE: a declaration naming no secret is refused, and one stating a property this
    /// module does not apply is refused rather than deployed with less than it asked for.
    #[test]
    fn a_declaration_this_module_cannot_apply_is_refused() {
        // (case, the declared properties, whether it is refused)
        let rows: &[(&str, Value, bool)] = &[
            (
                "the function and the rules alone",
                serde_json::json!({
                    "SecretId": "arn:aws:secretsmanager:us-west-2:000000000000:secret:db-AbCdEf",
                    "RotationLambdaARN": "arn:aws:lambda:us-west-2:000000000000:function:rotator",
                    "RotationRules": { "AutomaticallyAfterDays": 30 }
                }),
                false,
            ),
            (
                "a declaration holding the start back",
                serde_json::json!({
                    "SecretId": "db-AbCdEf",
                    "RotateImmediatelyOnUpdate": false
                }),
                false,
            ),
            (
                "a hosted rotation function, which nothing here creates",
                serde_json::json!({
                    "SecretId": "db-AbCdEf",
                    "HostedRotationLambda": { "RotationType": "PostgreSQLSingleUser" }
                }),
                true,
            ),
            (
                "a property the type does not publish",
                serde_json::json!({ "SecretId": "db-AbCdEf", "Elsewhere": "x" }),
                true,
            ),
        ];
        for (case, properties, refused) in rows {
            let reached =
                refuse_unapplied_properties(properties, SECRETSMANAGER_ROTATION_SCHEDULE).is_err();
            assert_eq!(
                reached, *refused,
                "case {case:?}: expected refused={refused}, got {reached} for {properties}"
            );
        }
        for (case, properties) in [
            ("no properties at all", Value::Null),
            (
                "properties without the secret",
                serde_json::json!({ "RotationRules": { "AutomaticallyAfterDays": 7 } }),
            ),
            ("an empty secret id", serde_json::json!({ "SecretId": "" })),
        ] {
            let reached = secret_of(&properties);
            assert!(
                reached.is_err(),
                "case {case:?}: expected a refusal naming the required property, got {reached:?}"
            );
        }
    }

    /// RULE: a rotation is in flight exactly while `AWSPENDING` names a version other than the
    /// one `AWSCURRENT` names.
    #[test]
    fn a_rotation_is_in_flight_only_while_pending_is_not_current() {
        // (case, what DescribeSecret reports, whether a rotation is in flight)
        let rows: &[(&str, Value, bool)] = &[
            ("no versions reported at all", serde_json::json!({}), false),
            (
                "one version, current",
                serde_json::json!({ "VersionIdsToStages": { "v1": ["AWSCURRENT"] } }),
                false,
            ),
            (
                "a staged version the rotation has not finished",
                serde_json::json!({
                    "VersionIdsToStages": { "v1": ["AWSCURRENT"], "v2": ["AWSPENDING"] }
                }),
                true,
            ),
            (
                "pending left on the version rotation finished onto",
                serde_json::json!({
                    "VersionIdsToStages": {
                        "v1": ["AWSPREVIOUS"],
                        "v2": ["AWSCURRENT", "AWSPENDING"]
                    }
                }),
                false,
            ),
            (
                "a staged version with no current one at all",
                serde_json::json!({ "VersionIdsToStages": { "v2": ["AWSPENDING"] } }),
                true,
            ),
        ];
        for (case, described, in_flight) in rows {
            let reached = rotation_in_flight(described);
            assert_eq!(
                reached, *in_flight,
                "case {case:?}: expected in_flight={in_flight}, got {reached} for {described}"
            );
        }
    }
}
