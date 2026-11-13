// SPDX-License-Identifier: CC0-1.0

//! The wording of the Clean Rooms "already exists" conflict, for the resources a
//! MEMBERSHIP owns.
//!
//! The rule under test: a create refused because the membership it addresses already
//! holds the resource it would create reports `ConflictException` (HTTP 409) whose
//! message is `The <resource kind> with id: <stored resource id>: with membership id:
//! <membershipIdentifier> already exists.` — fixed text around two variable parts, the
//! identifier the service minted for the resource that is in the way (read from the
//! store, never echoed from the request) and the membership the request addressed.
//!
//! The tables vary what that rule generalises over: which resource the create collides
//! on (an analysis template, a configured table association), the form of the name the
//! create reuses (a word, a name carrying spaces, a name at the published maximum
//! length), and what ELSE the colliding request carries (the same analysis as the stored
//! one, a different analysis, another analysis format, another role) — none of which the
//! sentence depends on. The recorded duplicate-analysis-template case is one row among
//! them. Every request goes over the wire through the same `MultiServiceRouter` the
//! production router uses.

use serde_json::{json, Value};

use super::cleanrooms_analysis_templates::{
    artifacts_source, create_analysis_template, get_analysis_template, membership_id, NAME_MAX_LEN,
};
use super::cleanrooms_collaboration_lifecycle::{
    account_id, error_code, error_message, Emulator,
};
use super::cleanrooms_configured_table::create_body;

/// The sentence AWS reports for an already-exists conflict on a resource a membership
/// owns, as the test's own statement of the rule: the leading `The `, the resource's kind
/// and minted id, the `: with membership id: ` that introduces the membership, and the
/// trailing ` already exists.`.
fn already_exists_message(kind: &str, resource_id: &str, membership: &str) -> String {
    format!("The {kind} with id: {resource_id}: with membership id: {membership} already exists.")
}

/// A `CreateAnalysisTemplate` body.
fn template_body(name: &str, format: &str, source: Value) -> Value {
    json!({ "name": name, "format": format, "source": source })
}

/// A `CreateConfiguredTableAssociation` request, which no other suite's helper on
/// `Emulator` is reachable from here.
async fn create_table_association(
    emulator: &Emulator,
    membership: &str,
    body: Value,
) -> (u16, Value) {
    emulator
        .call(
            "POST",
            &format!("/memberships/{membership}/configuredTableAssociations"),
            Some(body),
        )
        .await
}

/// Assert the refusal `refused` is the already-exists conflict this rule describes, for
/// the stored resource `resource_id` of kind `kind` in membership `membership`.
fn assert_already_exists(
    label: &str,
    status: u16,
    refused: &Value,
    kind: &str,
    resource_id: &str,
    membership: &str,
) {
    assert_eq!(
        status, 409,
        "case {label:?}: expected HTTP 409 for a name the membership already uses, got \
         {status} with {refused}"
    );
    assert_eq!(
        error_code(refused),
        "ConflictException",
        "case {label:?}: expected ConflictException, got {refused}"
    );
    let message = error_message(refused);
    assert_eq!(
        message,
        already_exists_message(kind, resource_id, membership),
        "case {label:?}: expected the already-exists conflict naming the stored {kind}, got \
         {message:?}"
    );
    // The two halves of the template, asserted on their own: a client that matches the
    // refusal reads the leading `The ` and extracts the membership from the tail.
    assert!(
        message.starts_with("The "),
        "case {label:?}: expected the message to open with \"The \", got {message:?}"
    );
    let tail = format!(": with membership id: {membership} already exists.");
    assert!(
        message.ends_with(&tail),
        "case {label:?}: expected the message to end with {tail:?}, got {message:?}"
    );
}

