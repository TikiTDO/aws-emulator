// SPDX-License-Identifier: CC0-1.0

//! Changing and removing a State Manager association, end to end: `UpdateAssociation`
//! appending a new version to the association an account already holds, and
//! `DeleteAssociation` taking it out of the store.
//!
//! RULE: an association is identified for its whole life by the `AssociationId` its create
//! minted, so every later call addresses it by that id — an update applies a new version of
//! THAT association, built from what the call supplied and nothing else ("when you update an
//! association, the system removes all optional parameters from the request and overwrites the
//! association with null values for those parameters"), and a delete removes it, after which
//! no read reports it.
//!
//! Every case goes through the front door a client uses, over the dispatch
//! [`crate::state_manager_associations`] publishes, so the two suites share one calling
//! convention and one reading of the store.
//!
//! The suites are tables. Each row is one input along the axis the rule generalises over —
//! which members an update restates, which it drops, which document and version it names, how
//! the association being deleted was targeted, and which handle the delete addresses it by.

use serde_json::{json, Value};

use crate::state_manager_associations::{
    call, create, created, described, error_code, service, stored_associations, without_overview,
    AGENT, NODE, OTHER_NODE, REGION, SHELL,
};

/// `UpdateAssociation` with `body`, signed for [`REGION`].
async fn update(router: &smithy_server_runtime::MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "UpdateAssociation", body).await
}

/// `DeleteAssociation` with `body`, signed for [`REGION`].
async fn delete(router: &smithy_server_runtime::MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "DeleteAssociation", body).await
}

/// The `AssociationDescription` of an update that must have succeeded.
async fn updated(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    body: &Value,
) -> Value {
    let (status, parsed) = update(router, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from UpdateAssociation, got {status} with {parsed:?}"
    );
    parsed["AssociationDescription"].clone()
}

