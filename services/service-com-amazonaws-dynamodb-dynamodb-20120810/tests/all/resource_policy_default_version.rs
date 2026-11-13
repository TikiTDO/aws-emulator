// SPDX-License-Identifier: CC0-1.0

//! The language VERSION the resource-based policy a resource carries is stored under.
//!
//! RULE under test: **a policy document the service stores states the version of the policy
//! language it is read under — "if you do not include a Version element, the value defaults
//! to 2008-10-17" (the IAM policy grammar reference), so a document that states none is
//! stored with that version, as the first element of the document, while a document that
//! states one keeps it exactly as written.** Every other element the request stated is
//! stored as it arrived, so the version is the ONE thing a stored document gains.
//!
//! The rows span what the rule generalises over: whether the document states a version and
//! WHICH version it states (both of the two the grammar defines, including the default
//! stated explicitly), the shape of the document around it (a statement list, a statement
//! written as one object, two statements, a document carrying an `Id`), and the element
//! order the request wrote. The version catalogue is the grammar reference's own, and the
//! rows include the version the recorded case did not state.
//!
//! The attach path is exercised from BOTH calls that attach a document — `PutResourcePolicy`
//! on a table that exists, and the `ResourcePolicy` a `CreateTable` arrives with — because
//! the rule is about the document, not about which call carried it.
//!
//! Driven over the wire (`POST /` with `X-Amz-Target: DynamoDB_20120810.<Operation>`)
//! through the same `MultiServiceRouter` the emulator's front doors use, reusing the harness
//! in [`super::table_items`].

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::table_items::{call, create_active_table, emulator, ok};

/// The versions the IAM policy grammar defines, and the one a document stating none is read
/// under: "if you do not include a Version element, the value defaults to 2008-10-17" (the
/// IAM policy grammar reference).
const DEFAULT_VERSION: &str = "2008-10-17";
const CURRENT_VERSION: &str = "2012-10-17";

/// How long a case waits for an attached policy to become readable, and how often it looks.
///
/// "`PutResourcePolicy` is an asynchronous operation", so a read made straight after the
/// write may still report the revision the resource carried before it; the case waits for
/// the REVISION the write reported rather than for a span, and the deadline is far longer
/// than the lag so a loaded host cannot turn the wait into a failure.
const POLICY_READ_DEADLINE: Duration = Duration::from_secs(60);
const POLICY_READ_INTERVAL: Duration = Duration::from_millis(50);

/// The ARN DynamoDB reports for the table it holds under `name`, which is the
/// `ResourceArn` a policy call addresses it by.
async fn table_arn(router: &MultiServiceRouter, name: &str) -> String {
    let described = ok(router, "DescribeTable", json!({"TableName": name})).await;
    described["Table"]["TableArn"]
        .as_str()
        .unwrap_or_else(|| panic!("expected DynamoDB to report an ARN for {name}, got {described}"))
        .to_string()
}

/// The ARN the table `name` WOULD be held under, derived from the ARN DynamoDB reported for
/// another table of the same endpoint: the ARNs of one endpoint's tables differ only in the
/// resource name after `table/`.
///
/// What this is for is a document that has to name the table its own CREATE is about, before
/// that create has answered an ARN of its own.
fn sibling_arn(reference_arn: &str, name: &str) -> String {
    match reference_arn.rsplit_once('/') {
        Some((head, _)) => format!("{head}/{name}"),
        None => name.to_string(),
    }
}

/// One `Allow` statement naming the account's root principal: a grant, so the document never
/// takes the caller's own policy-management access away and needs no confirmation.
///
/// The account is the one the table's own ARN carries, so nothing of the emulator's ARN shape
/// is written here.
fn allow(action: &str, resource_arn: &str) -> Value {
    let account = resource_arn.split(':').nth(4).unwrap_or_default();
    json!({
        "Effect": "Allow",
        "Principal": {"AWS": format!("arn:aws:iam::{account}:root")},
        "Action": action,
        "Resource": resource_arn,
    })
}

