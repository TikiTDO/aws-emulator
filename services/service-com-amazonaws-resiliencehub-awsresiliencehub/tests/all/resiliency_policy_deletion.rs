// SPDX-License-Identifier: CC0-1.0

//! `DeleteResiliencyPolicy`, end to end over HTTP.
//!
//! The rule under test: a resiliency policy is removed from the store only while no
//! stored application still names it in `policyArn` — a delete the reference blocks is
//! refused with the modelled 409 `ConflictException` and changes nothing, and once the
//! last reference is gone the delete removes exactly that one policy, which later
//! reads no longer find.
//!
//! The harness (`emulator`, `post`, `Reply`, `create_a_policy`) is the one
//! `application_lifecycle` already drives every request of this crate's suites
//! through: the real wire path, `MultiServiceRouter::dispatch` with a restJson1
//! request.

use serde_json::{json, Value};

use super::application_lifecycle::{
    create_a_policy, emulator, post, required_objectives, Reply, ACCOUNT, REGION,
};
use smithy_server_runtime::MultiServiceRouter;

/// The widest `message` a `ConflictException` carries: the member targets
/// `String500` (`smithy_query query="shape" shape="ConflictException"`).
const MAX_MESSAGE_LENGTH: usize = 500;

/// How a row releases one application's reference to the policy under test. Each is a
/// different way the stored `policyArn` stops pointing at it, so the rule is asserted
/// on the reference itself and not on one operation's side effect.
#[derive(Debug, Clone, Copy)]
enum Release {
    /// `DeleteApp` — the referencing application goes away with its reference.
    DeleteTheApp,
    /// `UpdateApp` with another `policyArn` — the application stays, pointing elsewhere.
    RepointTheApp,
    /// `UpdateApp` with `clearResiliencyPolicyArn` — the application stays with no policy.
    ClearTheReference,
}

/// Create an application named `name`, optionally pointing at `policy_arn`, and
/// answer its ARN.
async fn create_an_app(
    router: &MultiServiceRouter,
    name: &str,
    policy_arn: Option<&str>,
) -> String {
    let body = match policy_arn {
        Some(policy_arn) => json!({"name": name, "policyArn": policy_arn}),
        None => json!({"name": name}),
    };
    let reply = post(router, "/create-app", &body).await;
    reply.ok(name)["app"]["appArn"]
        .as_str()
        .expect("a minted appArn")
        .to_string()
}

async fn delete_policy(router: &MultiServiceRouter, policy_arn: &str) -> Reply {
    post(
        router,
        "/delete-resiliency-policy",
        &json!({"policyArn": policy_arn}),
    )
    .await
}

async fn describe_policy(router: &MultiServiceRouter, policy_arn: &str) -> Reply {
    post(
        router,
        "/describe-resiliency-policy",
        &json!({"policyArn": policy_arn}),
    )
    .await
}

/// Assert the delete was refused as a conflict naming the policy and every
/// application that still references it, and that the policy is still readable with
/// the name it was created under.
///
/// `referencing` is every application holding the reference, as the `(name, ARN)`
/// pair the refusal names it by.
async fn assert_blocked(
    router: &MultiServiceRouter,
    case: &str,
    policy_arn: &str,
    name: &str,
    referencing: &[(String, String)],
) {
    let refused = delete_policy(router, policy_arn).await;
    refused.refused(case, 409, "ConflictException");
    // The refusal carries the human-readable NAME of the policy and of each
    // application alongside the ARNs, so a caller needs no second read to tell the
    // user what blocks the delete.
    let mut named: Vec<(String, String)> = referencing.to_vec();
    named.sort();
    let apps: Vec<String> = named
        .iter()
        .map(|(app_name, app_arn)| format!("{app_name} ({app_arn})"))
        .collect();
    let expected = format!(
        "Cannot delete policy {name} ({policy_arn}). Policy currently associated with \
         app(s) {}",
        apps.join(", ")
    );
    let message = refused.body["message"].as_str().unwrap_or_default();
    if expected.chars().count() <= MAX_MESSAGE_LENGTH {
        assert_eq!(
            message, expected,
            "case {case:?}: expected the in-use refusal {expected:?}, got {:?}",
            refused.body
        );
    } else {
        // Past the member's modelled width the trailing applications are left out
        // rather than cut mid-ARN, so the message is a PREFIX of the full list and
        // still names the first application that blocks the delete.
        assert!(
            expected.starts_with(message) && message.contains(&apps[0]),
            "case {case:?}: expected a prefix of {expected:?} naming {:?}, got {message:?}",
            apps[0]
        );
        assert!(
            message.chars().count() <= MAX_MESSAGE_LENGTH,
            "case {case:?}: the message must stay inside the modelled {MAX_MESSAGE_LENGTH} \
             characters, got {} in {message:?}",
            message.chars().count()
        );
    }
    assert_eq!(
        refused.body["resourceId"].as_str(),
        Some(policy_arn),
        "case {case:?}: the conflict must name the policy the request named, got {:?}",
        refused.body
    );
    assert_eq!(
        refused.body["resourceType"].as_str(),
        Some("ResiliencyPolicy"),
        "case {case:?}: the conflict must name the resource type, got {:?}",
        refused.body
    );

    let still_there = describe_policy(router, policy_arn).await;
    assert_eq!(
        still_there.ok(case)["policy"]["policyName"].as_str(),
        Some(name),
        "case {case:?}: a refused delete must leave the policy untouched, got {:?}",
        still_there.body
    );
}

