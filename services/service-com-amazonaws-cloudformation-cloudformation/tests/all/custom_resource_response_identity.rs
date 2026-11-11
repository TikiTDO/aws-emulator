// SPDX-License-Identifier: CC0-1.0

//! What a CUSTOM RESOURCE PROVIDER's answer makes of the resource it answers for.
//!
//! Two rules, both read off the response document the provider PUTs to the request's
//! `ResponseURL`:
//!
//! 1. **The `PhysicalResourceId` the answer carries IS the resource's physical id.** The
//!    identifier CloudFormation computed for the record is a stand-in until the provider
//!    answers; from then on every read of the resource reports the answered id — the `Ref`
//!    and `Fn::GetAtt` a stack's outputs publish, `DescribeStackResources`,
//!    `DescribeStackResource` and `ListStackResources` — and so does every later request
//!    about the resource, including the `Delete` the stack's own removal sends.
//! 2. **An answer whose `Status` is `FAILED` fails the resource for the action its request
//!    was for, with the `Reason` it gave.** A `Create` that is answered `FAILED` leaves the
//!    resource `CREATE_FAILED` and rolls the stack back; a `Update` leaves it
//!    `UPDATE_FAILED` and rolls the update back; a `Delete` leaves the stack's deletion
//!    failed. The provider's own `Reason` is the resource's failure reason, and an answer
//!    stating none carries the fixed sentence for an unstated one.
//!
//! The rows differ along what each rule generalises over: for the first, the FORM of the id
//! a provider mints (an opaque token, a token of its own naming, an ARN, a URL, an id
//! carrying spaces and punctuation, and one as long as the response object allows) and the
//! read the id is asked of; for the second, the `RequestType` the answered request was for
//! and what the answer's `Reason` states (a sentence, a sentence with surrounding
//! whitespace, an empty one, none at all, and one longer than a response may carry).
//!
//! Every assertion drives the emulator through its own wire: the stack operations over
//! CloudFormation's awsQuery endpoint, and the provider's answer as the plain, unsigned HTTP
//! `PUT` to the published `ResponseURL` a provider really makes. The sequence those take is
//! the sibling suite's ([`super::custom_resource_providers`]), so the two cannot drift about
//! how a provider is answered.

use serde_json::json;
use service_com_amazonaws_cloudformation_cloudformation::state::provisioning::custom_resource;

use super::custom_resource_providers::{
    awaited_request, emulator, outputs_of, put_response, settled,
    settled_with_a_responsive_provider, template, POLL_ATTEMPTS, POLL_INTERVAL,
};
use super::harness::{call, create_stack, member, xml_text, xml_texts};
use super::stack_lifecycle::resource_event_reason;

/// The attribute every row's template publishes through `Fn::GetAtt`, so each case reads the
/// answered `Data` beside the answered id.
const ATTRIBUTE: &str = "Endpoint";

/// What one row's provider answers that attribute with.
const ATTRIBUTE_VALUE: &str = "https://provisioned.example.invalid/endpoint";

/// One form of a provider-minted `PhysicalResourceId`.
struct MintedId {
    case: &'static str,
    stack_name: &'static str,
    /// Built per row rather than held as text, so the row that is as long as the response
    /// object allows states its length rather than spelling it out.
    id: fn() -> String,
}

/// How many characters of `PhysicalResourceId` a response object may carry: "this value
/// should be an identifier unique to the custom resource vendor, and can be up to 1 KB in
/// size" (the custom resource response objects reference, quoted on
/// `custom_resource::ProviderResponse::physical_resource_id`).
const RESPONSE_ID_MAX_CHARS: usize = 1024;

/// The forms a provider's own id takes: the protocol fixes none of them, so each is one row.
const MINTED_IDS: &[MintedId] = &[
    MintedId {
        case: "an opaque token",
        stack_name: "answered-opaque",
        id: || "record-7f3c".to_string(),
    },
    MintedId {
        case: "a token of the provider's own naming",
        stack_name: "answered-prefixed",
        id: || "provisioner-cr-0f1e2d3c4b5a".to_string(),
    },
    MintedId {
        case: "an ARN",
        stack_name: "answered-arn",
        id: || "arn:aws:service:us-east-1:000000000000:record/42".to_string(),
    },
    MintedId {
        case: "a URL",
        stack_name: "answered-url",
        id: || "https://provider.example.invalid/records/42".to_string(),
    },
    MintedId {
        case: "an id carrying spaces and punctuation",
        stack_name: "answered-punctuated",
        id: || "tenant 42 / record #7".to_string(),
    },
    MintedId {
        case: "an id as long as a response object may carry",
        stack_name: "answered-longest",
        id: || "r".repeat(RESPONSE_ID_MAX_CHARS),
    },
];

