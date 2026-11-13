// SPDX-License-Identifier: CC0-1.0

//! The DIGEST a revision that carries its content inline reports, end to end over
//! the wire.
//!
//! RULE under test: both inline revision forms carry the digest beside the content
//! — `AppSpecContent` has `content` and `sha256`, "the SHA256 hash value of the
//! revision content" (`smithy_query(query="shape",shape="AppSpecContent")`), and the
//! `string` form's `RawString` the same pair — so CodeDeploy computes that digest
//! when the revision is recorded and reports it on EVERY read of that revision.
//! Two revisions carrying the same content therefore report the same digest, and
//! two carrying different content never do, whichever reader answers.
//!
//! Every digest expected below is computed in the suite the way a client computes
//! one: the hex SHA-256 of the content bytes it sent.

use serde_json::{json, Value};

use crate::codedeploy_lifecycle::{s3_revision, Emulator};

/// The service role every deployment group here is created with; what it grants is
/// another suite's subject.
const SERVICE_ROLE: &str = "arn:aws:iam::123456789012:role/CodeDeployRole";

/// The hex SHA-256 of `content`, as a client computes the digest of the revision it
/// sent.
fn digest_of(content: &str) -> String {
    use ::sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    ::hex::encode(hasher.finalize())
}

/// An `appSpecContent` revision carrying `content`.
fn app_spec_revision(content: &str) -> Value {
    json!({"revisionType": "AppSpecContent", "appSpecContent": {"content": content}})
}

/// A `string` revision carrying `content`: "the location of an Lambda deployment
/// revision stored as a RawString".
fn raw_string_revision(content: &str) -> Value {
    json!({"revisionType": "String", "string": {"content": content}})
}

/// The AppSpec of a Lambda deployment shifting an alias between two versions, which
/// is what makes two revisions of one application differ by content.
fn app_spec_yaml(function: &str, current: &str, target: &str) -> String {
    format!(
        "version: 0.0\n\
         Resources:\n\
         \x20 - {function}:\n\
         \x20     Type: AWS::Lambda::Function\n\
         \x20     Properties:\n\
         \x20       Name: {function}\n\
         \x20       Alias: live\n\
         \x20       CurrentVersion: \"{current}\"\n\
         \x20       TargetVersion: \"{target}\"\n"
    )
}

/// Create a Lambda application with one deployment group on it, answering the group
/// name.
async fn given_group(emulator: &Emulator, application: &str) -> String {
    emulator
        .ok(
            "CreateApplication",
            json!({"applicationName": application, "computePlatform": "Lambda"}),
        )
        .await;
    let group = format!("{application}-dg");
    emulator
        .ok(
            "CreateDeploymentGroup",
            json!({
                "applicationName": application,
                "deploymentGroupName": group,
                "deploymentConfigName": "CodeDeployDefault.LambdaAllAtOnce",
                "serviceRoleArn": SERVICE_ROLE,
            }),
        )
        .await;
    group
}

/// Create a deployment of `revision` and answer its minted id.
async fn deploy(emulator: &Emulator, application: &str, group: &str, revision: Value) -> String {
    let created = emulator
        .ok(
            "CreateDeployment",
            json!({
                "applicationName": application,
                "deploymentGroupName": group,
                "revision": revision,
            }),
        )
        .await;
    created["deploymentId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateDeployment answered no deploymentId: {created:?}"))
        .to_string()
}

/// The digest one reader reports for the inline content of `revision`.
fn reported_digest(revision: &Value) -> Option<String> {
    for member in ["appSpecContent", "string"] {
        if let Some(digest) = revision[member]["sha256"].as_str() {
            return Some(digest.to_string());
        }
    }
    None
}

