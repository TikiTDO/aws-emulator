// SPDX-License-Identifier: CC0-1.0

//! The ROLE a submitted S3 Batch Operations job runs as, driven end to end over
//! HTTP against IAM, Amazon S3 and S3 Control on one shared engine.
//!
//! The rule under test: a job's work is carried out AS the role the create named
//! ("the IAM role that Batch Operations will use to run this job's operation on
//! every object in the manifest", `JobDescriptor$RoleArn`), so that role is
//! assumed BEFORE the first task — and a role the account's IAM holds a trust
//! document for that withholds `sts:AssumeRole` from
//! `batchoperations.s3.amazonaws.com` is a job that cannot run: it settles
//! `Failed` with a `FailureReasons` entry naming the role and the trust it is
//! missing, accounts for the nothing it ran, and every object its manifest lists
//! is left exactly as it was. A role whose trust document DOES grant that
//! principal the assume runs the job's tasks and settles `Complete`.
//!
//! The rows span what the rule generalises over: how the trust document states
//! the grant (the principal alone, among several, through a wildcard action, in
//! another casing), what it states instead (another service's principal, a
//! near-miss principal, an explicit deny beside the grant, an action that is not
//! the assume), where IAM holds the role (at the root or under a path), and a
//! well-formed role ARN this emulator's IAM holds nothing under — which is
//! evidence about this store rather than about the role, so such a job still
//! runs.
//!
//! Every role here is created through IAM's own `CreateRole`, every object is
//! written and read through Amazon S3's own operations, and the tag a row looks
//! for is one the job's own run applied.

use serde_json::{json, Value};

use service_com_amazonaws_s3control_awss3controlservicev20180820::state::{
    JOB_STATUS_COMPLETE, JOB_STATUS_FAILED,
};

use crate::access_point_lifecycle::{xml_escape, Reply, REGION, XMLNS};
use crate::batch_job_execution::{csv_manifest, poll_terminal};
use crate::object_store::ObjectStore;

/// The service principal S3 Batch Operations assumes a job's role as.
const BATCH_OPERATIONS_PRINCIPAL: &str = "batchoperations.s3.amazonaws.com";

/// The action a trust document has to grant that principal.
const ASSUME_ROLE: &str = "sts:AssumeRole";

/// The tag every job below puts on each object its manifest lists.
const TAG: (&str, &str) = ("sim2real", "tagged");

/// The key of the one object each job's manifest lists.
const TARGET_KEY: &str = "data/target.txt";

/// The key the manifest object itself is written at.
const MANIFEST_KEY: &str = "manifest/manifest.csv";

/// The `FailureCode` a job carries when its role could not be assumed.
const ASSUME_ROLE_FAILURE_CODE: &str = "AccessDenied";

/// The fixed text of the `FailureReason` such a job carries. Source: the
/// recorded reason for this failure, whose text carries this phrase with the
/// role ARN as its variable part (`issues/`, the report this suite was written
/// for). The role the request named is the other part.
const ASSUME_ROLE_FAILURE_TEXT: &str = "Could not assume the IAM role";

/// The longest `FailureReason` the model lets that member carry: the
/// `@length(min: 1, max: 256)` of `JobFailureReason`
/// (`smithy_query(query="shape", shape="JobFailure")`).
const FAILURE_REASON_LENGTH_MAX: usize = 256;

/// What a row expects of the job its role is named on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Verdict {
    /// The role grants the assume: the job runs and tags its target.
    Runs,
    /// IAM holds a trust document that withholds the assume: the job settles
    /// failed and the target is left untagged.
    Unassumable,
}

/// One role the rule is asserted over.
struct Row {
    case: &'static str,
    /// The IAM path the role is created under.
    path: &'static str,
    /// The role name, unique within the row's own IAM store.
    role_name: &'static str,
    /// The role's `AssumeRolePolicyDocument`, or `None` for a row that creates
    /// no role at all and names one IAM holds nothing under.
    trust: Option<Value>,
    verdict: Verdict,
}

