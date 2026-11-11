// SPDX-License-Identifier: CC0-1.0

//! The awsQuery error document SNS answers a failed call with, driven over the wire:
//! every call below is an awsQuery HTTP request dispatched through the router the
//! emulator serves in production.
//!
//! Three rules are pinned here, each over a SET of inputs rather than one recorded
//! call:
//!
//! 1. every awsQuery error document this service returns carries a `<RequestId>`
//!    element beside `<Error>`, whatever layer refused the request;
//! 2. a constraint refusal on a member the model marks `@sensitive` states the
//!    constraint without reproducing the value, while every other member's clause
//!    keeps its value;
//! 3. an ARN-typed input member is checked for ARN shape BEFORE the store is
//!    consulted, so a value that is not an ARN is `400 InvalidParameter` and only an
//!    ARN-shaped value that names nothing is a lookup miss.

use std::collections::HashSet;

use super::topics_subscriptions_publish::Sns;

/// The phone-number pattern `PhoneNumberString` declares, from
/// `smithy_query(query="operation", operation="CreateSMSSandboxPhoneNumber", …)`:
/// `"pattern": "^(\\+[0-9]{8,}|[0-9]{0,9})$"`, `"length": {"min": 0, "max": 20}`,
/// `"sensitive": true`. The requirement clause the framework renders quotes the
/// pattern verbatim.
const PHONE_NUMBER_PATTERN: &str = r"^(\+[0-9]{8,}|[0-9]{0,9})$";

/// One row of the request-id table: what to call, and the answer it must carry a
/// request id on.
struct ErrorCase<'a> {
    case: &'static str,
    action: &'static str,
    params: Vec<(&'a str, &'a str)>,
    status: u16,
    code: &'static str,
}

