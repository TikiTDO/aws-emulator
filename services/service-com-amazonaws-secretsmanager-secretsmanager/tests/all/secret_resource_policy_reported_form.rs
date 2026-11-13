// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the LAYOUT a resource policy document comes back in.
//!
//! The rule under test: a resource policy travels as an opaque JSON document
//! (`GetResourcePolicy` "retrieves the JSON text of the resource-based policy
//! document attached to the secret", this service's Smithy model), and a service
//! that stores one parses it and re-serializes it rather than echoing the
//! caller's bytes, so what `GetResourcePolicy` reports is the canonical rendering
//! of the policy — a member of an object per line indented two spaces per
//! enclosing object with a space on both sides of the name separator, arrays laid
//! out inline with a space just inside each bracket, a one-value value list
//! written as that value alone — whatever layout the attach was written in.
//!
//! Two suites, because the rule has two halves:
//!
//! * a TABLE pinning the exact reported text, written out by hand, for documents
//!   spanning the class (member order, the `Statement` element as one statement or
//!   a list, nesting, value lists of none, one and two values, empty objects,
//!   several statements, non-ASCII and escaped characters, and a document already
//!   in the reported layout);
//! * the invariant itself, over the same policy submitted in every whitespacing a
//!   caller's renderer produces: all of them read back as the very same bytes,
//!   those bytes denote the document that was submitted, and re-attaching them
//!   changes nothing.
//!
//! `tests/all/secret_resource_policy.rs` covers everything else about the
//! document — that one is attached at a time, that it is replaced, deleted and
//! refused — and `tests/all/secret_policy_validation.rs` the syntax check.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 header naming `secretsmanager` in `us-west-2`; the router routes on
/// the credential scope and the emulator verifies signatures only when an
/// authenticator is registered, which this harness does not do.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/secretsmanager/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=deadbeef";

/// A fresh emulator holding only this service, over its own engine.
fn emulator() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one `awsJson1_1` request that must succeed, and return its body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: Value, case: &str) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("secretsmanager.{operation}"))
        .header("authorization", AUTHORIZATION)
        .body(bytes::Bytes::from(
            // A write that stores a value creates a version the caller has to
            // name; see `secret_client_request_token::with_staked_version`.
            serde_json::to_vec(&crate::secret_client_request_token::with_staked_version(
                operation, &body,
            ))
            .expect("the request body is serializable"),
        ))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from {operation}, got {status} with body {parsed}"
    );
    parsed
}

/// Create a secret under `name`.
async fn create(router: &MultiServiceRouter, name: &str, case: &str) {
    ok(
        router,
        "CreateSecret",
        json!({"Name": name, "SecretString": "s3cret"}),
        case,
    )
    .await;
}

/// Attach `document` to the secret `name` and return what `GetResourcePolicy`
/// reports for it.
async fn attach_then_read(
    router: &MultiServiceRouter,
    name: &str,
    document: &str,
    case: &str,
) -> String {
    ok(
        router,
        "PutResourcePolicy",
        json!({"SecretId": name, "ResourcePolicy": document}),
        case,
    )
    .await;
    let read = ok(router, "GetResourcePolicy", json!({"SecretId": name}), case).await;
    read.get("ResourcePolicy")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no ResourcePolicy in {read}"))
        .to_string()
}

/// A document written out as its lines, which is how the expected text below
/// states the layout under test.
fn document_of(lines: &[&str]) -> String {
    lines.join("\n")
}