/// A trust policy carrying `statements`.
fn trust(statements: Value) -> Value {
    json!({"Version": "2012-10-17", "Statement": statements})
}

/// One trust statement: `effect` the `action` for the service principals
/// `principals`.
fn statement(effect: &str, principals: Value, action: Value) -> Value {
    json!({"Effect": effect, "Principal": {"Service": principals}, "Action": action})
}

/// The role table.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the principal and the assume, as strings",
            path: "/",
            role_name: "batch-ops-plain",
            trust: Some(trust(json!([statement(
                "Allow",
                json!(BATCH_OPERATIONS_PRINCIPAL),
                json!(ASSUME_ROLE)
            )]))),
            verdict: Verdict::Runs,
        },
        Row {
            case: "the principal among several, the actions as a list",
            path: "/",
            role_name: "batch-ops-among-several",
            trust: Some(trust(json!([statement(
                "Allow",
                json!(["ec2.amazonaws.com", BATCH_OPERATIONS_PRINCIPAL]),
                json!(["sts:TagSession", ASSUME_ROLE])
            )]))),
            verdict: Verdict::Runs,
        },
        Row {
            case: "the grant through a wildcard action",
            path: "/",
            role_name: "batch-ops-wildcard",
            trust: Some(trust(json!([statement(
                "Allow",
                json!(BATCH_OPERATIONS_PRINCIPAL),
                json!("sts:*")
            )]))),
            verdict: Verdict::Runs,
        },
        Row {
            case: "the grant written in another casing",
            path: "/",
            role_name: "batch-ops-casing",
            trust: Some(trust(json!([statement(
                "Allow",
                json!("BatchOperations.S3.AmazonAWS.com"),
                json!("STS:ASSUMEROLE")
            )]))),
            verdict: Verdict::Runs,
        },
        Row {
            case: "a role under a path, trusting the principal",
            path: "/service-role/",
            role_name: "batch-ops-under-path",
            trust: Some(trust(json!(statement(
                "Allow",
                json!(BATCH_OPERATIONS_PRINCIPAL),
                json!(ASSUME_ROLE)
            )))),
            verdict: Verdict::Runs,
        },
        Row {
            case: "a role IAM holds nothing under",
            path: "/",
            role_name: "batch-ops-absent",
            trust: None,
            verdict: Verdict::Runs,
        },
        Row {
            case: "another service's principal only",
            path: "/",
            role_name: "batch-ops-other-service",
            trust: Some(trust(json!([statement(
                "Allow",
                json!("ec2.amazonaws.com"),
                json!(ASSUME_ROLE)
            )]))),
            verdict: Verdict::Unassumable,
        },
        Row {
            case: "a near-miss principal",
            path: "/",
            role_name: "batch-ops-near-miss",
            trust: Some(trust(json!([statement(
                "Allow",
                json!("s3.amazonaws.com"),
                json!(ASSUME_ROLE)
            )]))),
            verdict: Verdict::Unassumable,
        },
        Row {
            case: "an explicit deny beside the grant",
            path: "/",
            role_name: "batch-ops-denied",
            trust: Some(trust(json!([
                statement(
                    "Allow",
                    json!(BATCH_OPERATIONS_PRINCIPAL),
                    json!(ASSUME_ROLE)
                ),
                statement("Deny", json!(BATCH_OPERATIONS_PRINCIPAL), json!(ASSUME_ROLE)),
            ]))),
            verdict: Verdict::Unassumable,
        },
        Row {
            case: "the principal, for an action that is not the assume",
            path: "/",
            role_name: "batch-ops-other-action",
            trust: Some(trust(json!([statement(
                "Allow",
                json!(BATCH_OPERATIONS_PRINCIPAL),
                json!("sts:TagSession")
            )]))),
            verdict: Verdict::Unassumable,
        },
    ]
}