/// Every error document SNS answers with carries a `<RequestId>`, and a distinct one
/// per response.
///
/// The rows span the layers that can refuse a request — a handler's modelled 4xx
/// (both the `NotFound` and the `InvalidParameter` class), the boundary validator's
/// refusal of a modelled constraint, an absent required member, and an action this
/// service does not model — because the element belongs to the awsQuery error
/// DOCUMENT, not to any one of them. No row asks for the scaffold's 501: every
/// operation this service models is implemented, so nothing here answers one.
#[tokio::test]
async fn every_awsquery_error_document_carries_a_request_id() {
    let sns = Sns::new();
    let topic = sns.topic("request-id-topic").await;
    let fifo = sns
        .call(
            "CreateTopic",
            &[
                ("Name", "request-id.fifo"),
                ("Attributes.entry.1.key", "FifoTopic"),
                ("Attributes.entry.1.value", "true"),
            ],
        )
        .await
        .element("TopicArn")
        .expect("the FIFO topic ARN");
    let absent = "arn:aws:sns:us-east-1:000000000000:absent-topic";

    let cases: Vec<ErrorCase> = vec![
        ErrorCase {
            case: "GetTopicAttributes on a topic that does not exist",
            action: "GetTopicAttributes",
            params: vec![("TopicArn", absent)],
            status: 404,
            code: "NotFound",
        },
        ErrorCase {
            case: "ListSubscriptionsByTopic on a topic that does not exist",
            action: "ListSubscriptionsByTopic",
            params: vec![("TopicArn", absent)],
            status: 404,
            code: "NotFound",
        },
        ErrorCase {
            case: "Publish to a topic that does not exist",
            action: "Publish",
            params: vec![("TopicArn", absent), ("Message", "m")],
            status: 404,
            code: "NotFound",
        },
        ErrorCase {
            case: "Unsubscribe with a SubscriptionArn that is not a subscription ARN",
            action: "Unsubscribe",
            params: vec![("SubscriptionArn", "not-a-subscription-arn")],
            status: 400,
            code: "InvalidParameter",
        },
        ErrorCase {
            case: "CreateTopic with a FIFO attribute and a name that is not .fifo",
            action: "CreateTopic",
            params: vec![
                ("Name", "not-suffixed"),
                ("Attributes.entry.1.key", "FifoTopic"),
                ("Attributes.entry.1.value", "true"),
            ],
            status: 400,
            code: "InvalidParameter",
        },
        ErrorCase {
            case: "Subscribe with a protocol SNS does not support",
            action: "Subscribe",
            params: vec![
                ("TopicArn", topic.as_str()),
                ("Protocol", "carrierpigeon"),
                ("Endpoint", "nowhere"),
            ],
            status: 400,
            code: "InvalidParameter",
        },
        ErrorCase {
            case: "Publish to a FIFO topic with no MessageGroupId",
            action: "Publish",
            params: vec![("TopicArn", fifo.as_str()), ("Message", "m")],
            status: 400,
            code: "InvalidParameter",
        },
        ErrorCase {
            case: "a value outside a modelled constraint",
            action: "CreateSMSSandboxPhoneNumber",
            params: vec![("PhoneNumber", "not-a-phone-number")],
            status: 400,
            code: "ValidationError",
        },
        ErrorCase {
            case: "an absent required member",
            action: "GetTopicAttributes",
            params: vec![],
            status: 400,
            code: "MalformedInput",
        },
        ErrorCase {
            case: "an action this service does not model",
            action: "DescribeTheUniverse",
            params: vec![],
            status: 400,
            code: "InvalidAction",
        },
        ErrorCase {
            case: "SetEndpointAttributes on an endpoint that does not exist",
            action: "SetEndpointAttributes",
            params: vec![
                (
                    "EndpointArn",
                    "arn:aws:sns:us-east-1:000000000000:endpoint/GCM/push-app/\
                     11111111-1111-4111-8111-111111111111",
                ),
                ("Attributes.entry.1.key", "CustomUserData"),
                ("Attributes.entry.1.value", "user-data"),
            ],
            status: 404,
            code: "NotFound",
        },
    ];

    let mut seen: HashSet<String> = HashSet::new();
    for ErrorCase {
        case,
        action,
        params,
        status,
        code,
    } in &cases
    {
        let answer = sns.call(action, params).await;
        assert_eq!(
            answer.status, *status,
            "case {case:?}: expected {status}, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            *code,
            "case {case:?}: expected the {code} code, got {}",
            answer.body
        );
        let request_id = answer.element("RequestId").unwrap_or_else(|| {
            panic!("case {case:?}: the error document must carry a <RequestId>, got {}", answer.body)
        });
        assert!(
            !request_id.is_empty(),
            "case {case:?}: <RequestId> must name the request, got {request_id:?}"
        );
        assert_eq!(
            answer.request_id_header.as_deref(),
            Some(request_id.as_str()),
            "case {case:?}: the x-amzn-requestid header must repeat the document's id, got {:?}",
            answer.request_id_header
        );
        assert!(
            seen.insert(request_id.clone()),
            "case {case:?}: each response must carry its own id, but {request_id:?} was already used"
        );
    }

    // A success envelope keeps its own request id, inside <ResponseMetadata>, and
    // gains no second one.
    let success = sns.call("GetTopicAttributes", &[("TopicArn", &topic)]).await;
    assert_eq!(
        success.status, 200,
        "the success case must stay a success, got {} / {}",
        success.status, success.body
    );
    assert_eq!(
        success.elements("RequestId").len(),
        1,
        "a success envelope carries exactly one request id, got {}",
        success.body
    );
}

/// One row of the sensitive-value table.
struct RefusalCase {
    case: &'static str,
    action: &'static str,
    params: Vec<(&'static str, &'static str)>,
    /// Values that must NOT appear anywhere in the response document.
    withheld: Vec<&'static str>,
    /// Clauses the message must state verbatim.
    stated: Vec<String>,
}