/// One row: the document a request states, and the version the stored document must state.
struct Row {
    case: &'static str,
    /// The document as the request writes it.
    stated: Value,
    /// The version the stored document states.
    stored_version: &'static str,
}

fn rows(resource_arn: &str) -> Vec<Row> {
    vec![
        Row {
            case: "a document that states no Version",
            stated: json!({"Statement": [allow("dynamodb:GetItem", resource_arn)]}),
            stored_version: DEFAULT_VERSION,
        },
        Row {
            case: "no Version, with the statement written as one object",
            stated: json!({"Statement": allow("dynamodb:PutItem", resource_arn)}),
            stored_version: DEFAULT_VERSION,
        },
        Row {
            case: "no Version, with two statements and an Id",
            stated: json!({
                "Id": "two-statements",
                "Statement": [
                    allow("dynamodb:Query", resource_arn),
                    allow("dynamodb:Scan", resource_arn),
                ],
            }),
            stored_version: DEFAULT_VERSION,
        },
        Row {
            case: "a document that states the current version",
            stated: json!({
                "Version": CURRENT_VERSION,
                "Statement": [allow("dynamodb:DeleteItem", resource_arn)],
            }),
            stored_version: CURRENT_VERSION,
        },
        Row {
            case: "a document that states the default version itself",
            stated: json!({
                "Version": DEFAULT_VERSION,
                "Statement": [allow("dynamodb:UpdateItem", resource_arn)],
            }),
            stored_version: DEFAULT_VERSION,
        },
        Row {
            case: "a document that writes Version after its Statement",
            stated: json!({
                "Statement": [allow("dynamodb:BatchGetItem", resource_arn)],
                "Version": CURRENT_VERSION,
            }),
            stored_version: CURRENT_VERSION,
        },
    ]
}

/// The policy the resource carries once the revision `expected_revision_id` names is the one
/// a read reports: the document and that revision id.
///
/// The wait is on the CONDITION — the revision the write reported being the readable one —
/// and gives up only past [`POLICY_READ_DEADLINE`]. A read made inside the write's own span
/// reports the revision the resource carried before it, or refuses where it carried none at
/// all ("or return the `PolicyNotFoundException`" — the `PutResourcePolicy` doc), so neither
/// answer ends the wait.
async fn policy_once_applied(
    case: &str,
    router: &MultiServiceRouter,
    resource_arn: &str,
    expected_revision_id: &str,
) -> Value {
    let deadline = Instant::now() + POLICY_READ_DEADLINE;
    loop {
        let (status, read) = call(
            router,
            "GetResourcePolicy",
            &json!({"ResourceArn": resource_arn}),
        )
        .await;
        if status == 200 && read["RevisionId"] == json!(expected_revision_id) {
            return read;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the policy at revision {expected_revision_id:?} to \
             become readable, last read {status} {read}"
        );
        tokio::time::sleep(POLICY_READ_INTERVAL).await;
    }
}