/// Create `role_name` under `path` with `trust_document` through IAM itself, and
/// answer the ARN IAM minted for it.
async fn create_role(
    iam: &::aws_sdk_iam::Client,
    case: &str,
    path: &str,
    role_name: &str,
    trust_document: &Value,
) -> String {
    iam.create_role()
        .role_name(role_name)
        .path(path)
        .assume_role_policy_document(trust_document.to_string())
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("case {case:?}: IAM must create the role {role_name}: {error:?}")
        })
        .role()
        .map(|role| role.arn().to_string())
        .unwrap_or_else(|| panic!("case {case:?}: IAM must report an ARN for {role_name}"))
}

/// A `CreateJob` body: one `S3PutObjectTagging` action over the CSV manifest of
/// `bucket`, running as `role_arn`.
fn create_job_body(token: &str, bucket: &str, e_tag: &str, role_arn: &str) -> String {
    format!(
        "<CreateJobRequest xmlns=\"{XMLNS}\">\
         <ClientRequestToken>{}</ClientRequestToken><ConfirmationRequired>false\
         </ConfirmationRequired><Operation><S3PutObjectTagging><TagSet>\
         <member><Key>{}</Key><Value>{}</Value></member></TagSet></S3PutObjectTagging>\
         </Operation><Report><Enabled>false</Enabled></Report>{}\
         <Priority>1</Priority><RoleArn>{}</RoleArn></CreateJobRequest>",
        xml_escape(token),
        xml_escape(TAG.0),
        xml_escape(TAG.1),
        csv_manifest(bucket, MANIFEST_KEY, e_tag),
        xml_escape(role_arn)
    )
}

/// The `FailureReasons` entries a settled job reports, as (code, reason) pairs.
fn failures(settled: &Reply) -> Vec<(String, String)> {
    settled
        .texts("FailureCode")
        .into_iter()
        .zip(settled.texts("FailureReason"))
        .collect()
}

/// The three `ProgressSummary` counters a settled job reports, as the answer
/// carries them.
fn progress(settled: &Reply) -> (Option<String>, Option<String>, Option<String>) {
    (
        settled.text("TotalNumberOfTasks"),
        settled.text("NumberOfTasksSucceeded"),
        settled.text("NumberOfTasksFailed"),
    )
}

