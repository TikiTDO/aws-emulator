// SPDX-License-Identifier: CC0-1.0

//! The cost anomaly SUBSCRIPTION registry: the record every alert-subscription operation
//! reads, the admission rules the `AnomalySubscription` members document, and the
//! shorthand expansion the deprecated scalar threshold is defined by.
//!
//! It lives beside [`crate::state`] rather than in one handler's file because the create,
//! read, update, delete and tagging surfaces all have to agree on what a stored
//! subscription is and on which thresholds are admissible; duplicating that across those
//! handlers is how they would drift apart.

use crate::ops::types::{
    AnomalySubscription, DimensionValues, Expression, ResourceTag, Subscriber,
    UnknownSubscriptionException,
};

/// How many cost anomaly subscriptions an account may hold.
///
/// This service's model documents no number for it, so the bound is CHOSEN: it is what
/// keeps the registry from growing without limit however many subscriptions a caller
/// creates, and it is high enough that no ordinary use reaches it. `LimitExceededException`
/// (HTTP 400) is the client failure `CreateAnomalySubscription` models, so that is what a
/// create past the bound is refused with.
pub const MAX_ANOMALY_SUBSCRIPTIONS: usize = 100;

/// The `Dimension` values a `ThresholdExpression` may select on.
///
/// From `AnomalySubscription$ThresholdExpression`: "The supported dimensions are
/// ANOMALY_TOTAL_IMPACT_ABSOLUTE and ANOMALY_TOTAL_IMPACT_PERCENTAGE".
pub const THRESHOLD_DIMENSIONS: &[&str] = &[
    "ANOMALY_TOTAL_IMPACT_ABSOLUTE",
    "ANOMALY_TOTAL_IMPACT_PERCENTAGE",
];

/// The dimension the deprecated scalar `Threshold` names.
///
/// From `AnomalySubscription$Threshold`: "An absolute dollar value that must be exceeded
/// by the anomaly's total impact", which is the `ANOMALY_TOTAL_IMPACT_ABSOLUTE` dimension
/// `ThresholdExpression` documents as "corresponding to an anomaly's TotalImpact".
pub const THRESHOLD_ABSOLUTE_DIMENSION: &str = "ANOMALY_TOTAL_IMPACT_ABSOLUTE";

/// The one `MatchOption` a threshold expression carries.
///
/// From `AnomalySubscription$ThresholdExpression`: "The match option
/// GREATER_THAN_OR_EQUAL is required."
pub const THRESHOLD_MATCH_OPTION: &str = "GREATER_THAN_OR_EQUAL";

/// The largest threshold value a `ThresholdExpression` may carry.
///
/// From `AnomalySubscription$ThresholdExpression`: "Values must be numbers between 0 and
/// 10,000,000,000 in string format."
pub const THRESHOLD_VALUE_MAX: f64 = 10_000_000_000.0;

/// How deep a `ThresholdExpression`'s `And`/`Or` nesting may go.
///
/// The model documents no depth, so the bound is CHOSEN: it stops caller-supplied nesting
/// from being walked without limit, and it is far deeper than the two-branch combinations
/// the member's own examples show.
pub const MAX_THRESHOLD_EXPRESSION_DEPTH: usize = 10;

/// The `SubscriberStatus` a subscriber the service has just registered is reported with.
///
/// From the model's `SubscriberStatus` enum, whose values are `CONFIRMED` and `DECLINED`,
/// and `Subscriber$Status`, "Indicates if the subscriber accepts the notifications": a
/// subscriber the account has just named accepts them until it declines, which is a
/// decision only the subscriber can make. The member is therefore owned by the service,
/// not set by the caller.
pub const SUBSCRIBER_STATUS_CONFIRMED: &str = "CONFIRMED";

/// Check an `AnomalySubscription` against the rules its own members document, returning
/// the refusal text when it breaks one.
///
/// The rules come from `AnomalySubscription$MonitorArnList` ("a list of cost anomaly
/// monitors"), `$Subscribers` ("a list of subscribers to notify") — both required — and
/// `$Threshold`/`$ThresholdExpression`: "One of Threshold or ThresholdExpression is
/// required for this resource. You cannot specify both." The model expresses none of
/// this, so a subscription its schema admits but its documentation forbids is refused
/// here.
///
/// Shared by every operation that accepts a whole `AnomalySubscription`, so the write
/// paths cannot drift apart on it.
pub fn validate_anomaly_subscription(subscription: &AnomalySubscription) -> Result<(), String> {
    validate_monitor_arn_list(&subscription.monitor_arn_list)?;
    validate_subscribers(&subscription.subscribers)?;
    validate_threshold(
        subscription.threshold,
        subscription.threshold_expression.as_ref(),
    )
}

