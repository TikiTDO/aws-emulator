// SPDX-License-Identifier: CC0-1.0

//! How this service STATES an enumerated type's admissible set when it refuses a value
//! outside it, over the wire, driven exactly as an SDK drives it (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! ONE rule, about no operation and no member: the `Member must satisfy enum value set:
//! [<values>]` clause lists the admissible values in the order the set's own hash table is
//! walked — ascending bucket of each value's string hash, and where two values share a
//! bucket the later-declared one ahead of the earlier — rather than in the order the model
//! declares them or a sort of them. The order belongs to the enumerated TYPE, so one type
//! reads the same way under every member, every operation and every depth that carries it.
//!
//! The rows differ along what the rule generalises over: which enumerated type failed (two
//! to ten values; types whose stated order is the declared one and types whose is not),
//! how its values are spelled (SCREAMING_SNAKE, mixed case and lowercase), which operation
//! carried it (six), and where in the request the member sits (top level, a list element at
//! its 1-based index, a structure inside a list, and a structure inside a list at a later
//! index).
//!
//! Each row's admissible set is the model's own: it is checked against the model-derived
//! value list in `ops::types` — generated from
//! `smithy_query(query="shape", shape="<the type>")` — so a listing here cannot drift from
//! the type the model declares, and the ORDER is recomputed with the shared derivation
//! rather than transcribed. One row is also pinned as a literal, so a regression in that
//! derivation is caught here and not only by a round-trip through it.
//!
//! The harness (the engine, the router, the awsJson1_1 call) is the one
//! `log_delivery_configuration.rs` builds for this service, reached through the test crate
//! root rather than copied.

use serde_json::{json, Value};
use service_common::validation_message::{enum_value_bucket, hashed_enum_order};
use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::ops::types::{
    ATTRIBUTE_DATA_TYPE_VALUES, DELETION_PROTECTION_TYPE_VALUES, DELIVERY_MEDIUM_TYPE_VALUES,
    EXPLICIT_AUTH_FLOWS_TYPE_VALUES, FEEDBACK_VALUE_TYPE_VALUES, LOG_LEVEL_VALUES,
    MESSAGE_ACTION_TYPE_VALUES, PREVENT_USER_EXISTENCE_ERROR_TYPES_VALUES,
    USER_POOL_MFA_TYPE_VALUES, VERIFIED_ATTRIBUTE_TYPE_VALUES,
};

use super::log_delivery_configuration::{call, error_code, error_message, router};

/// The error code every constraint refusal of this service carries.
const INVALID_PARAMETER_EXCEPTION: &str = "InvalidParameterException";

/// The value every row submits: one no enumerated type of this model admits, so the only
/// thing the clause can be about is the set the member belongs to.
const OUTSIDE_EVERY_SET: &str = "NOT_A_MODELLED_VALUE";

/// A `UserPoolIdType`-shaped pool id (`@pattern ^[\w-]+_[0-9a-zA-Z]+$`), so no row is
/// refused for the SHAPE of the pool it names: no row's request reaches a handler, so no
/// pool has to exist.
const POOL_ID: &str = "us-east-1_enumvalueset";

/// One row of the table: the operation, the request, the member the clause names and the
/// enumerated type the member targets.
struct Row {
    case: &'static str,
    operation: &'static str,
    body: Value,
    /// The member path the clause states, in the spelling the framework's walk builds
    /// (lowerCamel members, `<1-based index>.member` for a list step).
    member: &'static str,
    /// The admissible values the model declares for the member's type, from
    /// `ops::types`.
    declared: &'static [&'static str],
}

