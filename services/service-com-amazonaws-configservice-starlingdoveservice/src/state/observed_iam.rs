// SPDX-License-Identifier: CC0-1.0

//! What a recording pass OBSERVES in IAM: the account's IAM roles, and the account's own
//! password policy.
//!
//! IAM is a GLOBAL service, so what is read here is read through IAM's own API over the
//! in-process callback, exactly as the Amazon S3 and Amazon SQS observations beside it are
//! (`super::recording`): this crate names no other service crate, and a resource or a
//! setting IAM does not report is not recorded at all rather than recorded as something
//! else.
//!
//! Two readers live here because both put their question to ONE service:
//!
//! * [`observed_iam_roles`] is the recorder's observation of a resource TYPE, answering
//!   the configuration items a pass records for this account's roles;
//! * [`observed_account`] is the ACCOUNT's own configuration, which an account-wide
//!   managed rule is evaluated against (`super::account_rule`) rather than recorded as a
//!   resource — Config evaluates such a rule against the account pseudo-resource, and the
//!   account is not a resource any recorder discovers.

use std::collections::BTreeMap;
use std::time::SystemTime;

use super::DiscoveredResourceEntry;

/// The resource type Config records an IAM role under, a
/// `com.amazonaws.configservice#ResourceType` value.
pub const IAM_ROLE_RESOURCE_TYPE: &str = "AWS::IAM::Role";

/// The resource types this emulator can observe that belong to a GLOBAL service — the
/// ones a recording group covers only when it asks for the global resource types
/// ([`records_global_resource_types`]).
///
/// Source: the `AWS::IAM::*` and other non-regional entries of
/// `com.amazonaws.configservice#ResourceType`, of which this emulator observes the roles
/// today; a global type a later pass learns to observe belongs here, so the gate below
/// stays one rule.
pub const GLOBAL_RESOURCE_TYPES: [&str; 1] = [IAM_ROLE_RESOURCE_TYPE];

/// Whether `resource_type` belongs to a global service, so that recording it is governed
/// by `RecordingGroup$includeGlobalResourceTypes` ("Specifies whether Config records
/// configuration changes for all supported global resources").
pub fn is_global_resource_type(resource_type: &str) -> bool {
    GLOBAL_RESOURCE_TYPES.contains(&resource_type)
}

/// Whether `group` covers `resource_type` as far as its being GLOBAL decides.
///
/// A group that records every supported type covers the global ones only when it asks for
/// them: "Default is to record all supported resource types, excluding the global IAM
/// resource types" (`PutConfigurationRecorder`), and
/// `RecordingGroup$includeGlobalResourceTypes` is the member that asks
/// ("`includeGlobalResourceTypes` and the resource exclusions in `recordingGroup` are not
/// compatible", so the member governs the all-supported strategy alone). A group that
/// ENUMERATES the types it records, or that records every type bar the ones it excludes,
/// has already said what it covers — [`super::group_records_resource_type`] is what reads
/// that — so being global changes nothing for it.
pub fn group_records_global_resource_type(
    group: &crate::ops::types::RecordingGroup,
    resource_type: &str,
) -> bool {
    if !is_global_resource_type(resource_type) {
        return true;
    }
    if super::recording_strategy_of(group) != super::RECORDING_STRATEGY_ALL_SUPPORTED {
        return true;
    }
    group.include_global_resource_types
}

/// How many pages of roles one pass reads before it stops asking for more.
///
/// A HOST bound: the inventory a pass writes into is bounded
/// ([`super::DISCOVERED_RESOURCES_KEPT`]), so one pass reads at most the pages that can
/// fill it rather than following a marker without end.
const MAX_ROLE_PAGES: usize = 20;

