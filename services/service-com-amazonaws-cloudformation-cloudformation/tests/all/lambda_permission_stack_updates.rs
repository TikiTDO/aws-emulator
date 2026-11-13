// SPDX-License-Identifier: CC0-1.0

//! A stack that grants another service permission to invoke a function: the ARN the grant
//! names, and what a later deployment of that stack does to it.
//!
//! Two rules, one per table.
//!
//! **A reference to a resource resolves to the identifier that resource really has wherever
//! the reference appears — composed by `Fn::Sub` or `Fn::Join` into a longer string of the
//! template's own as readily as carried whole — so a property naming a resource whose
//! identifier its OWNING SERVICE assigned reaches that service carrying the assigned
//! identifier, and a deployment that re-declares the identical template resolves to the
//! identical document and therefore acts on nothing.** The identifier only exists once the
//! create has answered, so a declaration resolved before that carries a stand-in: a grant
//! built on one would name an api nothing created, and the next deployment — which resolves
//! the same reference against the identifier in force — would read an unchanged declaration
//! as changed.
//!
//! **A declaration this emulator cannot change in place is applied through the calls the
//! owning service does publish, not failed:** `AWS::Lambda::Permission` is ONE statement of a
//! function's resource-based policy, which `AddPermission` adds and `RemovePermission`
//! removes, so a revision that states the grant differently replaces that statement and the
//! operation completes with the function's policy holding exactly what the revision declared.
//!
//! The rows differ along what each rule generalises over: which intrinsic composes the
//! reference and WHERE in the value it sits (mid-string between a colon and a slash, as one
//! whole slash-separated segment, at the very end, and joined rather than substituted), and
//! which member of the grant a revision restates (its principal, its action, the source ARN
//! added, the source ARN dropped, and nothing at all beside a resource the stack gains).
//! Every expected value is computed from the api id the stack published, never read back out
//! of the policy under test.
//!
//! A deployment has to differ from the one in force to be accepted at all ("No updates are
//! to be performed."), so every revision below adds a resource beside the grant: what the
//! rows vary is what the grant's own declaration says.

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    acknowledging_iam, call, create_stack, describe_stack, member, router, wait_for_the_transition,
    xml_text, xml_texts, ACCOUNT_ID, REGION,
};
use service_com_amazonaws_cloudformation_cloudformation::state::{
    failed_action, resource_failure_summary, status,
};

/// Call one AWS Lambda operation over its restJson1 wire, signed for this suite's Region —
/// the Region the function and the api the stack creates belong to.
async fn lambda(router: &MultiServiceRouter, method: &str, uri: &str) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/lambda/aws4_request, \
                 SignedHeaders=host, Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::new())
        .expect("a well-formed Lambda request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let answered: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, answered)
}

/// The function this suite's stacks declare.
const FUNCTION: &str = "invoke-grant-fn";

/// One statement of the function's resource-based policy, as AWS Lambda reports it: the
/// members `AddPermission` was given, read back through `GetPolicy`.
#[derive(Debug, Default, PartialEq, Eq)]
struct Statement {
    sid: String,
    principal: String,
    action: String,
    /// The `AWS:SourceArn` the statement's `ArnLike` condition names, empty for a statement
    /// carrying no condition at all.
    source_arn: String,
}

