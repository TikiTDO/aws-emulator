// SPDX-License-Identifier: CC0-1.0

//! That a submitted S3 Batch Operations job SETTLES, and what a client that
//! waited for it then finds — driven end to end over HTTP against IAM, Amazon S3
//! and S3 Control on one shared engine, exactly as a client drives them.
//!
//! The rule under test: a job created without a confirmation needs no further
//! caller input to finish. `CreateJob` answers with its id, and the job is then
//! advanced on its own to a TERMINAL status — the one observable a client waits
//! for — which a bounded `DescribeJob` poll observes: `Complete` once the job's
//! action has been carried out, as the role the create named, against every
//! object of its object list, or `Failed` with a `FailureReasons` entry naming
//! the role when the account's IAM withholds `sts:AssumeRole` from
//! `batchoperations.s3.amazonaws.com`. A job asked for a completion report
//! ("indicates whether the specified job will generate a job-completion
//! report", `JobReport$Enabled`) has it under `<Prefix>/job-<JobId>/` ("an
//! optional prefix to describe where in the specified bucket the job-completion
//! report will be stored", `JobReport$Prefix`) by the time it reports that
//! status — and a job that ran no task at all leaves nothing there, because a
//! report accounts for tasks that ran.
//!
//! The rows span what the rule generalises over: whether the role's trust
//! document grants the batch-operations principal the assume, how many objects
//! the manifest lists and whether a listed row names an object the bucket does
//! not hold, how many tags the action carries and whether the listed objects
//! already carried a tag set, the prefix the report is asked for under (none,
//! one segment, several) and the scope it covers.
//!
//! Every role here is created through IAM's own `CreateRole`, and every tag,
//! report manifest and report result file read back is an Amazon S3 object the
//! job's own run wrote, read through Amazon S3's own operations.

use serde_json::json;

use service_com_amazonaws_s3control_awss3controlservicev20180820::state::batch_job_plan::{
    TASK_FAILED, TASK_SUCCEEDED,
};
use service_com_amazonaws_s3control_awss3controlservicev20180820::state::{
    JOB_STATUS_COMPLETE, JOB_STATUS_FAILED,
};

use crate::access_point_lifecycle::{xml_escape, REGION, XMLNS};
use crate::batch_job_execution::{csv_manifest, poll_terminal, progress};
use crate::object_store::ObjectStore;

/// The service principal S3 Batch Operations assumes a job's role as.
const BATCH_OPERATIONS_PRINCIPAL: &str = "batchoperations.s3.amazonaws.com";

/// The fixed text the reason of a job whose role could not be assumed opens
/// with; the role the create named is its variable part.
const ASSUME_ROLE_FAILURE_TEXT: &str = "Could not assume the IAM role";

/// The key the manifest object of every job below is written at.
const MANIFEST_KEY: &str = "manifest.csv";

/// One object the row's bucket holds: its key and the tag set it carries before
/// the job runs.
type Object = (&'static str, &'static [(&'static str, &'static str)]);

/// One job the rule is asserted over.
struct Row {
    case: &'static str,
    /// The service principal the row's role grants `sts:AssumeRole` to: the job
    /// runs only where that is the batch-operations principal.
    trusted_principal: &'static str,
    /// The role name, unique within the row's own IAM store.
    role_name: &'static str,
    /// Every object written into the row's bucket, with the tag set it starts
    /// with.
    objects: &'static [Object],
    /// The keys the job's manifest lists, in manifest order. A key no object
    /// was written at is a row whose task fails.
    listed: &'static [&'static str],
    /// The tag set the job's `S3PutObjectTagging` action carries.
    tag_set: &'static [(&'static str, &'static str)],
    /// `JobReport$Prefix`, or `None` for a report asked for without one.
    prefix: Option<&'static str>,
    /// `JobReport$ReportScope`.
    scope: &'static str,
    /// The (total, succeeded, failed) task counts `ProgressSummary` must
    /// account for.
    progress: (i64, i64, i64),
    /// The keys the report's result file must carry a row for, in task order.
    reported: &'static [&'static str],
}

