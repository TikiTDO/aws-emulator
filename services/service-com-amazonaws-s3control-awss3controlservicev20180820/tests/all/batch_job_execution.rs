// SPDX-License-Identifier: CC0-1.0

//! What a submitted S3 Batch Operations job RUNS, driven end to end over HTTP
//! against Amazon S3 and S3 Control on one shared engine.
//!
//! The rule under test: `CreateJob` answers with a job id while the job is still
//! owed, and the job then RUNS — its one action is carried out against every
//! object its manifest lists, through Amazon S3's own API — after which it
//! reaches a terminal status that a caller polling `DescribeJob` observes:
//! `Complete` for a job whose tasks were run, however many of them failed, and
//! `Failed` for one that could not be run at all, with `FailureReasons` saying
//! why. `ProgressSummary` accounts for every row of the object list
//! (`TotalNumberOfTasks` = `NumberOfTasksSucceeded` + `NumberOfTasksFailed`),
//! objects outside the list are untouched, a manifest GENERATOR's filter is what
//! selects the list (and the manifest it writes out is an object of the output
//! location), and a job asked for a completion report has it written under
//! `<Prefix>/job-<JobId>/` before it reports its terminal status.
//!
//! The rows span what the rule generalises over: which action the job runs
//! (copy, tagging), how many rows the manifest carries and whether a row names
//! an object the bucket does not hold, the target prefix a copy writes under,
//! where the object list comes from (a manifest object, a generator with a key
//! constraint, a generator with a size bound), whether a completion report is
//! asked for and over which scope, and an action this emulator does not run.
//!
//! Every object read back here is one the object store itself holds, written by
//! the job's own run over the in-process callback and not by anything in this
//! suite.

use std::time::Duration;

use service_com_amazonaws_s3control_awss3controlservicev20180820::state::{
    JOB_RUN_AFTER, JOB_STATUS_COMPLETE, JOB_STATUS_FAILED,
};

use crate::access_point_lifecycle::{xml_escape, Reply, REGION, XMLNS};
use crate::object_store::{elements, ObjectStore};

/// The IAM role every job below runs its action with — the modelled
/// `IAMRoleArn` pattern (`^arn:[^:]+:iam::\d{12}:role/`).
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/S3BatchOperations";

/// How long to wait for a submitted job to settle: past the span the service
/// holds a job before its run begins, plus room for the tasks themselves.
fn settle_budget() -> Duration {
    JOB_RUN_AFTER + Duration::from_secs(10)
}

/// How often a caller polls `DescribeJob` for the job's outcome.
const POLL_EVERY: Duration = Duration::from_millis(100);

/// One `CreateJob` this suite submits.
struct JobRequest {
    /// The body of `<Operation>`: exactly one action.
    operation: String,
    /// The whole `<Manifest>` or `<ManifestGenerator>` element.
    source: String,
    /// The body of `<Report>`.
    report: String,
}

impl JobRequest {
    fn body(&self, client_request_token: &str) -> String {
        format!(
            "<CreateJobRequest xmlns=\"{XMLNS}\">\
             <ClientRequestToken>{}</ClientRequestToken>\
             <Operation>{}</Operation><Report>{}</Report>{}\
             <Priority>10</Priority><RoleArn>{ROLE_ARN}</RoleArn></CreateJobRequest>",
            xml_escape(client_request_token),
            self.operation,
            self.report,
            self.source
        )
    }
}

/// A `<Manifest>` element naming the manifest object `bucket`/`key`, in the CSV
/// format a manifest written for a batch job carries.
pub(crate) fn csv_manifest(bucket: &str, key: &str, e_tag: &str) -> String {
    format!(
        "<Manifest><Spec><Format>S3BatchOperations_CSV_20180820</Format>\
         <Fields><member>Bucket</member><member>Key</member></Fields></Spec>\
         <Location><ObjectArn>arn:aws:s3:::{bucket}/{key}</ObjectArn>\
         <ETag>{}</ETag></Location></Manifest>",
        xml_escape(e_tag)
    )
}

/// A `<Report>` element asking for a completion report in `bucket` under
/// `prefix`, over `scope`.
fn report(bucket: &str, prefix: &str, scope: &str) -> String {
    format!(
        "<Bucket>arn:aws:s3:::{bucket}</Bucket><Format>Report_CSV_20180820</Format>\
         <Enabled>true</Enabled><Prefix>{prefix}</Prefix><ReportScope>{scope}</ReportScope>"
    )
}