/// The rows, built here because several carry an owned JSON body.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a feedback value, on the administrative API",
            operation: "AdminUpdateAuthEventFeedback",
            body: json!({
                "UserPoolId": POOL_ID,
                "Username": "alice",
                "EventId": "event-1",
                "FeedbackValue": OUTSIDE_EVERY_SET,
            }),
            member: "feedbackValue",
            declared: FEEDBACK_VALUE_TYPE_VALUES,
        },
        Row {
            case: "the same type under the self-service API",
            operation: "UpdateAuthEventFeedback",
            body: json!({
                "UserPoolId": POOL_ID,
                "Username": "alice",
                "EventId": "event-1",
                "FeedbackToken": "feedback.token",
                "FeedbackValue": OUTSIDE_EVERY_SET,
            }),
            member: "feedbackValue",
            declared: FEEDBACK_VALUE_TYPE_VALUES,
        },
        Row {
            case: "a three-value type at the top level",
            operation: "CreateUserPool",
            body: json!({
                "PoolName": "enum-value-set-pool",
                "MfaConfiguration": OUTSIDE_EVERY_SET,
            }),
            member: "mfaConfiguration",
            declared: USER_POOL_MFA_TYPE_VALUES,
        },
        Row {
            case: "a two-value type at the top level",
            operation: "CreateUserPool",
            body: json!({
                "PoolName": "enum-value-set-pool",
                "DeletionProtection": OUTSIDE_EVERY_SET,
            }),
            member: "deletionProtection",
            declared: DELETION_PROTECTION_TYPE_VALUES,
        },
        Row {
            case: "a lowercase-valued type as a list element",
            operation: "CreateUserPool",
            body: json!({
                "PoolName": "enum-value-set-pool",
                "AutoVerifiedAttributes": [OUTSIDE_EVERY_SET],
            }),
            member: "autoVerifiedAttributes.1.member",
            declared: VERIFIED_ATTRIBUTE_TYPE_VALUES,
        },
        Row {
            case: "a four-value type inside a structure inside a list",
            operation: "CreateUserPool",
            body: json!({
                "PoolName": "enum-value-set-pool",
                "Schema": [{ "Name": "nickname", "AttributeDataType": OUTSIDE_EVERY_SET }],
            }),
            member: "schema.1.member.attributeDataType",
            declared: ATTRIBUTE_DATA_TYPE_VALUES,
        },
        Row {
            case: "a message action at the top level",
            operation: "AdminCreateUser",
            body: json!({
                "UserPoolId": POOL_ID,
                "Username": "alice",
                "MessageAction": OUTSIDE_EVERY_SET,
            }),
            member: "messageAction",
            declared: MESSAGE_ACTION_TYPE_VALUES,
        },
        Row {
            case: "a delivery medium at the SECOND position of a list",
            operation: "AdminCreateUser",
            body: json!({
                "UserPoolId": POOL_ID,
                "Username": "alice",
                "DesiredDeliveryMediums": ["EMAIL", OUTSIDE_EVERY_SET],
            }),
            member: "desiredDeliveryMediums.2.member",
            declared: DELIVERY_MEDIUM_TYPE_VALUES,
        },
        Row {
            case: "a ten-value type as a list element",
            operation: "CreateUserPoolClient",
            body: json!({
                "UserPoolId": POOL_ID,
                "ClientName": "enum-value-set-client",
                "ExplicitAuthFlows": [OUTSIDE_EVERY_SET],
            }),
            member: "explicitAuthFlows.1.member",
            declared: EXPLICIT_AUTH_FLOWS_TYPE_VALUES,
        },
        Row {
            case: "an existence-error preference at the top level",
            operation: "CreateUserPoolClient",
            body: json!({
                "UserPoolId": POOL_ID,
                "ClientName": "enum-value-set-client",
                "PreventUserExistenceErrors": OUTSIDE_EVERY_SET,
            }),
            member: "preventUserExistenceErrors",
            declared: PREVENT_USER_EXISTENCE_ERROR_TYPES_VALUES,
        },
        Row {
            case: "a log level inside a structure inside a list",
            operation: "SetLogDeliveryConfiguration",
            body: json!({
                "UserPoolId": POOL_ID,
                "LogConfigurations": [{
                    "EventSource": "userNotification",
                    "LogLevel": OUTSIDE_EVERY_SET,
                }],
            }),
            member: "logConfigurations.1.member.logLevel",
            declared: LOG_LEVEL_VALUES,
        },
    ]
}

/// The values a clause listed, taken out of its `enum value set: [...]` half, or `None`
/// when the message states no such clause.
fn listed_value_set(message: &str) -> Option<Vec<&str>> {
    let (_, tail) = message.split_once("enum value set: [")?;
    Some(tail.strip_suffix(']')?.split(", ").collect())
}