/// Every row: the id the provider answered a `Create` with is what every read of the resource
/// reports and what the `Delete` the stack's removal sends names — never the stand-in
/// CloudFormation had computed for the record.
#[tokio::test]
async fn the_id_a_provider_answers_with_is_what_every_read_reports() {
    for row in MINTED_IDS {
        let case = row.case;
        let minted = (row.id)();
        let (router, engine) = emulator(case).await;
        let body = template("Custom::Probe", "v1", &[ATTRIBUTE]);
        let (status_code, created) = create_stack(&router, row.stack_name, &body, &[]).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected CreateStack to be accepted, got {status_code} with {created}"
        );
        let (stack_id, request, stand_in) = awaited_request(&engine, row.stack_name, "Thing")
            .unwrap_or_else(|| {
                panic!("case {case:?}: the create must be waiting for a provider request")
            });
        assert_ne!(
            stand_in, minted,
            "case {case:?}: the row must answer with an id that is not the computed stand-in"
        );

        let (answered, answer_body) = put_response(
            &router,
            &engine,
            &stack_id,
            "Thing",
            &request.request_id,
            &json!({
                "Status": "SUCCESS",
                "PhysicalResourceId": minted,
                "Data": { ATTRIBUTE: ATTRIBUTE_VALUE },
            }),
        )
        .await;
        assert_eq!(
            answered, 200,
            "case {case:?}: expected the provider's answer to be accepted, got {answered} with \
             {answer_body}"
        );
        let (status, described) = settled(&router, case, row.stack_name).await;
        assert_eq!(
            status, "CREATE_COMPLETE",
            "case {case:?}: the answered resource must complete the stack, got {described}"
        );

        // What the stack PUBLISHES for the resource: `Ref` is the answered id and `Fn::GetAtt`
        // the answered `Data`.
        let outputs = outputs_of(&described);
        assert_eq!(
            outputs.get("ThingRef").map(String::as_str),
            Some(minted.as_str()),
            "case {case:?}: Ref must resolve to the answered id (not the stand-in \
             {stand_in:?}), got {outputs:?}"
        );
        assert_eq!(
            outputs.get(&format!("Thing{ATTRIBUTE}")).map(String::as_str),
            Some(ATTRIBUTE_VALUE),
            "case {case:?}: Fn::GetAtt must resolve to the answered Data, got {outputs:?}"
        );

        // ...and what every READ of the resource itself reports, each read asked through its
        // own operation.
        for (read, members) in [
            (
                "DescribeStackResources",
                vec![member("StackName", row.stack_name)],
            ),
            (
                "DescribeStackResource",
                vec![
                    member("StackName", row.stack_name),
                    member("LogicalResourceId", "Thing"),
                ],
            ),
            (
                "ListStackResources",
                vec![member("StackName", row.stack_name)],
            ),
        ] {
            let (status_code, reported) = call(&router, read, &members).await;
            assert_eq!(
                status_code, 200,
                "case {case:?}: expected {read} to answer, got {status_code} with {reported}"
            );
            assert_eq!(
                xml_texts(&reported, "PhysicalResourceId"),
                vec![minted.clone()],
                "case {case:?}: {read} must report the answered id, got {reported}"
            );
            assert!(
                !reported.contains(stand_in.as_str()),
                "case {case:?}: {read} must not report the stand-in {stand_in:?}, got {reported}"
            );
        }

        // The removal of the stack asks the provider to delete the resource it minted, under
        // the id it answered with.
        let (status_code, deleted) =
            call(&router, "DeleteStack", &[member("StackName", row.stack_name)]).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected DeleteStack to be accepted, got {status_code} with {deleted}"
        );
        let (_, delete_request, _) = awaited_request(&engine, row.stack_name, "Thing")
            .unwrap_or_else(|| {
                panic!("case {case:?}: the deletion must be waiting for a provider request")
            });
        assert_eq!(
            (
                delete_request.request_type.as_str(),
                delete_request.physical_resource_id.as_deref(),
            ),
            ("Delete", Some(minted.as_str())),
            "case {case:?}: the Delete must name the id the provider answered with, got \
             {delete_request:?}"
        );
    }
}

/// One row of [`a_failed_answer_fails_the_action_its_request_was_for`]: which stack operation
/// the answered request belongs to.
struct FailedAction {
    case: &'static str,
    stack_name: &'static str,
    /// Whether the row UPDATES the created resource before answering `FAILED`, and whether
    /// it answers the `Delete` of the stack's own removal instead of the `Create`.
    answered: Answered,
    /// The resource status its own event must carry, or `None` for a removal — where what
    /// the stack reported about the resource when the delete was accepted stands.
    resource_status: Option<&'static str>,
    /// The status the stack settles in.
    stack_status: &'static str,
    /// The action the stack's own summary names as the one that did not go through.
    failed_action: &'static str,
}