/// A `<Report>` element asking for no report at all.
const NO_REPORT: &str = "<Enabled>false</Enabled>";

/// Submit one job and answer the job id the service assigned.
async fn submit(store: &ObjectStore, case: &str, token: &str, request: &JobRequest) -> String {
    let reply = store
        .s3control(REGION, "POST", "/v20180820/jobs", &request.body(token))
        .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected CreateJob to answer 200, got {} with body {}",
        reply.status, reply.body
    );
    reply
        .text("JobId")
        .unwrap_or_else(|| panic!("case {case:?}: CreateJob answers a JobId: {}", reply.body))
}

/// `DescribeJob` of one job.
pub(crate) async fn describe(store: &ObjectStore, job_id: &str) -> Reply {
    store
        .s3control(REGION, "GET", &format!("/v20180820/jobs/{job_id}"), "")
        .await
}

/// Poll `DescribeJob` until the job reports a terminal status, exactly as a
/// client waits for one, and answer that answer.
pub(crate) async fn poll_terminal(store: &ObjectStore, case: &str, job_id: &str) -> Reply {
    let deadline = std::time::Instant::now() + settle_budget();
    let mut last = describe(store, job_id).await;
    loop {
        let status = last.text("Status").unwrap_or_default();
        if status == JOB_STATUS_COMPLETE
            || status == JOB_STATUS_FAILED
            || status == "Cancelled"
        {
            return last;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: the job did not reach a terminal status inside {:?}; last status \
             {status:?} in body {}",
            settle_budget(),
            last.body
        );
        tokio::time::sleep(POLL_EVERY).await;
        last = describe(store, job_id).await;
    }
}

/// The three `ProgressSummary` counters a settled job reports.
fn progress(case: &str, described: &Reply) -> (i64, i64, i64) {
    let read = |element: &str| -> i64 {
        described
            .text(element)
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected the settled job to report {element}, got body {}",
                    described.body
                )
            })
            .parse()
            .unwrap_or_else(|error| {
                panic!("case {case:?}: {element} is a number: {error}, body {}", described.body)
            })
    };
    (
        read("TotalNumberOfTasks"),
        read("NumberOfTasksSucceeded"),
        read("NumberOfTasksFailed"),
    )
}

/// One row of the copy table: the manifest rows a job runs over, the target
/// prefix it copies under, and which keys must end up in the destination.
struct CopyRow {
    case: &'static str,
    /// The keys written into the source bucket; every one of them is an object
    /// the destination must NOT hold unless the manifest lists it.
    written: &'static [(&'static str, &'static [u8])],
    /// The keys the manifest lists, in manifest order.
    listed: &'static [&'static str],
    /// `TargetKeyPrefix`, or `None` for a copy that keeps the source key.
    target_prefix: Option<&'static str>,
}

