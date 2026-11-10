// SPDX-License-Identifier: CC0-1.0

//! The per-type provisioning arms of Amazon Braket's own control plane: the spending limit
//! an `AWS::Braket::SpendingLimit` declares. One file of [`super`]'s per-type arms,
//! dispatched to by its `Provisioner`.
//!
//! A spending limit is a cost control ABOUT a quantum device rather than a named resource of
//! its own: `CreateSpendingLimit` takes the device's ARN, the amount and the period, takes no
//! identifier at all, and answers with the `spendingLimitArn` Amazon Braket minted — the
//! value this type's `Ref` returns and the handle `UpdateSpendingLimit` and
//! `DeleteSpendingLimit` each address one by. So the ARN that create answered is the
//! resource's physical id, replacing the stand-in the naming rules computed.
//!
//! The instant the limit was created is that service's clock rather than anything a template
//! states, and the create does not report it: Amazon Braket publishes no read of ONE spending
//! limit, so it is read out of the account's own listing — `SearchSpendingLimits` narrowed to
//! the ARN the create minted — which is also where the drift read below gets the limit's
//! configuration from.

use super::*;

/// Amazon Braket's signing name, from its model (`sigv4Name: braket`), which speaks
/// `restJson1` — so its calls ride on [`call_rest_json_named`], each on the URI its own
/// operation's `@http` binding declares, naming the operation so a refusal is reported the
/// way that service's own SDK raises it.
pub(super) const BRAKET_SIGNING_NAME: &str = "braket";

/// The collection `CreateSpendingLimit` is bound to (`POST /spending-limit`), and the
/// collection `SearchSpendingLimits` reads (`POST /spending-limits`). One limit is addressed
/// under the first by the ARN that service minted
/// (`PATCH /spending-limit/{spendingLimitArn}/update`,
/// `DELETE /spending-limit/{spendingLimitArn}/delete`).
const SPENDING_LIMIT_URI: &str = "/spending-limit";
const SPENDING_LIMITS_SEARCH_URI: &str = "/spending-limits";

/// The operations this module calls, as Amazon Braket's own model names them.
const CREATE_SPENDING_LIMIT: &str = "CreateSpendingLimit";
const UPDATE_SPENDING_LIMIT: &str = "UpdateSpendingLimit";
const DELETE_SPENDING_LIMIT: &str = "DeleteSpendingLimit";
const SEARCH_SPENDING_LIMITS: &str = "SearchSpendingLimits";

/// The error code Amazon Braket answers for a spending limit it does not hold
/// (`ResourceNotFoundException`, the modelled 404 of its update and delete).
const BRAKET_RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The members `CreateSpendingLimit` takes this type's two scalar properties as: every
/// property of the type is the member of the same name with a lower-cased initial.
const SPENDING_LIMIT_MEMBERS: &[(&str, &str)] = &[
    ("DeviceArn", "deviceArn"),
    ("SpendingLimit", "spendingLimit"),
];

/// The property the window the limit is active over is declared in, and the member both the
/// create and the update take it as. Its own two members (`StartAt`, `EndAt`) are the same
/// names with lower-cased initials, which is what [`camel_cased`] does to them.
const TIME_PERIOD_PROPERTY: &str = "TimePeriod";
const TIME_PERIOD_MEMBER: &str = "timePeriod";

/// The member the amount is sent as, which is the one member the update shares with the
/// create beside the period.
const SPENDING_LIMIT_MEMBER: &str = "spendingLimit";

/// The `@idempotencyToken` both the create and the update require, and how many characters
/// the token this provisioner sends carries.
///
/// The create's is derived from the resource's own computed physical id, so the same
/// declaration of the same stack asks for the same spending limit rather than a second one;
/// the update's is derived from the resource AND the change ([`change_token`]), so one limit's
/// successive changes are successive tokens. DERIVED rather than sent as that id: the member
/// is a `String64` (`@length 1..64`) while a computed id is an ARN, which is longer than
/// that.
const CLIENT_TOKEN_MEMBER: &str = "clientToken";
const CLIENT_TOKEN_CHARS: usize = 32;

/// The member the created limit's ARN is answered in, which is also the summary member a
/// listed limit carries it in and the filter name a search narrows on it by.
const SPENDING_LIMIT_ARN_MEMBER: &str = "spendingLimitArn";

