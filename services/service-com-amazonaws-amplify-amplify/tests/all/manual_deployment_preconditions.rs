// SPDX-License-Identifier: CC0-1.0

//! The manual-deployment contract a client branches on: which upload member a
//! reservation's answer carries, and what has to have HAPPENED before a reserved
//! deployment can be started.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `amplify` signing name, and every upload follows
//! the URL the service published — so what is asserted is what an AWS SDK client would
//! see.
//!
//! The rules under test, each over a TABLE spanning the class it generalises over
//! rather than over one recorded request:
//!
//! 1. `CreateDeployment`'s answer carries exactly ONE of its two upload members, chosen
//!    by the FORM of the request: `fileUploadUrls` is returned "when the fileMap
//!    argument is provided in the request" and `zipUploadUrl` "when the fileMap
//!    argument is not provided in the request"
//!    (`smithy_query(query="operation",operation="CreateDeployment")`), so the member
//!    the request's form does not select is ABSENT from the body rather than present
//!    and empty — which is how a client tells the two deployment modes apart.
//! 2. A reserved deployment is startable only once its content has ARRIVED at the
//!    destinations the reservation handed out: `StartDeployment` naming a reservation
//!    nothing was uploaded to is refused with the modelled `BadRequestException`
//!    (HTTP 400) and the service's fixed sentence, whatever form that reservation took,
//!    and the refusal starts nothing — the branch has still started no job. Uploading
//!    and calling again starts the same reservation.
//! 3. The `Branch` a `DeleteBranch` answers with reports the branch's job accounting as
//!    it stands: no active job and `'0'` jobs for a branch every start on which was
//!    refused, and the started job for a branch one did start.

use serde_json::{json, Value};

use service_com_amazonaws_amplify_amplify::ops::start_deployment::DEPLOYMENT_FILE_NOT_UPLOADED;
use service_com_amazonaws_amplify_amplify::state::JOB_STATUS_PENDING;

use crate::app_branch_lifecycle::{list_member, string_member, Emulator, Reply};

/// AWS's own sentence for a start whose deployment file never arrived, written out here
/// so the wording is pinned by the test and not only by the constant under test.
const EXPECTED_NOT_UPLOADED_MESSAGE: &str = "Deployment file has not been uploaded, \
     please make sure you uploaded the deployment file prior to invoke this api.";

/// `FileName`'s modelled maximum length (`smithy_query(query="shape",shape="FileMap")`:
/// key `length 0..255`), so a request may name a file 255 characters long.
const FILE_NAME_MAX: usize = 255;

/// `CreateDeployment` over the wire.
async fn create_deployment(
    emulator: &Emulator,
    app_id: &str,
    branch: &str,
    request: &Value,
) -> Reply {
    emulator
        .call(
            "POST",
            &format!("/apps/{app_id}/branches/{branch}/deployments"),
            Some(request),
        )
        .await
}

/// `StartDeployment` over the wire.
async fn start_deployment(
    emulator: &Emulator,
    app_id: &str,
    branch: &str,
    request: &Value,
) -> Reply {
    emulator
        .call(
            "POST",
            &format!("/apps/{app_id}/branches/{branch}/deployments/start"),
            Some(request),
        )
        .await
}

/// `StartJob` over the wire.
async fn start_job(emulator: &Emulator, app_id: &str, branch: &str, request: &Value) -> Reply {
    emulator
        .call(
            "POST",
            &format!("/apps/{app_id}/branches/{branch}/jobs"),
            Some(request),
        )
        .await
}

/// `GetJob` over the wire.
async fn get_job(emulator: &Emulator, app_id: &str, branch: &str, job_id: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!("/apps/{app_id}/branches/{branch}/jobs/{job_id}"),
            None,
        )
        .await
}

/// `ListJobs` over the wire.
async fn list_jobs(emulator: &Emulator, app_id: &str, branch: &str) -> Reply {
    emulator
        .call("GET", &format!("/apps/{app_id}/branches/{branch}/jobs"), None)
        .await
}

/// A branch on `app_id`.
async fn branch_on(emulator: &Emulator, app_id: &str, branch: &str) {
    let created = emulator
        .create_branch(app_id, &json!({ "branchName": branch }))
        .await;
    assert_eq!(
        created.status, 200,
        "app {app_id}: expected HTTP 200 from CreateBranch on {branch:?}, got {created:?}"
    );
}

/// Follow an upload destination exactly as a client does: the `PUT` the service
/// published, against the URL it published, with no credential of any kind.
async fn upload(emulator: &Emulator, case: &str, url: &str, body: &[u8]) {
    let request = http::Request::builder()
        .method("PUT")
        .uri(url)
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("an upload destination is a well-formed URL");
    let status = emulator.dispatch(request).await.status().as_u16();
    assert_eq!(
        status, 200,
        "case {case:?}: the destination {url:?} must accept the upload, got {status}"
    );
}

