// SPDX-License-Identifier: CC0-1.0

//! What a `NetworkConnectorIdentifier` has to BE before it can name anything.
//!
//! `NetworkConnectorIdentifier` is "a flexible identifier that accepts a network connector
//! ID, name, or ARN", and the two non-ARN forms are plain `[a-zA-Z0-9_-]` words
//! (`NetworkConnectorName`, `NetworkConnectorId`): a value spelled as an ARN is therefore
//! claiming to be the third form, and the service settles two things about it before it
//! looks for a connector at all:
//!
//! 1. is it one of THIS service's network-connector ARNs? `NetworkConnectorArn`'s own
//!    expression says what that is — `arn:<partition>:lambda:<region>:<12-digit
//!    account>:network-connector:<resource>` with an optional version suffix — and an ARN
//!    that is not one names no connector anywhere, so it is refused as a bad parameter
//!    rather than reported absent;
//! 2. is it an ARN of the CALLER's account? An account id is part of an ARN, so an ARN
//!    naming another account addresses a resource this caller has no access to, which is an
//!    authorization refusal and not an existence answer.
//!
//! Both precede the lookup, so neither answer depends on what this account happens to
//! hold — which is the point: a caller probing another account's ARNs learns nothing about
//! what is there.

use crate::ops::types::InvalidParameterValueException;

/// What an `Identifier` that cannot name a connector of this account is, in the order the
/// service settles it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnusableIdentifier {
    /// Spelled as an ARN, but not one of this service's network-connector ARNs.
    NotAConnectorArn,
    /// A well-formed network-connector ARN whose account is not the caller's.
    AnotherAccount,
}

/// What tells an identifier that claims to be an ARN from one that is a bare name or id:
/// both of those are `[a-zA-Z0-9_-]` words, so neither can carry a colon.
const ARN_PREFIX: &str = "arn:";

/// The AWS partition every ARN of an emulated resource carries, as its prefix: `aws`,
/// `aws-cn`, `aws-us-gov` and the rest all start with it, which is what
/// `NetworkConnectorArn`'s expression writes for the field.
const PARTITION_PREFIX: &str = "aws";

/// The service an ARN of a network connector names: the connector is a Lambda resource, so
/// `NetworkConnectorArn`'s expression fixes this segment to `lambda`.
const ARN_SERVICE: &str = "lambda";

/// The resource type segment of a network-connector ARN.
pub const ARN_RESOURCE_TYPE: &str = "network-connector";

/// How many characters an AWS account id has, all of them digits. The shared parse admits
/// an EMPTY account by design (a partition-global resource has none), which
/// `NetworkConnectorArn`'s expression does not, so this is the one field rule kept here.
const ACCOUNT_ID_DIGITS: usize = 12;

/// How the identifier `identifier` fails before it is resolved, or `None` when it is a name,
/// an id, or a network-connector ARN of the caller's own account.
pub fn unusable(identifier: &str) -> Option<UnusableIdentifier> {
    if !identifier.starts_with(ARN_PREFIX) {
        // A name or an id: there is nothing about it to settle before the lookup, which is
        // what answers for it.
        return None;
    }
    match connector_arn_account(identifier) {
        Err(unusable) => Some(unusable),
        Ok(account) => (account != ::engine::emulator_account_id())
            .then_some(UnusableIdentifier::AnotherAccount),
    }
}

/// The account segment of `identifier` when it IS a network-connector ARN, and
/// [`UnusableIdentifier::NotAConnectorArn`] when it is not.
///
/// The ARN itself is read through the tree's ONE ARN parse
/// (`service_common::arn`), so this service cannot disagree with another about what an ARN
/// is; what is checked here is only what `NetworkConnectorArn`'s own expression adds to it:
/// the vendor, the partition, the twelve-digit account and the `network-connector` resource
/// type with a resource actually named. WHICH connector that resource names is not read
/// here, because that is what the lookup answers.
fn connector_arn_account(identifier: &str) -> Result<&str, UnusableIdentifier> {
    let unusable = Err(UnusableIdentifier::NotAConnectorArn);
    let Some(parsed) = ::service_common::arn::parse_of(identifier, ARN_SERVICE) else {
        return unusable;
    };
    let names_a_connector =
        ::service_common::arn::resource_id_of_type(identifier, ARN_RESOURCE_TYPE)
            .is_some_and(|resource| !resource.is_empty());
    let owned_by_an_account = parsed.account.len() == ACCOUNT_ID_DIGITS
        && parsed.account.bytes().all(|digit| digit.is_ascii_digit());
    if names_a_connector && owned_by_an_account && parsed.partition.starts_with(PARTITION_PREFIX) {
        Ok(parsed.account)
    } else {
        unusable
    }
}

