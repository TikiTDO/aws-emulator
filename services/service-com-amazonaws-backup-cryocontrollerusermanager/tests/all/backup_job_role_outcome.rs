// SPDX-License-Identifier: CC0-1.0

//! What a started AWS Backup job COMES TO, decided by the permissions of the role
//! it runs under.
//!
//! The RULE under test: `StartBackupJob` is accepted on what it can be judged on
//! synchronously, and the backup is then performed UNDER the `IamRoleArn` it named,
//! so the job's TERMINAL state is decided by what that role is permitted to do — a
//! role whose policies do not allow the action that creates the recovery point of
//! the named resource ends the job `FAILED` with the denial as its `StatusMessage`
//! and leaves the vault no recovery point, while a role that is allowed it ends
//! `COMPLETED` with the point it wrote.
//!
//! The table below spans that rule along everything it generalises over rather
//! than one request: the KIND of protected resource (a DynamoDB table, an EBS
//! volume, an EC2 instance, an EFS file system, an Amazon RDS instance, an Aurora
//! cluster, and a resource type no creating action is known for), and the shape of
//! the grant the role carries (none at all, the creating action, that action
//! under a wildcard, a different action of the same service, the creating action
//! of a SIBLING type of the same service namespace, and the right action on
//! another resource). Each row is asserted on the job read, on the vault's
//! recovery points and — once, over the whole table — on the state-filtered job
//! listing, so a second reader can never disagree with the first.
//!
//! Every case drives real HTTP through the router: the Backup wire is the shared
//! harness in `vault_and_backup_job.rs`, and IAM is mounted beside Backup on one
//! engine (`plan_and_selection_refusals.rs`'s `router_with_iam`), because the
//! decision is put to IAM over the emulator's own in-process callback and the
//! documents it decides from are the ones `CreateRole` and `PutRolePolicy` wrote
//! through IAM's own wire.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::plan_and_selection_refusals::{create_role, form_value, router_with_iam};
use crate::vault_and_backup_job::{
    call, create_vault, start_job, string_at, ACCOUNT, PAST_SETTLE, REGION,
};

/// The vault every job of this suite is started into.
const VAULT: &str = "role-outcome-vault";

/// The service principal a backup role has to admit for AWS Backup to assume it,
/// so every row's call is accepted and only the role's PERMISSIONS decide the
/// outcome. Source: AWS Backup's service principal, as the trust policy of its
/// service role carries it.
const SERVICE_PRINCIPAL: &str = "backup.amazonaws.com";

/// The two terminal `BackupJobState` values a started job reaches, spelled out
/// here rather than imported so a wrong constant in the implementation cannot
/// make a test pass (model enum `BackupJobState`).
const COMPLETED: &str = "COMPLETED";
const FAILED: &str = "FAILED";

/// One statement of the inline policy a row's role carries: the effect, the action
/// it covers and the resource it covers.
struct Grant {
    effect: &'static str,
    action: &'static str,
    resource: String,
}

/// One row of the table: a protected resource, the grant its role carries, and
/// what the job started on it must come to.
struct Case {
    /// What this row is about, named in every assertion of it.
    name: &'static str,
    /// The IAM role name the row creates, unique so each row's policy is its own.
    role_name: &'static str,
    /// The protected resource the job backs up, unique per row so the vault's
    /// recovery points can be filtered down to this row's.
    resource_arn: String,
    /// The statements of the role's inline policy; empty for a role that carries
    /// no policy at all.
    policy: Vec<Grant>,
    /// The terminal state the job must reach.
    expected_state: &'static str,
    /// The action the denial must name, for a row the rule fails.
    withheld_action: Option<&'static str>,
}

/// An ARN of this emulator's account in this request's Region.
fn resource(namespace: &str, resource: &str) -> String {
    format!("arn:aws:{namespace}:{REGION}:{ACCOUNT}:{resource}")
}

/// One grant of `action` on `resource`.
fn allow(action: &'static str, resource: String) -> Grant {
    Grant {
        effect: "Allow",
        action,
        resource,
    }
}

/// Every grant of this account, so a row's outcome turns on the ACTION alone.
fn any_resource() -> String {
    "*".to_string()
}