impl Row {
    /// Whether the row's role lets the job run at all.
    fn runs(&self) -> bool {
        self.trusted_principal == BATCH_OPERATIONS_PRINCIPAL
    }
}

/// The job table.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one listed object of two, one tag, a report over all tasks",
            trusted_principal: BATCH_OPERATIONS_PRINCIPAL,
            role_name: "batch-report-one",
            objects: &[("data/target.txt", &[]), ("data/other.txt", &[])],
            listed: &["data/target.txt"],
            tag_set: &[("sim2real", "tagged")],
            prefix: Some("reports"),
            scope: "AllTasks",
            progress: (1, 1, 0),
            reported: &["data/target.txt"],
        },
        Row {
            case: "three listed objects, two tags over prior tag sets, a report under a nested \
                   prefix",
            trusted_principal: BATCH_OPERATIONS_PRINCIPAL,
            role_name: "batch-report-nested",
            objects: &[
                ("logs/a.log", &[("stage", "raw"), ("drop", "this")]),
                ("logs/b.log", &[]),
                ("logs/c.log", &[("stage", "raw")]),
            ],
            listed: &["logs/a.log", "logs/b.log", "logs/c.log"],
            tag_set: &[("stage", "archived"), ("retention", "30d")],
            prefix: Some("reports/2026/batch/"),
            scope: "AllTasks",
            progress: (3, 3, 0),
            reported: &["logs/a.log", "logs/b.log", "logs/c.log"],
        },
        Row {
            case: "a listed object the bucket does not hold, a report over failed tasks only",
            trusted_principal: BATCH_OPERATIONS_PRINCIPAL,
            role_name: "batch-report-failed-only",
            objects: &[("present.csv", &[("keep", "me")])],
            listed: &["present.csv", "absent.csv"],
            tag_set: &[("sim2real", "tagged")],
            prefix: Some("reports"),
            scope: "FailedTasksOnly",
            progress: (2, 1, 1),
            reported: &["absent.csv"],
        },
        Row {
            case: "a report asked for without a prefix",
            trusted_principal: BATCH_OPERATIONS_PRINCIPAL,
            role_name: "batch-report-no-prefix",
            objects: &[("top.txt", &[])],
            listed: &["top.txt"],
            tag_set: &[("sim2real", "tagged")],
            prefix: None,
            scope: "AllTasks",
            progress: (1, 1, 0),
            reported: &["top.txt"],
        },
        Row {
            case: "a role whose trust document withholds the assume",
            trusted_principal: "ec2.amazonaws.com",
            role_name: "batch-report-mistrusted",
            objects: &[("data/target.txt", &[])],
            listed: &["data/target.txt"],
            tag_set: &[("sim2real", "tagged")],
            prefix: Some("reports"),
            scope: "AllTasks",
            progress: (0, 0, 0),
            reported: &[],
        },
    ]
}

/// The row's tag set as the `<TagSet>` body of an `S3PutObjectTagging` action.
fn tag_set_xml(tags: &[(&str, &str)]) -> String {
    let members: String = tags
        .iter()
        .map(|(key, value)| {
            format!(
                "<member><Key>{}</Key><Value>{}</Value></member>",
                xml_escape(key),
                xml_escape(value)
            )
        })
        .collect();
    format!("<TagSet>{members}</TagSet>")
}

/// `(key, value)` pairs, sorted, so a comparison is about WHICH tags an object
/// carries rather than the order one answer happened to list them in.
fn sorted(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = tags
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    pairs.sort();
    pairs
}

/// The `<Report>` body asking for a completion report in `bucket`, under the
/// row's prefix and over its scope.
fn report_xml(row: &Row, bucket: &str) -> String {
    let prefix = match row.prefix {
        Some(prefix) => format!("<Prefix>{}</Prefix>", xml_escape(prefix)),
        None => String::new(),
    };
    format!(
        "<Bucket>arn:aws:s3:::{bucket}</Bucket><Format>Report_CSV_20180820</Format>\
         <Enabled>true</Enabled>{prefix}<ReportScope>{}</ReportScope>",
        row.scope
    )
}