/// Every statement AWS Lambda holds for the function, in the order its policy states them.
async fn held_statements(case: &str, router: &MultiServiceRouter) -> Vec<Statement> {
    let (code, answered) = lambda(
        router,
        "GET",
        &format!("/2015-03-31/functions/{FUNCTION}/policy"),
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected AWS Lambda to report the function's policy, got {code} \
         {answered}"
    );
    let document: Value = answered
        .get("Policy")
        .and_then(Value::as_str)
        .and_then(|policy| serde_json::from_str(policy).ok())
        .unwrap_or(Value::Null);
    document
        .get("Statement")
        .and_then(Value::as_array)
        .map(|statements| {
            statements
                .iter()
                .map(|statement| Statement {
                    sid: text(statement, "Sid"),
                    principal: statement
                        .pointer("/Principal/Service")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    action: text(statement, "Action"),
                    source_arn: statement
                        .pointer("/Condition/ArnLike/AWS:SourceArn")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One string member of a JSON object, or the empty text for a member it does not carry.
fn text(value: &Value, member: &str) -> String {
    value
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A template declaring the function, an HTTP api and ONE grant whose properties are
/// `grant`, plus whatever `extra` adds beside them.
fn template(grant: &str, extra: &str) -> String {
    format!(
        r#"{{"Resources":{{
              "HandlerRole":{{"Type":"AWS::IAM::Role","Properties":{{
                 "AssumeRolePolicyDocument":{{"Version":"2012-10-17","Statement":[{{
                   "Effect":"Allow","Principal":{{"Service":"lambda.amazonaws.com"}},
                   "Action":"sts:AssumeRole"}}]}}}}}},
              "Handler":{{"Type":"AWS::Lambda::Function","Properties":{{
                 "FunctionName":"{FUNCTION}","Runtime":"python3.12",
                 "Handler":"index.handler",
                 "Role":{{"Fn::GetAtt":["HandlerRole","Arn"]}},
                 "Code":{{"ZipFile":"def handler(event, context):\n    return {{}}\n"}}}}}},
              "HttpApi":{{"Type":"AWS::ApiGatewayV2::Api","Properties":{{
                 "Name":"invoke-grant-api","ProtocolType":"HTTP"}}}},
              "InvokePermission":{{"Type":"AWS::Lambda::Permission","Properties":{{
                 "FunctionName":{{"Fn::GetAtt":["Handler","Arn"]}},{grant}}}}}{extra}}},
            "Outputs":{{"ApiId":{{"Value":{{"Ref":"HttpApi"}}}}}}}}"#
    )
}

/// `CreateStack` of `template`, waited out, asserted to have completed; answers the api id
/// the stack published.
async fn given_stack(case: &str, router: &MultiServiceRouter, stack: &str, body: &str) -> String {
    let (code, answered) = create_stack(router, stack, body, &acknowledging_iam(&[])).await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected CreateStack to be accepted, got {code} {answered}"
    );
    wait_for_the_transition().await;
    let (_, described) = describe_stack(router, stack).await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::CREATE_COMPLETE),
        "case {case:?}: expected the create to complete, got {described}"
    );
    xml_text(&described, "OutputValue").unwrap_or_default()
}

/// `UpdateStack` of `template`, waited out; answers the status the stack reports and the
/// reason it carries.
async fn deployed_revision(
    case: &str,
    router: &MultiServiceRouter,
    stack: &str,
    body: &str,
) -> (String, String) {
    let (code, answered) = call(
        router,
        "UpdateStack",
        &acknowledging_iam(&[
            member("StackName", stack),
            member("TemplateBody", body),
        ]),
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected UpdateStack to be accepted, got {code} {answered}"
    );
    wait_for_the_transition().await;
    let (_, described) = describe_stack(router, stack).await;
    (
        xml_text(&described, "StackStatus").unwrap_or_default(),
        xml_text(&described, "StackStatusReason").unwrap_or_default(),
    )
}

/// One row of the composed-reference table: how the grant's `SourceArn` names the api, and
/// the ARN that names once the api id is known.
struct ComposedRow {
    case: &'static str,
    stack: &'static str,
    /// The `SourceArn` property as the template declares it.
    declared: &'static str,
    /// The ARN the declaration resolves to, as a function of the api id the stack published.
    expected: fn(&str) -> String,
}

fn composed_rows() -> Vec<ComposedRow> {
    vec![
        ComposedRow {
            case: "Fn::Sub with the reference between a colon and a slash",
            stack: "grant-sub-midstring",
            declared: r#""SourceArn":{"Fn::Sub":"arn:aws:execute-api:${AWS::Region}:${AWS::AccountId}:${HttpApi}/*/*"}"#,
            expected: |api| format!("arn:aws:execute-api:{REGION}:{ACCOUNT_ID}:{api}/*/*"),
        },
        ComposedRow {
            case: "Fn::Sub with the reference at the very end of the value",
            stack: "grant-sub-at-the-end",
            declared: r#""SourceArn":{"Fn::Sub":"arn:aws:execute-api:${AWS::Region}:${AWS::AccountId}:${HttpApi}"}"#,
            expected: |api| format!("arn:aws:execute-api:{REGION}:{ACCOUNT_ID}:{api}"),
        },
        ComposedRow {
            case: "Fn::Sub with the stage and route spelled out after the reference",
            stack: "grant-sub-stage-and-route",
            declared: r#""SourceArn":{"Fn::Sub":"arn:aws:execute-api:${AWS::Region}:${AWS::AccountId}:${HttpApi}/$default/GET/alpha"}"#,
            expected: |api| {
                format!("arn:aws:execute-api:{REGION}:{ACCOUNT_ID}:{api}/$default/GET/alpha")
            },
        },
        ComposedRow {
            case: "Fn::Join over the reference rather than a substitution",
            stack: "grant-joined",
            declared: r#""SourceArn":{"Fn::Join":["",["arn:aws:execute-api:",{"Ref":"AWS::Region"},":",{"Ref":"AWS::AccountId"},":",{"Ref":"HttpApi"},"/*/*"]]}"#,
            expected: |api| format!("arn:aws:execute-api:{REGION}:{ACCOUNT_ID}:{api}/*/*"),
        },
        ComposedRow {
            case: "the reference as one whole slash-separated segment",
            stack: "grant-whole-segment",
            declared: r#""SourceArn":{"Fn::Join":["/",[{"Fn::Sub":"arn:aws:execute-api:${AWS::Region}:${AWS::AccountId}:apis"},{"Ref":"HttpApi"},"*"]]}"#,
            expected: |api| format!("arn:aws:execute-api:{REGION}:{ACCOUNT_ID}:apis/{api}/*"),
        },
    ]
}