/// The exact text `GetResourcePolicy` reports, for documents spanning the class:
/// the member order the caller wrote, the `Statement` element as one statement or
/// as a list of them, nested objects, value lists of two, one and no values, an
/// object carrying nothing, several statements, non-ASCII and escaped characters,
/// and a document already in the reported layout.
#[tokio::test]
async fn the_reported_document_is_in_the_canonical_layout() {
    let rows: Vec<(&str, String, String)> = vec![
        (
            // The whole layout in one row: nested object, inline list of two
            // values, one statement inside the `Statement` list.
            "compact-one-statement",
            concat!(
                r#"{"Version":"2012-10-17","Statement":[{"Sid":"reader","Effect":"Allow","#,
                r#""Principal":{"AWS":"arn:aws:iam::123456789012:root"},"#,
                r#""Action":["secretsmanager:GetSecretValue","secretsmanager:DescribeSecret"],"#,
                r#""Resource":"*"}]}"#
            )
            .to_string(),
            document_of(&[
                "{",
                r#"  "Version" : "2012-10-17","#,
                r#"  "Statement" : [ {"#,
                r#"    "Sid" : "reader","#,
                r#"    "Effect" : "Allow","#,
                r#"    "Principal" : {"#,
                r#"      "AWS" : "arn:aws:iam::123456789012:root""#,
                "    },",
                r#"    "Action" : [ "secretsmanager:GetSecretValue", "secretsmanager:DescribeSecret" ],"#,
                r#"    "Resource" : "*""#,
                "  } ]",
                "}",
            ]),
        ),
        (
            // The SAME document, four-space indented and with a compact name
            // separator: the submitted whitespace is not part of the document, so
            // the reported text is the one above, character for character.
            "four-space-indented-same-document",
            document_of(&[
                "{",
                r#"    "Version": "2012-10-17",   "#,
                r#"    "Statement": ["#,
                "        {",
                r#"            "Sid": "reader","#,
                r#"            "Effect": "Allow","#,
                r#"            "Principal": {"AWS": "arn:aws:iam::123456789012:root"},"#,
                r#"            "Action": ["#,
                r#"                "secretsmanager:GetSecretValue","#,
                r#"                "secretsmanager:DescribeSecret""#,
                "            ],",
                r#"            "Resource": "*""#,
                "        }",
                "    ]",
                "}",
            ]),
            document_of(&[
                "{",
                r#"  "Version" : "2012-10-17","#,
                r#"  "Statement" : [ {"#,
                r#"    "Sid" : "reader","#,
                r#"    "Effect" : "Allow","#,
                r#"    "Principal" : {"#,
                r#"      "AWS" : "arn:aws:iam::123456789012:root""#,
                "    },",
                r#"    "Action" : [ "secretsmanager:GetSecretValue", "secretsmanager:DescribeSecret" ],"#,
                r#"    "Resource" : "*""#,
                "  } ]",
                "}",
            ]),
        ),
        (
            // A document already in the reported layout comes back unchanged: the
            // rendering is a fixed point, which is what lets a client diff the
            // policy it reads against the one it holds.
            "already-in-the-reported-layout",
            document_of(&[
                "{",
                r#"  "Version" : "2012-10-17","#,
                r#"  "Statement" : [ {"#,
                r#"    "Effect" : "Allow","#,
                r#"    "Principal" : {"#,
                r#"      "AWS" : "arn:aws:iam::123456789012:root""#,
                "    },",
                r#"    "Action" : "secretsmanager:GetSecretValue","#,
                r#"    "Resource" : "*""#,
                "  } ]",
                "}",
            ]),
            document_of(&[
                "{",
                r#"  "Version" : "2012-10-17","#,
                r#"  "Statement" : [ {"#,
                r#"    "Effect" : "Allow","#,
                r#"    "Principal" : {"#,
                r#"      "AWS" : "arn:aws:iam::123456789012:root""#,
                "    },",
                r#"    "Action" : "secretsmanager:GetSecretValue","#,
                r#"    "Resource" : "*""#,
                "  } ]",
                "}",
            ]),
        ),
        (
            // The `Statement` element as ONE statement rather than a list, and
            // written BEFORE `Version`: the members keep the order the caller
            // wrote them in rather than being sorted. The statement denies an
            // action other than the read below, so what it reports is its layout
            // rather than a lockout.
            "statement-object-written-before-version",
            concat!(
                r#"{"Statement":{"Effect":"Deny","Principal":"*","#,
                r#""Action":"secretsmanager:GetSecretValue","Resource":"*"},"#,
                r#""Version":"2012-10-17"}"#
            )
            .to_string(),
            document_of(&[
                "{",
                r#"  "Statement" : {"#,
                r#"    "Effect" : "Deny","#,
                r#"    "Principal" : "*","#,
                r#"    "Action" : "secretsmanager:GetSecretValue","#,
                r#"    "Resource" : "*""#,
                "  },",
                r#"  "Version" : "2012-10-17""#,
                "}",
            ]),
        ),
        (
            // A value list holding exactly ONE value is reported as that value
            // alone: the IAM policy language documents `Action` and the per-type
            // identifiers of a `Principal` as "a string or an array of strings",
            // and the one-value array is reported in its string form. A list of
            // STATEMENTS is not a value list, so the brackets around the single
            // statement stay.
            "one-value-lists",
            concat!(
                r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","#,
                r#""Principal":{"AWS":["arn:aws:iam::123456789012:root"]},"#,
                r#""Action":["secretsmanager:GetSecretValue"],"Resource":["*"]}]}"#
            )
            .to_string(),
            document_of(&[
                "{",
                r#"  "Version" : "2012-10-17","#,
                r#"  "Statement" : [ {"#,
                r#"    "Effect" : "Allow","#,
                r#"    "Principal" : {"#,
                r#"      "AWS" : "arn:aws:iam::123456789012:root""#,
                "    },",
                r#"    "Action" : "secretsmanager:GetSecretValue","#,
                r#"    "Resource" : "*""#,
                "  } ]",
                "}",
            ]),
        ),
        (
            // A list holding NO value, and an object carrying nothing: each keeps
            // its brackets with their one inner space.
            "empty-list-and-empty-object",
            concat!(
                r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","#,
                r#""Principal":"*","Action":[],"Resource":"*","Condition":{}}]}"#
            )
            .to_string(),
            document_of(&[
                "{",
                r#"  "Version" : "2012-10-17","#,
                r#"  "Statement" : [ {"#,
                r#"    "Effect" : "Allow","#,
                r#"    "Principal" : "*","#,
                r#"    "Action" : [ ],"#,
                r#"    "Resource" : "*","#,
                r#"    "Condition" : { }"#,
                "  } ]",
                "}",
            ]),
        ),
        (
            // Deeper nesting and non-string scalars: a principal naming two
            // identifiers, a two-clause `Condition`, a boolean and a number.
            "nested-conditions-and-scalars",
            concat!(
                r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","#,
                r#""Principal":{"AWS":["arn:aws:iam::123456789012:root","#,
                r#""arn:aws:iam::210987654321:root"]},"#,
                r#""Action":"secretsmanager:GetSecretValue","Resource":"*","#,
                r#""Condition":{"Bool":{"aws:SecureTransport":true},"#,
                r#""NumericLessThan":{"example:attempts":3}}}]}"#
            )
            .to_string(),
            document_of(&[
                "{",
                r#"  "Version" : "2012-10-17","#,
                r#"  "Statement" : [ {"#,
                r#"    "Effect" : "Allow","#,
                r#"    "Principal" : {"#,
                r#"      "AWS" : [ "arn:aws:iam::123456789012:root", "arn:aws:iam::210987654321:root" ]"#,
                "    },",
                r#"    "Action" : "secretsmanager:GetSecretValue","#,
                r#"    "Resource" : "*","#,
                r#"    "Condition" : {"#,
                r#"      "Bool" : {"#,
                r#"        "aws:SecureTransport" : true"#,
                "      },",
                r#"      "NumericLessThan" : {"#,
                r#"        "example:attempts" : 3"#,
                "      }",
                "    }",
                "  } ]",
                "}",
            ]),
        ),
        (
            // Several statements in the list: the join between two of them is the
            // list's inline separator, so one closes and the next opens on the
            // same line.
            "two-statements",
            concat!(
                r#"{"Version":"2012-10-17","Statement":[{"Sid":"first","Effect":"Allow","#,
                r#""Principal":{"AWS":"arn:aws:iam::123456789012:root"},"#,
                r#""Action":"secretsmanager:GetSecretValue","Resource":"*"},"#,
                r#"{"Sid":"second","Effect":"Allow","#,
                r#""Principal":{"AWS":"arn:aws:iam::210987654321:root"},"#,
                r#""Action":"secretsmanager:GetSecretValue","Resource":"*"}]}"#
            )
            .to_string(),
            document_of(&[
                "{",
                r#"  "Version" : "2012-10-17","#,
                r#"  "Statement" : [ {"#,
                r#"    "Sid" : "first","#,
                r#"    "Effect" : "Allow","#,
                r#"    "Principal" : {"#,
                r#"      "AWS" : "arn:aws:iam::123456789012:root""#,
                "    },",
                r#"    "Action" : "secretsmanager:GetSecretValue","#,
                r#"    "Resource" : "*""#,
                r#"  }, {"#,
                r#"    "Sid" : "second","#,
                r#"    "Effect" : "Allow","#,
                r#"    "Principal" : {"#,
                r#"      "AWS" : "arn:aws:iam::210987654321:root""#,
                "    },",
                r#"    "Action" : "secretsmanager:GetSecretValue","#,
                r#"    "Resource" : "*""#,
                "  } ]",
                "}",
            ]),
        ),
        (
            // A string literal keeps the characters it was written with: the
            // escaped quote, backslash and tab stay escaped and the non-ASCII
            // letters stay as themselves.
            "escaped-and-non-ascii-characters",
            concat!(
                r#"{"Version":"2012-10-17","Statement":[{"Sid":"l\"äsare\\☃\t","Effect":"Allow","#,
                r#""Principal":{"AWS":"arn:aws:iam::123456789012:root"},"#,
                r#""Action":"secretsmanager:GetSecretValue","Resource":"*"}]}"#
            )
            .to_string(),
            document_of(&[
                "{",
                r#"  "Version" : "2012-10-17","#,
                r#"  "Statement" : [ {"#,
                r#"    "Sid" : "l\"äsare\\☃\t","#,
                r#"    "Effect" : "Allow","#,
                r#"    "Principal" : {"#,
                r#"      "AWS" : "arn:aws:iam::123456789012:root""#,
                "    },",
                r#"    "Action" : "secretsmanager:GetSecretValue","#,
                r#"    "Resource" : "*""#,
                "  } ]",
                "}",
            ]),
        ),
    ];

    for (case, submitted, expected) in rows {
        let router = emulator();
        let name = format!("policy-layout-{case}");
        create(&router, &name, case).await;

        let reported = attach_then_read(&router, &name, &submitted, case).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the reported document\n{expected}\ngot\n{reported}"
        );
        // Re-attaching what was read reports the same text again: the layout is a
        // fixed point, whichever layout the first attach used.
        let again = attach_then_read(&router, &name, &reported, case).await;
        assert_eq!(
            again, expected,
            "case {case:?}: re-attaching the reported document must report it \
             unchanged\n{expected}\ngot\n{again}"
        );
    }
}