/// The stored document a read reports, parsed.
fn stored_document(case: &str, read: &Value) -> Value {
    let text = read["Policy"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Policy document, got {read}"));
    serde_json::from_str(text)
        .unwrap_or_else(|_| panic!("case {case:?}: expected a JSON document, got {text:?}"))
}

/// Hold one stored document to the rule: the version it states, the elements it keeps, and
/// where the version sits.
fn assert_stored(case: &str, stated: &Value, stored: &Value, stored_version: &str) {
    assert_eq!(
        stored["Version"],
        json!(stored_version),
        "case {case:?}: expected the stored document to state Version {stored_version:?}, \
         got {stored}"
    );
    assert_eq!(
        stored["Statement"], stated["Statement"],
        "case {case:?}: expected the statements to be stored as they arrived, got {stored}"
    );
    assert_eq!(
        stored.get("Id"),
        stated.get("Id"),
        "case {case:?}: expected the document's other elements to be stored as they \
         arrived, got {stored}"
    );
    // The grammar states `Version` ahead of `Id` and `Statement`, and a document that
    // stated one keeps the place it wrote it in.
    let elements: Vec<&String> = stored
        .as_object()
        .map(|members| members.keys().collect())
        .unwrap_or_default();
    let expected_first = match stated.get("Version") {
        Some(_) => stated
            .as_object()
            .and_then(|members| members.keys().next())
            .cloned()
            .unwrap_or_default(),
        None => "Version".to_string(),
    };
    assert_eq!(
        elements.first().map(|element| element.as_str()),
        Some(expected_first.as_str()),
        "case {case:?}: expected {expected_first:?} first in the stored document, got \
         {elements:?}"
    );
}

#[tokio::test]
async fn a_put_policy_is_stored_under_the_version_it_is_read_under() {
    let router = emulator();
    let table = "PolicyVersionDefaults";
    create_active_table(&router, table, &[("pk", "HASH", "S")]).await;
    let arn = table_arn(&router, table).await;

    for Row {
        case,
        stated,
        stored_version,
    } in rows(&arn)
    {
        let written = ok(
            &router,
            "PutResourcePolicy",
            json!({"ResourceArn": &arn, "Policy": stated.to_string()}),
        )
        .await;
        let revision_id = written["RevisionId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a RevisionId, got {written}"))
            .to_string();
        let read = policy_once_applied(case, &router, &arn, &revision_id).await;
        let stored = stored_document(case, &read);
        assert_stored(case, &stated, &stored, stored_version);

        // "PutResourcePolicy is an idempotent operation; running it multiple times on the
        // same resource using the same policy document will return the same revision id" —
        // which holds only because the version the document is stored under is decided the
        // same way every time.
        let again = ok(
            &router,
            "PutResourcePolicy",
            json!({"ResourceArn": &arn, "Policy": stated.to_string()}),
        )
        .await;
        assert_eq!(
            again["RevisionId"],
            json!(&revision_id),
            "case {case:?}: expected re-attaching the same document to report the same \
             revision, got {again}"
        );
    }
}

#[tokio::test]
async fn a_policy_a_create_table_arrives_with_is_stored_under_the_same_version() {
    let router = emulator();
    // The ARN shape of this endpoint, as DynamoDB reports it for a table of its own: each
    // document below has to name the table its own create is about, which has answered no
    // ARN yet ([`sibling_arn`]).
    create_active_table(&router, "PolicyArnReference", &[("pk", "HASH", "S")]).await;
    let reference = table_arn(&router, "PolicyArnReference").await;
    // "When you attach a resource-based policy while creating a table, the policy
    // application is strongly consistent" (`CreateTable$ResourcePolicy`), so this document is
    // readable from the create itself and no wait is owed.
    for (case, table, stated, stored_version) in [
        (
            "a create whose document states no Version",
            "CreatedWithBarePolicy",
            json!({"Statement": [allow(
                "dynamodb:GetItem",
                &sibling_arn(&reference, "CreatedWithBarePolicy"),
            )]}),
            DEFAULT_VERSION,
        ),
        (
            "a create whose document states one",
            "CreatedWithVersionedPolicy",
            json!({
                "Version": CURRENT_VERSION,
                "Statement": [allow(
                    "dynamodb:GetItem",
                    &sibling_arn(&reference, "CreatedWithVersionedPolicy"),
                )],
            }),
            CURRENT_VERSION,
        ),
    ] {
        ok(
            &router,
            "CreateTable",
            json!({
                "TableName": table,
                "BillingMode": "PAY_PER_REQUEST",
                "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
                "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
                "ResourcePolicy": stated.to_string(),
            }),
        )
        .await;
        let read = ok(
            &router,
            "GetResourcePolicy",
            json!({"ResourceArn": table_arn(&router, table).await}),
        )
        .await;
        let stored = stored_document(case, &read);
        assert_stored(case, &stated, &stored, stored_version);
    }
}