/// The members of a `SearchSpendingLimits` filter, and the one operator
/// `SearchSpendingLimitsFilterOperator` models.
const FILTER_NAME_MEMBER: &str = "name";
const FILTER_VALUES_MEMBER: &str = "values";
const FILTER_OPERATOR_MEMBER: &str = "operator";
const FILTER_OPERATOR_EQUAL: &str = "EQUAL";

/// The member a search answers the matching limits in, and the summary member this module
/// reads the instant off.
const SPENDING_LIMITS_MEMBER: &str = "spendingLimits";
const CREATED_AT_MEMBER: &str = "createdAt";

/// The attributes a spending limit publishes: the ARN Amazon Braket minted and the instant it
/// recorded for the limit, both read off that service's own report rather than recomputed
/// here.
const SPENDING_LIMIT_ARN_ATTRIBUTE: &str = "SpendingLimitArn";
const CREATED_AT_ATTRIBUTE: &str = "CreatedAt";

/// Create the spending limit an `AWS::Braket::SpendingLimit` declares, so a client that read
/// the ARN out of the stack's outputs finds the limit in Amazon Braket's own
/// `SearchSpendingLimits` and the device really is capped at the amount the template asked
/// for.
///
/// The declared properties ARE the create's members once their initials are lower-cased, and
/// the declared tags travel beside the ones CloudFormation gives the resource ([`tags_of`])
/// as the `TagsMap` that call's own `tags` member takes.
///
/// The identity is Amazon Braket's: the ARN it minted becomes this resource's physical id —
/// the primary identifier of the type, and the handle every spending-limit API takes — and it
/// is reported back as the `SpendingLimitArn` attribute beside the `CreatedAt` that service
/// recorded.
pub(super) async fn create_braket_spending_limit(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let properties = resource
        .properties
        .as_object()
        .cloned()
        .unwrap_or_else(Map::new);
    let mut request = Map::new();
    request.insert(
        CLIENT_TOKEN_MEMBER.to_string(),
        Value::String(client_token(resource)),
    );
    copy_members(&properties, SPENDING_LIMIT_MEMBERS, &mut request);
    if let Some(period) = properties.get(TIME_PERIOD_PROPERTY) {
        request.insert(TIME_PERIOD_MEMBER.to_string(), time_period_member(period)?);
    }
    let tags = tags_as_map(&tags_of(&resource.properties, propagated));
    if !tags.is_empty() {
        request.insert("tags".to_string(), Value::Object(tags));
    }
    let answered = call_rest_json_named(
        ctx,
        BRAKET_SIGNING_NAME,
        region,
        CREATE_SPENDING_LIMIT,
        "POST",
        SPENDING_LIMIT_URI,
        Some(&Value::Object(request)),
    )
    .await?;
    let arn = reported_text(&answered, SPENDING_LIMIT_ARN_MEMBER).unwrap_or_default();
    if arn.is_empty() {
        // There would be no handle a client could call Amazon Braket with, and nothing for
        // the stack to publish, so this is a failed create rather than a created resource.
        return Err(Refusal {
            code: "InternalFailure".to_string(),
            message: "the Amazon Braket control plane answered a created spending limit \
                      without an ARN"
                .to_string(),
        });
    }
    // The instant the limit carries is that service's: read off its own report of the limit
    // the create just minted, never from this service's clock.
    let reported = reported_spending_limit(ctx, region, &arn).await?;
    let mut attributes = reported_attributes_of(reported.as_ref());
    attributes.insert(SPENDING_LIMIT_ARN_ATTRIBUTE.to_string(), arn.clone());
    Ok(Created {
        physical_resource_id: Some(arn),
        attributes,
    })
}