/// The table spans the rule's two degrees of freedom: HOW MANY applications reference
/// the policy (none, one, several) and HOW each reference is released. Every row also
/// carries a bystander application pointing at a second policy, so a handler that
/// refused whenever any application existed would fail the row with zero references.
#[tokio::test]
async fn a_policy_is_deleted_only_once_no_application_references_it() {
    let table: &[(&str, usize, Release)] = &[
        ("no application references it", 0, Release::DeleteTheApp),
        ("one reference, released by deleting the app", 1, Release::DeleteTheApp),
        ("one reference, released by repointing the app", 1, Release::RepointTheApp),
        ("one reference, released by clearing the reference", 1, Release::ClearTheReference),
        ("two references, released by repointing", 2, Release::RepointTheApp),
        ("three references, released by deleting the apps", 3, Release::DeleteTheApp),
        ("four references, released by clearing them", 4, Release::ClearTheReference),
    ];

    for (case, references, release) in table.iter().copied() {
        let router = emulator();
        let target = create_a_policy(&router, "target-policy").await;
        let spare = create_a_policy(&router, "spare-policy").await;
        // The bystander never references `target`, so it must never block its delete.
        let bystander = create_an_app(&router, "bystander", Some(&spare)).await;
        // Every application pointing at the SPARE, which is what blocks the spare's
        // own delete at the end: the bystander, plus whatever this row repoints.
        let mut spare_references: Vec<(String, String)> =
            vec![("bystander".to_string(), bystander.clone())];

        let mut referencing: Vec<(String, String)> = Vec::new();
        for index in 0..references {
            let name = format!("user-{index}");
            let app_arn = create_an_app(&router, &name, Some(&target)).await;
            referencing.push((name, app_arn));
        }

        // Every reference still standing blocks the delete, and the refusal names
        // every application that holds one.
        for remaining in (1..=references).rev() {
            assert_blocked(
                &router,
                &format!("{case}: {remaining} reference(s) left"),
                &target,
                "target-policy",
                &referencing,
            )
            .await;
            let (app_name, app_arn) = referencing.pop().expect("a referencing application");
            match release {
                Release::DeleteTheApp => {
                    post(&router, "/delete-app", &json!({"appArn": app_arn}))
                        .await
                        .ok(&format!("{case}: DeleteApp {app_arn}"));
                }
                Release::RepointTheApp => {
                    let repointed = post(
                        &router,
                        "/update-app",
                        &json!({"appArn": app_arn, "policyArn": spare}),
                    )
                    .await;
                    assert_eq!(
                        repointed.ok(&format!("{case}: UpdateApp {app_arn}"))["app"]["policyArn"]
                            .as_str(),
                        Some(spare.as_str()),
                        "case {case:?}: repointing must store the new policyArn, got {:?}",
                        repointed.body
                    );
                    spare_references.push((app_name, app_arn.clone()));
                }
                Release::ClearTheReference => {
                    let cleared = post(
                        &router,
                        "/update-app",
                        &json!({"appArn": app_arn, "clearResiliencyPolicyArn": true}),
                    )
                    .await;
                    assert_eq!(
                        cleared.ok(&format!("{case}: UpdateApp {app_arn}"))["app"]["policyArn"],
                        Value::Null,
                        "case {case:?}: clearing must drop the stored policyArn, got {:?}",
                        cleared.body
                    );
                }
            }
        }

        // Nothing references it now, so the delete goes through and echoes the ARN.
        let deleted = delete_policy(&router, &target).await;
        assert_eq!(
            deleted.ok(case)["policyArn"].as_str(),
            Some(target.as_str()),
            "case {case:?}: the delete must echo the policyArn it removed, got {:?}",
            deleted.body
        );

        // The removal is what a later read sees, and it is not idempotent: the same
        // delete again finds nothing.
        describe_policy(&router, &target)
            .await
            .refused(&format!("{case}: DescribeResiliencyPolicy after the delete"), 404, "ResourceNotFoundException");
        delete_policy(&router, &target)
            .await
            .refused(&format!("{case}: the second delete of one ARN"), 404, "ResourceNotFoundException");

        // Exactly one policy went: the spare, and the bystander that names it, are
        // untouched — and the spare is now the one the reference protects.
        let spare_read = describe_policy(&router, &spare).await;
        assert_eq!(
            spare_read.ok(case)["policy"]["policyName"].as_str(),
            Some("spare-policy"),
            "case {case:?}: deleting one policy must leave the others, got {:?}",
            spare_read.body
        );
        let bystander_read = post(&router, "/describe-app", &json!({"appArn": bystander})).await;
        assert_eq!(
            bystander_read.ok(case)["app"]["policyArn"].as_str(),
            Some(spare.as_str()),
            "case {case:?}: an application that never named the policy must be untouched, got {:?}",
            bystander_read.body
        );
        assert_blocked(
            &router,
            &format!("{case}: the spare is now referenced"),
            &spare,
            "spare-policy",
            &spare_references,
        )
        .await;
    }
}