/// RULE: a copy job writes every object its manifest lists into the target
/// bucket under the target prefix, byte for byte, leaves every other object
/// alone, and settles `Complete` with a `ProgressSummary` accounting for each
/// row.
#[tokio::test]
async fn a_copy_job_copies_the_objects_its_manifest_lists_and_leaves_the_rest() {
    let rows = [
        CopyRow {
            case: "two of three objects, copied under a folder",
            written: &[
                ("finance/q1.csv", b"q1-bytes"),
                ("finance/q2.csv", b"q2-bytes-longer"),
                ("finance/q3.csv", b"q3-is-not-listed"),
            ],
            listed: &["finance/q1.csv", "finance/q2.csv"],
            target_prefix: Some("copies"),
        },
        CopyRow {
            case: "one object, no target prefix",
            written: &[("report.txt", b"the-one-row"), ("other.txt", b"untouched")],
            listed: &["report.txt"],
            target_prefix: None,
        },
        CopyRow {
            case: "a prefix written with a separator, over an empty object",
            written: &[("logs/empty", b""), ("logs/kept", b"kept")],
            listed: &["logs/empty"],
            target_prefix: Some("archive/2026/"),
        },
    ];

    for (row_at, row) in rows.iter().enumerate() {
        let store = ObjectStore::start();
        let source = format!("batch-copy-source-{row_at}");
        let destination = format!("batch-copy-destination-{row_at}");
        let manifests = format!("batch-copy-manifests-{row_at}");
        store.create_bucket(row.case, REGION, &source).await;
        store.create_bucket(row.case, REGION, &destination).await;
        store.create_bucket(row.case, REGION, &manifests).await;
        for (key, body) in row.written {
            store
                .put_object(row.case, REGION, &source, key, body)
                .await;
        }
        let manifest_body: String = row
            .listed
            .iter()
            .map(|key| format!("{source},{key}\n"))
            .collect();
        let e_tag = store
            .put_object(
                row.case,
                REGION,
                &manifests,
                "objects.csv",
                manifest_body.as_bytes(),
            )
            .await;

        let copy = match row.target_prefix {
            Some(prefix) => format!(
                "<S3PutObjectCopy><TargetResource>arn:aws:s3:::{destination}</TargetResource>\
                 <TargetKeyPrefix>{prefix}</TargetKeyPrefix></S3PutObjectCopy>"
            ),
            None => format!(
                "<S3PutObjectCopy><TargetResource>arn:aws:s3:::{destination}</TargetResource>\
                 </S3PutObjectCopy>"
            ),
        };
        let job_id = submit(
            &store,
            row.case,
            &format!("copy-{row_at}"),
            &JobRequest {
                operation: copy,
                source: csv_manifest(&manifests, "objects.csv", &e_tag),
                report: NO_REPORT.to_string(),
            },
        )
        .await;

        let settled = poll_terminal(&store, row.case, &job_id).await;
        assert_eq!(
            settled.text("Status").as_deref(),
            Some(JOB_STATUS_COMPLETE),
            "case {:?}: expected the job to complete, got body {}",
            row.case,
            settled.body
        );
        let listed = i64::try_from(row.listed.len()).expect("the row count fits");
        assert_eq!(
            progress(row.case, &settled),
            (listed, listed, 0),
            "case {:?}: expected every manifest row accounted for as a task that ran, got \
             body {}",
            row.case,
            settled.body
        );

        // Every listed object is in the destination, byte for byte, under the
        // target prefix.
        for key in row.listed {
            let expected = row
                .written
                .iter()
                .find(|(written, _)| written == key)
                .map(|(_, body)| *body)
                .expect("the row lists a key it wrote");
            let copied_key = match row.target_prefix {
                Some(prefix) => format!("{}/{key}", prefix.trim_end_matches('/')),
                None => (*key).to_string(),
            };
            let (status, bytes) = store.object(REGION, &destination, &copied_key).await;
            assert_eq!(
                status, 200,
                "case {:?}: expected the copy at {copied_key} to be readable, got {status}",
                row.case
            );
            assert_eq!(
                bytes,
                expected.to_vec(),
                "case {:?}: expected the copy at {copied_key} to carry the source bytes, got \
                 {:?}",
                row.case,
                String::from_utf8_lossy(&bytes)
            );
        }
        // Nothing the manifest did not list was copied.
        let copied = store.keys_under(REGION, &destination, "").await;
        assert_eq!(
            copied.len(),
            row.listed.len(),
            "case {:?}: expected the destination to hold one object per manifest row, got \
             {copied:?}",
            row.case
        );
    }
}