/// A constraint refusal never reproduces the value of a member the model marks
/// `@sensitive`, and always states the constraint it failed.
///
/// The rows span the three operations whose phone-number member is both
/// `@sensitive` and constrained, two constraint classes on it (`@pattern` and
/// `@length`), and — as the control — a constrained member of the SAME request that
/// is NOT sensitive, whose clause must still echo its value.
#[tokio::test]
async fn a_sensitive_member_value_is_withheld_from_a_validation_refusal() {
    let sns = Sns::new();
    let pattern_clause = |path: &str| {
        format!(
            "Value at '{path}' failed to satisfy constraint: Member must satisfy regular \
             expression pattern: {PHONE_NUMBER_PATTERN}"
        )
    };

    let cases: Vec<RefusalCase> = vec![
        RefusalCase {
            case: "CreateSMSSandboxPhoneNumber with a number outside the pattern",
            action: "CreateSMSSandboxPhoneNumber",
            params: vec![("PhoneNumber", "not-a-phone-number")],
            withheld: vec!["not-a-phone-number"],
            stated: vec![format!(
                "1 validation error detected: {}",
                pattern_clause("phoneNumber")
            )],
        },
        RefusalCase {
            case: "DeleteSMSSandboxPhoneNumber with a number outside the pattern",
            action: "DeleteSMSSandboxPhoneNumber",
            params: vec![("PhoneNumber", "555-0100")],
            withheld: vec!["555-0100"],
            stated: vec![format!(
                "1 validation error detected: {}",
                pattern_clause("phoneNumber")
            )],
        },
        RefusalCase {
            case: "VerifySMSSandboxPhoneNumber with a number outside the pattern",
            action: "VerifySMSSandboxPhoneNumber",
            params: vec![
                ("PhoneNumber", "subscriber-number"),
                ("OneTimePassword", "123456"),
            ],
            withheld: vec!["subscriber-number"],
            stated: vec![format!(
                "1 validation error detected: {}",
                pattern_clause("phoneNumber")
            )],
        },
        RefusalCase {
            case: "a number longer than the modelled maximum",
            action: "CreateSMSSandboxPhoneNumber",
            params: vec![("PhoneNumber", "+123456789012345678901")],
            withheld: vec!["+123456789012345678901"],
            stated: vec![
                "1 validation error detected: Value at 'phoneNumber' failed to satisfy \
                 constraint: Member must have length less than or equal to 20"
                    .to_string(),
            ],
        },
        RefusalCase {
            case: "a sensitive member and a non-sensitive one both outside their pattern",
            action: "VerifySMSSandboxPhoneNumber",
            params: vec![
                ("PhoneNumber", "subscriber-number"),
                ("OneTimePassword", "abcde"),
            ],
            withheld: vec!["subscriber-number"],
            stated: vec![
                "2 validation errors detected: ".to_string(),
                pattern_clause("phoneNumber"),
                // The control: a member the model does NOT mark @sensitive keeps the
                // echo, which is what makes the request fixable from the message.
                "Value 'abcde' at 'oneTimePassword' failed to satisfy constraint: \
                 Member must satisfy regular expression pattern: ^[0-9]+$"
                    .to_string(),
            ],
        },
    ];

    for RefusalCase {
        case,
        action,
        params,
        withheld,
        stated,
    } in &cases
    {
        let answer = sns.call(action, params).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a value outside a modelled constraint is a 400, got {} / {}",
            answer.status, answer.body
        );
        let message = answer
            .element("Message")
            .unwrap_or_else(|| panic!("case {case:?}: no <Message> in {}", answer.body));
        for clause in stated {
            assert!(
                message.contains(clause.as_str()),
                "case {case:?}: the message must state {clause:?}, got {message:?}"
            );
        }
        for value in withheld {
            assert!(
                !answer.body.contains(value),
                "case {case:?}: the document must not reproduce the sensitive value \
                 {value:?}, got {}",
                answer.body
            );
        }
        assert!(
            answer.element("RequestId").is_some(),
            "case {case:?}: the error document must carry a <RequestId>, got {}",
            answer.body
        );
    }
}

/// One row of the error-sentence table.
struct MessageCase<'a> {
    case: &'static str,
    action: &'static str,
    params: Vec<(&'a str, &'a str)>,
    status: u16,
    /// The whole message the response must state.
    message: &'a str,
    /// Caller input that must NOT appear anywhere in the document.
    withheld: Vec<&'a str>,
}

