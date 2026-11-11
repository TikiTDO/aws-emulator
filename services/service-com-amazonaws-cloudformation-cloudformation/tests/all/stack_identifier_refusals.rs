// SPDX-License-Identifier: CC0-1.0

//! How this service refuses a request by the STACK IDENTIFIER it names, over the awsQuery
//! wire.
//!
//! **The rules every case here holds the service to:**
//!
//! 1. A stack identifier a request carries is checked against CloudFormation's own
//!    constraints for it — at least one character, and the name-or-ARN expression — BEFORE
//!    the stack is resolved, and a value that breaks them is refused with the aggregated
//!    `<n> validation errors detected: …` envelope naming the member path the operation
//!    states, one clause per constraint broken, rather than with a stack-not-found message.
//!    An identifier the expression ADMITS reaches the lookup, so an absent stack is still
//!    answered as the missing stack it is.
//! 2. Whichever layer refused a request before a handler ran, the wire `<Code>` is the one
//!    code CloudFormation's Query API publishes for a refused request, `ValidationError`;
//!    the MESSAGE is the framework's own envelope, unchanged.
//! 3. A `PhysicalResourceId` names a resource that EXISTS, so the reverse lookup from a
//!    resource to its stack resolves only while that stack lives: once the stack is gone the
//!    request is refused, even though the stack itself stays readable by its own id.
//!
//! The tables span the class rather than one recorded request: the malformed-identifier side
//! over TWELVE values differing along what the rule generalises over (an empty value,
//! characters the expression does not admit in the positions it does not admit them, and
//! values in or near the form a stack's unique id takes, which the expression does not admit
//! either — `stack_unique_id_identifiers.rs` pins that form on its own) crossed with every
//! operation whose identifier member this service checks itself; the
//! absent-member side over four operations and three different required members; and the
//! physical-id side over a live stack, the same stack once deleted, and an id nothing ever
//! created.

use crate::harness::{
    call, create_stack, member, router, wait_for_the_transition, xml_text, xml_texts,
};

/// A template with two resources, so the physical-id cases have a resource to find a stack
/// from and a sibling the answer must also report.
const TEMPLATE: &str = r#"{"Resources":{
     "First":{"Type":"AWS::SNS::Topic","Properties":{"DisplayName":"identifier-first"}},
     "Second":{"Type":"AWS::SNS::Topic","Properties":{"DisplayName":"identifier-second"}}}}"#;

/// A stack policy body, for the row that reaches `SetStackPolicy`.
const POLICY: &str = r#"{"Statement":[{"Effect":"Allow","Action":"Update:*",
     "Principal":"*","Resource":"*"}]}"#;

/// The expression CloudFormation's own input validation states a stack identifier has to
/// match, quoted in the refusal exactly as the service states it.
///
/// Source: the service's own constant (`state::stack_identifier::STACK_IDENTIFIER_PATTERN`),
/// spelled out here because the sentence IS the requirement — a reworded message must fail
/// this suite. It is not the anchored spelling the model writes for the members that do carry
/// a `@pattern` (`smithy_query(query="shape", shape="StackNameOrId")`).
const IDENTIFIER_PATTERN: &str = "[a-zA-Z][-a-zA-Z0-9]*|arn:[-a-zA-Z0-9:/._+]*";

/// The whole refusal a malformed identifier is answered with: one clause per constraint the
/// value breaks, the expression first, in the framework's envelope.
fn constraint_refusal(path: &str, value: &str) -> String {
    let mut clauses = vec![format!(
        "Value '{value}' at '{path}' failed to satisfy constraint: Member must satisfy \
         regular expression pattern: {IDENTIFIER_PATTERN}"
    )];
    if value.is_empty() {
        clauses.push(format!(
            "Value '{value}' at '{path}' failed to satisfy constraint: Member must have \
             length greater than or equal to 1"
        ));
    }
    match clauses.len() {
        1 => format!("1 validation error detected: {}", clauses[0]),
        n => format!("{n} validation errors detected: {}", clauses.join("; ")),
    }
}