/// Which request of the three a row answers `FAILED`.
#[derive(PartialEq, Eq)]
enum Answered {
    Create,
    Update,
    Delete,
}

/// The id the provider answers the create of every row that gets past it with.
const CREATED_ID: &str = "record-answered";

/// The reason the rows of [`a_failed_answer_fails_the_action_its_request_was_for`] state.
const STATED_REASON: &str = "the provisioner could not reach its backend";

const FAILED_ACTIONS: &[FailedAction] = &[
    FailedAction {
        case: "a Create answered FAILED",
        stack_name: "failed-create",
        answered: Answered::Create,
        resource_status: Some("CREATE_FAILED"),
        stack_status: "ROLLBACK_COMPLETE",
        failed_action: "create",
    },
    FailedAction {
        case: "an Update answered FAILED",
        stack_name: "failed-update",
        answered: Answered::Update,
        resource_status: Some("UPDATE_FAILED"),
        stack_status: "UPDATE_ROLLBACK_COMPLETE",
        failed_action: "update",
    },
    FailedAction {
        case: "a Delete answered FAILED",
        stack_name: "failed-delete",
        answered: Answered::Delete,
        resource_status: None,
        stack_status: "DELETE_FAILED",
        failed_action: "delete",
    },
];

/// Every row: the `FAILED` answer fails the resource for the action the request it answers
/// was for — the resource's own event is that action's failure carrying the provider's
/// reason, and the stack takes the failed status of the operation in flight and names that
/// action in its summary.
#[tokio::test]
async fn a_failed_answer_fails_the_action_its_request_was_for() {
    for row in FAILED_ACTIONS {
        let case = row.case;
        let (router, engine) = emulator(case).await;
        let (status_code, created) = create_stack(
            &router,
            row.stack_name,
            &template("Custom::Probe", "v1", &[]),
            &[],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected CreateStack to be accepted, got {status_code} with {created}"
        );
        let (stack_id, request, _) = awaited_request(&engine, row.stack_name, "Thing")
            .unwrap_or_else(|| panic!("case {case:?}: the create must be waiting for a request"));

        // Every row but the one that fails the create gets a resource first.
        if row.answered != Answered::Create {
            put_response(
                &router,
                &engine,
                &stack_id,
                "Thing",
                &request.request_id,
                &json!({ "Status": "SUCCESS", "PhysicalResourceId": CREATED_ID }),
            )
            .await;
            let (status, described) = settled(&router, case, row.stack_name).await;
            assert_eq!(
                status, "CREATE_COMPLETE",
                "case {case:?}: the created resource must complete the stack, got {described}"
            );
        }

        // The operation whose request this row answers FAILED.
        let awaited = match row.answered {
            Answered::Create => request.request_id.clone(),
            Answered::Update => {
                let (status_code, updated) = call(
                    &router,
                    "UpdateStack",
                    &[
                        member("StackName", row.stack_name),
                        member("TemplateBody", &template("Custom::Probe", "v2", &[])),
                    ],
                )
                .await;
                assert_eq!(
                    status_code, 200,
                    "case {case:?}: expected UpdateStack to be accepted, got {status_code} with \
                     {updated}"
                );
                awaited_request(&engine, row.stack_name, "Thing")
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: the update must be waiting for a request")
                    })
                    .1
                    .request_id
            }
            Answered::Delete => {
                let (status_code, deleted) =
                    call(&router, "DeleteStack", &[member("StackName", row.stack_name)]).await;
                assert_eq!(
                    status_code, 200,
                    "case {case:?}: expected DeleteStack to be accepted, got {status_code} with \
                     {deleted}"
                );
                awaited_request(&engine, row.stack_name, "Thing")
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: the deletion must be waiting for a request")
                    })
                    .1
                    .request_id
            }
        };
        let (answered, answer_body) = put_response(
            &router,
            &engine,
            &stack_id,
            "Thing",
            &awaited,
            &json!({
                "Status": "FAILED",
                "Reason": STATED_REASON,
                "PhysicalResourceId": CREATED_ID,
            }),
        )
        .await;
        assert_eq!(
            answered, 200,
            "case {case:?}: expected the provider's answer to be accepted, got {answered} with \
             {answer_body}"
        );

        // A failed create is rolled back, and a failed update is rolled back to the previous
        // configuration: both ask the provider for further requests, which a provider that
        // answers whatever it is sent answers.
        let (status, described) =
            settled_with_a_responsive_provider(&router, &engine, case, row.stack_name, CREATED_ID)
                .await;
        assert_eq!(
            status, row.stack_status,
            "case {case:?}: expected the stack to settle in {:?}, got {described}",
            row.stack_status
        );
        let summary = xml_text(&described, "StackStatusReason").unwrap_or_default();
        assert!(
            summary.contains(&format!("failed to {}: [Thing]", row.failed_action)),
            "case {case:?}: the stack must report which action failed for which resource, got \
             {summary:?}"
        );

        // The resource's OWN event is that action's failure, carrying the provider's reason.
        if let Some(expected) = row.resource_status {
            let reason = recorded_event_reason(&router, row.stack_name, "Thing", expected).await;
            assert_eq!(
                reason, STATED_REASON,
                "case {case:?}: the resource's {expected} event must carry the provider's reason"
            );
        }
    }
}