/// The reported layout is a property of the document, not of the request: the same
/// policy submitted in every whitespacing a renderer produces reads back as the
/// very same bytes, which denote the policy that was submitted.
#[tokio::test]
async fn the_reported_layout_does_not_depend_on_the_submitted_whitespace() {
    // The policies span the class the rule generalises over: one statement, a
    // single statement object, a statement with nested conditions, and a
    // many-statement document of the kind a widely shared secret carries. None
    // writes a value list of exactly one value, which is reported in its string
    // form — the table above is where that form is pinned — so each row's reported
    // document denotes the very policy that was submitted.
    let policies: Vec<(&str, Value)> = vec![
        (
            "one-statement",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "reader",
                    "Effect": "Allow",
                    "Principal": {"AWS": "arn:aws:iam::123456789012:root"},
                    "Action": ["secretsmanager:GetSecretValue", "secretsmanager:DescribeSecret"],
                    "Resource": "*"
                }]
            }),
        ),
        (
            "statement-object",
            json!({
                "Version": "2012-10-17",
                "Statement": {
                    "Effect": "Allow",
                    "Principal": {"AWS": "arn:aws:iam::123456789012:root"},
                    "Action": "secretsmanager:GetSecretValue",
                    "Resource": "*"
                }
            }),
        ),
        (
            "nested-conditions",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {
                        "AWS": [
                            "arn:aws:iam::123456789012:root",
                            "arn:aws:iam::210987654321:root"
                        ]
                    },
                    "Action": "secretsmanager:GetSecretValue",
                    "Resource": "*",
                    "Condition": {
                        "StringEquals": {"aws:PrincipalOrgID": ["o-one", "o-two"]},
                        "Bool": {"aws:SecureTransport": true}
                    }
                }]
            }),
        ),
        ("many-statements", many_statement_policy(24)),
    ];

    for (case, policy) in policies {
        // The whitespacings a caller's renderer produces: compact, two-space
        // pretty, four-space pretty and tab-indented.
        let compact = serde_json::to_string(&policy).expect("the policy is serializable");
        let two_space = serde_json::to_string_pretty(&policy).expect("the policy is serializable");
        let four_space = pretty_with_indent(&policy, b"    ");
        let tabbed = pretty_with_indent(&policy, b"\t");

        let router = emulator();
        let name = format!("policy-whitespace-{case}");
        create(&router, &name, case).await;

        let mut reported: Option<String> = None;
        for (form, document) in [
            ("compact", &compact),
            ("two-space", &two_space),
            ("four-space", &four_space),
            ("tabbed", &tabbed),
        ] {
            let row = format!("{case}/{form}");
            let answer = attach_then_read(&router, &name, document, &row).await;
            let content: Value =
                serde_json::from_str(&answer).expect("the reported document is JSON");
            assert_eq!(
                content, policy,
                "case {row:?}: the reported document must denote the submitted policy, got \
                 {answer}"
            );
            match &reported {
                None => reported = Some(answer),
                Some(first) => assert_eq!(
                    &answer, first,
                    "case {row:?}: expected the same reported bytes as the first form\n{first}\n\
                     got\n{answer}"
                ),
            }
        }

        // Putting back exactly what was read reports exactly the same document.
        let first = reported.expect("the loop above read the policy at least once");
        let round_tripped =
            attach_then_read(&router, &name, &first, &format!("{case}/reported")).await;
        assert_eq!(
            round_tripped, first,
            "case {case:?}: re-attaching the reported document must report it unchanged\n{first}\n\
             got\n{round_tripped}"
        );
    }
}