/// The refusal an absent required member is answered with.
fn absent_member_refusal(path: &str) -> String {
    format!(
        "1 validation error detected: Value null at '{path}' failed to satisfy constraint: \
         Member must not be null"
    )
}

/// One operation that takes a stack identifier this service checks itself: the members it
/// needs besides `StackName`, the member path its refusal states the identifier at, and
/// whether a request naming a LIVE stack is served (a mutation is left out of that control,
/// since it would move the stack the other rows read).
struct IdentifierRow {
    action: &'static str,
    extra: &'static [(&'static str, &'static str)],
    path: &'static str,
    /// Whether a request naming a well-formed identifier nothing holds is REFUSED about the
    /// stack. A delete is the one operation that is not: a stack that is already gone needs
    /// no deletion, so it answers success.
    refuses_an_absent_stack: bool,
    /// Whether a request naming a LIVE stack is served (a mutation is left out of that
    /// control, since it would move the stack the other rows read).
    serves_a_live_stack: bool,
}

/// Every operation of this service whose stack-identifier member the model leaves
/// unconstrained, so the operation is what checks it: the members that target
/// `StackNameOrId` carry the constraints in the model and are refused at the boundary
/// instead.
///
/// `ListStackResources` states the identifier at `stackNameOrId`, which is what
/// CloudFormation's own validator names that operation's member; every other one states it at
/// `stackName`.
const IDENTIFIER_ROWS: &[IdentifierRow] = &[
    IdentifierRow {
        action: "ListStackResources",
        extra: &[],
        path: "stackNameOrId",
        refuses_an_absent_stack: true,
        serves_a_live_stack: true,
    },
    IdentifierRow {
        action: "DescribeStackResources",
        extra: &[],
        path: "stackName",
        refuses_an_absent_stack: true,
        serves_a_live_stack: true,
    },
    IdentifierRow {
        action: "DescribeStackResource",
        extra: &[("LogicalResourceId", "First")],
        path: "stackName",
        refuses_an_absent_stack: true,
        serves_a_live_stack: true,
    },
    IdentifierRow {
        action: "DescribeStacks",
        extra: &[],
        path: "stackName",
        refuses_an_absent_stack: true,
        serves_a_live_stack: true,
    },
    IdentifierRow {
        action: "DescribeStackEvents",
        extra: &[],
        path: "stackName",
        refuses_an_absent_stack: true,
        serves_a_live_stack: true,
    },
    IdentifierRow {
        action: "GetTemplate",
        extra: &[],
        path: "stackName",
        refuses_an_absent_stack: true,
        serves_a_live_stack: true,
    },
    IdentifierRow {
        action: "GetStackPolicy",
        extra: &[],
        path: "stackName",
        refuses_an_absent_stack: true,
        serves_a_live_stack: true,
    },
    IdentifierRow {
        action: "SetStackPolicy",
        extra: &[("StackPolicyBody", POLICY)],
        path: "stackName",
        refuses_an_absent_stack: true,
        serves_a_live_stack: true,
    },
    IdentifierRow {
        action: "UpdateStack",
        extra: &[("TemplateBody", TEMPLATE)],
        path: "stackName",
        refuses_an_absent_stack: true,
        serves_a_live_stack: false,
    },
    IdentifierRow {
        action: "DeleteStack",
        extra: &[],
        path: "stackName",
        refuses_an_absent_stack: false,
        serves_a_live_stack: false,
    },
    IdentifierRow {
        action: "CancelUpdateStack",
        extra: &[],
        path: "stackName",
        refuses_an_absent_stack: true,
        serves_a_live_stack: false,
    },
];