/// Whether two of `declared` fall in one bucket of the table the stated order walks.
///
/// A type without a collision reads the same whatever order its values reach the clause
/// in, so its whole listing can be asserted; a type WITH one depends on the order the
/// model declares its values in, which this crate's value lists do not carry, so only the
/// membership and the bucket order are asserted for it.
fn shares_a_bucket(declared: &[&str]) -> bool {
    let mut buckets: Vec<u32> = declared.iter().map(|value| enum_value_bucket(value)).collect();
    buckets.sort_unstable();
    buckets.windows(2).any(|pair| pair[0] == pair[1])
}

/// The envelope and clause a refusal of ONE broken constraint states, over the set it
/// listed.
fn one_enum_value_error(member: &str, listed: &str) -> String {
    format!(
        "1 validation error detected: Value '{OUTSIDE_EVERY_SET}' at '{member}' failed to \
         satisfy constraint: Member must satisfy enum value set: [{listed}]"
    )
}

#[tokio::test]
async fn an_enum_value_set_is_listed_in_the_order_its_table_is_walked() {
    let router = router();

    for row in rows() {
        let case = row.case;
        let (status, payload) = call(&router, row.operation, &row.body).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, INVALID_PARAMETER_EXCEPTION),
            "case {case:?}: a value outside an enumerated type is refused, got {status} \
             with {payload}"
        );
        let message = error_message(&payload);
        let listed = listed_value_set(message).unwrap_or_else(|| {
            panic!("case {case:?}: the refusal must state an enum value set, got {message:?}")
        });

        // The whole sentence is the framework's, over the listing the clause carried: the
        // count, the quoted value, the member path and the requirement's wording.
        assert_eq!(
            message,
            one_enum_value_error(row.member, &listed.join(", ")),
            "case {case:?}: expected the one-error envelope for {:?}, got {message:?}",
            row.member
        );

        // The listing holds exactly the values the model declares — no value dropped, none
        // invented and none repeated.
        let mut sorted_listing = listed.clone();
        sorted_listing.sort_unstable();
        let mut sorted_declared: Vec<&str> = row.declared.to_vec();
        sorted_declared.sort_unstable();
        assert_eq!(
            sorted_listing, sorted_declared,
            "case {case:?}: the listing must hold the model's values, got {listed:?}"
        );

        // The buckets rise across the listing, which is the order the table is walked in.
        let buckets: Vec<u32> = listed.iter().map(|value| enum_value_bucket(value)).collect();
        assert!(
            buckets.windows(2).all(|pair| pair[0] <= pair[1]),
            "case {case:?}: expected a listing in ascending bucket order, got {listed:?} at \
             buckets {buckets:?}"
        );

        // For a type no two of whose values share a bucket, the whole listing is decided
        // by the values alone, so it is asserted entire.
        if !shares_a_bucket(row.declared) {
            assert_eq!(
                listed,
                hashed_enum_order(row.declared),
                "case {case:?}: expected the table's own walk of {:?}, got {listed:?}",
                row.declared
            );
        }
    }
}

#[tokio::test]
async fn a_two_value_set_is_listed_against_its_declaration_when_the_table_says_so() {
    let router = router();

    // The pinned row: `FeedbackValueType` is declared `Valid`, `Invalid`
    // (`smithy_query(query="shape", shape="FeedbackValueType")`), and the two values'
    // buckets fall with the declaration, so the stated order is its REVERSE. Written out
    // as a literal here, so this file fails if the shared derivation starts answering the
    // declared order or a sort.
    let expected = one_enum_value_error("feedbackValue", "Invalid, Valid");
    for operation in ["AdminUpdateAuthEventFeedback", "UpdateAuthEventFeedback"] {
        let mut body = json!({
            "UserPoolId": POOL_ID,
            "Username": "alice",
            "EventId": "event-1",
            "FeedbackValue": OUTSIDE_EVERY_SET,
        });
        if operation == "UpdateAuthEventFeedback" {
            body["FeedbackToken"] = json!("feedback.token");
        }
        let (status, payload) = call(&router, operation, &body).await;
        assert_eq!(
            (status, error_code(&payload), error_message(&payload)),
            (400, INVALID_PARAMETER_EXCEPTION, expected.as_str()),
            "case {operation:?}: expected {expected:?}, got {status} with {payload}"
        );
    }
}