/// The table: one row per (resource kind, grant shape) pair the rule generalises
/// over.
///
/// The creating action each row expects is the action AWS publishes for the backup
/// role of that resource type (AWS Backup, *Managed policies*): `dynamodb:CreateBackup`
/// for a table, `ec2:CreateSnapshot` for a volume, `ec2:CreateImage` for an
/// instance, `elasticfilesystem:Backup` for a file system, `rds:CreateDBSnapshot`
/// for a database instance and `rds:CreateDBClusterSnapshot` for a cluster. The
/// last row's resource type is one no creating action is known for, so nothing
/// decides its outcome.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "a table whose role is allowed the creating action",
            role_name: "table-allowed",
            resource_arn: resource("dynamodb", "table/orders-allowed"),
            policy: vec![allow("dynamodb:CreateBackup", any_resource())],
            expected_state: COMPLETED,
            withheld_action: None,
        },
        Case {
            name: "a table whose role carries no policy at all",
            role_name: "table-bare",
            resource_arn: resource("dynamodb", "table/orders-bare"),
            policy: Vec::new(),
            expected_state: FAILED,
            withheld_action: Some("dynamodb:CreateBackup"),
        },
        Case {
            name: "a table whose role is allowed every action of the service",
            role_name: "table-wildcard",
            resource_arn: resource("dynamodb", "table/orders-wildcard"),
            policy: vec![allow("dynamodb:*", any_resource())],
            expected_state: COMPLETED,
            withheld_action: None,
        },
        Case {
            name: "a table whose role is allowed only another action of the service",
            role_name: "table-other-action",
            resource_arn: resource("dynamodb", "table/orders-other-action"),
            policy: vec![allow("dynamodb:DescribeTable", any_resource())],
            expected_state: FAILED,
            withheld_action: Some("dynamodb:CreateBackup"),
        },
        Case {
            name: "a table whose role is allowed the creating action on another table",
            role_name: "table-other-resource",
            resource_arn: resource("dynamodb", "table/orders-other-resource"),
            policy: vec![allow(
                "dynamodb:CreateBackup",
                resource("dynamodb", "table/somewhere-else"),
            )],
            expected_state: FAILED,
            withheld_action: Some("dynamodb:CreateBackup"),
        },
        Case {
            name: "a volume whose role is allowed the creating action",
            role_name: "volume-allowed",
            resource_arn: resource("ec2", "volume/vol-0123456789abcdef0"),
            policy: vec![allow("ec2:CreateSnapshot", any_resource())],
            expected_state: COMPLETED,
            withheld_action: None,
        },
        Case {
            name: "a volume whose role carries no policy at all",
            role_name: "volume-bare",
            resource_arn: resource("ec2", "volume/vol-00000000000000000"),
            policy: Vec::new(),
            expected_state: FAILED,
            withheld_action: Some("ec2:CreateSnapshot"),
        },
        Case {
            name: "an instance whose role is allowed the creating action",
            role_name: "instance-allowed",
            resource_arn: resource("ec2", "instance/i-0123456789abcdef0"),
            policy: vec![allow("ec2:CreateImage", any_resource())],
            expected_state: COMPLETED,
            withheld_action: None,
        },
        Case {
            name: "an instance whose role is allowed a sibling type's creating action",
            role_name: "instance-sibling",
            resource_arn: resource("ec2", "instance/i-00000000000000000"),
            policy: vec![allow("ec2:CreateSnapshot", any_resource())],
            expected_state: FAILED,
            withheld_action: Some("ec2:CreateImage"),
        },
        Case {
            name: "a file system whose role is allowed the creating action",
            role_name: "filesystem-allowed",
            resource_arn: resource("elasticfilesystem", "file-system/fs-0123456789abcdef0"),
            policy: vec![allow("elasticfilesystem:Backup", any_resource())],
            expected_state: COMPLETED,
            withheld_action: None,
        },
        Case {
            name: "a database instance whose role is allowed the creating action",
            role_name: "database-allowed",
            resource_arn: resource("rds", "db:orders-instance"),
            policy: vec![allow("rds:CreateDBSnapshot", any_resource())],
            expected_state: COMPLETED,
            withheld_action: None,
        },
        Case {
            name: "a cluster whose role is allowed only the instance creating action",
            role_name: "cluster-instance-action",
            resource_arn: resource("rds", "cluster:orders-cluster"),
            policy: vec![allow("rds:CreateDBSnapshot", any_resource())],
            expected_state: FAILED,
            withheld_action: Some("rds:CreateDBClusterSnapshot"),
        },
        Case {
            name: "a resource type no creating action is known for",
            role_name: "stack-bare",
            resource_arn: resource(
                "cloudformation",
                "stack/orders-stack/00000000-1111-2222-3333-444444444444",
            ),
            policy: Vec::new(),
            expected_state: COMPLETED,
            withheld_action: None,
        },
    ]
}

