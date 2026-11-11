// SPDX-License-Identifier: CC0-1.0

//! The documented bounds of the retry policy inside a delivery policy document.
//!
//! A delivery policy is schema-validated when it is WRITTEN, so a retry budget the
//! service would never run under is refused rather than stored: "the value must be a
//! delivery policy document" ([`super::delivery_policy_refusal`]) judges the document's
//! SHAPE, and the bounds here judge the numbers in it.
//!
//! The same retry policy appears in both documents a delivery policy is written as — a
//! topic's `http.defaultHealthyRetryPolicy` and a subscription's own
//! `healthyRetryPolicy` — so one catalog of bounds serves both, and the member path the
//! refusal names is the only difference between them.
//!
//! The bounds are the published ones (SNS Developer Guide, "Amazon SNS message delivery
//! retries": `minDelayTarget` and `maxDelayTarget` are delays in seconds of at least 1
//! and at most 3600, `numRetries` is a count of 0 to 100, and each phase's retry count is
//! a count of at least 0), not this emulator's choice, so a policy this refuses is one a
//! real account refuses too.

use super::{
    ConfiguredDeliveryPolicy, ConfiguredRetryPolicy, ConfiguredSubscriptionDeliveryPolicy,
    DELIVERY_POLICY_ATTRIBUTE,
};

/// The member a SUBSCRIPTION's delivery policy carries its retry policy under: the
/// document governs one subscription, so the retry policy is at its top level.
pub const SUBSCRIPTION_RETRY_POLICY_PATH: &str = "healthyRetryPolicy";

/// The member a TOPIC's delivery policy carries the retry policy its HTTP/S deliveries
/// default to under.
pub const TOPIC_RETRY_POLICY_PATH: &str = "http.defaultHealthyRetryPolicy";

/// The shortest delay a retry may be asked to wait, in seconds.
const MIN_DELAY_SECONDS: i64 = 1;

/// The longest delay a retry may be asked to wait, in seconds — the ceiling of the
/// backoff window a delivery policy can describe.
const MAX_DELAY_SECONDS: i64 = 3600;

/// The largest total retry count a delivery policy may ask for.
const MAX_RETRIES: i64 = 100;

/// One retry-policy member's documented bounds, and how to read that member out of the
/// policy a caller wrote.
struct RetryPolicyBound {
    /// The member's name in the document, which the refusal names.
    member: &'static str,
    /// The smallest value the member takes.
    minimum: i64,
    /// The largest value the member takes, or `None` for a member whose published
    /// constraint is a floor alone.
    maximum: Option<i64>,
    /// The value the caller wrote for it, or `None` when the caller named no value.
    value: fn(&ConfiguredRetryPolicy) -> Option<i64>,
}

/// Every retry-policy member whose value SNS bounds, in the order a document lists them,
/// so a document that crosses more than one bound is refused for the same member every
/// time.
const RETRY_POLICY_BOUNDS: &[RetryPolicyBound] = &[
    RetryPolicyBound {
        member: "minDelayTarget",
        minimum: MIN_DELAY_SECONDS,
        maximum: Some(MAX_DELAY_SECONDS),
        value: |retry| retry.min_delay_target,
    },
    RetryPolicyBound {
        member: "maxDelayTarget",
        minimum: MIN_DELAY_SECONDS,
        maximum: Some(MAX_DELAY_SECONDS),
        value: |retry| retry.max_delay_target,
    },
    RetryPolicyBound {
        member: "numRetries",
        minimum: 0,
        maximum: Some(MAX_RETRIES),
        value: |retry| retry.num_retries,
    },
    RetryPolicyBound {
        member: "numNoDelayRetries",
        minimum: 0,
        maximum: None,
        value: |retry| retry.num_no_delay_retries,
    },
    RetryPolicyBound {
        member: "numMinDelayRetries",
        minimum: 0,
        maximum: None,
        value: |retry| retry.num_min_delay_retries,
    },
    RetryPolicyBound {
        member: "numMaxDelayRetries",
        minimum: 0,
        maximum: None,
        value: |retry| retry.num_max_delay_retries,
    },
];

/// The WHOLE `InvalidParameter` message a delivery policy whose retry member is outside
/// its documented bounds is refused with: the attribute, the member that failed by the
/// path it is written at, and the bound it crossed.
///
/// The limit is the DOCUMENTED bound rather than anything the request carried, and the
/// member path is the one the document writes it at, so the sentence tells the caller
/// exactly which number to change and what to change it to.
fn bound_refusal(path: &str, member: &str, relation: &str, limit: i64) -> String {
    format!("Invalid parameter: {DELIVERY_POLICY_ATTRIBUTE}: {path}.{member} must be {relation} {limit}")
}

/// Why the retry policy a caller wrote is outside its documented bounds, or `None` when
/// every member it names is within them.
///
/// `path` is where the document carries this retry policy
/// ([`SUBSCRIPTION_RETRY_POLICY_PATH`] / [`TOPIC_RETRY_POLICY_PATH`]), which is how the
/// refusal names the member.
fn retry_policy_refusal(retry: &ConfiguredRetryPolicy, path: &str) -> Option<String> {
    RETRY_POLICY_BOUNDS.iter().find_map(|bound| {
        let value = (bound.value)(retry)?;
        if value < bound.minimum {
            return Some(bound_refusal(
                path,
                bound.member,
                "greater than or equal to",
                bound.minimum,
            ));
        }
        let maximum = bound.maximum?;
        (value > maximum).then(|| {
            bound_refusal(path, bound.member, "less than or equal to", maximum)
        })
    })
}

/// The WHOLE message a write of a SUBSCRIPTION's `DeliveryPolicy` is refused with when
/// the retry policy it names is outside its documented bounds, or `None` when it is
/// within them.
///
/// A value whose SHAPE is not a delivery policy document at all carries no bounded
/// member either, so it is `None` here and refused by
/// [`super::subscription_delivery_policy_refusal`] instead.
pub fn subscription_delivery_policy_bounds_refusal(value: &str) -> Option<String> {
    let configured = serde_json::from_str::<ConfiguredSubscriptionDeliveryPolicy>(value).ok()?;
    retry_policy_refusal(
        configured.healthy_retry_policy.as_ref()?,
        SUBSCRIPTION_RETRY_POLICY_PATH,
    )
}

/// The WHOLE message a write of a TOPIC's `DeliveryPolicy` is refused with when the
/// retry policy it defaults its HTTP/S deliveries to is outside its documented bounds,
/// or `None` when it is within them.
///
/// The same bounds as a subscription's, because it is the same retry policy: a topic
/// writes it one level down, under the protocol it states the defaults for.
pub fn topic_delivery_policy_bounds_refusal(value: &str) -> Option<String> {
    let configured = serde_json::from_str::<ConfiguredDeliveryPolicy>(value).ok()?;
    retry_policy_refusal(
        configured.http?.default_healthy_retry_policy.as_ref()?,
        TOPIC_RETRY_POLICY_PATH,
    )
}