/// The identifiers no stack could ever carry: an empty value, values whose characters — or the
/// position of them — the expression does not admit, and values in or near the form a stack's
/// unique id takes (`stack_unique_id_identifiers.rs` pins that form on its own), which is
/// hexadecimal and so outside the name alternative whenever it starts with a digit.
const MALFORMED: &[(&str, &str)] = &[
    ("an empty identifier", ""),
    ("a name with an underscore", "identifier_stack"),
    ("a name with a dot", "identifier.stack"),
    ("a name starting with a digit", "9identifier"),
    ("a name starting with a hyphen", "-identifier"),
    ("a name with a space", "identifier stack"),
    ("a stack's unique id", "0e81d159-090a-7b61-9856-04d0999cb465"),
    ("a unique id a group short", "0e81d159-090a-7b61-04d0999cb465"),
    (
        "a unique id with a short last group",
        "0e81d159-090a-7b61-9856-04d0999cb46",
    ),
    (
        "a unique id with a group too many",
        "0e81d159-090a-7b61-9856-04d0999cb465-9856",
    ),
    (
        "a unique id with a non-hexadecimal digit",
        "0g81d159-090a-7b61-9856-04d0999cb465",
    ),
    (
        "a unique id with no groups at all",
        "0e81d159090a7b61985604d0999cb465",
    ),
];

fn members_of(row: &IdentifierRow, identifier: &str) -> Vec<(String, String)> {
    let mut members = vec![member("StackName", identifier)];
    members.extend(
        row.extra
            .iter()
            .map(|(key, value)| member(key, value)),
    );
    members
}

#[tokio::test]
async fn a_malformed_stack_identifier_is_refused_before_the_stack_is_resolved() {
    let router = router();
    // A stack that really is there, so no row can pass by the request being unservable for
    // another reason: the identifier is what each row changes.
    let (status, body) = create_stack(&router, "identifier-live", TEMPLATE, &[]).await;
    assert_eq!(
        status, 200,
        "expected CreateStack to be accepted, got {status} {body}"
    );
    wait_for_the_transition().await;

    for row in IDENTIFIER_ROWS {
        for (case, identifier) in MALFORMED {
            let case = format!("{} naming {case}", row.action);
            let (status, body) = call(&router, row.action, &members_of(row, identifier)).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} {body}"
            );
            assert_eq!(
                xml_text(&body, "Code").as_deref(),
                Some("ValidationError"),
                "case {case:?}: expected <Code>ValidationError</Code>, got {body}"
            );
            let expected = constraint_refusal(row.path, identifier);
            assert_eq!(
                xml_text(&body, "Message").as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected {expected:?}, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_well_formed_identifier_still_reaches_the_stack_lookup() {
    let router = router();
    let (status, body) = create_stack(&router, "identifier-served", TEMPLATE, &[]).await;
    assert_eq!(
        status, 200,
        "expected CreateStack to be accepted, got {status} {body}"
    );
    wait_for_the_transition().await;
    let (status, described) = call(
        &router,
        "DescribeStacks",
        &[member("StackName", "identifier-served")],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the stack to be readable, got {status} {described}"
    );
    let stack_id = xml_text(&described, "StackId").expect("the stack's own id");

    // Two well-formed identifiers nothing holds: a name the expression admits, and a stack
    // ARN of the form the expression's other alternative admits. Both reach the lookup, so
    // each operation answers its OWN not-found refusal rather than a constraint clause.
    let absent_id = format!("{stack_id}-absent");
    for (case, identifier) in [
        ("a name nothing created", "identifier-absent"),
        ("a stack id no stack carries", absent_id.as_str()),
    ] {
        for row in IDENTIFIER_ROWS {
            if !row.refuses_an_absent_stack {
                continue;
            }
            let case = format!("{} naming {case}", row.action);
            let (status, body) = call(&router, row.action, &members_of(row, identifier)).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} {body}"
            );
            let message = xml_text(&body, "Message").unwrap_or_default();
            assert!(
                message.contains("does not exist"),
                "case {case:?}: expected the stack-not-found refusal, got {message:?}"
            );
            assert!(
                !message.contains("validation error"),
                "case {case:?}: expected no constraint clause for a well-formed identifier, \
                 got {message:?}"
            );
        }
    }

    // ...and the identifier of a stack that IS there is served, so the check above refuses
    // nothing the service would otherwise answer.
    for row in IDENTIFIER_ROWS {
        if !row.serves_a_live_stack {
            continue;
        }
        let case = format!("{} naming a live stack", row.action);
        let (status, body) = call(&router, row.action, &members_of(row, "identifier-served")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the call to be served, got {status} {body}"
        );
    }
}