/// The grant's two unchanging members, declared ahead of whatever `SourceArn` a row states.
const GRANT_HEAD: &str =
    r#""Action":"lambda:InvokeFunction","Principal":"apigateway.amazonaws.com","#;

#[tokio::test]
async fn a_grant_naming_an_api_carries_the_id_that_service_assigned() {
    for row in composed_rows() {
        let ComposedRow {
            case,
            stack,
            declared,
            expected,
        } = row;
        let router = router();
        let body = template(&format!("{GRANT_HEAD}{declared}"), "");
        let api_id = given_stack(case, &router, stack, &body).await;
        assert!(
            !api_id.is_empty(),
            "case {case:?}: expected the stack to publish the api id it created"
        );

        // AWS Lambda really holds the grant, and the ARN it is conditioned on names the api
        // the api service assigned the id of — not an identifier nothing minted.
        let held = held_statements(case, &router).await;
        let wanted = expected(&api_id);
        assert_eq!(
            held.len(),
            1,
            "case {case:?}: expected exactly the declared statement, got {held:?}"
        );
        assert_eq!(
            held[0].source_arn, wanted,
            "case {case:?}: expected the grant to be conditioned on {wanted:?}, got {:?}",
            held[0]
        );

        // ...and a deployment that leaves the grant's declaration alone resolves the same
        // reference to the same value, so it acts on nothing and the stack completes. The
        // revision has to differ somewhere for the update to be accepted at all, so it is
        // the same stack with one more resource beside the grant.
        let extra =
            r#","Spare":{"Type":"AWS::SNS::Topic","Properties":{"TopicName":"invoke-grant-spare"}}"#;
        let revised = template(&format!("{GRANT_HEAD}{declared}"), extra);
        let (reported, reason) = deployed_revision(case, &router, stack, &revised).await;
        assert_eq!(
            reported,
            status::UPDATE_COMPLETE,
            "case {case:?}: expected the re-deploy to complete, got {reported:?} ({reason:?})"
        );
        let after = held_statements(case, &router).await;
        assert_eq!(
            after, held,
            "case {case:?}: expected the grant to be left exactly as it was, got {after:?}"
        );
    }
}

/// One row of the revised-grant table: what the create declares beside the function, what
/// the revision declares instead, and the statement the function's policy must end up
/// holding.
struct RevisionRow {
    case: &'static str,
    stack: &'static str,
    created: &'static str,
    revised: &'static str,
    /// The principal, action and source ARN (as a function of the api id) the one remaining
    /// statement must carry.
    principal: &'static str,
    action: &'static str,
    source_arn: fn(&str) -> String,
}

