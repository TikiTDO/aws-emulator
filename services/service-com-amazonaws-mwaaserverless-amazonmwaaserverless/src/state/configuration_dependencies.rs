// SPDX-License-Identifier: CC0-1.0

//! The DEPENDENCIES a workflow configuration names, resolved against the services that
//! own them before a version freezes that configuration.
//!
//! A workflow version is a frozen configuration this service RUNS: it takes the workflow
//! definition out of the Amazon S3 object `DefinitionS3Location` addresses, and it
//! executes the workflow as the IAM role `RoleArn` names ("the IAM role that Amazon
//! Managed Workflows for Apache Airflow Serverless assumes when it executes the …
//! workflow"). Both are resources of OTHER services, and neither member declares a
//! constraint the request boundary could judge them by — `DefinitionS3Location$Bucket`
//! and `$ObjectKey` target a bare `String`, and a `RoleARN` within its `@pattern` still
//! says nothing about the role it names — so whether the configuration could ever run is
//! a question only those services can answer.
//!
//! So it is PUT to them, over the in-process callback into this same emulator, before
//! anything is written:
//!
//! * the definition object is read out of Amazon S3 — the read that takes the snapshot
//!   anyway ([`super::workflow_definition::read_definition`]) — and a location that store
//!   answers serves no readable object is refused with the modelled `ValidationException`,
//!   which is documented for exactly this ("This can occur … when S3 locations are
//!   inaccessible");
//! * the execution role is resolved against this account's IAM — the tree's one reading of
//!   a caller-supplied role (`service_common::iam_role::role_is_unassumable`): IAM holds a
//!   role of this account under that name and its trust policy admits this service's
//!   principal, or it does not — and a role IAM answers about that way is refused with the
//!   modelled `AccessDeniedException` (HTTP 403).
//!
//! Nothing is refused on evidence this emulator does not have. An answer that established
//! nothing — no Amazon S3 or no IAM mounted to ask, a hop that did not complete, a value
//! naming another account's role, which this emulator's IAM is not the authority on — is
//! not a refusal, because a refusal has to be grounded in what the owning service SAID.
//! That is also why an object the store does hold but this service cannot snapshot (one
//! holding no bytes, one that is not text, one past
//! [`super::workflow_definition::MAX_DEFINITION_BYTES`]) is not refused here: the location
//! is readable, and what such a version reports is no definition content.
//!
//! The question is stated ONCE here for every operation that freezes a configuration, so
//! the two dependencies are resolved the same way, with the same grounds and the same
//! refusals, wherever a version is appended; `UpdateWorkflow` asks it. A refused request
//! writes nothing, so the workflow is left exactly as it was — with the version history,
//! and the latest version, it already had.

use ::smithy_server_runtime::OperationContext;

use crate::ops::types::{AccessDeniedException, DefinitionS3Location, ValidationException};
use crate::state::workflow_definition::DefinitionRead;

/// The service principal a workflow's execution role has to trust for this service to
/// execute the workflow under it.
///
/// An AWS service principal is `<service>.amazonaws.com` over the service's own name in
/// IAM, which for this service is its ARN namespace and signing name,
/// [`crate::state::ARN_NAMESPACE`] (`smithy_query(query="service_info")`:
/// `awsService.arnNamespace` / `sigv4Name` / `endpointPrefix`).
pub const SERVICE_PRINCIPAL: &str = "airflow-serverless.amazonaws.com";

/// The wire path of the member that addresses the definition object, as a refusal keys
/// its field by.
pub const DEFINITION_LOCATION_MEMBER: &str = "DefinitionS3Location";

/// A dependency of the configuration that the service owning it answered about, and the
/// refusal this service reports for it.
///
/// One variant per owning service, because the two are refused with different modelled
/// errors at different statuses: a caller branches on them.
#[derive(Debug, Clone)]
pub enum UnmetDependency {
    /// Amazon S3 answered that the definition location serves no object this service can
    /// read: `ValidationException`, HTTP 400.
    DefinitionLocation(ValidationException),
    /// IAM answered that the execution role is not one this service can assume:
    /// `AccessDeniedException`, HTTP 403.
    ExecutionRole(AccessDeniedException),
}

/// Whether a dependency of the configuration a version would freeze is UNMET — asked of
/// the services that own them, and answered with the refusal to report when one is.
///
/// `definition` is what reading the definition object already established
/// ([`super::workflow_definition::read_definition`]), so the object is read once per call
/// and this asks nothing of Amazon S3 a second time. The role is resolved here, because
/// nothing else in the call needs it.
///
/// `None` — serve the request — when every dependency the configuration names is met, and
/// whenever the owning service established nothing about one (see the module docs).
///
/// Called with NO store guard held: resolving the role is a call into another emulated
/// service.
pub async fn unmet_dependency(
    ctx: &OperationContext<'_>,
    region: &str,
    definition: &DefinitionRead,
    location: &DefinitionS3Location,
    role_arn: &str,
) -> Option<UnmetDependency> {
    if matches!(definition, DefinitionRead::Inaccessible) {
        return Some(UnmetDependency::DefinitionLocation(
            inaccessible_definition_location(location),
        ));
    }
    if ::service_common::iam_role::role_is_unassumable(ctx, region, role_arn, SERVICE_PRINCIPAL)
        .await
    {
        return Some(UnmetDependency::ExecutionRole(AccessDeniedException {
            message: ACCESS_DENIED_MESSAGE.to_string(),
        }));
    }
    None
}