/// Check the monitors a subscription alerts on.
///
/// Taken as the list rather than as a whole subscription so `UpdateAnomalySubscription`,
/// which carries the same list as a top-level member, applies the same rule to the value
/// it would store.
pub fn validate_monitor_arn_list(monitor_arn_list: &[String]) -> Result<(), String> {
    if monitor_arn_list.is_empty() {
        return Err(
            "MonitorArnList must name at least one cost anomaly monitor for the subscription \
             to alert on"
                .to_string(),
        );
    }
    Ok(())
}

/// Check the subscribers a subscription notifies, for the same reason
/// [`validate_monitor_arn_list`] is taken as a list.
pub fn validate_subscribers(subscribers: &[Subscriber]) -> Result<(), String> {
    if subscribers.is_empty() {
        return Err("Subscribers must name at least one subscriber to notify".to_string());
    }
    Ok(())
}

/// Check the threshold half of a subscription: exactly one of the two members, and a
/// `ThresholdExpression` that is one the member documents.
///
/// Taken as two values rather than as a whole subscription so `UpdateAnomalySubscription`,
/// which carries the same pair as top-level members, applies the same rule.
///
/// The sentence a request carrying both is refused with is Cost Explorer's own, in the
/// terms the service states the rule in — the two member names and the condition on them —
/// rather than a paraphrase of `$Threshold`'s "You can specify either Threshold or
/// ThresholdExpression, but not both".
pub fn validate_threshold(
    threshold: Option<f64>,
    threshold_expression: Option<&Expression>,
) -> Result<(), String> {
    match (threshold, threshold_expression) {
        (None, None) => Err(
            "One of Threshold or ThresholdExpression is required for an anomaly subscription"
                .to_string(),
        ),
        (Some(_), Some(_)) => {
            Err("Threshold and ThresholdExpression cannot both be non-null".to_string())
        }
        // The scalar's own range (0 or more) is modelled, so a value reaching here
        // satisfies it and there is nothing left to check.
        (Some(_), None) => Ok(()),
        (None, Some(expression)) => validate_threshold_expression(expression, 0),
    }
}

/// Check one level of a `ThresholdExpression`, then its `And`/`Or` branches.
///
/// The member admits exactly two forms: a `Dimensions` leaf selecting one of the threshold
/// dimensions, or an `And`/`Or` list of such expressions ("The supported nested expression
/// types are AND and OR"). Every other `Expression` selector — `Not`, `Tags`,
/// `CostCategories`, `ProductAttributes` — selects costs rather than an anomaly's impact,
/// so none of them is a threshold.
fn validate_threshold_expression(expression: &Expression, depth: usize) -> Result<(), String> {
    if depth > MAX_THRESHOLD_EXPRESSION_DEPTH {
        return Err(format!(
            "ThresholdExpression nests more than {MAX_THRESHOLD_EXPRESSION_DEPTH} levels of \
             And or Or"
        ));
    }
    for (member, present) in [
        ("Not", expression.not.is_some()),
        ("Tags", expression.tags.is_some()),
        ("CostCategories", expression.cost_categories.is_some()),
        ("ProductAttributes", expression.product_attributes.is_some()),
    ] {
        if present {
            return Err(format!(
                "ThresholdExpression does not support {member}; it supports Dimensions on \
                 {}, nested under And or Or",
                THRESHOLD_DIMENSIONS.join(" or ")
            ));
        }
    }

    let branches = match (expression.and.as_deref(), expression.or.as_deref()) {
        (Some(_), Some(_)) => {
            return Err(
                "ThresholdExpression must nest its thresholds under either And or Or, not both"
                    .to_string(),
            )
        }
        (Some(branches), None) | (None, Some(branches)) => Some(branches),
        (None, None) => None,
    };

    match (branches, expression.dimensions.as_ref()) {
        (Some(_), Some(_)) => Err(
            "ThresholdExpression must either select a threshold dimension or nest threshold \
             expressions under And or Or, not both"
                .to_string(),
        ),
        (Some(branches), None) => {
            if branches.is_empty() {
                return Err(
                    "ThresholdExpression must nest at least one threshold expression under \
                     And or Or"
                        .to_string(),
                );
            }
            for branch in branches {
                validate_threshold_expression(branch, depth + 1)?;
            }
            Ok(())
        }
        (None, Some(dimensions)) => validate_threshold_dimensions(dimensions),
        (None, None) => Err(format!(
            "ThresholdExpression must select Dimensions on {}",
            THRESHOLD_DIMENSIONS.join(" or ")
        )),
    }
}