/// One update of one association, and what the association then reports.
struct UpdateRow {
    case: &'static str,
    /// The create the association comes from, beside its `Name` and `InstanceId`.
    created_with: Value,
    /// The members the update supplies, beside the `AssociationId`.
    update_with: Value,
    /// `(member, value)` pairs the updated association reports.
    reported: Vec<(&'static str, Value)>,
    /// The members the updated association reports nothing for, because this update did not
    /// supply them — whether or not the create did.
    unset: Vec<&'static str>,
}

fn update_rows() -> Vec<UpdateRow> {
    vec![
        UpdateRow {
            case: "a restated schedule, with the parameters the document requires restated \
                   beside it",
            created_with: json!({
                "ScheduleExpression": "rate(30 minutes)",
                "Parameters": {"commands": ["echo first"]}
            }),
            update_with: json!({
                "ScheduleExpression": "rate(2 hours)",
                "Parameters": {"commands": ["echo second"]}
            }),
            reported: vec![
                ("ScheduleExpression", json!("rate(2 hours)")),
                ("Parameters", json!({"commands": ["echo second"]})),
                ("AssociationVersion", json!("2")),
            ],
            unset: vec!["AssociationName"],
        },
        UpdateRow {
            case: "an optional member the create set and the update does not restate is \
                   reported by neither",
            created_with: json!({
                "AssociationName": "the-name",
                "ComplianceSeverity": "HIGH",
                "MaxConcurrency": "2",
                "Parameters": {"commands": ["echo first"]}
            }),
            update_with: json!({"Parameters": {"commands": ["echo first"]}}),
            reported: vec![
                ("Parameters", json!({"commands": ["echo first"]})),
                ("AssociationVersion", json!("2")),
            ],
            unset: vec![
                "AssociationName",
                "ComplianceSeverity",
                "MaxConcurrency",
                "ScheduleExpression",
            ],
        },
        UpdateRow {
            case: "a restated name, severity and cron schedule, every member carried by the \
                   update alone",
            created_with: json!({"Parameters": {"commands": ["echo first"]}}),
            update_with: json!({
                "AssociationName": "renamed",
                "ComplianceSeverity": "LOW",
                "SyncCompliance": "MANUAL",
                "ScheduleExpression": "cron(0 2 ? * SUN *)",
                "ApplyOnlyAtCronInterval": true,
                "Parameters": {"commands": ["echo first"]}
            }),
            reported: vec![
                ("AssociationName", json!("renamed")),
                ("ComplianceSeverity", json!("LOW")),
                ("SyncCompliance", json!("MANUAL")),
                ("ScheduleExpression", json!("cron(0 2 ? * SUN *)")),
                ("ApplyOnlyAtCronInterval", json!(true)),
                ("AssociationVersion", json!("2")),
            ],
            unset: vec!["MaxErrors"],
        },
        UpdateRow {
            case: "another document, which the association applies from then on",
            created_with: json!({"Parameters": {"commands": ["echo first"]}}),
            update_with: json!({"Name": AGENT, "DocumentVersion": "$DEFAULT"}),
            reported: vec![
                ("Name", json!(AGENT)),
                ("DocumentVersion", json!("$DEFAULT")),
                ("AssociationVersion", json!("2")),
            ],
            unset: vec!["Parameters"],
        },
    ]
}

/// RULE: an update applies a new version to the association its id names — the id and the node
/// it was created for are unchanged, the version number counts up, and the configuration it
/// reports is exactly what the update supplied.
#[tokio::test]
async fn an_update_applies_a_new_version_of_the_association_its_id_names() {
    for row in update_rows() {
        let case = row.case;
        let (router, engine) = service();
        let mut create_body = json!({"Name": SHELL, "InstanceId": NODE});
        merge(&mut create_body, &row.created_with);
        let created = created(&router, case, &create_body).await;
        let association_id = created["AssociationId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            created["AssociationVersion"],
            json!("1"),
            "case {case:?}: expected a create to mint the first version, got {created:?}"
        );

        let mut update_body = json!({"AssociationId": association_id});
        merge(&mut update_body, &row.update_with);
        let applied = updated(&router, case, &update_body).await;
        assert_eq!(
            applied["AssociationId"].as_str(),
            Some(association_id.as_str()),
            "case {case:?}: expected the update to answer the same association, got {applied:?}"
        );
        assert_eq!(
            applied["InstanceId"].as_str(),
            Some(NODE),
            "case {case:?}: expected the node the association was created for to be kept, got \
             {applied:?}"
        );
        for (member, expected) in &row.reported {
            assert_eq!(
                &applied[*member], expected,
                "case {case:?}: expected {member} to be {expected}, got {applied:?}"
            );
        }
        for member in &row.unset {
            assert!(
                applied[*member].is_null(),
                "case {case:?}: expected {member} to be reported by nothing, got {applied:?}"
            );
        }
        // The read reports what the update applied, and the store still holds ONE
        // association: an update changes the association rather than making a second one.
        let read = described(&router, case, &json!({"AssociationId": association_id})).await;
        assert_eq!(
            without_overview(&read),
            without_overview(&applied),
            "case {case:?}: expected the read to report what the update applied"
        );
        assert_eq!(
            stored_associations(&engine),
            1,
            "case {case:?}: expected the update to leave one association"
        );
        // The version before it is still readable by its own number, carrying what the
        // create supplied.
        let first = described(
            &router,
            case,
            &json!({"AssociationId": association_id, "AssociationVersion": "1"}),
        )
        .await;
        assert_eq!(
            first["AssociationVersion"],
            json!("1"),
            "case {case:?}: expected the create's version to be readable, got {first:?}"
        );
    }
}

/// RULE: an update this service cannot carry out is refused with the error that names why, and
/// the association is left exactly as it was.
#[tokio::test]
async fn an_update_it_cannot_carry_out_is_refused_and_changes_nothing() {
    // (case, the members the update carries beside the id, the error code it is refused with)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a version that is not the one in force",
            json!({"AssociationVersion": "1", "Parameters": {"commands": ["echo"]}}),
            "InvalidAssociationVersion",
        ),
        (
            "a document this account holds none of",
            json!({"Name": "AWS-NoSuchDocument"}),
            "InvalidDocument",
        ),
        (
            "a version the document does not hold",
            json!({"DocumentVersion": "7", "Parameters": {"commands": ["echo"]}}),
            "InvalidDocumentVersion",
        ),
        (
            "a schedule in neither form an expression is written in",
            json!({"ScheduleExpression": "every half hour", "Parameters": {"commands": ["echo"]}}),
            "InvalidSchedule",
        ),
        (
            "no value for the parameter the document requires",
            json!({"Parameters": {}}),
            "InvalidParameters",
        ),
        (
            "a parameter the document does not declare",
            json!({"Parameters": {"commands": ["echo"], "nothing": ["x"]}}),
            "InvalidParameters",
        ),
        (
            "an output location naming no bucket",
            json!({
                "OutputLocation": {"S3Location": {"OutputS3KeyPrefix": "logs"}},
                "Parameters": {"commands": ["echo"]}
            }),
            "InvalidOutputLocation",
        ),
    ];
    for (case, members, expected) in rows {
        let (router, _) = service();
        // The association every row updates: created at version 1 and then UPDATED once, so
        // the version in force is 2 and the row that names version 1 names a stale one.
        let created = created(
            &router,
            case,
            &json!({
                "Name": SHELL,
                "InstanceId": NODE,
                "Parameters": {"commands": ["echo first"]}
            }),
        )
        .await;
        let association_id = created["AssociationId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let before = updated(
            &router,
            case,
            &json!({
                "AssociationId": association_id,
                "Parameters": {"commands": ["echo second"]}
            }),
        )
        .await;

        let mut body = json!({"AssociationId": association_id});
        merge(&mut body, &members);
        let (status, parsed) = update(&router, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from UpdateAssociation, got {status} with \
             {parsed:?}"
        );
        let code = error_code(&parsed);
        assert_eq!(
            code, expected,
            "case {case:?}: expected {expected}, got {code} with {parsed:?}"
        );
        let read = described(&router, case, &json!({"AssociationId": association_id})).await;
        assert_eq!(
            without_overview(&read),
            without_overview(&before),
            "case {case:?}: expected the refused update to change nothing"
        );
    }
}