/// An SNS error message is the sentence the service states for that condition, and
/// it does not reproduce the identifier or the value the caller sent.
///
/// Two sentence families are pinned, each over several operations: a resource that
/// does not exist (`<Resource> does not exist`, the same words whichever operation
/// looked it up) and a refused parameter (`Invalid parameter: <parameter>`, with
/// the value echoed only where the sentence SNS states itself names it — the
/// unsupported-protocol row).
#[tokio::test]
async fn an_error_message_states_the_services_sentence_and_not_the_callers_value() {
    let sns = Sns::new();
    let topic = sns.topic("sentence-topic").await;
    let fifo = sns.topic("sentence.fifo").await;
    let absent = "arn:aws:sns:us-east-1:000000000000:gone-topic";
    let long_name = "n".repeat(257);

    let cases: Vec<MessageCase> = vec![
        MessageCase {
            case: "GetTopicAttributes on a topic that does not exist",
            action: "GetTopicAttributes",
            params: vec![("TopicArn", absent)],
            status: 404,
            message: "Topic does not exist",
            withheld: vec![absent],
        },
        MessageCase {
            case: "ListSubscriptionsByTopic on a topic that does not exist",
            action: "ListSubscriptionsByTopic",
            params: vec![("TopicArn", absent)],
            status: 404,
            message: "Topic does not exist",
            withheld: vec![absent],
        },
        MessageCase {
            case: "Publish to a topic that does not exist",
            action: "Publish",
            params: vec![("TopicArn", absent), ("Message", "m")],
            status: 404,
            message: "Topic does not exist",
            withheld: vec![absent],
        },
        MessageCase {
            case: "Subscribe to a topic that does not exist",
            action: "Subscribe",
            params: vec![
                ("TopicArn", absent),
                ("Protocol", "sqs"),
                ("Endpoint", "arn:aws:sqs:us-east-1:000000000000:q"),
            ],
            status: 404,
            message: "Topic does not exist",
            withheld: vec![absent],
        },
        MessageCase {
            case: "Unsubscribe with a SubscriptionArn that is not an ARN at all",
            action: "Unsubscribe",
            params: vec![("SubscriptionArn", "gone-subscription")],
            status: 400,
            message: "Invalid parameter: SubscriptionArn Reason: An ARN must have at least 6 \
                      elements, not 1",
            withheld: vec![],
        },
        MessageCase {
            case: "CreateTopic with a FIFO attribute and a name that is not .fifo",
            action: "CreateTopic",
            params: vec![
                ("Name", "unsuffixed-name"),
                ("Attributes.entry.1.key", "FifoTopic"),
                ("Attributes.entry.1.value", "true"),
            ],
            status: 400,
            message: "Invalid parameter: Topic Name",
            withheld: vec!["unsuffixed-name"],
        },
        MessageCase {
            case: "CreateTopic with a character the name rule does not allow",
            action: "CreateTopic",
            params: vec![("Name", "spaced name")],
            status: 400,
            message: "Invalid parameter: Topic Name",
            withheld: vec!["spaced name"],
        },
        MessageCase {
            case: "CreateTopic with a name past the documented maximum",
            action: "CreateTopic",
            params: vec![("Name", long_name.as_str())],
            status: 400,
            message: "Invalid parameter: Topic Name",
            withheld: vec![long_name.as_str()],
        },
        MessageCase {
            case: "Subscribe with a protocol SNS does not support",
            action: "Subscribe",
            params: vec![
                ("TopicArn", topic.as_str()),
                ("Protocol", "carrierpigeon"),
                ("Endpoint", "nowhere"),
            ],
            status: 400,
            // SNS's own sentence names the string, so this row echoes it.
            message: "Invalid parameter: Amazon SNS does not support this protocol string: \
                      carrierpigeon",
            withheld: vec![],
        },
        MessageCase {
            case: "Publish to a FIFO topic with no MessageGroupId",
            action: "Publish",
            params: vec![("TopicArn", fifo.as_str()), ("Message", "m")],
            status: 400,
            message: "Invalid parameter: The MessageGroupId parameter is required for FIFO topics",
            withheld: vec![],
        },
    ];

    for MessageCase {
        case,
        action,
        params,
        status,
        message,
        withheld,
    } in &cases
    {
        let answer = sns.call(action, params).await;
        assert_eq!(
            answer.status, *status,
            "case {case:?}: expected {status}, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.element("Message").as_deref(),
            Some(*message),
            "case {case:?}: expected the message {message:?}, got {}",
            answer.body
        );
        for value in withheld {
            assert!(
                !answer.body.contains(value),
                "case {case:?}: the message must not reproduce the caller's {value:?}, got {}",
                answer.body
            );
        }
    }
}