/// Check a `Dimensions` leaf of a `ThresholdExpression`: the dimension it selects, the
/// match option it compares with, and the numbers it compares against.
fn validate_threshold_dimensions(dimensions: &DimensionValues) -> Result<(), String> {
    match dimensions.key.as_deref() {
        Some(key) if THRESHOLD_DIMENSIONS.contains(&key) => {}
        other => {
            return Err(format!(
                "ThresholdExpression Dimensions Key must be {}, got {}",
                THRESHOLD_DIMENSIONS.join(" or "),
                other.unwrap_or("none")
            ))
        }
    }

    let options = dimensions.match_options.as_deref().unwrap_or_default();
    // Exactly the one required option: a threshold is compared one way, and the default
    // match options (`EQUALS`, `CASE_SENSITIVE`) are not comparisons of an impact against
    // a bound. The slice pattern is what keeps the read total.
    if !matches!(options, [only] if only == THRESHOLD_MATCH_OPTION) {
        return Err(format!(
            "ThresholdExpression Dimensions MatchOptions must be [{THRESHOLD_MATCH_OPTION}]"
        ));
    }

    let values = dimensions.values.as_deref().unwrap_or_default();
    if values.is_empty() {
        return Err("ThresholdExpression Dimensions Values must carry a threshold".to_string());
    }
    for value in values {
        let admissible = value
            .parse::<f64>()
            .ok()
            .filter(|number| number.is_finite() && (0.0..=THRESHOLD_VALUE_MAX).contains(number))
            .is_some();
        if !admissible {
            return Err(format!(
                "ThresholdExpression Dimensions Values must be numbers between 0 and \
                 {THRESHOLD_VALUE_MAX:.0} in string format, got {value:?}"
            ));
        }
    }
    Ok(())
}

/// Render a threshold as the string form a `ThresholdExpression` value takes.
///
/// `Values` is a list of strings while `Threshold` is a double, so the shorthand expansion
/// has to render one as the other: the double's own decimal form, which keeps a fractional
/// part even when the threshold is a whole number of dollars, since what is rendered is a
/// double and not an integer.
fn render_threshold(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.1}")
    } else {
        // The shortest decimal that reads back as the same number, which is what the
        // standard formatter produces for a value that already has a fractional part.
        format!("{value}")
    }
}

/// The `ThresholdExpression` the deprecated scalar `Threshold` is shorthand for.
///
/// From `AnomalySubscription$Threshold`: "Continued use of Threshold will be treated as
/// shorthand syntax for a ThresholdExpression." The expansion is the absolute-threshold
/// form `ThresholdExpression` documents — the absolute-impact dimension, compared with the
/// one required match option, against the value the caller named.
pub fn threshold_expression_for(threshold: f64) -> Expression {
    Expression {
        dimensions: Some(DimensionValues {
            key: Some(THRESHOLD_ABSOLUTE_DIMENSION.to_string()),
            values: Some(vec![render_threshold(threshold)]),
            match_options: Some(vec![THRESHOLD_MATCH_OPTION.to_string()]),
        }),
        ..Expression::default()
    }
}

/// The dollar value the deprecated scalar `Threshold` reports for a threshold expression,
/// when that expression names one.
///
/// `AnomalySubscription$Threshold` is "the dollar value that triggers a notification if the
/// threshold is exceeded", and `$ThresholdExpression` documents
/// `ANOMALY_TOTAL_IMPACT_ABSOLUTE` as "corresponding to an anomaly's TotalImpact" — that
/// impact in dollars. The deprecated member is therefore a READING of the one threshold a
/// subscription holds rather than a second value a write stores: whichever member the
/// caller wrote, a read reports the dollar threshold the stored expression names, and
/// reports none for an expression that names only a percentage.
///
/// `And`/`Or` branches are searched in order, since a nested expression's dollar threshold
/// is the absolute one nested inside it. The depth is bounded by
/// [`MAX_THRESHOLD_EXPRESSION_DEPTH`], the same bound admission applies, so no stored
/// expression is walked without limit.
pub fn absolute_threshold_of(expression: &Expression) -> Option<f64> {
    absolute_threshold_at(expression, 0)
}

fn absolute_threshold_at(expression: &Expression, depth: usize) -> Option<f64> {
    if depth > MAX_THRESHOLD_EXPRESSION_DEPTH {
        return None;
    }
    if let Some(dimensions) = expression.dimensions.as_ref() {
        if dimensions.key.as_deref() == Some(THRESHOLD_ABSOLUTE_DIMENSION) {
            return dimensions
                .values
                .as_deref()
                .unwrap_or_default()
                .iter()
                .find_map(|value| value.parse::<f64>().ok().filter(|number| number.is_finite()));
        }
    }
    expression
        .and
        .as_deref()
        .into_iter()
        .chain(expression.or.as_deref())
        .flatten()
        .find_map(|branch| absolute_threshold_at(branch, depth + 1))
}