/// Where everything one job writes about itself goes: `<Prefix>/job-<JobId>`,
/// the location `JobReport$Prefix` documents the report is stored under, with
/// the prefix left out where the create named none.
fn job_prefix(prefix: Option<&str>, job_id: &str) -> String {
    match prefix.map(|prefix| prefix.trim_end_matches('/')) {
        Some(prefix) if !prefix.is_empty() => format!("{prefix}/job-{job_id}"),
        _ => format!("job-{job_id}"),
    }
}

/// RULE: a job created without a confirmation reaches a terminal status on its
/// own — `Complete` once its action has been carried out as its role against
/// every object of its list, `Failed` naming the role its account's IAM
/// withholds the assume for — and the completion report it asked for is under
/// `<Prefix>/job-<JobId>/` exactly when its tasks ran.
#[tokio::test]
async fn a_submitted_job_settles_and_leaves_the_report_its_tasks_are_accounted_for_in() {
    for (row_at, row) in rows().iter().enumerate() {
        let case = row.case;
        let (store, iam) = ObjectStore::with_iam();
        let bucket = format!("batch-settle-{row_at}");
        let reports = format!("batch-settle-reports-{row_at}");
        store.create_bucket(case, REGION, &bucket).await;
        store.create_bucket(case, REGION, &reports).await;
        for (key, tags) in row.objects {
            store
                .put_object(case, REGION, &bucket, key, b"to-be-tagged")
                .await;
            if !tags.is_empty() {
                store
                    .put_object_tags(case, REGION, &bucket, key, tags)
                    .await;
            }
        }
        let manifest_body: String = row
            .listed
            .iter()
            .map(|key| format!("{bucket},{key}\n"))
            .collect();
        let e_tag = store
            .put_object(case, REGION, &bucket, MANIFEST_KEY, manifest_body.as_bytes())
            .await;

        // The role exists in IAM, which is the authority on who may assume it.
        let role_arn = iam
            .create_role()
            .role_name(row.role_name)
            .assume_role_policy_document(
                json!({
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": {"Service": row.trusted_principal},
                        "Action": "sts:AssumeRole",
                    }],
                })
                .to_string(),
            )
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!("case {case:?}: IAM must create {}: {error:?}", row.role_name)
            })
            .role()
            .map(|role| role.arn().to_string())
            .unwrap_or_else(|| panic!("case {case:?}: IAM must report an ARN for the role"));

        let body = format!(
            "<CreateJobRequest xmlns=\"{XMLNS}\">\
             <ClientRequestToken>settle-{row_at}</ClientRequestToken>\
             <ConfirmationRequired>false</ConfirmationRequired>\
             <Operation><S3PutObjectTagging>{}</S3PutObjectTagging></Operation>\
             <Report>{}</Report>{}<Priority>1</Priority><RoleArn>{}</RoleArn>\
             </CreateJobRequest>",
            tag_set_xml(row.tag_set),
            report_xml(row, &reports),
            csv_manifest(&bucket, MANIFEST_KEY, &e_tag),
            xml_escape(&role_arn)
        );
        let created = store
            .s3control(REGION, "POST", "/v20180820/jobs", &body)
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected CreateJob to answer 200, got {} with body {}",
            created.status, created.body
        );
        let job_id = created.text("JobId").unwrap_or_else(|| {
            panic!("case {case:?}: CreateJob answers a JobId, got body {}", created.body)
        });

        // Nothing but DescribeJob is called from here: the job settles on its
        // own, and the terminal status is what a client waits for.
        let settled = poll_terminal(&store, case, &job_id).await;
        let status = settled.text("Status").unwrap_or_default();
        let expected_status = if row.runs() {
            JOB_STATUS_COMPLETE
        } else {
            JOB_STATUS_FAILED
        };
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected the job to settle {expected_status}, got {status:?} with \
             body {}",
            settled.body
        );
        assert_eq!(
            progress(case, &settled),
            row.progress,
            "case {case:?}: expected the settled job to account for {:?} tasks, got body {}",
            row.progress,
            settled.body
        );

        // A job the role stopped reports WHICH role it could not assume; one
        // that ran reports no failure of its own.
        let reasons = settled.texts("FailureReason");
        if row.runs() {
            assert!(
                reasons.is_empty(),
                "case {case:?}: expected no FailureReasons on a job that ran, got {reasons:?}"
            );
        } else {
            assert!(
                reasons.iter().any(|reason| {
                    reason.starts_with(ASSUME_ROLE_FAILURE_TEXT) && reason.contains(&role_arn)
                }),
                "case {case:?}: expected a reason opening {ASSUME_ROLE_FAILURE_TEXT:?} and naming \
                 {role_arn}, got {reasons:?}"
            );
        }

        // Every listed object carries exactly the job's tag set once the job
        // ran, whatever it held before; every other object, and every object of
        // a job that never ran, keeps the tag set it had.
        for (key, before) in row.objects {
            let expected = if row.runs() && row.listed.contains(key) {
                sorted(row.tag_set)
            } else {
                sorted(before)
            };
            let mut found = store.object_tags(REGION, &bucket, key).await;
            found.sort();
            assert_eq!(
                found, expected,
                "case {case:?}: expected {key} to carry {expected:?}, got {found:?}"
            );
        }

        // The report is under `<Prefix>/job-<JobId>/` for a job whose tasks
        // ran, and the bucket holds nothing at all for one that ran none.
        let prefix = job_prefix(row.prefix, &job_id);
        let written = store.keys_under(REGION, &reports, "").await;
        if !row.runs() {
            assert!(
                written.is_empty(),
                "case {case:?}: expected a job that ran no task to write no report, got {written:?}"
            );
            continue;
        }
        assert!(
            written
                .iter()
                .all(|key| key.starts_with(&format!("{prefix}/"))),
            "case {case:?}: expected everything the job wrote to be under {prefix}/, got \
             {written:?}"
        );

        // The report's own manifest is the one fixed key a reader starts from,
        // and it names the result file beside it.
        let manifest_key = format!("{prefix}/manifest.json");
        let (status, manifest) = store.object(REGION, &reports, &manifest_key).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the report manifest at {manifest_key} to be readable, got \
             {status}"
        );
        let manifest: serde_json::Value = serde_json::from_slice(&manifest).unwrap_or_else(|error| {
            panic!("case {case:?}: the report manifest is JSON: {error}")
        });
        let results_key = manifest["Results"][0]["Key"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the report manifest names its result file: {manifest}")
            })
            .to_string();
        assert!(
            results_key.starts_with(&format!("{prefix}/")),
            "case {case:?}: expected the result file under {prefix}/, got {results_key}"
        );

        // The result file accounts for exactly the tasks the report's scope
        // asks for, each row naming the object and what the task did.
        let (status, results) = store.object(REGION, &reports, &results_key).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the result file at {results_key} to be readable, got {status}"
        );
        let results = String::from_utf8_lossy(&results).into_owned();
        let accounted: Vec<&str> = results.lines().filter(|line| !line.is_empty()).collect();
        assert_eq!(
            accounted.len(),
            row.reported.len(),
            "case {case:?}: expected {} row(s) in a report over {}, got {accounted:?}",
            row.reported.len(),
            row.scope
        );
        for (key, reported) in row.reported.iter().zip(accounted.iter()) {
            let outcome = if row
                .objects
                .iter()
                .any(|(written_key, _)| written_key == key)
            {
                TASK_SUCCEEDED
            } else {
                TASK_FAILED
            };
            assert!(
                reported.contains(key) && reported.contains(outcome),
                "case {case:?}: expected the row for {key} to name it and the outcome \
                 {outcome:?}, got {reported:?}"
            );
        }
    }
}