/// Does this branch record report NO active job?
///
/// `Branch$activeJobId` is `@required` with a `length` minimum of 0
/// (`smithy_query(query="shape",shape="Branch")`), and Amplify reports a member it has
/// no value for as JSON `null` rather than as the empty string — so a branch with no
/// active job carries no id, whether the member is `null` or absent.
fn reports_no_active_job(record: &Value) -> bool {
    record.get("activeJobId").is_none_or(Value::is_null)
}

/// One `CreateDeployment` request, and which of the two upload members its answer has
/// to carry.
struct FormCase {
    case: String,
    request: Value,
    /// The file names the answer must carry a destination for: empty for the zipped
    /// form, which carries `zipUploadUrl` instead.
    named_files: Vec<String>,
}

/// The table for rule 1: one row per way a request can describe what it is about to
/// upload.
fn form_cases() -> Vec<FormCase> {
    // `MD5Hash` is modelled `length 0..32` with no further constraint
    // (`smithy_query(query="shape",shape="FileMap")`); nothing in this rule reads it.
    let hash = "d41d8cd98f00b204e9800998ecf8427e";
    let longest = "f".repeat(FILE_NAME_MAX);
    let mut longest_map = serde_json::Map::new();
    longest_map.insert(longest.clone(), Value::String(hash.to_string()));
    vec![
        FormCase {
            case: "no fileMap member".to_string(),
            request: json!({}),
            named_files: Vec::new(),
        },
        FormCase {
            case: "a fileMap naming no file".to_string(),
            request: json!({ "fileMap": {} }),
            named_files: Vec::new(),
        },
        FormCase {
            case: "one named file".to_string(),
            request: json!({ "fileMap": { "index.html": hash } }),
            named_files: vec!["index.html".to_string()],
        },
        FormCase {
            case: "three named files under nested prefixes".to_string(),
            request: json!({ "fileMap": {
                "index.html": hash,
                "assets/app.js": hash,
                "assets/styles/main.css": hash,
            }}),
            named_files: vec![
                "assets/app.js".to_string(),
                "assets/styles/main.css".to_string(),
                "index.html".to_string(),
            ],
        },
        FormCase {
            case: "a file name carrying spaces and non-ASCII".to_string(),
            request: json!({ "fileMap": { "a ünïcode ƒile.txt": hash } }),
            named_files: vec!["a ünïcode ƒile.txt".to_string()],
        },
        FormCase {
            case: "the longest modelled file name".to_string(),
            request: json!({ "fileMap": Value::Object(longest_map) }),
            named_files: vec![longest],
        },
    ]
}

/// Rule 1: the answer carries the upload member the request's form selects, and leaves
/// the other one OUT of the body.
#[tokio::test]
async fn a_reservation_reports_only_the_upload_member_its_form_selects() {
    let emulator = Emulator::new();
    let app_id = emulator.app_id_of_new_app("deployment-answer-form").await;

    for (index, row) in form_cases().into_iter().enumerate() {
        // One row per BRANCH: a branch serializes its deployments, and what this rule is
        // about is the shape of ONE answer.
        let branch = format!("row-{index}");
        branch_on(&emulator, &app_id, &branch).await;

        let reply = create_deployment(&emulator, &app_id, &branch, &row.request).await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected HTTP 200 from CreateDeployment, got {reply:?}",
            row.case
        );

        let per_file = reply.body.get("fileUploadUrls");
        let zip = reply.body.get("zipUploadUrl");
        if row.named_files.is_empty() {
            assert!(
                per_file.is_none(),
                "case {:?}: a request naming no file selects the zipped form, so the \
                 answer must not carry fileUploadUrls at all, got {per_file:?}",
                row.case
            );
            let zip = zip.and_then(Value::as_str).unwrap_or_default();
            assert!(
                !zip.is_empty(),
                "case {:?}: the zipped form carries the single upload destination, \
                 got {zip:?}",
                row.case
            );
        } else {
            assert!(
                zip.is_none(),
                "case {:?}: a request naming its files selects the per-file form, so the \
                 answer must not carry zipUploadUrl at all, got {zip:?}",
                row.case
            );
            // `fileUploadUrls` is a MAP, so the ORDER its members arrive in is not part
            // of the contract — what the rule fixes is WHICH files carry a destination.
            // Collected into a sorted set, like the sibling suite's `file_upload_urls`,
            // so the comparison reads the same whichever order the body enumerates.
            let destinations: std::collections::BTreeSet<String> = per_file
                .and_then(Value::as_object)
                .map(|members| members.keys().cloned().collect())
                .unwrap_or_default();
            let named: std::collections::BTreeSet<String> =
                row.named_files.iter().cloned().collect();
            assert_eq!(
                destinations, named,
                "case {:?}: expected a destination for exactly the files named, \
                 got {destinations:?}",
                row.case
            );
        }
    }
}