/// A policy carrying `statements` statements, the kind of document a secret shared
/// with many principals carries.
fn many_statement_policy(statements: usize) -> Value {
    let rendered: Vec<Value> = (0..statements)
        .map(|index| {
            let account = 100_000_000_000u64 + index as u64;
            json!({
                "Sid": format!("AllowAccountRootStatement{index}"),
                "Effect": "Allow",
                "Principal": {"AWS": format!("arn:aws:iam::{account:012}:root")},
                "Action": ["secretsmanager:GetSecretValue", "secretsmanager:DescribeSecret"],
                "Resource": "*",
                "Condition": {"StringEquals": {"aws:PrincipalOrgID": format!("o-{index}")}}
            })
        })
        .collect();
    json!({"Version": "2012-10-17", "Statement": rendered})
}

/// `policy` rendered with `indent` per level, which is one of the whitespacings a
/// caller's renderer produces.
fn pretty_with_indent(policy: &Value, indent: &[u8]) -> String {
    let mut rendered: Vec<u8> = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(indent);
    let mut serializer = serde_json::Serializer::with_formatter(&mut rendered, formatter);
    serde::Serialize::serialize(policy, &mut serializer).expect("the policy is serializable");
    String::from_utf8(rendered).expect("serde_json writes UTF-8")
}