#[tokio::test]
async fn duplicate_analysis_template_conflict_names_the_stored_template_and_the_membership() {
    let emulator = Emulator::new();
    let membership = membership_id(&emulator, "template-conflict-wording").await;

    // Rows differ along the form of the reused name and along what else the colliding
    // create carries (the same analysis, a different one, another format): the sentence
    // names the stored template and the membership, and nothing of the second request.
    let cases: [(&str, String, &str, Value, &str, Value); 4] = [
        (
            "plain name, identical analysis",
            "taken".to_string(),
            "SQL",
            json!({ "text": "SELECT id FROM sales" }),
            "SQL",
            json!({ "text": "SELECT id FROM sales" }),
        ),
        (
            "name with spaces, different analysis",
            "taken name with spaces".to_string(),
            "SQL",
            json!({ "text": "SELECT id FROM sales" }),
            "SQL",
            json!({ "text": "SELECT segment FROM consumers" }),
        ),
        (
            "name at the published maximum length, artifacts analysis",
            "t".repeat(NAME_MAX_LEN),
            "SQL",
            json!({ "text": "SELECT 1 AS one" }),
            "PYSPARK_1_0",
            artifacts_source(),
        ),
        (
            "name with an underscore, PySpark stored first",
            "taken_template".to_string(),
            "PYSPARK_1_0",
            artifacts_source(),
            "SQL",
            json!({ "text": "SELECT id FROM sales" }),
        ),
    ];

    for (label, name, first_format, first_source, second_format, second_source) in cases {
        let (status, created) = create_analysis_template(
            &emulator,
            &membership,
            template_body(&name, first_format, first_source),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the first CreateAnalysisTemplate 200, got {status} with \
             {created}"
        );
        let stored_id = created["analysisTemplate"]["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: no analysisTemplate.id in {created}"))
            .to_string();

        let (status, refused) = create_analysis_template(
            &emulator,
            &membership,
            template_body(&name, second_format, second_source),
        )
        .await;
        assert_already_exists(
            label,
            status,
            &refused,
            "analysis template",
            &stored_id,
            &membership,
        );

        // The id in the sentence is the STORED template's, which is what a read of that
        // template answers with — not a value the refused request carried.
        let (status, fetched) = get_analysis_template(&emulator, &membership, &stored_id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the stored template to survive the refusal, got {status} \
             with {fetched}"
        );
        assert_eq!(
            fetched["analysisTemplate"]["id"].as_str(),
            Some(stored_id.as_str()),
            "case {label:?}: expected the refusal to name the readable template, got {:?}",
            fetched["analysisTemplate"]["id"]
        );
    }
}

#[tokio::test]
async fn duplicate_configured_table_association_conflict_is_worded_the_same_way() {
    let emulator = Emulator::new();
    let membership = membership_id(&emulator, "association-conflict-wording").await;

    let (status, table) = emulator
        .create_configured_table(create_body(
            "association_conflict_table",
            "GLUE",
            "DIRECT_QUERY",
            None,
            2,
        ))
        .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateConfiguredTable 200, got {status} with {table}"
    );
    let table_id = table["configuredTable"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: no configuredTable.id in {table}"))
        .to_string();

    let role = format!("arn:aws:iam::{}:role/clean-rooms", account_id());
    let other_role = format!("arn:aws:iam::{}:role/clean-rooms-other", account_id());

    // Rows differ along the alias form and along the role the colliding create carries:
    // the alias is the uniqueness key, and the sentence names the stored association.
    let cases: [(&str, String, &str); 3] = [
        ("plain alias", "sales".to_string(), role.as_str()),
        (
            "alias with spaces, another role",
            "sales table".to_string(),
            other_role.as_str(),
        ),
        (
            "alias at the published maximum length",
            "a".repeat(NAME_MAX_LEN),
            role.as_str(),
        ),
    ];

    for (label, alias, second_role) in cases {
        let (status, created) = create_table_association(
            &emulator,
            &membership,
            json!({
                "name": alias,
                "configuredTableIdentifier": table_id,
                "roleArn": role,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the first CreateConfiguredTableAssociation 200, got \
             {status} with {created}"
        );
        let stored_id = created["configuredTableAssociation"]["id"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {label:?}: no configuredTableAssociation.id in {created}")
            })
            .to_string();

        let (status, refused) = create_table_association(
            &emulator,
            &membership,
            json!({
                "name": alias,
                "configuredTableIdentifier": table_id,
                "roleArn": second_role,
                "description": "a second association under a taken alias",
            }),
        )
        .await;
        assert_already_exists(
            label,
            status,
            &refused,
            "configured table association",
            &stored_id,
            &membership,
        );
    }
}