/// The subscribers a write stores: the caller's channel and address, with the `Status` the
/// service owns ([`SUBSCRIBER_STATUS_CONFIRMED`]) rather than whatever the caller sent.
pub fn admitted_subscribers(subscribers: Vec<Subscriber>) -> Vec<Subscriber> {
    subscribers
        .into_iter()
        .map(|subscriber| Subscriber {
            address: subscriber.address,
            r#type: subscriber.r#type,
            status: Some(SUBSCRIBER_STATUS_CONFIRMED.to_string()),
        })
        .collect()
}

/// The fixed half of the sentence Cost Explorer refuses an unresolvable subscription
/// reference with. The only variable part is the subscription id, which
/// [`unknown_subscription`] appends.
///
/// The same sentence form the service's unknown-MONITOR refusal carries ("No monitor
/// present with <member>: <id>"), over the resource this one could not resolve: one
/// service, one way of saying a reference resolves to nothing.
const NO_SUBSCRIPTION_PRESENT: &str = "No subscription present with";

/// The member word an unknown-subscription refusal names the id under.
///
/// Every operation that resolves a `SubscriptionArn` (`UpdateAnomalySubscription`,
/// `DeleteAnomalySubscription`) has the subscription as the resource the call was MADE
/// AGAINST, which is the same word [`crate::state::MONITOR_ID_AS_RESOURCE`] carries for a
/// monitor in that position.
pub const SUBSCRIPTION_ID_AS_RESOURCE: &str = "resourceId";

/// The refusal every operation that resolves a subscription ARN against
/// [`anomaly_subscriptions`](crate::state::ComAmazonawsCostexplorerAwsinsightsindexserviceState::anomaly_subscriptions)
/// answers with when the account holds no subscription under it.
///
/// One helper, so no two operations can drift apart on the wording: the modelled
/// `UnknownSubscriptionException` carrying the service's fixed sentence over the bare
/// subscription id the request named, never the whole ARN.
pub fn unknown_subscription(subscription_arn: &str) -> UnknownSubscriptionException {
    UnknownSubscriptionException {
        message: Some(format!(
            "{NO_SUBSCRIPTION_PRESENT} {SUBSCRIPTION_ID_AS_RESOURCE}: {}",
            crate::state::resource_id(subscription_arn)
        )),
    }
}

/// A stored cost anomaly subscription: everything `GetAnomalySubscriptions` reports, plus
/// the resource tags the tagging surface reports for it.
#[derive(Debug, Clone)]
pub struct AnomalySubscriptionRecord {
    /// The generated ARN this subscription is registered under.
    pub arn: String,
    /// The account the subscription belongs to.
    pub account_id: String,
    /// The monitors whose anomalies the subscription alerts on.
    pub monitor_arn_list: Vec<String>,
    /// Who is notified, and over which channel.
    pub subscribers: Vec<Subscriber>,
    /// The threshold the subscription alerts above — the caller's expression, or the one
    /// [`threshold_expression_for`] expands their scalar into.
    ///
    /// The ONE place a subscription's threshold is held: the deprecated scalar is read off
    /// this expression by [`absolute_threshold_of`] rather than stored beside it, so the
    /// two members a read reports can never disagree.
    pub threshold_expression: Option<Expression>,
    /// `AnomalySubscriptionFrequency` as the caller sent it.
    pub frequency: String,
    /// The caller-chosen subscription name.
    pub subscription_name: String,
    /// Resource tags supplied at create time, kept here so the tagging operations report
    /// the tags the subscription was created with.
    pub resource_tags: Vec<ResourceTag>,
}

impl AnomalySubscriptionRecord {
    /// The `AnomalySubscription` structure the read operations answer with.
    pub fn to_anomaly_subscription(&self) -> AnomalySubscription {
        AnomalySubscription {
            subscription_arn: Some(self.arn.clone()),
            account_id: Some(self.account_id.clone()),
            monitor_arn_list: self.monitor_arn_list.clone(),
            subscribers: self.subscribers.clone(),
            threshold: self
                .threshold_expression
                .as_ref()
                .and_then(absolute_threshold_of),
            frequency: self.frequency.clone(),
            subscription_name: self.subscription_name.clone(),
            threshold_expression: self.threshold_expression.clone(),
        }
    }

    /// Whether this subscription alerts on the monitor `monitor_arn` names.
    pub fn monitors(&self, monitor_arn: &str) -> bool {
        self.monitor_arn_list
            .iter()
            .any(|arn| arn == monitor_arn)
    }
}