/// The configuration items the recorder observes for this account's IAM roles, or `None`
/// when IAM could not be read at all — which is not the same as an account with no roles.
///
/// Every member of the recorded configuration document comes from an IAM read named on
/// it; nothing else is recorded, because nothing else is observed. A role is identified by
/// the `RoleId` IAM minted for it — the id that does not change when the role is renamed —
/// and its custom name is the role name, which is how Config identifies a role in its
/// inventory.
pub async fn observed_iam_roles(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    now: SystemTime,
) -> Option<Vec<DiscoveredResourceEntry>> {
    let client = ::service_common::iam_client::iam_client_through(callback, region);

    // "IAM resource-listing operations return a subset of the available attributes"
    // (`ListRoles`), and the listing is paged: a pass follows the marker so an account
    // holding more roles than one page is observed in full.
    let mut listed = Vec::new();
    let mut marker: Option<String> = None;
    for _ in 0..MAX_ROLE_PAGES {
        let page = match client
            .list_roles()
            .set_marker(marker.clone())
            .send()
            .await
        {
            Ok(page) => page,
            Err(error) => {
                ::tracing::warn!(
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the configuration recorder could not read this account's IAM roles"
                );
                return None;
            }
        };
        listed.extend(page.roles.clone());
        marker = page.marker.clone();
        if !page.is_truncated || marker.is_none() {
            break;
        }
    }

    let mut observed = Vec::new();
    for role in listed {
        // `ListRoleTags`' `Tags`: the tags a rule's scope can narrow by, which the
        // listing above withholds ("This operation does not return the following
        // attributes ... Tags").
        let tags: BTreeMap<String, String> = client
            .list_role_tags()
            .role_name(&role.role_name)
            .send()
            .await
            .ok()
            .map(|answer| {
                answer
                    .tags
                    .into_iter()
                    .map(|tag| (tag.key, tag.value))
                    .collect()
            })
            .unwrap_or_default();

        // The configuration item Config records for a role: the members IAM's own read
        // reports for it. IAM reports a role's whole configuration through one call, so
        // there is no further block to supplement it with.
        let configuration = ::serde_json::json!({
            // "The path to the role" (`Role$Path`).
            "path": role.path,
            // "The friendly name that identifies the role" (`Role$RoleName`).
            "roleName": role.role_name,
            // "The stable and unique string identifying the role" (`Role$RoleId`).
            "roleId": role.role_id,
            "arn": role.arn,
            // "The date and time, in ISO 8601 date-time format, when the role was
            // created" (`Role$CreateDate`).
            "createDate": role
                .create_date
                .fmt(::aws_smithy_types::date_time::Format::DateTime)
                .ok(),
            // "The policy that grants an entity permission to assume the role"
            // (`Role$AssumeRolePolicyDocument`).
            "assumeRolePolicyDocument": role.assume_role_policy_document,
            // "A description of the role that you provide" (`Role$Description`).
            "description": role.description,
            // "The maximum session duration (in seconds) for the specified role"
            // (`Role$MaxSessionDuration`).
            "maxSessionDuration": role.max_session_duration,
        });

        observed.push(DiscoveredResourceEntry {
            resource_type: IAM_ROLE_RESOURCE_TYPE.to_string(),
            resource_id: role.role_id.clone(),
            resource_name: Some(role.role_name.clone()),
            // The ARN IAM minted for the role, which is the ARN its configuration item
            // carries: it names the role by its path and name, neither of which the
            // recorded id is.
            arn: Some(role.arn.clone()),
            deleted_at: None,
            // The schema version of a configuration item Config itself recorded is not
            // one a caller supplied, and this emulator has no catalogue of them, so none
            // is reported rather than an invented one.
            schema_version_id: None,
            configuration: Some(configuration.to_string()),
            supplementary_configuration: BTreeMap::new(),
            tags,
            first_recorded_at: Some(now),
            last_recorded_at: Some(now),
            // The states this role has already left are kept by the store's own write
            // path, which carries them over from the item this one replaces.
            history: Vec::new(),
        });
    }
    Some(observed)
}

/// The account's own configuration, as an account-wide rule is evaluated against it.
#[derive(Debug, Clone, Default)]
pub struct ObservedAccount {
    /// The account's password policy, or `None` for an account that has NONE — which is
    /// a fact about the account ("The request was rejected because it referenced a
    /// resource entity that does not exist", which is how IAM answers a read of a policy
    /// the account never set), not a failure to read it.
    pub password_policy: Option<ObservedPasswordPolicy>,
}

/// The account password policy IAM reports, as the members an account-wide rule reads it
/// by (`com.amazonaws.iam#PasswordPolicy`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObservedPasswordPolicy {
    /// "Minimum length to require for IAM user passwords"
    /// (`PasswordPolicy$MinimumPasswordLength`).
    pub minimum_password_length: Option<i32>,
    /// Whether a password must carry one of the symbols the member lists
    /// (`PasswordPolicy$RequireSymbols`).
    pub require_symbols: bool,
    /// Whether a password must carry a numeric character
    /// (`PasswordPolicy$RequireNumbers`).
    pub require_numbers: bool,
    /// Whether a password must carry an uppercase character
    /// (`PasswordPolicy$RequireUppercaseCharacters`).
    pub require_uppercase_characters: bool,
    /// Whether a password must carry a lowercase character
    /// (`PasswordPolicy$RequireLowercaseCharacters`).
    pub require_lowercase_characters: bool,
    /// "The number of days that an IAM user password is valid"
    /// (`PasswordPolicy$MaxPasswordAge`), absent where passwords never expire.
    pub max_password_age: Option<i32>,
    /// "The number of previous passwords that IAM users are prevented from reusing"
    /// (`PasswordPolicy$PasswordReusePrevention`), absent where reuse is not prevented.
    pub password_reuse_prevention: Option<i32>,
}

/// The account's own configuration as IAM reports it, or `None` when IAM could not be
/// read at all — which is not the same as an account that has set nothing.
///
/// `GetAccountPasswordPolicy` answers the policy in force, and answers `NoSuchEntity` for
/// an account that has never set one; both are facts the account's rules are evaluated
/// against, so they are told apart here and only a read that FAILED leaves the account
/// unobserved.
pub async fn observed_account(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) -> Option<ObservedAccount> {
    let client = ::service_common::iam_client::iam_client_through(callback, region);
    match client.get_account_password_policy().send().await {
        Ok(answer) => {
            let policy = answer.password_policy?;
            Some(ObservedAccount {
                password_policy: Some(ObservedPasswordPolicy {
                    minimum_password_length: policy.minimum_password_length,
                    require_symbols: policy.require_symbols,
                    require_numbers: policy.require_numbers,
                    require_uppercase_characters: policy.require_uppercase_characters,
                    require_lowercase_characters: policy.require_lowercase_characters,
                    max_password_age: policy.max_password_age,
                    password_reuse_prevention: policy.password_reuse_prevention,
                }),
            })
        }
        Err(error) => {
            // The account holds no password policy: IAM says so with the entity-not-found
            // refusal, which is an OBSERVATION of the account rather than a failed read.
            if error
                .as_service_error()
                .is_some_and(|service| service.is_no_such_entity_exception())
            {
                return Some(ObservedAccount {
                    password_policy: None,
                });
            }
            ::tracing::warn!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "Config could not read this account's password policy"
            );
            None
        }
    }
}