/// Rule 2: a reservation nothing was uploaded to cannot be started, and uploading then
/// starting the same reservation does start it.
#[tokio::test]
async fn a_deployment_whose_file_never_arrived_is_not_started() {
    assert_eq!(
        EXPECTED_NOT_UPLOADED_MESSAGE, DEPLOYMENT_FILE_NOT_UPLOADED,
        "the refusal this service states must be AWS's sentence"
    );

    let emulator = Emulator::new();
    let app_id = emulator.app_id_of_new_app("deployment-upload-precondition").await;

    // One row per form a reservation can take: the fixed sentence carries nothing from
    // the request, so every row takes the identical refusal.
    let hash = "d41d8cd98f00b204e9800998ecf8427e";
    let rows: Vec<(&str, Value)> = vec![
        ("a zipped deployment", json!({})),
        ("a fileMap naming no file", json!({ "fileMap": {} })),
        (
            "one named file",
            json!({ "fileMap": { "index.html": hash } }),
        ),
        (
            "three named files",
            json!({ "fileMap": {
                "index.html": hash,
                "assets/app.js": hash,
                "assets/styles/main.css": hash,
            }}),
        ),
    ];

    for (index, (case, request)) in rows.into_iter().enumerate() {
        let branch = format!("row-{index}");
        branch_on(&emulator, &app_id, &branch).await;

        let reserved = create_deployment(&emulator, &app_id, &branch, &request).await;
        assert_eq!(
            reserved.status, 200,
            "case {case:?}: expected HTTP 200 from CreateDeployment, got {reserved:?}"
        );
        let job_id = string_member(&reserved.body, "jobId");

        // Nothing has been uploaded to any destination this reservation handed out.
        let refused =
            start_deployment(&emulator, &app_id, &branch, &json!({ "jobId": job_id })).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected the modelled HTTP 400, got {refused:?}"
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("BadRequestException"),
            "case {case:?}: expected the modelled BadRequestException, got {:?}",
            refused.error_code
        );
        assert_eq!(
            refused.message().as_deref(),
            Some(EXPECTED_NOT_UPLOADED_MESSAGE),
            "case {case:?}: expected {EXPECTED_NOT_UPLOADED_MESSAGE:?}, got {:?}",
            refused.message()
        );

        // The refusal started nothing: the reservation's job is still the one nothing has
        // started, and the branch has started no job.
        let job = get_job(&emulator, &app_id, &branch, &job_id).await;
        let summary = job.member("job").get("summary").cloned().unwrap_or(Value::Null);
        assert_eq!(
            string_member(&summary, "status"),
            JOB_STATUS_PENDING,
            "case {case:?}: a refused start leaves the job nothing has started, \
             got {summary:?}"
        );
        let record = emulator.get_branch(&app_id, &branch).await.member("branch");
        assert_eq!(
            string_member(&record, "totalNumberOfJobs"),
            "0",
            "case {case:?}: a refused start is not a started job, got {record:?}"
        );
        assert!(
            reports_no_active_job(&record),
            "case {case:?}: a refused start leaves no active job, got {record:?}"
        );

        // The refusal is a PRECONDITION, not a dead end: the upload it asked for, and
        // the identical call, start the same reservation.
        let zip = reserved
            .body
            .get("zipUploadUrl")
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(url) = zip {
            upload(&emulator, case, &url, &zip_of(&["index.html"])).await;
        }
        let per_file: Vec<(String, String)> = reserved
            .body
            .get("fileUploadUrls")
            .and_then(Value::as_object)
            .map(|members| {
                members
                    .iter()
                    .map(|(name, url)| {
                        (
                            name.clone(),
                            url.as_str().expect("a destination is a string").to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        for (name, url) in &per_file {
            upload(&emulator, case, url, format!("the bytes of {name}").as_bytes()).await;
        }

        let started =
            start_deployment(&emulator, &app_id, &branch, &json!({ "jobId": job_id })).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: an uploaded reservation starts, got {started:?}"
        );
        assert_eq!(
            string_member(&started.member("jobSummary"), "jobId"),
            job_id,
            "case {case:?}: the deployment runs under the id it reserved, got {started:?}"
        );
        let record = emulator.get_branch(&app_id, &branch).await.member("branch");
        assert_eq!(
            string_member(&record, "totalNumberOfJobs"),
            "1",
            "case {case:?}: the started deployment is counted, got {record:?}"
        );
        assert_eq!(
            string_member(&record, "activeJobId"),
            job_id,
            "case {case:?}: the started deployment is the branch's active job, \
             got {record:?}"
        );
        let listed = list_jobs(&emulator, &app_id, &branch).await;
        assert_eq!(
            list_member(&listed.body, "jobSummaries").len(),
            1,
            "case {case:?}: the branch holds the one job it started, got {listed:?}"
        );
    }
}

/// One branch a `DeleteBranch` is asked for, and the accounting its answer has to
/// report.
struct TeardownCase {
    case: &'static str,
    /// Whether the app the branch sits on is connected to a repository provider.
    repository: bool,
    /// The active job the deleted branch reports: `None` for a branch with none.
    active: Option<&'static str>,
    /// The `totalNumberOfJobs` it reports.
    total: &'static str,
}

/// Rule 3: the deleted branch reports the job accounting as it stands — nothing for a
/// branch every start on which was refused, and the started job for one that started.
#[tokio::test]
async fn a_deleted_branch_reports_the_jobs_it_actually_started() {
    let emulator = Emulator::new();

    let rows = vec![
        TeardownCase {
            case: "a refused StartJob on an app with no repository",
            repository: false,
            active: None,
            total: "0",
        },
        TeardownCase {
            case: "a refused StartDeployment of a reservation nothing was uploaded to",
            repository: false,
            active: None,
            total: "0",
        },
        TeardownCase {
            case: "a build that did start",
            repository: true,
            active: Some("1"),
            total: "1",
        },
        TeardownCase {
            case: "a deployment that did start",
            repository: false,
            active: Some("1"),
            total: "1",
        },
    ];

    for row in rows {
        let name = format!("teardown-{}", row.case.replace(' ', "-"));
        let app_id = if row.repository {
            emulator.app_id_of_new_repository_app(&name).await
        } else {
            emulator.app_id_of_new_app(&name).await
        };
        branch_on(&emulator, &app_id, "main").await;

        match (row.case, row.active) {
            ("a refused StartJob on an app with no repository", _) => {
                let refused =
                    start_job(&emulator, &app_id, "main", &json!({ "jobType": "MANUAL" })).await;
                assert_eq!(
                    refused.status, 400,
                    "case {:?}: expected the modelled HTTP 400, got {refused:?}",
                    row.case
                );
            }
            (_, None) => {
                let reserved = create_deployment(&emulator, &app_id, "main", &json!({})).await;
                let job_id = string_member(&reserved.body, "jobId");
                let refused =
                    start_deployment(&emulator, &app_id, "main", &json!({ "jobId": job_id }))
                        .await;
                assert_eq!(
                    refused.status, 400,
                    "case {:?}: expected the modelled HTTP 400, got {refused:?}",
                    row.case
                );
            }
            ("a build that did start", _) => {
                let started =
                    start_job(&emulator, &app_id, "main", &json!({ "jobType": "RELEASE" })).await;
                assert_eq!(
                    started.status, 200,
                    "case {:?}: expected HTTP 200 from StartJob, got {started:?}",
                    row.case
                );
            }
            _ => {
                let reserved = create_deployment(&emulator, &app_id, "main", &json!({})).await;
                let job_id = string_member(&reserved.body, "jobId");
                let url = string_member(&reserved.body, "zipUploadUrl");
                upload(&emulator, row.case, &url, &zip_of(&["index.html"])).await;
                let started =
                    start_deployment(&emulator, &app_id, "main", &json!({ "jobId": job_id }))
                        .await;
                assert_eq!(
                    started.status, 200,
                    "case {:?}: expected HTTP 200 from StartDeployment, got {started:?}",
                    row.case
                );
            }
        }

        let deleted = emulator.delete_branch(&app_id, "main").await;
        assert_eq!(
            deleted.status, 200,
            "case {:?}: expected HTTP 200 from DeleteBranch, got {deleted:?}",
            row.case
        );
        let record = deleted.member("branch");
        assert_eq!(
            string_member(&record, "totalNumberOfJobs"),
            row.total,
            "case {:?}: expected totalNumberOfJobs {:?}, got {record:?}",
            row.case,
            row.total
        );
        match row.active {
            None => assert!(
                reports_no_active_job(&record),
                "case {:?}: expected no active job on the deleted branch, got {record:?}",
                row.case
            ),
            Some(job_id) => assert_eq!(
                string_member(&record, "activeJobId"),
                job_id,
                "case {:?}: expected the started job as the active one, got {record:?}",
                row.case
            ),
        }
    }
}

/// A `.zip` carrying one entry per path named, as a real archive a deployment can be:
/// the archive format is the `zip` crate's, here and in the handler that reads it back.
fn zip_of(paths: &[&str]) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(&mut buffer);
    for path in paths {
        writer
            .start_file(*path, zip::write::SimpleFileOptions::default())
            .expect("an archive entry starts");
        std::io::Write::write_all(&mut writer, b"deployed").expect("an archive entry is written");
    }
    writer.finish().expect("the archive closes");
    buffer.into_inner()
}
