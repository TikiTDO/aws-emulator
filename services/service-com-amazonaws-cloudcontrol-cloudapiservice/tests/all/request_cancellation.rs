// SPDX-License-Identifier: CC0-1.0

//! What a cancelled resource operation request leaves in the store of the service that OWNS
//! the resource type.
//!
//! The rule every case here holds the service to: **a cancelled request abandons the work
//! it owed, so for a type whose resources live in another service's store of this emulator
//! the resource a cancelled create was provisioning is never provisioned THERE — that
//! service answers for it as a resource it does not hold, Cloud Control API reports the
//! identifier as not found, and the type's inventory names it nowhere.**
//!
//! `cancel_resource_request` holds the cancel rule itself — which requests a cancel acts on,
//! how the two refusals are worded, and what the abandoned work leaves behind in THIS
//! service's own store. This suite is its complement: the rows differ along the service that
//! owns the type, so what is asserted is where the absence is observable rather than
//! anything about one owning service.

use serde_json::{json, Value};

use super::emulator::{
    code, descriptions, message, progress, text, Emulator, STATUS_CANCEL_COMPLETE,
    STATUS_CANCEL_IN_PROGRESS,
};

/// A service that OWNS the resources of a type Cloud Control API acts on.
#[derive(Debug, Clone, Copy)]
enum Owning {
    /// Amazon SQS, which owns the `AWS::SQS::*` types.
    Sqs,
    /// Amazon SNS, which owns the `AWS::SNS::*` types.
    Sns,
}

impl Owning {
    /// One emulator with this owning service mounted beside Cloud Control API.
    fn emulator(self) -> Emulator {
        match self {
            Self::Sqs => Emulator::with_sqs(),
            Self::Sns => Emulator::with_sns(),
        }
    }

    /// A `TypeName` whose resources this service owns.
    fn type_name(self) -> &'static str {
        match self {
            Self::Sqs => "AWS::SQS::Queue",
            Self::Sns => "AWS::SNS::Topic",
        }
    }

    /// The desired state of the resource called `name`.
    fn desired(self, name: &str) -> Value {
        match self {
            Self::Sqs => json!({ "QueueName": name }),
            Self::Sns => {
                json!({ "TopicName": name, "Tags": [{ "Key": "team", "Value": "platform" }] })
            }
        }
    }

    /// Assert that this service holds no resource called `name` — the one Cloud Control API
    /// addresses as `identifier` — stating the refusal in its own terms.
    async fn refuses_the_absent_resource(
        self,
        emulator: &Emulator,
        case: &str,
        name: &str,
        identifier: &str,
    ) {
        match self {
            Self::Sqs => {
                let (http, body) = emulator
                    .sqs("GetQueueUrl", json!({ "QueueName": name }))
                    .await;
                assert_eq!(
                    http, 400,
                    "case {case:?}: sqs:GetQueueUrl expected HTTP 400, got {http} with body \
                     {body:?}"
                );
                assert!(
                    code(&body).contains("QueueDoesNotExist"),
                    "case {case:?}: expected the queue-does-not-exist refusal, got {:?}",
                    code(&body)
                );
                assert_eq!(
                    message(&body),
                    "The specified queue does not exist.",
                    "case {case:?}: unexpected wording, got {:?}",
                    message(&body)
                );
            }
            Self::Sns => {
                let (http, body) = emulator
                    .sns("GetTopicAttributes", &[("TopicArn", identifier)])
                    .await;
                assert_eq!(
                    http, 404,
                    "case {case:?}: sns:GetTopicAttributes expected HTTP 404, got {http} with \
                     body {body:?}"
                );
                assert!(
                    body.contains("<Code>NotFound</Code>"),
                    "case {case:?}: expected the topic-does-not-exist refusal, got {body:?}"
                );
                assert!(
                    body.contains("<Message>Topic does not exist</Message>"),
                    "case {case:?}: unexpected wording, got {body:?}"
                );
            }
        }
    }
}

/// The identifiers `ListResources` reports for `type_name`.
async fn inventory(emulator: &Emulator, type_name: &str) -> Vec<String> {
    let listed = emulator.list(type_name, None, None).await;
    descriptions(&listed)
        .iter()
        .map(|description| text(description, "Identifier"))
        .collect()
}

#[tokio::test]
async fn a_cancelled_create_provisions_nothing_in_the_service_that_owns_the_type() {
    // The rows differ along the service that OWNS the type, so "nothing was provisioned" is
    // that service's own answer about the resource the cancelled create named: a queue
    // Amazon SQS resolves no url for, and a topic Amazon SNS holds no record of.
    let rows: [(&str, Owning, &str); 2] = [
        ("a cancelled queue create", Owning::Sqs, "cancelled-queue"),
        ("a cancelled topic create", Owning::Sns, "cancelled-topic"),
    ];
    for (case, owning, name) in rows {
        let emulator = owning.emulator();
        let type_name = owning.type_name();
        let (http, body) = emulator.create(type_name, &owning.desired(name), None).await;
        assert_eq!(
            http, 200,
            "case {case:?}: expected HTTP 200, got {http} with body {body:?}"
        );
        let accepted = progress(&body);
        let token = text(&accepted, "RequestToken");
        let identifier = text(&accepted, "Identifier");
        assert!(
            !identifier.is_empty(),
            "case {case:?}: the create must name the identifier it is provisioning, got \
             {accepted:?}"
        );

        let (http, body) = emulator.cancel(&token).await;
        assert_eq!(
            http, 200,
            "case {case:?}: expected the cancellation to be accepted, got {http} with body \
             {body:?}"
        );
        assert_eq!(
            text(&progress(&body), "OperationStatus"),
            STATUS_CANCEL_IN_PROGRESS,
            "case {case:?}: expected the request to report being cancelled, got {body:?}"
        );
        let settled = emulator.reaching(&token, &[STATUS_CANCEL_COMPLETE]).await;
        assert!(
            settled["RetryAfter"].is_null(),
            "case {case:?}: a settled request must name no instant to poll at, got \
             {settled:?}"
        );

        // The service that OWNS the type holds no such resource, and says so in its own
        // terms.
        owning
            .refuses_the_absent_resource(&emulator, case, name, &identifier)
            .await;

        // And Cloud Control API agrees: nothing answers to the identifier the cancelled
        // create was provisioning, and the type's inventory names it nowhere.
        let (http, body) = emulator.get(type_name, &identifier).await;
        assert_eq!(
            http, 404,
            "case {case:?}: GetResource after the cancellation: expected HTTP 404, got {http} \
             with body {body:?}"
        );
        let identifiers = inventory(&emulator, type_name).await;
        assert!(
            identifiers.is_empty(),
            "case {case:?}: a cancelled create must leave the type's inventory empty, got \
             {identifiers:?}"
        );
    }
}