/// RULE: every read of a deployment's inline revision reports the SHA-256 of the
/// content that deployment was created with.
///
/// Rows differ along what the rule generalises over: the inline FORM the revision
/// takes, and the content it carries — a forward shift, its exact reverse (the same
/// members, different values), a JSON spelling, content differing by one character,
/// and empty content.
#[tokio::test]
async fn a_deployments_inline_revision_reports_the_digest_of_its_content() {
    let emulator = Emulator::new();

    // (case, the revision sent, the content whose digest it must report)
    let forward = app_spec_yaml("orders-fn", "1", "2");
    let reverse = app_spec_yaml("orders-fn", "2", "1");
    let padded = format!("{forward} ");
    let json_spelling = json!({
        "version": 0.0,
        "Resources": [{"orders-fn": {"Type": "AWS::Lambda::Function"}}],
    })
    .to_string();
    let cases: &[(&str, Value, &str)] = &[
        (
            "an AppSpecContent revision shifting 1 -> 2",
            app_spec_revision(&forward),
            &forward,
        ),
        (
            "the reverse shift, 2 -> 1",
            app_spec_revision(&reverse),
            &reverse,
        ),
        (
            "the same AppSpec written as JSON",
            app_spec_revision(&json_spelling),
            &json_spelling,
        ),
        (
            "content differing from the forward shift by one character",
            app_spec_revision(&padded),
            &padded,
        ),
        (
            "a string revision carrying the forward shift",
            raw_string_revision(&forward),
            &forward,
        ),
        (
            "a string revision carrying empty content",
            raw_string_revision(""),
            "",
        ),
    ];

    let mut seen: Vec<(&str, &str, String)> = Vec::new();
    for (index, (case, revision, content)) in cases.iter().enumerate() {
        // One deployment group per row: a group runs one deployment at a time, so
        // the rows are not a sequence of deployments to one group.
        let application = format!("digest-app-{index}");
        let group = given_group(&emulator, &application).await;
        let expected = digest_of(content);
        let deployment_id = deploy(&emulator, &application, &group, revision.clone()).await;
        // Every reader of a deployment's revision answers the same digest.
        let read = emulator
            .ok("GetDeployment", json!({"deploymentId": &deployment_id}))
            .await;
        let batch = emulator
            .ok(
                "BatchGetDeployments",
                json!({"deploymentIds": [&deployment_id]}),
            )
            .await;
        let group_read = emulator
            .ok(
                "GetDeploymentGroup",
                json!({"applicationName": &application, "deploymentGroupName": &group}),
            )
            .await;
        for (reader, revision) in [
            ("GetDeployment", &read["deploymentInfo"]["revision"]),
            (
                "BatchGetDeployments",
                &batch["deploymentsInfo"][0]["revision"],
            ),
            (
                "GetDeploymentGroup.targetRevision",
                &group_read["deploymentGroupInfo"]["targetRevision"],
            ),
        ] {
            let answered = reported_digest(revision);
            assert_eq!(
                answered.as_deref(),
                Some(expected.as_str()),
                "case {case:?}: {reader} must report the digest of the content, expected \
                 {expected:?}, got {answered:?} in {revision:?}"
            );
        }
        seen.push((case, content, expected));
    }

    // Different content, different digest; the same content, the same digest —
    // whichever inline form carried it.
    for (left_case, left_content, left) in &seen {
        for (right_case, right_content, right) in &seen {
            if left_content == right_content {
                assert_eq!(
                    left, right,
                    "case {left_case:?} vs {right_case:?}: one content has one digest, got \
                     {left:?} and {right:?}"
                );
                continue;
            }
            assert_ne!(
                left, right,
                "case {left_case:?} vs {right_case:?}: two different AppSpecs must not report \
                 one digest, both reported {left:?}"
            );
        }
    }
}

/// RULE: an application's REGISTRY reports the same digest for the same revision —
/// the one a `RegisterApplicationRevision` recorded, read back through
/// `GetApplicationRevision` and `BatchGetApplicationRevisions`.
#[tokio::test]
async fn a_registered_inline_revision_reads_back_with_its_digest() {
    let emulator = Emulator::new();
    let application = "digest-registry";
    given_group(&emulator, application).await;

    // Rows differ along the inline form and its content, plus the S3 form, which
    // names an artifact elsewhere and so carries no inline digest at all.
    let content = app_spec_yaml("registry-fn", "3", "4");
    let cases: &[(&str, Value, Option<String>)] = &[
        (
            "an AppSpecContent revision",
            app_spec_revision(&content),
            Some(digest_of(&content)),
        ),
        (
            "a string revision carrying the same content",
            raw_string_revision(&content),
            Some(digest_of(&content)),
        ),
        (
            "an S3 revision, which carries no inline content",
            s3_revision("artifacts", "app.zip"),
            None,
        ),
    ];

    for (case, revision, expected) in cases {
        emulator
            .ok(
                "RegisterApplicationRevision",
                json!({"applicationName": application, "revision": revision}),
            )
            .await;
        let read = emulator
            .ok(
                "GetApplicationRevision",
                json!({"applicationName": application, "revision": revision}),
            )
            .await;
        let batch = emulator
            .ok(
                "BatchGetApplicationRevisions",
                json!({"applicationName": application, "revisions": [revision]}),
            )
            .await;
        for (reader, answered) in [
            ("GetApplicationRevision", &read["revision"]),
            (
                "BatchGetApplicationRevisions",
                &batch["revisions"][0]["revisionLocation"],
            ),
        ] {
            let digest = reported_digest(answered);
            assert_eq!(
                &digest, expected,
                "case {case:?}: {reader} must report {expected:?}, got {digest:?} in \
                 {answered:?}"
            );
        }
    }
}