/// RULE: a tagging job sets the job's tag set on every object its manifest
/// lists; a row naming an object the bucket does not hold is a task that
/// FAILED, which leaves the job `Complete` with both outcomes accounted for —
/// and the completion report under `<Prefix>/job-<JobId>/` carries the tasks its
/// scope asks for.
#[tokio::test]
async fn a_tagging_job_accounts_for_mixed_task_outcomes_in_its_completion_report() {
    let scopes = [
        ("a report over all tasks", "AllTasks", 2_usize),
        ("a report over failed tasks only", "FailedTasksOnly", 1),
    ];

    for (row_at, (case, scope, report_rows)) in scopes.iter().enumerate() {
        let store = ObjectStore::start();
        let bucket = format!("batch-tagging-{row_at}");
        let reports = format!("batch-tagging-reports-{row_at}");
        store.create_bucket(case, REGION, &bucket).await;
        store.create_bucket(case, REGION, &reports).await;
        store
            .put_object(case, REGION, &bucket, "present.csv", b"present")
            .await;
        store
            .put_object(case, REGION, &bucket, "untouched.csv", b"untouched")
            .await;
        let manifest_body = format!("{bucket},present.csv\n{bucket},absent.csv\n");
        let e_tag = store
            .put_object(
                case,
                REGION,
                &bucket,
                "manifest.csv",
                manifest_body.as_bytes(),
            )
            .await;

        let job_id = submit(
            &store,
            case,
            &format!("tagging-{row_at}"),
            &JobRequest {
                operation: "<S3PutObjectTagging><TagSet>\
                            <member><Key>stage</Key><Value>archived</Value></member>\
                            </TagSet></S3PutObjectTagging>"
                    .to_string(),
                source: csv_manifest(&bucket, "manifest.csv", &e_tag),
                report: report(&reports, "reports/batch", scope),
            },
        )
        .await;

        let settled = poll_terminal(&store, case, &job_id).await;
        assert_eq!(
            settled.text("Status").as_deref(),
            Some(JOB_STATUS_COMPLETE),
            "case {case:?}: a job whose tasks ran is Complete however many failed, got body {}",
            settled.body
        );
        assert_eq!(
            progress(case, &settled),
            (2, 1, 1),
            "case {case:?}: expected both manifest rows accounted for, one task each way, got \
             body {}",
            settled.body
        );

        // The object the manifest listed carries the job's tag set; the one it
        // did not is untouched.
        assert_eq!(
            store.object_tags(REGION, &bucket, "present.csv").await,
            vec![("stage".to_string(), "archived".to_string())],
            "case {case:?}: expected the listed object to carry the job's tag set"
        );
        assert!(
            store
                .object_tags(REGION, &bucket, "untouched.csv")
                .await
                .is_empty(),
            "case {case:?}: expected an object the manifest did not list to stay untagged"
        );

        // The completion report is under `<Prefix>/job-<JobId>/`, and its own
        // manifest names the result file it wrote.
        let prefix = format!("reports/batch/job-{job_id}");
        let written = store.keys_under(REGION, &reports, "reports/batch").await;
        assert!(
            written.iter().any(|key| key == &format!("{prefix}/manifest.json")),
            "case {case:?}: expected the report's manifest under {prefix}/, got {written:?}"
        );
        let (status, manifest) = store
            .object(REGION, &reports, &format!("{prefix}/manifest.json"))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the report manifest to be readable, got {status}"
        );
        let manifest: serde_json::Value = serde_json::from_slice(&manifest)
            .unwrap_or_else(|error| panic!("case {case:?}: the report manifest is JSON: {error}"));
        let results_key = manifest["Results"][0]["Key"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the report manifest names its result file: {manifest}"))
            .to_string();
        assert!(
            results_key.starts_with(&prefix),
            "case {case:?}: expected the result file under {prefix}/, got {results_key}"
        );
        let (status, results) = store.object(REGION, &reports, &results_key).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the result file to be readable, got {status}"
        );
        let results = String::from_utf8_lossy(&results).into_owned();
        let rows: Vec<&str> = results.lines().filter(|line| !line.is_empty()).collect();
        assert_eq!(
            rows.len(),
            *report_rows,
            "case {case:?}: expected {report_rows} row(s) in the report for scope {scope}, got \
             {rows:?}"
        );
        assert!(
            rows.iter()
                .any(|row| row.contains("absent.csv") && row.contains("failed")),
            "case {case:?}: expected the failed task's row to name the object and the outcome, \
             got {rows:?}"
        );
    }
}

/// One row of the generator table: what the source bucket holds, the filter the
/// generator carries, and which keys that filter selects.
struct GeneratorRow {
    case: &'static str,
    /// The objects written into the source bucket, each with its own bytes (the
    /// size bound reads their length).
    written: &'static [(&'static str, &'static [u8])],
    /// The body of `<Filter>`.
    filter: &'static str,
    /// The keys the filter must select, in listing order.
    selected: &'static [&'static str],
}