/// One row of the ARN-shape table.
struct ArnCase<'a> {
    case: &'static str,
    action: &'static str,
    params: Vec<(&'a str, &'a str)>,
    status: u16,
    code: &'static str,
    /// The clause the message must state, or `""` when the row is not a refusal.
    message: String,
}

/// An ARN-typed member is judged for ARN SHAPE before any lookup: a value that is
/// not an ARN is `400 InvalidParameter` naming the member and the reason, and only
/// an ARN-shaped value that names no resource is a lookup miss.
///
/// The rows span every implemented operation that takes an ARN-typed member, the
/// element counts an ARN can be short by, a six-element value that does not open
/// with `arn:`, and — as the boundary — the ARN-shaped values each operation still
/// answers with its own `NotFound` (or, for the idempotent deletes, a success).
#[tokio::test]
async fn an_arn_typed_member_is_refused_before_the_lookup() {
    let sns = Sns::new();
    let topic = sns.topic("arn-shape-topic").await;
    let absent = "arn:aws:sns:us-east-1:000000000000:no-such-topic";
    let absent_subscription =
        "arn:aws:sns:us-east-1:000000000000:no-such-topic:00000000-0000-4000-8000-000000000000";
    let elements = |member: &str, count: usize| {
        format!("Invalid parameter: {member} Reason: An ARN must have at least 6 elements, not {count}")
    };
    let prefix =
        |member: &str| format!("Invalid parameter: {member} Reason: An ARN must begin with 'arn:'");

    let cases: Vec<ArnCase> = vec![
        ArnCase {
            case: "Unsubscribe with a value that is not an ARN at all",
            action: "Unsubscribe",
            params: vec![("SubscriptionArn", "not-a-valid-arn-7d1f")],
            status: 400,
            code: "InvalidParameter",
            message: elements("SubscriptionArn", 1),
        },
        ArnCase {
            case: "Unsubscribe with the placeholder a pending Subscribe answers with",
            action: "Unsubscribe",
            params: vec![("SubscriptionArn", "pending confirmation")],
            status: 400,
            code: "InvalidParameter",
            message: "Invalid parameter: SubscriptionArn Reason: Cannot unsubscribe a \
                      subscription that is pending confirmation"
                .to_string(),
        },
        ArnCase {
            case: "Unsubscribe with an ARN one element short",
            action: "Unsubscribe",
            params: vec![("SubscriptionArn", "arn:aws:sns:us-east-1:000000000000")],
            status: 400,
            code: "InvalidParameter",
            message: elements("SubscriptionArn", 5),
        },
        ArnCase {
            case: "Unsubscribe with six elements that do not open with arn:",
            action: "Unsubscribe",
            params: vec![("SubscriptionArn", "urn:aws:sns:us-east-1:000000000000:t:s")],
            status: 400,
            code: "InvalidParameter",
            message: prefix("SubscriptionArn"),
        },
        ArnCase {
            case: "Unsubscribe stays idempotent for an ARN-shaped subscription that is gone",
            action: "Unsubscribe",
            params: vec![("SubscriptionArn", absent_subscription)],
            status: 200,
            code: "",
            message: String::new(),
        },
        ArnCase {
            case: "GetTopicAttributes with a value that is not an ARN at all",
            action: "GetTopicAttributes",
            params: vec![("TopicArn", "arn-shape-topic")],
            status: 400,
            code: "InvalidParameter",
            message: elements("TopicArn", 1),
        },
        ArnCase {
            case: "GetTopicAttributes with an ARN-shaped topic that does not exist",
            action: "GetTopicAttributes",
            params: vec![("TopicArn", absent)],
            status: 404,
            code: "NotFound",
            message: String::new(),
        },
        ArnCase {
            case: "ListSubscriptionsByTopic with two elements",
            action: "ListSubscriptionsByTopic",
            params: vec![("TopicArn", "sns:arn-shape-topic")],
            status: 400,
            code: "InvalidParameter",
            message: elements("TopicArn", 2),
        },
        ArnCase {
            case: "ListSubscriptionsByTopic with an ARN-shaped topic that does not exist",
            action: "ListSubscriptionsByTopic",
            params: vec![("TopicArn", absent)],
            status: 404,
            code: "NotFound",
            message: String::new(),
        },
        ArnCase {
            case: "DeleteTopic with four elements",
            action: "DeleteTopic",
            params: vec![("TopicArn", "arn:aws:sns:us-east-1")],
            status: 400,
            code: "InvalidParameter",
            message: elements("TopicArn", 4),
        },
        ArnCase {
            case: "DeleteTopic stays idempotent for an ARN-shaped topic that does not exist",
            action: "DeleteTopic",
            params: vec![("TopicArn", absent)],
            status: 200,
            code: "",
            message: String::new(),
        },
        ArnCase {
            case: "Subscribe with a value that is not an ARN at all",
            action: "Subscribe",
            params: vec![
                ("TopicArn", "arn-shape-topic"),
                ("Protocol", "sqs"),
                ("Endpoint", "arn:aws:sqs:us-east-1:000000000000:q"),
            ],
            status: 400,
            code: "InvalidParameter",
            message: elements("TopicArn", 1),
        },
        ArnCase {
            case: "Publish with a value that is not an ARN at all",
            action: "Publish",
            params: vec![("TopicArn", "arn-shape-topic"), ("Message", "m")],
            status: 400,
            code: "InvalidParameter",
            message: elements("TopicArn", 1),
        },
        ArnCase {
            case: "Publish with a TargetArn that is not an ARN at all",
            action: "Publish",
            params: vec![("TargetArn", "arn-shape-topic"), ("Message", "m")],
            status: 400,
            code: "InvalidParameter",
            message: elements("TargetArn", 1),
        },
        ArnCase {
            case: "ListTagsForResource with a value that is not an ARN at all",
            action: "ListTagsForResource",
            params: vec![("ResourceArn", "arn-shape-topic")],
            status: 400,
            code: "InvalidParameter",
            message: elements("ResourceArn", 1),
        },
        ArnCase {
            case: "ListTagsForResource with six elements that do not open with arn:",
            action: "ListTagsForResource",
            params: vec![("ResourceArn", "urn:aws:sns:us-east-1:000000000000:t")],
            status: 400,
            code: "InvalidParameter",
            message: prefix("ResourceArn"),
        },
        ArnCase {
            // The one operation here whose lookup miss is `ResourceNotFound` rather
            // than `NotFound`: the tag operations model their own not-found shape.
            case: "ListTagsForResource with an ARN-shaped topic that does not exist",
            action: "ListTagsForResource",
            params: vec![("ResourceArn", absent)],
            status: 404,
            code: "ResourceNotFound",
            message: String::new(),
        },
    ];

    for ArnCase {
        case,
        action,
        params,
        status,
        code,
        message,
    } in &cases
    {
        let answer = sns.call(action, params).await;
        assert_eq!(
            answer.status, *status,
            "case {case:?}: expected {status}, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            *code,
            "case {case:?}: expected the {code:?} code, got {}",
            answer.body
        );
        if !message.is_empty() {
            assert_eq!(
                answer.element("Message").as_deref(),
                Some(message.as_str()),
                "case {case:?}: expected the message {message:?}, got {}",
                answer.body
            );
        }
    }

    // The well-formed ARN the topic was minted with still reads back, so the shape
    // check refuses only what is not an ARN.
    let read = sns.call("GetTopicAttributes", &[("TopicArn", &topic)]).await;
    assert_eq!(
        read.status, 200,
        "the topic's own ARN must still read back, got {} / {}",
        read.status, read.body
    );
}