fn revision_rows() -> Vec<RevisionRow> {
    vec![
        RevisionRow {
            case: "nothing about the grant changes, and the stack gains a resource",
            stack: "grant-unchanged",
            created: r#""Action":"lambda:InvokeFunction","Principal":"apigateway.amazonaws.com","SourceArn":{"Fn::Sub":"arn:aws:execute-api:${AWS::Region}:${AWS::AccountId}:${HttpApi}/*/*"}"#,
            revised: r#""Action":"lambda:InvokeFunction","Principal":"apigateway.amazonaws.com","SourceArn":{"Fn::Sub":"arn:aws:execute-api:${AWS::Region}:${AWS::AccountId}:${HttpApi}/*/*"}"#,
            principal: "apigateway.amazonaws.com",
            action: "lambda:InvokeFunction",
            source_arn: |api| format!("arn:aws:execute-api:{REGION}:{ACCOUNT_ID}:{api}/*/*"),
        },
        RevisionRow {
            case: "the principal the grant is made for",
            stack: "grant-reprincipalled",
            created: r#""Action":"lambda:InvokeFunction","Principal":"apigateway.amazonaws.com""#,
            revised: r#""Action":"lambda:InvokeFunction","Principal":"events.amazonaws.com""#,
            principal: "events.amazonaws.com",
            action: "lambda:InvokeFunction",
            source_arn: |_| String::new(),
        },
        RevisionRow {
            case: "the action the grant allows",
            stack: "grant-reactioned",
            created: r#""Action":"lambda:InvokeFunction","Principal":"apigateway.amazonaws.com""#,
            revised: r#""Action":"lambda:InvokeFunctionUrl","Principal":"apigateway.amazonaws.com""#,
            principal: "apigateway.amazonaws.com",
            action: "lambda:InvokeFunctionUrl",
            source_arn: |_| String::new(),
        },
        RevisionRow {
            case: "a source ARN the create declared none of",
            stack: "grant-narrowed",
            created: r#""Action":"lambda:InvokeFunction","Principal":"apigateway.amazonaws.com""#,
            revised: r#""Action":"lambda:InvokeFunction","Principal":"apigateway.amazonaws.com","SourceArn":{"Fn::Sub":"arn:aws:execute-api:${AWS::Region}:${AWS::AccountId}:${HttpApi}/*/*/*"}"#,
            principal: "apigateway.amazonaws.com",
            action: "lambda:InvokeFunction",
            source_arn: |api| format!("arn:aws:execute-api:{REGION}:{ACCOUNT_ID}:{api}/*/*/*"),
        },
        RevisionRow {
            case: "the source ARN the create declared, dropped",
            stack: "grant-widened",
            created: r#""Action":"lambda:InvokeFunction","Principal":"apigateway.amazonaws.com","SourceArn":{"Fn::Sub":"arn:aws:execute-api:${AWS::Region}:${AWS::AccountId}:${HttpApi}/*/*"}"#,
            revised: r#""Action":"lambda:InvokeFunction","Principal":"apigateway.amazonaws.com""#,
            principal: "apigateway.amazonaws.com",
            action: "lambda:InvokeFunction",
            source_arn: |_| String::new(),
        },
    ]
}

#[tokio::test]
async fn a_revised_grant_is_applied_to_the_functions_policy() {
    for row in revision_rows() {
        let RevisionRow {
            case,
            stack,
            created,
            revised,
            principal,
            action,
            source_arn,
        } = row;
        let router = router();
        let api_id = given_stack(case, &router, stack, &template(created, "")).await;
        let before = held_statements(case, &router).await;
        assert_eq!(
            before.len(),
            1,
            "case {case:?}: expected the create to leave exactly one statement, got {before:?}"
        );

        // The revision adds a resource too, so the deployment is a change whichever row it
        // is: what differs between the rows is the grant itself.
        let extra =
            r#","Extra":{"Type":"AWS::SNS::Topic","Properties":{"TopicName":"invoke-grant-extra"}}"#;
        let (reported, reason) =
            deployed_revision(case, &router, stack, &template(revised, extra)).await;
        assert_eq!(
            reported,
            status::UPDATE_COMPLETE,
            "case {case:?}: expected the revision to complete, got {reported:?} ({reason:?})"
        );

        // ...and the function's policy holds exactly the grant the revision declared: one
        // statement, under the statement id the stack published for the resource.
        let held = held_statements(case, &router).await;
        let wanted = Statement {
            sid: before[0].sid.clone(),
            principal: principal.to_string(),
            action: action.to_string(),
            source_arn: source_arn(&api_id),
        };
        assert_eq!(
            held,
            vec![wanted],
            "case {case:?}: expected the policy to hold exactly the declared grant, got {held:?}"
        );
    }
}