/// RULE: a job whose object list comes from a manifest GENERATOR runs over
/// exactly the objects of the source bucket its `Filter` selects, and a
/// generator asked to write its manifest out leaves it as an object of the
/// output location, described by the job itself.
#[tokio::test]
async fn a_generated_manifest_limits_the_job_to_the_objects_its_filter_selects() {
    let rows = [
        GeneratorRow {
            case: "a key-name prefix constraint",
            written: &[
                ("logs/a.txt", b"aaa"),
                ("logs/b.txt", b"bbb"),
                ("metrics/c.txt", b"ccc"),
            ],
            filter: "<KeyNameConstraint><MatchAnyPrefix><member>logs/</member>\
                     </MatchAnyPrefix></KeyNameConstraint>",
            selected: &["logs/a.txt", "logs/b.txt"],
        },
        GeneratorRow {
            case: "a key-name suffix constraint",
            written: &[
                ("one.csv", b"1"),
                ("two.txt", b"2"),
                ("three.csv", b"3"),
            ],
            filter: "<KeyNameConstraint><MatchAnySuffix><member>.csv</member>\
                     </MatchAnySuffix></KeyNameConstraint>",
            selected: &["one.csv", "three.csv"],
        },
        GeneratorRow {
            case: "a size bound",
            written: &[("small", b"ab"), ("large", b"abcdefghij")],
            filter: "<ObjectSizeGreaterThanBytes>5</ObjectSizeGreaterThanBytes>",
            selected: &["large"],
        },
    ];

    for (row_at, row) in rows.iter().enumerate() {
        let store = ObjectStore::start();
        let source = format!("batch-generated-source-{row_at}");
        let destination = format!("batch-generated-destination-{row_at}");
        store.create_bucket(row.case, REGION, &source).await;
        store.create_bucket(row.case, REGION, &destination).await;
        for (key, body) in row.written {
            store
                .put_object(row.case, REGION, &source, key, body)
                .await;
        }

        let generator = format!(
            "<ManifestGenerator><S3JobManifestGenerator>\
             <SourceBucket>arn:aws:s3:::{source}</SourceBucket>\
             <ManifestOutputLocation><Bucket>arn:aws:s3:::{destination}</Bucket>\
             <ManifestPrefix>generated</ManifestPrefix>\
             <ManifestFormat>S3InventoryReport_CSV_20211130</ManifestFormat>\
             </ManifestOutputLocation>\
             <EnableManifestOutput>true</EnableManifestOutput>\
             <Filter>{}</Filter></S3JobManifestGenerator></ManifestGenerator>",
            row.filter
        );
        let job_id = submit(
            &store,
            row.case,
            &format!("generated-{row_at}"),
            &JobRequest {
                operation: "<S3PutObjectTagging><TagSet>\
                            <member><Key>selected</Key><Value>yes</Value></member>\
                            </TagSet></S3PutObjectTagging>"
                    .to_string(),
                source: generator,
                report: NO_REPORT.to_string(),
            },
        )
        .await;

        let settled = poll_terminal(&store, row.case, &job_id).await;
        assert_eq!(
            settled.text("Status").as_deref(),
            Some(JOB_STATUS_COMPLETE),
            "case {:?}: expected the job to complete, got body {}",
            row.case,
            settled.body
        );
        let selected = i64::try_from(row.selected.len()).expect("the row count fits");
        assert_eq!(
            progress(row.case, &settled),
            (selected, selected, 0),
            "case {:?}: expected one task per selected object, got body {}",
            row.case,
            settled.body
        );

        // Only the selected objects were acted on.
        for (key, _) in row.written {
            let tagged = !store.object_tags(REGION, &source, key).await.is_empty();
            assert_eq!(
                tagged,
                row.selected.contains(key),
                "case {:?}: expected {key} to be acted on {}, got {tagged}",
                row.case,
                row.selected.contains(key)
            );
        }

        // The generated manifest is an object of the output location, located by
        // the job's own `GeneratedManifestDescriptor`: that object names the
        // data file beside it, which lists exactly the selected keys.
        let descriptor = settled.text("ObjectArn").unwrap_or_else(|| {
            panic!(
                "case {:?}: the job describes its generated manifest, got body {}",
                row.case, settled.body
            )
        });
        let manifest_key = descriptor
            .split_once(&format!("{destination}/"))
            .map(|(_, key)| key.to_string())
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the descriptor names an object of the output bucket, got \
                     {descriptor}",
                    row.case
                )
            });
        let (status, manifest) = store.object(REGION, &destination, &manifest_key).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the generated manifest at {manifest_key} to be readable, got \
             {status}",
            row.case
        );
        let manifest: serde_json::Value = serde_json::from_slice(&manifest).unwrap_or_else(|error| {
            panic!(
                "case {:?}: the generated manifest's own manifest is JSON: {error}",
                row.case
            )
        });
        let data_key = manifest["files"][0]["key"].as_str().unwrap_or_else(|| {
            panic!(
                "case {:?}: the manifest names its data file, got {manifest}",
                row.case
            )
        });
        let (status, body) = store.object(REGION, &destination, data_key).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the manifest's data file at {data_key} to be readable, got \
             {status}",
            row.case
        );
        let rendered = String::from_utf8_lossy(&body).into_owned();
        let listed: Vec<&str> = rendered
            .lines()
            .filter_map(|line| line.split_once(','))
            .map(|(_, key)| key)
            .collect();
        assert_eq!(
            listed, *row.selected,
            "case {:?}: expected the written manifest to list exactly the selected keys, got \
             {rendered:?}",
            row.case
        );
    }
}