/// RULE: an update naming an association this Region holds none of is refused
/// `AssociationDoesNotExist`, whatever else the call carries.
#[tokio::test]
async fn an_update_of_an_association_nothing_holds_is_refused() {
    let (router, _) = service();
    let (status, parsed) = update(
        &router,
        &json!({
            "AssociationId": "00000000-0000-4000-8000-000000000000",
            "Parameters": {"commands": ["echo"]}
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"an id nothing holds\": expected HTTP 400, got {status} with {parsed:?}"
    );
    let code = error_code(&parsed);
    assert_eq!(
        code, "AssociationDoesNotExist",
        "case \"an id nothing holds\": expected AssociationDoesNotExist, got {code} with \
         {parsed:?}"
    );
}

/// How a row addresses the association it deletes: the handle the delete carries, built from
/// the id the create minted.
type Handle = fn(&str) -> Value;

/// RULE: a delete removes the association its handle names and nothing else, after which no
/// read reports it.
#[tokio::test]
async fn a_delete_removes_the_association_its_handle_names() {
    // (case, the create of the association to delete, the delete's own handle)
    let rows: Vec<(&str, Value, Handle)> = vec![
        (
            "by the id the create minted, for an association paired with one node",
            json!({"Name": AGENT, "InstanceId": NODE}),
            |association_id| json!({"AssociationId": association_id}),
        ),
        (
            "by the id the create minted, for an association that names TARGETS",
            json!({
                "Name": AGENT,
                "Targets": [{"Key": "InstanceIds", "Values": [NODE]}]
            }),
            |association_id| json!({"AssociationId": association_id}),
        ),
        (
            "by the document and the node, the pair a create named through InstanceId",
            json!({"Name": AGENT, "InstanceId": NODE}),
            |_| json!({"Name": AGENT, "InstanceId": NODE}),
        ),
    ];
    for (case, create_body, handle) in rows {
        let (router, engine) = service();
        // A second association, of another document on another node, which every row must
        // leave exactly as it was: a delete removes ONE association.
        let other = created(
            &router,
            case,
            &json!({
                "Name": SHELL,
                "InstanceId": OTHER_NODE,
                "Parameters": {"commands": ["echo other"]}
            }),
        )
        .await;
        let other_id = other["AssociationId"].as_str().unwrap_or_default().to_string();
        let created = created(&router, case, &create_body).await;
        let association_id = created["AssociationId"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let (status, parsed) = delete(&router, &handle(&association_id)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteAssociation, got {status} with \
             {parsed:?}"
        );
        let (status, parsed) = call(
            &router,
            "DescribeAssociation",
            &json!({"AssociationId": association_id}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the deleted association to be gone, got {status} with \
             {parsed:?}"
        );
        let code = error_code(&parsed);
        assert_eq!(
            code, "AssociationDoesNotExist",
            "case {case:?}: expected AssociationDoesNotExist, got {code} with {parsed:?}"
        );
        assert_eq!(
            stored_associations(&engine),
            1,
            "case {case:?}: expected the delete to leave the other association alone"
        );
        let held = described(&router, case, &json!({"AssociationId": other_id})).await;
        assert_eq!(
            held["Name"].as_str(),
            Some(SHELL),
            "case {case:?}: expected the other association to be untouched, got {held:?}"
        );
    }
}

/// RULE: a delete that names no association this Region holds is refused
/// `AssociationDoesNotExist`, and one that identifies no association at all is a malformed
/// request rather than a miss.
#[tokio::test]
async fn a_delete_naming_no_association_is_refused() {
    // (case, the delete's members, the error code it is refused with)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "an id nothing holds",
            json!({"AssociationId": "00000000-0000-4000-8000-000000000000"}),
            "AssociationDoesNotExist",
        ),
        (
            "a document and a node nothing pairs",
            json!({"Name": AGENT, "InstanceId": OTHER_NODE}),
            "AssociationDoesNotExist",
        ),
        (
            "a document with no node beside it",
            json!({"Name": AGENT}),
            "ValidationException",
        ),
        ("no handle at all", json!({}), "ValidationException"),
    ];
    for (case, body, expected) in rows {
        let (router, engine) = service();
        let created = created(&router, case, &json!({"Name": AGENT, "InstanceId": NODE})).await;
        assert!(
            created["AssociationId"].is_string(),
            "case {case:?}: expected the create to mint an id, got {created:?}"
        );
        let (status, parsed) = delete(&router, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from DeleteAssociation, got {status} with \
             {parsed:?}"
        );
        let code = error_code(&parsed);
        assert_eq!(
            code, expected,
            "case {case:?}: expected {expected}, got {code} with {parsed:?}"
        );
        assert_eq!(
            stored_associations(&engine),
            1,
            "case {case:?}: expected the refused delete to remove nothing"
        );
    }
}

/// RULE: a create whose refusal this suite does not cover still stands alone — the create of
/// an association and the delete of it are reachable in the Region the request was signed for
/// and nowhere else, which is what the id is scoped by.
#[tokio::test]
async fn a_delete_reaches_only_the_region_the_association_was_created_in() {
    let (router, engine) = service();
    let created = created(&router, "the create", &json!({"Name": AGENT, "InstanceId": NODE})).await;
    let association_id = created["AssociationId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let case = "a delete signed for another Region";
    let (status, parsed) = crate::state_manager_associations::call_in_region(
        &router,
        "eu-west-1",
        "DeleteAssociation",
        &json!({"AssociationId": association_id}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {parsed:?}"
    );
    let code = error_code(&parsed);
    assert_eq!(
        code, "AssociationDoesNotExist",
        "case {case:?}: expected AssociationDoesNotExist, got {code} with {parsed:?}"
    );
    assert_eq!(
        stored_associations(&engine),
        1,
        "case {case:?}: expected the association to still be held in {REGION}"
    );
    let (status, parsed) = create(&router, &json!({"Name": AGENT, "InstanceId": NODE})).await;
    assert_eq!(
        status, 400,
        "case \"a second create of the same pair\": expected HTTP 400, got {status} with \
         {parsed:?}"
    );
}

/// Merge the members of `extra` onto the object `body`.
fn merge(body: &mut Value, extra: &Value) {
    let (Some(target), Some(source)) = (body.as_object_mut(), extra.as_object()) else {
        return;
    };
    for (member, value) in source {
        target.insert(member.clone(), value.clone());
    }
}