#[tokio::test]
async fn a_revision_the_owning_service_refuses_leaves_the_grant_in_force() {
    // The other side of the replacement: a revision AWS Lambda will not accept. `SourceArn`
    // is an `Arn` ("^arn:(aws[a-zA-Z0-9-]*):([a-zA-Z0-9\-])+:…", `AddPermission`'s own
    // member), so a value that is no ARN is refused by the service that owns the statement —
    // and the deployment must report THAT, naming the resource, with the grant the stack
    // already had still in force rather than removed by a replacement that could not finish.
    let case = "a source ARN AWS Lambda refuses";
    let stack = "grant-refused";
    let router = router();
    let created =
        r#""Action":"lambda:InvokeFunction","Principal":"apigateway.amazonaws.com","SourceArn":{"Fn::Sub":"arn:aws:execute-api:${AWS::Region}:${AWS::AccountId}:${HttpApi}/*/*"}"#;
    let api_id = given_stack(case, &router, stack, &template(created, "")).await;
    let before = held_statements(case, &router).await;

    let revised = r#""Action":"lambda:InvokeFunction","Principal":"apigateway.amazonaws.com","SourceArn":"not-an-arn-at-all""#;
    let extra =
        r#","Extra":{"Type":"AWS::SNS::Topic","Properties":{"TopicName":"invoke-grant-refused-extra"}}"#;
    let (reported, reason) =
        deployed_revision(case, &router, stack, &template(revised, extra)).await;
    assert_eq!(
        reported,
        status::UPDATE_ROLLBACK_COMPLETE,
        "case {case:?}: expected the refused revision to fail and roll back, got {reported:?} \
         ({reason:?})"
    );
    assert_eq!(
        reason,
        resource_failure_summary(failed_action::UPDATE, ["InvokePermission"]),
        "case {case:?}: expected the failure to name the resource the owning service refused, \
         got {reason:?}"
    );

    let held = held_statements(case, &router).await;
    assert_eq!(
        held,
        before,
        "case {case:?}: expected the grant in force to be left on the function, got {held:?} \
         (the api is {api_id:?})"
    );
}


#[tokio::test]
async fn a_long_generated_statement_id_is_the_same_ref_update_and_removal_key() {
    let case = "bounded-permission-lifecycle";
    let router = router();
    let stack = "SyntheticPermissionStack".repeat(5);
    let extra = r#","SentinelPermission":{"Type":"AWS::Lambda::Permission","Properties":{
        "FunctionName":{"Fn::GetAtt":["Handler","Arn"]},
        "Action":"lambda:InvokeFunction","Principal":"sns.amazonaws.com"}}"#;
    let with_reference = |grant: &str| {
        let mut document: Value = serde_json::from_str(&template(grant, extra))
            .expect("the synthetic permission template");
        document["Outputs"]["StatementId"] = serde_json::json!({"Value":{"Ref":"InvokePermission"}});
        document.to_string()
    };
    let initial = with_reference(r#""Action":"lambda:InvokeFunction","Principal":"s3.amazonaws.com""#);
    given_stack(case, &router, &stack, &initial).await;
    let before = held_statements(case, &router).await;
    assert_eq!(before.len(), 2);
    let statement_id = before.iter().find(|s| s.principal == "s3.amazonaws.com")
        .expect("the primary grant").sid.clone();
    let sentinel_id = before.iter().find(|s| s.principal == "sns.amazonaws.com")
        .expect("the independent grant").sid.clone();
    assert!(statement_id.len() <= 100);
    assert_ne!(statement_id, sentinel_id);
    let (_, described) = describe_stack(&router, &stack).await;
    let reference = xml_texts(&described, "OutputKey").into_iter()
        .zip(xml_texts(&described, "OutputValue"))
        .find(|(key, _)| key == "StatementId")
        .map(|(_, value)| value).expect("the permission Ref output");
    assert_eq!(reference, statement_id, "Ref must not publish an unfitted statement id");

    let revised = with_reference(r#""Action":"lambda:InvokeFunction","Principal":"events.amazonaws.com""#);
    let (reported, reason) = deployed_revision(case, &router, &stack, &revised).await;
    assert_eq!(reported, status::UPDATE_COMPLETE, "{reason}");
    let changed = held_statements(case, &router).await;
    assert_eq!(changed.len(), 2);
    assert_eq!(changed.iter().find(|s| s.principal == "events.amazonaws.com")
        .expect("the updated grant").sid, statement_id);

    // Remove only this grant, retaining the function and an independent statement. A
    // deleted function therefore cannot make a wrong RemovePermission key look correct.
    let mut retained: Value = serde_json::from_str(&revised).expect("the revised template");
    retained["Resources"].as_object_mut().expect("resources").remove("InvokePermission");
    retained["Outputs"].as_object_mut().expect("outputs").remove("StatementId");
    let (reported, reason) = deployed_revision(case, &router, &stack, &retained.to_string()).await;
    assert_eq!(reported, status::UPDATE_COMPLETE, "{reason}");
    let remaining = held_statements(case, &router).await;
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].sid, sentinel_id);
    assert_eq!(remaining[0].principal, "sns.amazonaws.com");
}