/// RULE: a job whose run cannot be carried out reaches `Failed` with
/// `FailureReasons` naming what stopped it — never a status claiming work that
/// nothing did, and never a job left waiting for ever.
#[tokio::test]
async fn a_job_that_cannot_be_run_fails_with_the_reason_that_stopped_it() {
    let store = ObjectStore::start();
    let bucket = "batch-failing";
    store.create_bucket("fixture", REGION, bucket).await;
    let e_tag = store
        .put_object(
            "fixture",
            REGION,
            bucket,
            "manifest.csv",
            format!("{bucket},manifest.csv\n").as_bytes(),
        )
        .await;

    let rows: [(&str, String, String); 3] = [
        (
            "a manifest object the bucket does not hold",
            "<S3PutObjectTagging><TagSet><member><Key>k</Key><Value>v</Value></member>\
             </TagSet></S3PutObjectTagging>"
                .to_string(),
            csv_manifest(bucket, "absent-manifest.csv", "\"0\""),
        ),
        (
            "an action this emulator does not run",
            "<S3InitiateRestoreObject><ExpirationInDays>7</ExpirationInDays>\
             <GlacierJobTier>BULK</GlacierJobTier></S3InitiateRestoreObject>"
                .to_string(),
            csv_manifest(bucket, "manifest.csv", &e_tag),
        ),
        (
            "a generated manifest whose filter this emulator cannot apply",
            "<S3PutObjectTagging><TagSet><member><Key>k</Key><Value>v</Value></member>\
             </TagSet></S3PutObjectTagging>"
                .to_string(),
            format!(
                "<ManifestGenerator><S3JobManifestGenerator>\
                 <SourceBucket>arn:aws:s3:::{bucket}</SourceBucket>\
                 <EnableManifestOutput>false</EnableManifestOutput>\
                 <Filter><EligibleForReplication>true</EligibleForReplication></Filter>\
                 </S3JobManifestGenerator></ManifestGenerator>"
            ),
        ),
    ];

    for (row_at, (case, operation, source)) in rows.iter().enumerate() {
        let job_id = submit(
            &store,
            case,
            &format!("failing-{row_at}"),
            &JobRequest {
                operation: operation.clone(),
                source: source.clone(),
                report: NO_REPORT.to_string(),
            },
        )
        .await;
        let settled = poll_terminal(&store, case, &job_id).await;
        assert_eq!(
            settled.text("Status").as_deref(),
            Some(JOB_STATUS_FAILED),
            "case {case:?}: expected the job to fail, got body {}",
            settled.body
        );
        let reasons = elements(&settled.body, "FailureReason");
        assert_eq!(
            reasons.len(),
            1,
            "case {case:?}: expected one failure reason, got body {}",
            settled.body
        );
        assert!(
            !reasons[0].is_empty(),
            "case {case:?}: expected the failure reason to say what stopped the job, got body {}",
            settled.body
        );
        assert!(
            settled.text("TotalNumberOfTasks").is_none(),
            "case {case:?}: expected no task count on a job that ran nothing, got body {}",
            settled.body
        );
    }
}