/// The denial a job whose role withholds `action` on `resource` reports, as IAM
/// words a request no identity-based document allows: the principal, the action
/// and the resource the backup was asked for.
fn denial(role_arn: &str, action: &str, resource: &str) -> String {
    format!(
        "User: {role_arn} is not authorized to perform: {action} on resource: {resource} \
         because no identity-based policy allows the {action} action"
    )
}

/// A trust document admitting [`SERVICE_PRINCIPAL`], so every row's role is one
/// AWS Backup can assume and only its permissions decide the job's outcome.
fn backup_trust() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": SERVICE_PRINCIPAL},
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// Attach `policy` to the role `role_name` as an inline policy, through IAM's OWN
/// awsQuery wire, so the documents the decision reads are the ones IAM stored.
async fn put_role_policy(router: &MultiServiceRouter, role_name: &str, policy: &[Grant]) {
    let document = json!({
        "Version": "2012-10-17",
        "Statement": policy
            .iter()
            .map(|grant| json!({
                "Effect": grant.effect,
                "Action": grant.action,
                "Resource": grant.resource,
            }))
            .collect::<Vec<Value>>(),
    })
    .to_string();
    let body = format!(
        "Action=PutRolePolicy&Version=2010-05-08&RoleName={}&PolicyName={}&PolicyDocument={}",
        form_value(role_name),
        form_value("backup-permissions"),
        form_value(&document)
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let xml = String::from_utf8(response.into_body().to_vec()).expect("a UTF-8 response body");
    assert_eq!(
        status, 200,
        "setup {role_name:?}: expected PutRolePolicy 200, got {status} {xml}"
    );
}

/// The `RecoveryPointArn`s the vault holds for `resource_arn`, with the `Status` of
/// each.
async fn points_of(router: &MultiServiceRouter, resource_arn: &str) -> Vec<(String, String)> {
    let path = format!("/backup-vaults/{VAULT}/recovery-points?resourceArn={resource_arn}");
    let (status, listed) = call(router, "GET", &path, None).await;
    assert_eq!(
        status, 200,
        "expected ListRecoveryPointsByBackupVault 200 for {resource_arn:?}, got {status} \
         {listed:?}"
    );
    listed["RecoveryPoints"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|point| {
            (
                string_at(point, "RecoveryPointArn"),
                string_at(point, "Status"),
            )
        })
        .collect()
}

/// The `BackupJobId`s one `ListBackupJobs` page reports.
async fn jobs_in_state(router: &MultiServiceRouter, state: &str) -> Vec<String> {
    let (status, listed) = call(router, "GET", &format!("/backup-jobs?state={state}"), None).await;
    assert_eq!(
        status, 200,
        "expected ListBackupJobs 200 for state {state:?}, got {status} {listed:?}"
    );
    listed["BackupJobs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|job| string_at(job, "BackupJobId"))
        .collect()
}

/// RULE: the terminal state of a started backup job is decided by whether the role
/// it runs under is allowed the action that creates the recovery point of the named
/// resource — `FAILED` with the denial as `StatusMessage` and no recovery point
/// when it is not, `COMPLETED` with the point it wrote when it is — and the call
/// itself is accepted either way.
#[tokio::test]
async fn a_jobs_terminal_state_follows_what_its_role_is_allowed_to_create() {
    let router = router_with_iam();
    let (status, created) = create_vault(&router, VAULT, "{}").await;
    assert_eq!(
        status, 200,
        "setup: expected CreateBackupVault 200, got {status} {created:?}"
    );

    let cases = cases();
    // Every job is started FIRST and read afterwards, so the suite waits out the
    // settle span once rather than once per row.
    let mut started = Vec::new();
    for case in &cases {
        let role_arn = create_role(&router, "/", case.role_name, &backup_trust()).await;
        if !case.policy.is_empty() {
            put_role_policy(&router, case.role_name, &case.policy).await;
        }
        let body = json!({
            "BackupVaultName": VAULT,
            "ResourceArn": case.resource_arn,
            "IamRoleArn": role_arn,
        })
        .to_string();
        let (status, job) = start_job(&router, &body).await;
        // The call is ACCEPTED whatever the role allows: the backup fails later,
        // asynchronously, and this response is not where that shows.
        assert_eq!(
            status, 200,
            "case {:?}: expected StartBackupJob 200, got {status} {job:?}",
            case.name
        );
        let job_id = string_at(&job, "BackupJobId");
        let point_arn = string_at(&job, "RecoveryPointArn");
        let (status, described) =
            call(&router, "GET", &format!("/backup-jobs/{job_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected DescribeBackupJob 200, got {status} {described:?}",
            case.name
        );
        assert_eq!(
            described.get("State").and_then(Value::as_str),
            Some("CREATED"),
            "case {:?}: a started job is in its transient state before it settles, got \
             {described:?}",
            case.name
        );
        started.push((job_id, point_arn, role_arn));
    }

    std::thread::sleep(PAST_SETTLE);

    let mut expected_points = 0;
    for (case, (job_id, point_arn, role_arn)) in cases.iter().zip(&started) {
        let (status, settled) = call(&router, "GET", &format!("/backup-jobs/{job_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected DescribeBackupJob 200, got {status} {settled:?}",
            case.name
        );
        assert_eq!(
            settled.get("State").and_then(Value::as_str),
            Some(case.expected_state),
            "case {:?}: expected State {}, got {settled:?}",
            case.name,
            case.expected_state
        );
        // The completion instant is the one the clock made due, whichever terminal
        // state the job reached.
        assert_eq!(
            settled.get("CompletionDate"),
            settled.get("ExpectedCompletionDate"),
            "case {:?}: a settled job completed at the instant it came due, got {settled:?}",
            case.name
        );

        let points = points_of(&router, &case.resource_arn).await;
        match case.withheld_action {
            Some(action) => {
                assert_eq!(
                    settled.get("StatusMessage").and_then(Value::as_str),
                    Some(denial(role_arn, action, &case.resource_arn).as_str()),
                    "case {:?}: expected the denial of {action} as the StatusMessage, got \
                     {settled:?}",
                    case.name
                );
                // The backup nothing performed wrote no recovery point, so the
                // vault holds none for this resource — while the job still names
                // the one its start call answered with.
                assert!(
                    points.is_empty(),
                    "case {:?}: a failed job leaves the vault no recovery point, got {points:?}",
                    case.name
                );
                let (status, point) = call(
                    &router,
                    "GET",
                    &format!("/backup-vaults/{VAULT}/recovery-points/{point_arn}"),
                    None,
                )
                .await;
                assert_eq!(
                    status, 400,
                    "case {:?}: the recovery point of a failed job resolves to nothing, got \
                     {status} {point:?}",
                    case.name
                );
            }
            None => {
                assert_eq!(
                    settled.get("StatusMessage"),
                    None,
                    "case {:?}: a job nothing withheld reports no StatusMessage, got {settled:?}",
                    case.name
                );
                assert_eq!(
                    points,
                    vec![(point_arn.clone(), COMPLETED.to_string())],
                    "case {:?}: expected the completed recovery point the job wrote",
                    case.name
                );
                expected_points += 1;
            }
        }
    }

    // A SECOND reader of the same jobs: the state-filtered listing selects exactly
    // the jobs each terminal state belongs to.
    for state in [COMPLETED, FAILED] {
        let listed = jobs_in_state(&router, state).await;
        let expected: Vec<&String> = cases
            .iter()
            .zip(&started)
            .filter(|(case, _)| case.expected_state == state)
            .map(|(_, (job_id, _, _))| job_id)
            .collect();
        assert_eq!(
            listed.len(),
            expected.len(),
            "state {state:?}: expected {} jobs, got {listed:?}",
            expected.len()
        );
        for job_id in expected {
            assert!(
                listed.contains(job_id),
                "state {state:?}: expected the listing to report {job_id:?}, got {listed:?}"
            );
        }
    }

    // And the vault's own count of what it holds follows the points that were
    // actually written.
    let (status, vault) = call(&router, "GET", &format!("/backup-vaults/{VAULT}"), None).await;
    assert_eq!(
        status, 200,
        "expected DescribeBackupVault 200, got {status} {vault:?}"
    );
    assert_eq!(
        vault.get("NumberOfRecoveryPoints").and_then(Value::as_i64),
        Some(expected_points),
        "expected the vault to hold one recovery point per completed job, got {vault:?}"
    );
}