/// A `policyArn` no `CreateResiliencyPolicy` ever minted is a 404, whatever its shape,
/// and never a 501 or a 200 — and the refusal persists nothing.
#[tokio::test]
async fn deleting_a_policy_that_does_not_exist_is_a_resource_not_found() {
    let router = emulator();
    // Rows differ along the shape of a well-formed-but-unknown `Arn` (the member's
    // @pattern is what bounds them: partition, region, account and resource id).
    let table: &[(&str, String)] = &[
        (
            "never created, in this region",
            format!("arn:aws:resiliencehub:{REGION}:{ACCOUNT}:resiliency-policy/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"),
        ),
        (
            "never created, another region",
            format!("arn:aws:resiliencehub:eu-west-3:{ACCOUNT}:resiliency-policy/11111111-2222-3333-4444-555555555555"),
        ),
        (
            "never created, another partition",
            format!("arn:aws-us-gov:resiliencehub:us-gov-west-1:{ACCOUNT}:resiliency-policy/99999999-8888-7777-6666-555555555555"),
        ),
        (
            "never created, another account",
            "arn:aws:resiliencehub:us-east-1:123456789012:resiliency-policy/deadbeef-dead-beef-dead-beefdeadbeef".to_string(),
        ),
        (
            "an app ARN, not a policy ARN",
            format!("arn:aws:resiliencehub:{REGION}:{ACCOUNT}:app/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"),
        ),
    ];

    for (case, policy_arn) in table {
        let refused = delete_policy(&router, policy_arn).await;
        refused.refused(case, 404, "ResourceNotFoundException");
        assert_eq!(
            refused.body["message"].as_str(),
            Some(format!("ResiliencyPolicy {policy_arn} not found").as_str()),
            "case {case:?}: the 404 must name the policy the request named, got {:?}",
            refused.body
        );
        let described = describe_policy(&router, policy_arn).await;
        described.refused(case, 404, "ResourceNotFoundException");
        assert_eq!(
            described.body["message"].as_str(),
            Some(format!("ResiliencyPolicy {policy_arn} not found").as_str()),
            "case {case:?}: a read of the same unknown policy is worded the same way, got {:?}",
            described.body
        );
    }

    // A real policy still deletes afterwards: the 404s above are about the ARN, not a
    // store the refusals broke.
    let policy_arn = create_a_policy(&router, "deletable").await;
    delete_policy(&router, &policy_arn)
        .await
        .ok("a policy nothing references");
}

/// Deleting a policy releases the `@idempotencyToken` its create recorded, so the
/// token cannot keep replaying a resource that is gone: the next create with that
/// token mints a NEW policy.
#[tokio::test]
async fn deleting_a_policy_releases_the_client_token_its_create_recorded() {
    let router = emulator();
    let body = json!({
        "clientToken": "token-reused-after-a-delete",
        "policyName": "token-policy",
        "tier": "CoreServices",
        "policy": required_objectives(60),
    });

    let first = post(&router, "/create-resiliency-policy", &body).await;
    let first_arn = first.ok("first create")["policy"]["policyArn"]
        .as_str()
        .expect("a minted policyArn")
        .to_string();

    // While the policy stands, the replayed token replays it.
    let replayed = post(&router, "/create-resiliency-policy", &body).await;
    assert_eq!(
        replayed.ok("replayed token")["policy"]["policyArn"].as_str(),
        Some(first_arn.as_str()),
        "case \"replayed token\": the first policy must be replayed, got {:?}",
        replayed.body
    );

    delete_policy(&router, &first_arn)
        .await
        .ok("delete the tokened policy");

    let after = post(&router, "/create-resiliency-policy", &body).await;
    let second_arn = after.ok("create after the delete")["policy"]["policyArn"]
        .as_str()
        .expect("a minted policyArn")
        .to_string();
    assert_ne!(
        second_arn, first_arn,
        "case \"create after the delete\": the token must no longer replay the deleted \
         policy, got {second_arn}"
    );
    describe_policy(&router, &second_arn)
        .await
        .ok("the replacement policy is readable");
    describe_policy(&router, &first_arn).await.refused(
        "the deleted policy stays deleted",
        404,
        "ResourceNotFoundException",
    );
}