/// RULE: the job's `RoleArn` is assumed before its tasks run, so the trust
/// document IAM holds for that role decides whether the job runs at all — and a
/// job it stops touches nothing.
#[tokio::test]
async fn a_job_runs_only_under_a_role_whose_trust_policy_admits_batch_operations() {
    for (row_at, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let (store, iam) = ObjectStore::with_iam();
        let bucket = format!("batch-role-trust-{row_at}");
        store.create_bucket(case, REGION, &bucket).await;
        store
            .put_object(case, REGION, &bucket, TARGET_KEY, b"target-bytes")
            .await;
        let manifest = format!("{bucket},{TARGET_KEY}\n");
        let e_tag = store
            .put_object(case, REGION, &bucket, MANIFEST_KEY, manifest.as_bytes())
            .await;

        // Either IAM holds the role the job names, or the row deliberately
        // names one it holds nothing under: an ARN of the same form, which the
        // modelled `IAMRoleArn` pattern admits.
        let role_arn = match row.trust.as_ref() {
            Some(document) => create_role(&iam, case, row.path, row.role_name, document).await,
            None => format!(
                "arn:aws:iam::{}:role/{}",
                ::emulator_server::EMULATOR_ACCOUNT_ID,
                row.role_name
            ),
        };

        let created = store
            .s3control(
                REGION,
                "POST",
                "/v20180820/jobs",
                &create_job_body(&format!("role-trust-{row_at}"), &bucket, &e_tag, &role_arn),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateJob accepts a job without validating the role's trust policy, \
             got {} with body {}",
            created.status, created.body
        );
        let job_id = created
            .text("JobId")
            .unwrap_or_else(|| panic!("case {case:?}: CreateJob answers a JobId: {}", created.body));

        let settled = poll_terminal(&store, case, &job_id).await;
        let status = settled.text("Status").unwrap_or_default();
        let reported = failures(&settled);
        let tags = store.object_tags(REGION, &bucket, TARGET_KEY).await;
        let tagged = vec![(TAG.0.to_string(), TAG.1.to_string())];

        match row.verdict {
            Verdict::Runs => {
                assert_eq!(
                    status, JOB_STATUS_COMPLETE,
                    "case {case:?}: expected the job to run and settle {JOB_STATUS_COMPLETE}, got \
                     {status:?} with body {}",
                    settled.body
                );
                assert!(
                    reported.is_empty(),
                    "case {case:?}: expected no FailureReasons on a job that ran, got {reported:?}"
                );
                assert_eq!(
                    tags, tagged,
                    "case {case:?}: expected the job's run to tag {TARGET_KEY}, got {tags:?}"
                );
            }
            Verdict::Unassumable => {
                assert_eq!(
                    status, JOB_STATUS_FAILED,
                    "case {case:?}: expected a job whose role cannot be assumed to settle \
                     {JOB_STATUS_FAILED}, got {status:?} with body {}",
                    settled.body
                );
                assert!(
                    reported.iter().any(|(code, reason)| {
                        code == ASSUME_ROLE_FAILURE_CODE
                            && reason.starts_with(ASSUME_ROLE_FAILURE_TEXT)
                            && reason.contains(&role_arn)
                            && reason.contains(BATCH_OPERATIONS_PRINCIPAL)
                            && reason.contains(ASSUME_ROLE)
                    }),
                    "case {case:?}: expected a FailureReasons entry with code \
                     {ASSUME_ROLE_FAILURE_CODE:?} opening {ASSUME_ROLE_FAILURE_TEXT:?} and \
                     naming {role_arn}, {BATCH_OPERATIONS_PRINCIPAL} and {ASSUME_ROLE}, got \
                     {reported:?}"
                );
                // The job ran no task at all, which its `ProgressSummary`
                // accounts for: the role comes before the object list.
                assert_eq!(
                    progress(&settled),
                    (
                        Some("0".to_string()),
                        Some("0".to_string()),
                        Some("0".to_string())
                    ),
                    "case {case:?}: expected a settled job to account for no task, got body {}",
                    settled.body
                );
                assert!(
                    tags.is_empty(),
                    "case {case:?}: expected {TARGET_KEY} to be left untagged, got {tags:?}"
                );
            }
        }
    }
}

/// RULE: the reason is a legal value of the member that carries it for every
/// role ARN the model admits — `JobFailureReason` is `@length(max: 256)` while
/// `IAMRoleArn` is `@length(max: 2048)` — and whatever it has to leave out, it
/// still opens with the failure and names the role the job could not assume.
#[tokio::test]
async fn the_reason_stays_inside_the_modelled_bound_for_a_long_role_arn() {
    let case = "a role under a long path";
    let (store, iam) = ObjectStore::with_iam();
    let bucket = "batch-role-trust-long-arn";
    store.create_bucket(case, REGION, bucket).await;
    store
        .put_object(case, REGION, bucket, TARGET_KEY, b"target-bytes")
        .await;
    let manifest = format!("{bucket},{TARGET_KEY}\n");
    let e_tag = store
        .put_object(case, REGION, bucket, MANIFEST_KEY, manifest.as_bytes())
        .await;

    // An IAM path is `@length(max: 512)` and a role name `@length(max: 64)`, so
    // a role IAM itself holds can carry an ARN far longer than the reason.
    let path = format!("/{}/{}/", "a".repeat(63), "b".repeat(63));
    let role_name = "c".repeat(64);
    let role_arn = create_role(
        &iam,
        case,
        &path,
        &role_name,
        &trust(json!([statement(
            "Allow",
            json!("ec2.amazonaws.com"),
            json!(ASSUME_ROLE)
        )])),
    )
    .await;

    let created = store
        .s3control(
            REGION,
            "POST",
            "/v20180820/jobs",
            &create_job_body("long-arn", bucket, &e_tag, &role_arn),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected CreateJob to answer 200, got {} with body {}",
        created.status, created.body
    );
    let job_id = created
        .text("JobId")
        .unwrap_or_else(|| panic!("case {case:?}: CreateJob answers a JobId: {}", created.body));
    let settled = poll_terminal(&store, case, &job_id).await;
    assert_eq!(
        settled.text("Status").unwrap_or_default(),
        JOB_STATUS_FAILED,
        "case {case:?}: expected {JOB_STATUS_FAILED}, got body {}",
        settled.body
    );
    let reported = failures(&settled);
    assert!(
        reported.iter().any(|(code, reason)| {
            code == ASSUME_ROLE_FAILURE_CODE
                && reason.starts_with(ASSUME_ROLE_FAILURE_TEXT)
                && reason.contains(&role_arn)
                && reason.chars().count() <= FAILURE_REASON_LENGTH_MAX
        }),
        "case {case:?}: expected a reason of at most {FAILURE_REASON_LENGTH_MAX} characters \
         opening {ASSUME_ROLE_FAILURE_TEXT:?} and naming {role_arn}, got {reported:?}"
    );
    assert!(
        store.object_tags(REGION, bucket, TARGET_KEY).await.is_empty(),
        "case {case:?}: expected {TARGET_KEY} to be left untagged"
    );
}