/// What an `Identifier` spelled as an ARN that is not a network connector's is refused
/// with.
///
/// The sentence names the connector RESOURCE the service was asked to address — the
/// `network-connector` resource type followed by the value as supplied, which is the
/// resource part the service could not read as an ARN — so the caller sees exactly what it
/// sent back. That is the form the recorded refusal of this class states, with the
/// identifier as its only variable part.
pub fn invalid_connector_arn(identifier: &str) -> InvalidParameterValueException {
    crate::state::invalid_parameter(format!(
        "{INVALID_ARN_PREFIX}{ARN_RESOURCE_TYPE}:{identifier}"
    ))
}

/// What [`invalid_connector_arn`] states before the resource it could not read.
const INVALID_ARN_PREFIX: &str = "Network connector arn invalid: ";

/// What an `Identifier` naming another account's connector is refused with: a fixed
/// sentence, because the only thing the service will say about another account's resource is
/// that this caller may not reach it.
pub const CROSS_ACCOUNT_DENIED: &str = "Cross-account access is not allowed.";

#[cfg(test)]
mod tests {
    //! The two rules this module settles, each over a SET of identifiers.

    use super::*;

    /// The account every ARN of an emulated resource carries, which is the caller's.
    fn own_account() -> &'static str {
        ::engine::emulator_account_id()
    }

    /// RULE: a name or an id has nothing to settle; an ARN must be a network-connector ARN
    /// of the caller's own account.
    #[test]
    fn an_identifier_is_settled_before_it_is_resolved() {
        let own = own_account();
        let rows: Vec<(&str, String, Option<UnusableIdentifier>)> = vec![
            ("a bare name", "my-connector".to_string(), None),
            ("an id", "nc-0123456789abcdef".to_string(), None),
            (
                "a name that looks like a resource type",
                "network-connector".to_string(),
                None,
            ),
            (
                "this account's connector ARN",
                format!("arn:aws:lambda:us-west-2:{own}:network-connector:my-connector"),
                None,
            ),
            (
                "this account's version-qualified connector ARN",
                format!("arn:aws:lambda:eu-west-1:{own}:network-connector:my-connector:7"),
                None,
            ),
            (
                "another partition, this account",
                format!("arn:aws-us-gov:lambda:us-gov-west-1:{own}:network-connector:gov"),
                None,
            ),
            (
                "another account's connector ARN",
                "arn:aws:lambda:us-west-2:111122223333:network-connector:theirs".to_string(),
                Some(UnusableIdentifier::AnotherAccount),
            ),
            (
                "another account's version-qualified ARN",
                "arn:aws:lambda:us-west-2:444455556666:network-connector:theirs:2".to_string(),
                Some(UnusableIdentifier::AnotherAccount),
            ),
            (
                "an ARN of another service",
                "arn:aws:s3:::some-bucket".to_string(),
                Some(UnusableIdentifier::NotAConnectorArn),
            ),
            (
                "a Lambda ARN of another resource type",
                format!("arn:aws:lambda:us-west-2:{own}:function:some-function"),
                Some(UnusableIdentifier::NotAConnectorArn),
            ),
            (
                "a connector ARN with no resource named",
                format!("arn:aws:lambda:us-west-2:{own}:network-connector:"),
                Some(UnusableIdentifier::NotAConnectorArn),
            ),
            (
                "a connector ARN whose account is not 12 digits",
                "arn:aws:lambda:us-west-2:12345:network-connector:theirs".to_string(),
                Some(UnusableIdentifier::NotAConnectorArn),
            ),
            (
                "a connector ARN whose account is not digits at all",
                "arn:aws:lambda:us-west-2:not-an-accou:network-connector:theirs".to_string(),
                Some(UnusableIdentifier::NotAConnectorArn),
            ),
            (
                "a value that only starts like an ARN",
                "arn:nonsense".to_string(),
                Some(UnusableIdentifier::NotAConnectorArn),
            ),
        ];

        for (case, identifier, expected) in rows {
            let settled = unusable(&identifier);
            assert_eq!(
                settled, expected,
                "case {case:?} ({identifier:?}): expected {expected:?}, got {settled:?}"
            );
        }
    }

    /// RULE: the invalid-ARN refusal names the resource the service was asked to address —
    /// the resource type and the value as supplied — and carries the parameter class's own
    /// `Type`.
    #[test]
    fn the_invalid_arn_refusal_names_the_value_as_supplied() {
        for case in [
            "arn:aws:s3:::some-bucket",
            "arn:aws:lambda:us-west-2:000000000000:function:some-function",
            "arn:nonsense",
        ] {
            let refused = invalid_connector_arn(case);
            let expected = format!("Network connector arn invalid: network-connector:{case}");
            assert_eq!(
                refused.message.as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected the message {expected:?}, got {:?}",
                refused.message
            );
            assert_eq!(
                refused.r#type.as_deref(),
                Some(crate::state::EXCEPTION_TYPE_USER),
                "case {case:?}: expected the parameter class's Type, got {:?}",
                refused.r#type
            );
        }
    }
}
