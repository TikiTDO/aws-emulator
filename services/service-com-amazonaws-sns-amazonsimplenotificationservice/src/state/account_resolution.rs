// SPDX-License-Identifier: CC0-1.0

//! Which ACCOUNT the ARNs a request carries address.
//!
//! An endpoint of this emulator serves one account ([`super::owner_account_id`]). An ARN
//! naming any other account names a resource that endpoint cannot resolve: there is no
//! such account behind it to look anything up in, so the call is refused BEFORE the
//! service resolves the resource the ARN names — and therefore whether or not a resource
//! of that name exists in the account this endpoint does serve. Recorded evidence: a
//! `GetTopicAttributes` and a `ListSubscriptionsByTopic` whose `TopicArn` names another
//! account are both answered `403` with `InvalidClientTokenId` and
//! [`NO_ACCOUNT_MESSAGE`], where a topic of this account that does not exist is answered
//! `404 NotFound`.
//!
//! The check belongs here rather than in each handler because it is not about any one
//! operation: it is the layer in front of all of them, which is why its refusal names
//! the CREDENTIALS rather than the parameter, and why it reports nothing about what the
//! account holds. [`crate::service`] applies it to every request this service claims.

use super::{arn_region, owner_account_id};

/// The error code this endpoint refuses a request addressing an account it does not
/// serve with: the code AWS names a request whose credentials resolve to no account by.
///
/// Not an error any operation's model declares — no SNS operation lists an error for an
/// unresolvable account, because the refusal happens before an operation is reached —
/// so it is answered as the wire refusal it is rather than as one of them.
pub const NO_ACCOUNT_CODE: &str = "InvalidClientTokenId";

/// The status of that refusal: the caller is not entitled to the account it addressed,
/// which is a 403 and not a 404 — a client treats "no such account for these
/// credentials" as a non-retryable credential fault, where a 404 reads as "the resource
/// is not there (yet)" and is routinely answered by creating it.
pub const NO_ACCOUNT_STATUS: u16 = 403;

/// The sentence that refusal states. Wholly fixed: nothing of the request is in it —
/// neither the ARN, nor the account it named, nor the member it arrived in — because the
/// layer that states it has not read the request as any operation's input.
pub const NO_ACCOUNT_MESSAGE: &str = "No account found for the given parameters";

/// The input members of this service that carry an ARN of an Amazon SNS resource, as the
/// query protocol spells them on the wire.
///
/// Source: `smithy_query(query="digest", model_path="services/service-com-amazonaws-sns-\
/// amazonsimplenotificationservice/smithy-model/com-amazonaws-sns-amazonsimplenotificati\
/// onservice.json")` — every input member of every operation whose target is one of the
/// service's ARN shapes: the topic a call addresses (`TopicArn`), the topic or platform
/// endpoint a publish addresses (`TargetArn`), a subscription (`SubscriptionArn`), the
/// resource a tag or data-protection call addresses (`ResourceArn`), a platform
/// endpoint (`EndpointArn`) and a platform application (`PlatformApplicationArn`).
///
/// It is the ARN members alone: a member that merely CARRIES text a caller chose — a
/// published message, a policy document, a subscription's endpoint in another service —
/// may name any account's resources, and names no resource of this endpoint.
pub const ARN_MEMBERS: &[&str] = &[
    "EndpointArn",
    "PlatformApplicationArn",
    "ResourceArn",
    "SubscriptionArn",
    "TargetArn",
    "TopicArn",
];

/// The wire member name whose validation path is `path`, or `None` when no ARN member of
/// this service has that path.
///
/// A validation path spells a member in lowerCamel (`topicArn`), which is not how the
/// request spelled it; a refusal that names the member has to name it as the CALLER
/// wrote it, so the path is resolved back through [`ARN_MEMBERS`].
pub fn arn_member_of_path(path: &str) -> Option<&'static str> {
    ARN_MEMBERS
        .iter()
        .copied()
        .find(|member| ::aws_smithy_schema_server::constraints::path::member(member) == path)
}

/// Whether `value` is an ARN of `region` naming an account this endpoint does not serve.
///
/// The REGION is judged first and separately: an ARN of another region is refused by
/// every operation that resolves one as the parameter fault it is
/// ([`super::foreign_region_refusal`]), so an endpoint of one region never reports a
/// credential fault about another region's account. `arn_region` answers only for a value
/// with all six of an ARN's fields filled, so a value that is not an ARN at all is left
/// to the operation's own shape rule.
pub fn names_another_account(value: &str, region: &str) -> bool {
    if arn_region(value) != Some(region) {
        return false;
    }
    account_of(value).is_some_and(|account| account != owner_account_id())
}

/// The account id `arn` names, or `None` when `arn` has fewer fields than an ARN.
///
/// The fifth field of `arn:<partition>:<service>:<region>:<account-id>:<resource>`.
fn account_of(arn: &str) -> Option<&str> {
    arn.split(':').nth(4)
}

/// Whether the form-urlencoded `body` of a query request addresses an account this
/// endpoint does not serve: any of its [`ARN_MEMBERS`] carries an ARN of `region` whose
/// account id is not this endpoint's.
pub fn addresses_another_account(body: &[u8], region: &str) -> bool {
    ::aws_smithy_schema_server::parse_form_urlencoded(body)
        .into_iter()
        .any(|(name, value)| {
            ARN_MEMBERS.contains(&name.as_str()) && names_another_account(&value, region)
        })
}