/// Apply a changed declaration to the spending limit the stack created, through
/// `UpdateSpendingLimit` — the one call that changes a limit that exists.
///
/// That call takes the limit's AMOUNT and its PERIOD and nothing else: the device a limit
/// caps is not a member of it at all, which is why a declaration naming another device
/// replaces the resource ([`template::replacement_properties`]) instead of reaching here.
///
/// Its `@idempotencyToken` is derived from the CHANGE, not from the resource alone
/// ([`change_token`]): a limit takes one change after another over its life, and a token
/// repeated across two of them would let the second be read as a replay of the first and
/// leave the caller's change unapplied.
pub(super) async fn update_braket_spending_limit(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Created, Refusal> {
    let mut request = Map::new();
    if let Some(amount) = declared.get("SpendingLimit") {
        request.insert(SPENDING_LIMIT_MEMBER.to_string(), amount.clone());
    }
    if let Some(period) = declared.get(TIME_PERIOD_PROPERTY) {
        request.insert(TIME_PERIOD_MEMBER.to_string(), time_period_member(period)?);
    }
    request.insert(
        CLIENT_TOKEN_MEMBER.to_string(),
        Value::String(change_token(resource, &Value::Object(request.clone()))),
    );
    call_rest_json_named(
        ctx,
        BRAKET_SIGNING_NAME,
        region,
        UPDATE_SPENDING_LIMIT,
        "PATCH",
        &spending_limit_uri(&resource.physical_resource_id, "update"),
        Some(&Value::Object(request)),
    )
    .await?;
    // The update answers no document, so what the resource now reports is read off that
    // service's own report of the limit. A limit that report does not carry leaves the
    // resource with the ARN it already publishes rather than with no identity at all.
    let reported = reported_spending_limit(ctx, region, &resource.physical_resource_id).await?;
    let mut attributes = reported_attributes_of(reported.as_ref());
    attributes
        .entry(SPENDING_LIMIT_ARN_ATTRIBUTE.to_string())
        .or_insert_with(|| resource.physical_resource_id.clone());
    Ok(Created::attributes(attributes))
}

/// Delete the spending limit a stack created, by the ARN it was created under — this
/// resource's physical id. One the service no longer holds (`ResourceNotFoundException`,
/// judged on the CODE and never on a message that carries the request's own values) is
/// already in the state the delete asks for.
pub(super) async fn delete_braket_spending_limit(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    match call_rest_json_named(
        ctx,
        BRAKET_SIGNING_NAME,
        region,
        DELETE_SPENDING_LIMIT,
        "DELETE",
        &spending_limit_uri(physical_resource_id, "delete"),
        None,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == BRAKET_RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The spending limit's configuration as Amazon Braket reports it, for drift detection: the
/// summary its own listing carries for the ARN that is this resource's physical id. A limit
/// it no longer lists was deleted, which is what `None` reports.
///
/// The members it reports under the names this type declares are the device the limit caps
/// and the amount it caps it at; a property the summary does not report is not evidence of
/// anything ([`super::drift`]).
pub(super) async fn describe_braket_spending_limit(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    Ok(reported_spending_limit(ctx, region, physical_resource_id)
        .await?
        .map(|summary| {
            reported_as_properties(
                &summary,
                &[
                    ("DeviceArn", "deviceArn"),
                    ("SpendingLimit", SPENDING_LIMIT_MEMBER),
                ],
            )
        }))
}

/// The summary Amazon Braket's own listing carries for ONE spending limit, by the ARN it
/// minted, or `None` where it lists none.
///
/// Amazon Braket publishes no read of a single spending limit: `SearchSpendingLimits` is the
/// one operation that reports one, narrowed here by an `EQUAL` filter on the limit's own ARN
/// — the identifier member `SpendingLimitSummary` carries — so exactly the limit asked about
/// is read rather than a page of the account's.
async fn reported_spending_limit(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    spending_limit_arn: &str,
) -> Result<Option<Value>, Refusal> {
    let mut filter = Map::new();
    filter.insert(
        FILTER_NAME_MEMBER.to_string(),
        Value::String(SPENDING_LIMIT_ARN_MEMBER.to_string()),
    );
    filter.insert(
        FILTER_VALUES_MEMBER.to_string(),
        Value::Array(vec![Value::String(spending_limit_arn.to_string())]),
    );
    filter.insert(
        FILTER_OPERATOR_MEMBER.to_string(),
        Value::String(FILTER_OPERATOR_EQUAL.to_string()),
    );
    let mut request = Map::new();
    request.insert(
        "filters".to_string(),
        Value::Array(vec![Value::Object(filter)]),
    );
    let answered = call_rest_json_named(
        ctx,
        BRAKET_SIGNING_NAME,
        region,
        SEARCH_SPENDING_LIMITS,
        "POST",
        SPENDING_LIMITS_SEARCH_URI,
        Some(&Value::Object(request)),
    )
    .await?;
    let Some(Value::Array(listed)) = answered.get(SPENDING_LIMITS_MEMBER) else {
        return Ok(None);
    };
    Ok(listed
        .iter()
        .find(|summary| {
            reported_text(summary, SPENDING_LIMIT_ARN_MEMBER).as_deref()
                == Some(spending_limit_arn)
        })
        .cloned())
}

/// The URI one spending limit's `action` is addressed at: the collection, the
/// `spendingLimitArn` `@httpLabel` escaped as a single path segment ([`percent_encoded`]) so
/// an ARN's own `:` and `/` address one limit rather than another path, and the trailing
/// segment that binding ends with.
fn spending_limit_uri(spending_limit_arn: &str, action: &str) -> String {
    format!(
        "{SPENDING_LIMIT_URI}/{}/{action}",
        percent_encoded(spending_limit_arn)
    )
}

/// The `@idempotencyToken` the CREATE of this resource carries: [`CLIENT_TOKEN_CHARS`]
/// characters derived from the resource's own computed physical id, which is a function of
/// the stack and the resource's logical id — so the same declaration of the same stack asks
/// for the same spending limit rather than a second one.
fn client_token(resource: &StoredResource) -> String {
    crate::state::stable_identifier_digits(&resource.physical_resource_id, CLIENT_TOKEN_CHARS)
}

/// The `@idempotencyToken` one CHANGE to this resource carries: the same derivation over the
/// resource AND the members the change asks for, so two changes to one limit are two tokens
/// while a repeat of the same change is one.
fn change_token(resource: &StoredResource, change: &Value) -> String {
    crate::state::stable_identifier_digits(
        &format!("{}/{change}", resource.physical_resource_id),
        CLIENT_TOKEN_CHARS,
    )
}

/// One declared `TimePeriod` as the member both the create and the update take it as: its two
/// members re-cased ([`camel_cased`] lower-cases each initial, so `StartAt` is `startAt`),
/// with each INSTANT put onto the wire in the form that member is carried in.
///
/// A template states an instant as ISO-8601 text, while `TimePeriod`'s members are the
/// seconds since the epoch their own `@timestampFormat` declares — so a declared instant is
/// converted ([`epoch_seconds`], the one reading of that direction, whose inverse is what
/// reports one back). A value that is not such text is passed through as it came, so a
/// template that already states the seconds reaches that member unchanged and anything else
/// is refused by that operation's own validation rather than silently dropped here.
fn time_period_member(declared: &Value) -> Result<Value, Refusal> {
    match camel_cased(declared, 0)? {
        Value::Object(members) => Ok(Value::Object(
            members
                .into_iter()
                .map(|(member, value)| {
                    let converted = value.as_str().and_then(epoch_seconds).unwrap_or(value);
                    (member, converted)
                })
                .collect(),
        )),
        // A declaration that is not an object states no member of the period; the owning
        // operation's own validation is what refuses it.
        other => Ok(other),
    }
}

/// The attributes one reported spending limit gives the resource: the ARN it carries and the
/// instant Amazon Braket recorded for it. A limit that service reports nothing about gives
/// none, so the resource keeps what the naming rules computed rather than taking an empty
/// value.
fn reported_attributes_of(summary: Option<&Value>) -> BTreeMap<String, String> {
    let mut attributes = BTreeMap::new();
    let Some(summary) = summary else {
        return attributes;
    };
    if let Some(arn) = reported_text(summary, SPENDING_LIMIT_ARN_MEMBER) {
        attributes.insert(SPENDING_LIMIT_ARN_ATTRIBUTE.to_string(), arn);
    }
    if let Some(created_at) = created_at_text(summary) {
        attributes.insert(CREATED_AT_ATTRIBUTE.to_string(), created_at);
    }
    attributes
}

/// The instant Amazon Braket recorded for the limit, as the text this type's `CreatedAt`
/// return value carries: the ONE reading of a `restJson1` timestamp this provisioner has
/// ([`instant_text`]), since a CloudFormation attribute is text and a timestamp is a value on
/// the wire. A limit whose report carries no instant at all contributes no attribute.
fn created_at_text(summary: &Value) -> Option<String> {
    instant_text(summary.get(CREATED_AT_MEMBER)?)
}
