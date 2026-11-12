// SPDX-License-Identifier: CC0-1.0

//! Whether the TARGET a remediation configuration names exists: the Systems Manager
//! automation document its `TargetId` names, asked of Systems Manager itself.
//!
//! "The target (SSM document) must exist and have permissions to use the target"
//! (`PutRemediationConfigurations`), so the document is a resource ANOTHER service owns
//! and the only thing that can say whether it exists is that service's own read. Systems
//! Manager is reached the way this service reaches every other one: the AWS SDK driven
//! over the in-process callback ([`smithy_server_runtime::OperationContext::callback`]),
//! so nothing here names another service crate and nothing leaves this process.
//!
//! A `TargetId` is "the target ID is the name of the SSM document" and a Systems Manager
//! document read takes "the name … [which] can be an Amazon Resource Name (ARN)", so the
//! value is passed as sent and Systems Manager resolves either spelling.
//!
//! Three answers, because the question has three outcomes and only one of them is a
//! refusal: the registry HOLDS the document, the registry ANSWERS that it holds none of
//! that name (its own `InvalidDocument`), or nothing was answered about the document at
//! all — Systems Manager could not be reached, or it failed — in which case nothing is
//! known about the target and the configuration is taken as sent. That is the fail-safe
//! direction, and the only one that is true to the question: Config cannot conclude that a
//! target is missing from a registry it did not get an answer from.

/// What Systems Manager said about the document a remediation configuration targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemediationTarget {
    /// Systems Manager holds the document the configuration names.
    Found,
    /// Systems Manager answered that it holds no such document — the target does not
    /// exist, which is what `PutRemediationConfigurations` refuses.
    NotFound,
    /// Systems Manager could not be asked, so nothing is known about the target.
    Unknown,
}

/// How Config words a remediation configuration whose target it could not find.
///
/// BUILT FROM THE REQUEST: the fixed sentence pair carries the name of the Config rule
/// the configuration was sent for (`RemediationConfiguration$ConfigRuleName`) as its one
/// variable part, and names neither the target nor the account's documents.
pub fn remediation_target_not_found_message(config_rule_name: &str) -> String {
    format!(
        "The remediation target for {config_rule_name} could not be found. Specify the \
         appropriate remediation target and try again."
    )
}

/// Whether `target_type` names an SSM automation document as the configuration's target
/// (`RemediationConfiguration$TargetType`).
///
/// Read off the generated value set of `com.amazonaws.configservice#RemediationTargetType`
/// rather than a spelling written here: that enum declares exactly ONE target type, the
/// SSM document, so a configuration whose target type is a modelled one targets a
/// document of the registry this module asks about — and a target type the model does not
/// declare is refused at the request boundary, never resolved here.
pub fn targets_an_ssm_document(target_type: &str) -> bool {
    crate::ops::types::REMEDIATION_TARGET_TYPE_VALUES.contains(&target_type.trim())
}

/// Ask Systems Manager whether it holds the automation document `target_id` names, in the
/// Region the request was signed for.
pub async fn remediation_target(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    target_id: &str,
) -> RemediationTarget {
    let client = ssm_client(callback, region);
    match client.describe_document().name(target_id).send().await {
        Ok(_) => RemediationTarget::Found,
        Err(error) => {
            // Systems Manager ANSWERING that it holds no document of that name is its own
            // modelled `InvalidDocument` ("The specified SSM document doesn't exist"), and
            // that one answer is the only thing that makes a target missing. Every other
            // outcome — no answer at all, a failure of the service, an answer this client
            // cannot place — says nothing about the document, so the target stays unknown
            // and the configuration is taken as sent.
            match error.as_service_error() {
                Some(
                    ::aws_sdk_ssm::operation::describe_document::DescribeDocumentError::InvalidDocument(
                        _,
                    ),
                ) => RemediationTarget::NotFound,
                _ => {
                    ::tracing::warn!(
                        error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                        "Config could not ask Systems Manager for this remediation target"
                    );
                    RemediationTarget::Unknown
                }
            }
        }
    }
}

/// Why the remediation configuration `requested` is not one this account can create, or
/// `None` for one it can.
///
/// These are the preconditions `PutRemediationConfigurations` enforces on a remediation
/// action a caller writes directly — a parameter that selects neither or both of the two
/// value kinds a `RemediationParameterValue` carries, an AUTOMATIC configuration with no
/// value for the role the remediation runs as, and a target Systems Manager holds no
/// automation document for — asked in ONE place so a remediation action a conformance
/// pack's template declares is judged by exactly the same rules: nothing can create a
/// remediation a create of it would refuse.
///
/// The answer is a REASON for a deployment that could not be performed rather than the
/// refusal the API answers a caller with, because that is what the caller of this is
/// reporting; it names the Config rule the configuration was declared for and what about
/// the configuration cannot be created.
pub async fn unusable_remediation(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    requested: &crate::ops::types::RemediationConfiguration,
) -> Option<String> {
    let config_rule_name = requested.config_rule_name.trim();
    if let Some((parameter, wrong)) = super::ill_formed_parameter(requested) {
        return Some(format!(
            "the remediation action for {config_rule_name} carries parameter '{parameter}' \
             with {wrong}"
        ));
    }
    if requested.automatic && !super::has_assume_role(requested) {
        return Some(format!(
            "the remediation action for {config_rule_name} provides no value for {}, which \
             the role an automatic remediation runs as is taken from",
            super::assume_role_parameter_names(requested.automatic).join(" or "),
        ));
    }
    if targets_an_ssm_document(&requested.target_type)
        && remediation_target(callback, region, requested.target_id.trim()).await
            == RemediationTarget::NotFound
    {
        return Some(format!(
            "the remediation action for {config_rule_name} targets the automation document \
             '{}', which Systems Manager holds none of",
            requested.target_id.trim(),
        ));
    }
    None
}

/// A Systems Manager client over the in-process callback, built the way this crate's
/// other cross-service clients are ([`super::s3_client`]): the emulator supplies the
/// transport,
/// so there is nothing to retry, time out, cache an identity for or watch for a stalled
/// stream.
fn ssm_client(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) -> ::aws_sdk_ssm::Client {
    let config = ::aws_sdk_ssm::Config::builder()
        .behavior_version(::aws_sdk_ssm::config::BehaviorVersion::latest())
        .region(::aws_sdk_ssm::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://ssm.{region}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(::aws_sdk_ssm::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_ssm::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_ssm::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_ssm::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_ssm::Client::from_conf(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The refusal names the rule the configuration was sent for and nothing else, for
    /// rule names differing along what they carry.
    #[test]
    fn the_refusal_names_the_rule_the_configuration_was_sent_for() {
        for rule in ["a-rule", "rule.with.dots", "RULE-7", &"r".repeat(128)] {
            let message = remediation_target_not_found_message(rule);
            assert_eq!(
                message,
                format!(
                    "The remediation target for {rule} could not be found. Specify the \
                     appropriate remediation target and try again."
                ),
                "case {rule:?}: expected the rule's name interpolated unquoted, got \
                 {message:?}"
            );
            assert!(
                message.contains(rule) && !message.contains(&format!("'{rule}'")),
                "case {rule:?}: the rule name is carried unquoted, got {message:?}"
            );
        }
    }
}