#[tokio::test]
async fn an_absent_required_member_is_refused_under_the_services_own_code() {
    let router = router();
    // Three different required members, over four operations: whichever member the request
    // leaves out, the code is the one this service publishes and the sentence is the
    // framework's.
    for (case, action, members, path) in [
        (
            "a listing with no stack identifier",
            "ListStackResources",
            vec![],
            "stackName",
        ),
        (
            "a resource read with no stack identifier",
            "DescribeStackResource",
            vec![member("LogicalResourceId", "First")],
            "stackName",
        ),
        (
            "an importers read with no export name",
            "ListImports",
            vec![],
            "exportName",
        ),
        (
            "a signal with no logical resource id",
            "SignalResource",
            vec![
                member("StackName", "identifier-absent"),
                member("UniqueId", "signal-1"),
                member("Status", "SUCCESS"),
            ],
            "logicalResourceId",
        ),
    ] {
        let (status, body) = call(&router, action, &members).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected <Code>ValidationError</Code>, got {body}"
        );
        let expected = absent_member_refusal(path);
        assert_eq!(
            xml_text(&body, "Message").as_deref(),
            Some(expected.as_str()),
            "case {case:?}: expected {expected:?}, got {body}"
        );
    }
}

#[tokio::test]
async fn the_stack_of_a_physical_resource_is_resolved_only_while_that_stack_lives() {
    let router = router();
    let (status, body) = create_stack(&router, "identifier-physical", TEMPLATE, &[]).await;
    assert_eq!(
        status, 200,
        "expected CreateStack to be accepted, got {status} {body}"
    );
    wait_for_the_transition().await;

    let (status, resources) = call(
        &router,
        "DescribeStackResources",
        &[member("StackName", "identifier-physical")],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the stack's resources to be readable, got {status} {resources}"
    );
    let physical_ids = xml_texts(&resources, "PhysicalResourceId");
    assert_eq!(
        physical_ids.len(),
        2,
        "expected both declared resources to report a physical id, got {physical_ids:?}"
    );
    let (status, described) = call(
        &router,
        "DescribeStacks",
        &[member("StackName", "identifier-physical")],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the stack to be readable, got {status} {described}"
    );
    let stack_id = xml_text(&described, "StackId").expect("the stack's own id");

    // While the stack lives, EITHER of its resources finds it, and the answer is the whole
    // stack's resources rather than the one the request named.
    for physical_id in &physical_ids {
        let (status, body) = call(
            &router,
            "DescribeStackResources",
            &[member("PhysicalResourceId", physical_id)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {physical_id:?}: expected the owning stack to be found, got {status} {body}"
        );
        assert_eq!(
            xml_texts(&body, "StackId"),
            vec![stack_id.clone(), stack_id.clone()],
            "case {physical_id:?}: expected both of the stack's resources, got {body}"
        );
    }

    let (status, body) = call(
        &router,
        "DeleteStack",
        &[member("StackName", "identifier-physical")],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected DeleteStack to be accepted, got {status} {body}"
    );
    wait_for_the_transition().await;

    // Once the stack is gone, nothing of its resources is there to find it from — so every
    // one of them is refused, exactly as an id nothing ever created is.
    let mut asked: Vec<String> = physical_ids.clone();
    asked.push("identifier-never-created".to_string());
    for physical_id in &asked {
        let (status, body) = call(
            &router,
            "DescribeStackResources",
            &[member("PhysicalResourceId", physical_id)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {physical_id:?}: expected the read to be refused, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {physical_id:?}: expected <Code>ValidationError</Code>, got {body}"
        );
        let expected = format!("Stack for {physical_id} does not exist");
        assert_eq!(
            xml_text(&body, "Message").as_deref(),
            Some(expected.as_str()),
            "case {physical_id:?}: expected {expected:?}, got {body}"
        );
    }

    // The stack itself is still readable by its own id, which is what makes the refusals
    // above about the RESOURCES rather than about the records being gone.
    let (status, body) = call(
        &router,
        "DescribeStackResources",
        &[member("StackName", &stack_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the deleted stack by its own id\": expected the records to be readable, got \
         {status} {body}"
    );
}