/// The sentence this service refuses a request it may not carry out with: the one
/// `AccessDeniedException` is documented with (`smithy_query(query="shape",
/// shape="AccessDeniedException")`), which is fixed text naming nothing of the request.
const ACCESS_DENIED_MESSAGE: &str = "You do not have sufficient permission to perform this action.";

/// The modelled `ValidationException` for a definition location Amazon S3 answered serves
/// no object this service can read.
///
/// `Bucket` and `ObjectKey` declare no constraint the boundary could check, so this
/// refusal carries the same envelope, reason and `FieldList` a constraint failure does and
/// names the member at fault, exactly as the other service rule over an unconstrained
/// member does ([`crate::state::check_logging_configuration`]). The address itself comes
/// from the request, so a caller sees which location was read.
fn inaccessible_definition_location(location: &DefinitionS3Location) -> ValidationException {
    let clause = format!(
        "Value at '{DEFINITION_LOCATION_MEMBER}' failed to satisfy constraint: Member must \
         address an Amazon S3 object this service can read, and {} is inaccessible",
        object_address(location)
    );
    ValidationException {
        message: format!("1 validation error detected: {clause}"),
        reason: crate::state::REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: Some(vec![crate::ops::types::ValidationExceptionField {
            name: DEFINITION_LOCATION_MEMBER.to_string(),
            message: clause,
        }]),
    }
}

/// The object a `DefinitionS3Location` addresses, written the way Amazon S3 addresses one:
/// `s3://<bucket>/<key>`, plus the object version when the location selects one.
fn object_address(location: &DefinitionS3Location) -> String {
    let address = format!("s3://{}/{}", location.bucket, location.object_key);
    match location.version_id.as_deref() {
        Some(version) => format!("{address}?versionId={version}"),
        None => address,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: the refusal of an inaccessible definition location names the member at
    /// fault and the address the REQUEST carried, whichever parts of the location it
    /// carried.
    #[test]
    fn the_refusal_names_the_member_and_the_address_the_request_carried() {
        // Rows differ along which parts of the address the location carries: a plain
        // bucket and key, a key with path segments, an object version, and a blank
        // bucket — the value that names no object anywhere.
        let rows: &[(&str, DefinitionS3Location, &str)] = &[
            (
                "a bucket and a key",
                DefinitionS3Location {
                    bucket: "definitions".to_string(),
                    object_key: "dag.yaml".to_string(),
                    version_id: None,
                },
                "s3://definitions/dag.yaml",
            ),
            (
                "a key under a prefix",
                DefinitionS3Location {
                    bucket: "definitions".to_string(),
                    object_key: "dags/nightly/etl.yaml".to_string(),
                    version_id: None,
                },
                "s3://definitions/dags/nightly/etl.yaml",
            ),
            (
                "a location selecting one object version",
                DefinitionS3Location {
                    bucket: "definitions".to_string(),
                    object_key: "dag.yaml".to_string(),
                    version_id: Some("3HL4kqtJlcpXroDTDmjVBH40Nrjfkd".to_string()),
                },
                "s3://definitions/dag.yaml?versionId=3HL4kqtJlcpXroDTDmjVBH40Nrjfkd",
            ),
            (
                "a blank bucket",
                DefinitionS3Location {
                    bucket: String::new(),
                    object_key: "dag.yaml".to_string(),
                    version_id: None,
                },
                "s3:///dag.yaml",
            ),
        ];
        for (case, location, address) in rows {
            let refused = inaccessible_definition_location(location);
            assert_eq!(
                refused.reason,
                crate::state::REASON_FIELD_VALIDATION_FAILED,
                "case {case:?}: expected the field-validation reason, got {:?}",
                refused.reason
            );
            assert!(
                refused.message.contains(address),
                "case {case:?}: expected the message to carry {address:?}, got {:?}",
                refused.message
            );
            let named: Vec<String> = refused
                .field_list
                .unwrap_or_default()
                .into_iter()
                .map(|field| field.name)
                .collect();
            assert_eq!(
                named,
                vec![DEFINITION_LOCATION_MEMBER.to_string()],
                "case {case:?}: expected the member at fault to be named, got {named:?}"
            );
        }
    }
}