/// RULE: the two outcomes differ ONLY in the trust principal — the same job, the
/// same manifest and the same permissions under two roles that differ in nothing
/// else settle differently, and the failed one leaves its target as it found it.
#[tokio::test]
async fn two_jobs_differing_only_in_the_trust_principal_settle_differently() {
    let (store, iam) = ObjectStore::with_iam();
    let bucket = "batch-role-trust-isolated";
    store.create_bucket("isolated", REGION, bucket).await;
    store
        .put_object("isolated", REGION, bucket, TARGET_KEY, b"target-bytes")
        .await;
    let manifest = format!("{bucket},{TARGET_KEY}\n");
    let e_tag = store
        .put_object("isolated", REGION, bucket, MANIFEST_KEY, manifest.as_bytes())
        .await;

    // The two roles differ in the trust PRINCIPAL and in nothing else.
    let trusted = create_role(
        &iam,
        "trusted",
        "/",
        "batchgoodtrust",
        &trust(json!([statement(
            "Allow",
            json!(BATCH_OPERATIONS_PRINCIPAL),
            json!(ASSUME_ROLE)
        )])),
    )
    .await;
    let mistrusted = create_role(
        &iam,
        "mistrusted",
        "/",
        "batchbadtrust",
        &trust(json!([statement(
            "Allow",
            json!("ec2.amazonaws.com"),
            json!(ASSUME_ROLE)
        )])),
    )
    .await;

    // The mistrusted job first, so the target is untagged when it runs: what it
    // leaves behind is what proves none of its tasks was carried out.
    for (case, role_arn, expected) in [
        ("mistrusted", &mistrusted, JOB_STATUS_FAILED),
        ("trusted", &trusted, JOB_STATUS_COMPLETE),
    ] {
        let created = store
            .s3control(
                REGION,
                "POST",
                "/v20180820/jobs",
                &create_job_body(case, bucket, &e_tag, role_arn),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected CreateJob to answer 200, got {} with body {}",
            created.status, created.body
        );
        let job_id = created
            .text("JobId")
            .unwrap_or_else(|| panic!("case {case:?}: CreateJob answers a JobId: {}", created.body));
        let settled = poll_terminal(&store, case, &job_id).await;
        let status = settled.text("Status").unwrap_or_default();
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status:?} with body {}",
            settled.body
        );
        let tags = store.object_tags(REGION, bucket, TARGET_KEY).await;
        if expected == JOB_STATUS_FAILED {
            assert!(
                tags.is_empty(),
                "case {case:?}: expected the target to stay untagged, got {tags:?}"
            );
        } else {
            assert_eq!(
                tags,
                vec![(TAG.0.to_string(), TAG.1.to_string())],
                "case {case:?}: expected the target to carry the job's tag, got {tags:?}"
            );
        }
    }
}