/// The `Reason` a `FAILED` answer carries is the resource's failure reason, as the protocol
/// states it: the provider's own sentence, and the fixed one for an answer that states none.
///
/// The rows differ in what the answer's `Reason` member is.
#[tokio::test]
async fn the_reason_a_failed_answer_states_is_the_resources_failure_reason() {
    // (case, stack name, the `Reason` the answer carries, the reason the resource reports)
    let longest: String = "n".repeat(custom_resource::MAX_REASON_CHARS + 64);
    let rows: Vec<(&str, &str, Option<String>, String)> = vec![
        (
            "a stated reason",
            "reason-stated",
            Some(STATED_REASON.to_string()),
            STATED_REASON.to_string(),
        ),
        (
            "a stated reason with surrounding whitespace",
            "reason-padded",
            Some(format!("  {STATED_REASON}\n")),
            STATED_REASON.to_string(),
        ),
        (
            "an empty reason",
            "reason-empty",
            Some(String::new()),
            custom_resource::UNSTATED_REASON.to_string(),
        ),
        (
            "no reason at all",
            "reason-absent",
            None,
            custom_resource::UNSTATED_REASON.to_string(),
        ),
        (
            // A `Reason` this long cannot reach CloudFormation at all: the document
            // carrying it is past the size a response may be, and such a document is not
            // read ([`custom_resource::MAX_RESPONSE_BODY_BYTES`]) — the resource fails with
            // the response-too-long reason instead of with the provider's own.
            "a reason longer than a response may carry",
            "reason-longest",
            Some(longest.clone()),
            custom_resource::RESPONSE_TOO_LONG_REASON.to_string(),
        ),
    ];
    for (case, stack_name, stated, expected) in rows {
        let (router, engine) = emulator(case).await;
        let (status_code, created) = create_stack(
            &router,
            stack_name,
            &template("Custom::Probe", "v1", &[]),
            &[],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected CreateStack to be accepted, got {status_code} with {created}"
        );
        let (stack_id, request, _) = awaited_request(&engine, stack_name, "Thing")
            .unwrap_or_else(|| panic!("case {case:?}: the create must be waiting for a request"));
        let mut answer = json!({ "Status": "FAILED", "PhysicalResourceId": CREATED_ID });
        if let Some(stated) = &stated {
            answer["Reason"] = json!(stated);
        }
        let (answered, answer_body) =
            put_response(&router, &engine, &stack_id, "Thing", &request.request_id, &answer)
                .await;
        assert_eq!(
            answered, 200,
            "case {case:?}: expected the provider's answer to be accepted, got {answered} with \
             {answer_body}"
        );
        let (status, described) =
            settled_with_a_responsive_provider(&router, &engine, case, stack_name, CREATED_ID)
                .await;
        assert_eq!(
            status, "ROLLBACK_COMPLETE",
            "case {case:?}: a FAILED answer must fail the create, got {described}"
        );
        let reason = recorded_event_reason(&router, stack_name, "Thing", "CREATE_FAILED").await;
        assert_eq!(
            reason, expected,
            "case {case:?}: expected the resource to report the provider's reason, got {reason:?}"
        );
    }
}

/// The reason the event in which `logical_id` reached `status` carries, once the stack has
/// recorded that event.
///
/// The read itself is the shared one every suite of this crate asks an event's
/// `ResourceStatusReason` with ([`resource_event_reason`]); what this adds is the WAIT, since
/// a stack's events become readable as its own clock reaches them. Every row here expects a
/// reason the provider's answer put there — a `FAILED` answer always leaves one, its own or
/// the fixed sentence for an unstated one — so an empty answer means the event is not there
/// yet, and the last one read is returned for the assertion to report.
async fn recorded_event_reason(
    router: &smithy_server_runtime::MultiServiceRouter,
    stack_name: &str,
    logical_id: &str,
    status: &str,
) -> String {
    let mut last = String::new();
    for _ in 0..POLL_ATTEMPTS {
        last = resource_event_reason(router, stack_name, logical_id, status).await;
        if !last.is_empty() {
            return last;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    last
}
