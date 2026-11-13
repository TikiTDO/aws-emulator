// SPDX-License-Identifier: CC0-1.0

//! `CreateDeployment`: reserving a manual deployment on a branch of an Amplify app.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `amplify` signing name — so what is asserted is
//! what an AWS SDK client would see.
//!
//! The rules under test, each over a TABLE spanning the class it generalises over
//! rather than over one recorded request:
//!
//! 1. A reservation answers the upload destinations for what the request asked to
//!    upload: a request that NAMES files gets one destination per file name, keyed by
//!    that name, and no single zip destination; a request that names none gets the one
//!    destination for the zipped deployment. Every destination is unique and inside
//!    `UploadUrl`'s modelled length bound, whatever the request named its files.
//! 2. A reservation takes a `jobId` out of its branch's id sequence — decimal digits,
//!    as `JobId` is modelled — and that id is SPENT: a later deployment and a later
//!    `StartJob` on the same branch are handed different ones.
//! 3. A reservation CREATES the deployment's job: "the maximum duration between the
//!    CreateDeployment call and the StartDeployment call cannot exceed 8 hours. If the
//!    duration exceeds 8 hours, the StartDeployment call and the associated Job will
//!    fail" (`smithy_query(query="doc",shape="CreateDeployment")`), so the job is
//!    readable through `GetJob` under the reserved id, in the `PENDING` status a job
//!    nothing has started carries and reporting none of the members such a job has no
//!    value for — while the branch has still STARTED no job.
//! 4. A deployment is reserved on a branch of an app, so an app id or a branch name
//!    that names nothing is refused with HTTP 404 and the not-found error type rather
//!    than created on the way.
//! 5. One branch's deployments are SERIALIZED: while the branch's last job is
//!    unfinished — reserved and unstarted, or started and still running — a further
//!    `CreateDeployment` is refused with the modelled 400 naming the job number THAT
//!    call spent, and finishing the branch's last job (by stopping it, or by letting the
//!    clock settle it) frees the branch again. The rule is per branch: another branch of
//!    the same app still reserves.
//!
//! The harness (`Emulator`, `Reply`) is the one `app_branch_lifecycle` already drives;
//! only the cases are new.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use service_com_amazonaws_amplify_amplify::state::JOB_STATUS_PENDING;

use crate::app_branch_lifecycle::{string_member, Emulator, Reply};

/// `UploadUrl`'s modelled maximum length, which every destination this operation
/// returns has to fit in (`smithy_query(query="shape",shape="UploadUrl")`:
/// `length 0..1000`).
const UPLOAD_URL_MAX: usize = 1000;

/// `FileName`'s modelled maximum length
/// (`smithy_query(query="shape",shape="FileMap")`: key `length 0..255`), so a request
/// may name a file 255 characters long.
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

/// `StopJob` over the wire. Stopping a job NOTHING HAS STARTED is complete when the
/// call answers, so the job is terminal from here on without waiting for any clock.
async fn stop_the_job(emulator: &Emulator, app_id: &str, branch: &str, job_id: &str) {
    let stopped = emulator
        .call(
            "DELETE",
            &format!("/apps/{app_id}/branches/{branch}/jobs/{job_id}/stop"),
            None,
        )
        .await;
    assert_eq!(
        stopped.status, 200,
        "stopping job {job_id:?} on {branch:?}: expected HTTP 200 from StopJob, got {stopped:?}"
    );
}

/// Wait until the clock has settled `job_id` out of its non-terminal status, looking
/// again until it has or a generous deadline passes: the span the service settles a job
/// over is its own, so a case waits for the CONDITION rather than for a fixed time.
///
/// The pause between reads is a blocking one because this workspace's `tokio` carries no
/// `time` feature, and a `#[tokio::test]` body owns its runtime thread for the duration
/// of the case.
async fn settled_status(emulator: &Emulator, app_id: &str, branch: &str, job_id: &str) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let job = get_job(emulator, app_id, branch, job_id).await;
        let summary = job.member("job").get("summary").cloned().unwrap_or(Value::Null);
        let status = string_member(&summary, "status");
        if ["SUCCEED", "FAILED", "CANCELLED"].contains(&status.as_str()) {
            return status;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "job {job_id:?} on {branch:?} never settled: last read {summary:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

/// An app with one branch on it.
async fn app_with_branch(emulator: &Emulator, name: &str, branch: &str) -> String {
    let app_id = emulator.app_id_of_new_app(name).await;
    let created = emulator
        .create_branch(&app_id, &json!({ "branchName": branch }))
        .await;
    assert_eq!(
        created.status, 200,
        "case {name:?}: expected HTTP 200 from CreateBranch, got {created:?}"
    );
    app_id
}

/// An app with one branch on it, CONNECTED TO A REPOSITORY PROVIDER.
///
/// For the case whose second half starts a JOB on that branch: `StartJob` runs a build
/// of the app's connected repository, so it is served only for an app that has one
/// (see `start_job_repository_precondition.rs`). Reserving a deployment takes no view
/// of the app's repository, so the reservations below are the same ones any app gets.
async fn repository_app_with_branch(emulator: &Emulator, name: &str, branch: &str) -> String {
    let app_id = emulator.app_id_of_new_repository_app(name).await;
    let created = emulator
        .create_branch(&app_id, &json!({ "branchName": branch }))
        .await;
    assert_eq!(
        created.status, 200,
        "case {name:?}: expected HTTP 200 from CreateBranch, got {created:?}"
    );
    app_id
}

/// The `fileUploadUrls` of a reply, as the map a client reads: the empty map when the
/// body carries no destination per file.
fn file_upload_urls(reply: &Reply) -> BTreeMap<String, String> {
    reply
        .body
        .get("fileUploadUrls")
        .and_then(Value::as_object)
        .map(|members| {
            members
                .iter()
                .map(|(name, url)| {
                    (
                        name.clone(),
                        url.as_str()
                            .unwrap_or_else(|| panic!("`{name}` is not a string in {reply:?}"))
                            .to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `zipUploadUrl` of a reply, as the value a client reads: the empty string when
/// the body carries no single zip destination.
fn zip_upload_url(reply: &Reply) -> String {
    reply
        .body
        .get("zipUploadUrl")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Does `candidate` satisfy `JobId`'s modelled `pattern ^[0-9]+$` and `length 0..255`
/// (`smithy_query(query="shape",shape="JobId")`)?
fn is_modelled_job_id(candidate: &str) -> bool {
    !candidate.is_empty()
        && candidate.len() <= 255
        && candidate.chars().all(|digit| digit.is_ascii_digit())
}

/// One `CreateDeployment` request, and the file names the rule says its answer must
/// carry a destination for.
struct DeploymentCase {
    case: String,
    request: Value,
    expected_files: Vec<String>,
}

/// The table: one row per way a request can describe what it is about to upload.
fn deployment_cases() -> Vec<DeploymentCase> {
    // `MD5Hash` is modelled `length 0..32` with no further constraint
    // (`smithy_query(query="shape",shape="FileMap")`), so the empty hash is a value a
    // request may legitimately declare and the rule does not read any of them.
    let hash = "d41d8cd98f00b204e9800998ecf8427e";
    let longest_name = "f".repeat(FILE_NAME_MAX);
    let mut longest = serde_json::Map::new();
    longest.insert(longest_name.clone(), Value::String(hash.to_string()));
    vec![
        DeploymentCase {
            case: "no fileMap member".to_string(),
            request: json!({}),
            expected_files: Vec::new(),
        },
        DeploymentCase {
            case: "fileMap naming no file".to_string(),
            request: json!({ "fileMap": {} }),
            expected_files: Vec::new(),
        },
        DeploymentCase {
            case: "one file".to_string(),
            request: json!({ "fileMap": { "index.html": hash } }),
            expected_files: vec!["index.html".to_string()],
        },
        DeploymentCase {
            case: "several files under nested prefixes".to_string(),
            request: json!({
                "fileMap": {
                    "index.html": hash,
                    "assets/app.js": hash,
                    "assets/styles/main.css": hash,
                }
            }),
            expected_files: vec![
                "assets/app.js".to_string(),
                "assets/styles/main.css".to_string(),
                "index.html".to_string(),
            ],
        },
        DeploymentCase {
            case: "file names carrying spaces, query syntax and non-ASCII".to_string(),
            request: json!({
                "fileMap": {
                    "a file with spaces.txt": hash,
                    "weird?name=1&x#y.txt": hash,
                    "ünïcode-ƒile.txt": hash,
                }
            }),
            expected_files: vec![
                "a file with spaces.txt".to_string(),
                "weird?name=1&x#y.txt".to_string(),
                "ünïcode-ƒile.txt".to_string(),
            ],
        },
        DeploymentCase {
            case: "a file whose declared hash is empty".to_string(),
            request: json!({ "fileMap": { "empty.txt": "" } }),
            expected_files: vec!["empty.txt".to_string()],
        },
        DeploymentCase {
            case: "the longest modelled file name".to_string(),
            request: json!({ "fileMap": Value::Object(longest) }),
            expected_files: vec![longest_name],
        },
    ]
}

/// Rule 1: the destinations a reservation hands back are the ones the request asked
/// for — one per named file, or the single zip destination when it named none.
#[tokio::test]
async fn reserves_one_upload_destination_per_thing_to_upload() {
    let emulator = Emulator::new();
    let app_id = app_with_branch(&emulator, "deployment-destinations", "main").await;

    let mut seen: BTreeSet<String> = BTreeSet::new();
    for (index, DeploymentCase {
        case,
        request,
        expected_files,
    }) in deployment_cases().into_iter().enumerate()
    {
        // One row per BRANCH: a branch serializes its deployments (rule 5), and what
        // this rule is about is the shape of ONE answer, so each row gets a branch with
        // no job of its own.
        let branch = format!("row-{index}");
        let created = emulator
            .create_branch(&app_id, &json!({ "branchName": branch }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from CreateBranch, got {created:?}"
        );

        let reply = create_deployment(&emulator, &app_id, &branch, &request).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200 from CreateDeployment, got {reply:?}"
        );

        let job_id = string_member(&reply.body, "jobId");
        assert!(
            is_modelled_job_id(&job_id),
            "case {case:?}: expected a modelled jobId of decimal digits, got {job_id:?}"
        );

        let urls = file_upload_urls(&reply);
        assert_eq!(
            urls.keys().cloned().collect::<Vec<String>>(),
            expected_files,
            "case {case:?}: expected a destination for exactly the files named, got {urls:?}"
        );

        let zip = zip_upload_url(&reply);
        if expected_files.is_empty() {
            assert!(
                !zip.is_empty(),
                "case {case:?}: a request naming no file uploads one zip, got {zip:?}"
            );
        } else {
            assert!(
                zip.is_empty(),
                "case {case:?}: a request naming its files uploads no zip, got {zip:?}"
            );
        }

        // Every destination of every reservation: well formed, inside the modelled
        // length bound, naming the deployment it uploads to, and never repeated.
        for url in urls.values().chain(if zip.is_empty() { None } else { Some(&zip) }) {
            assert!(
                url.starts_with("https://") && url.len() <= UPLOAD_URL_MAX,
                "case {case:?}: expected an https destination of at most \
                 {UPLOAD_URL_MAX} characters, got {} characters: {url:?}",
                url.len()
            );
            assert!(
                url.contains(&app_id) && url.contains(&format!("/{job_id}/")),
                "case {case:?}: a destination names the deployment it uploads to, got {url:?}"
            );
            assert!(
                seen.insert(url.clone()),
                "case {case:?}: every upload has its OWN destination, got the repeated {url:?}"
            );
        }
    }
}

/// Rules 2 and 3: a reserved `jobId` is spent, and the deployment it reserves HAS a job
/// — readable, in the status nothing has started, and not counted as a started job.
#[tokio::test]
async fn a_reserved_job_id_is_spent_and_names_the_deployments_job() {
    let emulator = Emulator::new();
    let app_id = repository_app_with_branch(&emulator, "deployment-ids", "main").await;

    // Three deployments, differing in what each asked to upload. A branch serializes
    // its deployments, so each is stopped before the next is created — which is what
    // the refusal of a second deployment tells a caller to do.
    let mut reserved: Vec<String> = Vec::new();
    for request in [
        json!({}),
        json!({ "fileMap": { "index.html": "d41d8cd98f00b204e9800998ecf8427e" } }),
        json!({ "fileMap": {} }),
    ] {
        let reply = create_deployment(&emulator, &app_id, "main", &request).await;
        assert_eq!(
            reply.status, 200,
            "case {request:?}: expected HTTP 200 from CreateDeployment, got {reply:?}"
        );
        let job_id = string_member(&reply.body, "jobId");

        // Rule 3: the deployment's job exists under the reserved id, created and not
        // started.
        let job = get_job(&emulator, &app_id, "main", &job_id).await;
        assert_eq!(
            job.status, 200,
            "case {request:?}: a deployment creates its job, got {job:?}"
        );
        let summary = job.member("job").get("summary").cloned().unwrap_or(Value::Null);
        assert_eq!(
            string_member(&summary, "status"),
            JOB_STATUS_PENDING,
            "case {request:?}: a deployment nothing started is pending, got {summary:?}"
        );
        assert!(
            summary.get("jobType").is_none(),
            "case {request:?}: a job nothing has started reports no jobType, got {summary:?}"
        );

        reserved.push(job_id.clone());
        stop_the_job(&emulator, &app_id, "main", &job_id).await;
    }

    assert_eq!(
        reserved.iter().collect::<BTreeSet<&String>>().len(),
        reserved.len(),
        "case \"three deployments\": each deployment spends its own jobId, got {reserved:?}"
    );
    let numeric: Vec<u64> = reserved
        .iter()
        .map(|job_id| job_id.parse::<u64>().expect("a modelled jobId parses"))
        .collect();
    assert!(
        numeric.windows(2).all(|pair| pair[0] < pair[1]),
        "case \"three deployments\": ids come from the branch's sequence, got {reserved:?}"
    );

    // Rule 3's other half: a deployment nobody started is not a job the branch has
    // STARTED, so `totalNumberOfJobs` still counts none.
    let record = emulator.get_branch(&app_id, "main").await.member("branch");
    assert_eq!(
        string_member(&record, "totalNumberOfJobs"),
        "0",
        "case \"deployments only\": a created deployment is not a started job, got {record:?}"
    );

    // The next job this branch starts is handed a DIFFERENT id.
    let started = start_job(&emulator, &app_id, "main", &json!({ "jobType": "MANUAL" })).await;
    assert_eq!(
        started.status, 200,
        "case \"StartJob after deployments\": expected HTTP 200, got {started:?}"
    );
    let started_id = string_member(&started.member("jobSummary"), "jobId");
    assert!(
        !reserved.contains(&started_id),
        "case \"StartJob after deployments\": a reserved id is spent, got {started_id:?} \
         among {reserved:?}"
    );
    assert!(
        started_id.parse::<u64>().expect("a modelled jobId parses")
            > *numeric.last().expect("three deployments"),
        "case \"StartJob after deployments\": the id sequence only moves forward, \
         got {started_id:?} after {reserved:?}"
    );
    assert_eq!(
        get_job(&emulator, &app_id, "main", &started_id).await.status,
        200,
        "case \"StartJob after deployments\": the started job is readable"
    );
    let record = emulator.get_branch(&app_id, "main").await.member("branch");
    assert_eq!(
        string_member(&record, "totalNumberOfJobs"),
        "1",
        "case \"StartJob after deployments\": only started jobs are counted, got {record:?}"
    );
}

/// Rule 4: a deployment is reserved on a branch of an app, so an identifier that
/// names nothing is refused with HTTP 404 and the not-found error type.
#[tokio::test]
async fn refuses_an_app_or_branch_that_names_nothing() {
    let emulator = Emulator::new();
    let app_id = app_with_branch(&emulator, "deployment-refusals", "main").await;
    // A branch of ANOTHER app: a reservation is reached through its own app, so this
    // name exists in the store and still names nothing on this app.
    let elsewhere = app_with_branch(&emulator, "deployment-refusals-elsewhere", "staging").await;
    assert_ne!(
        app_id, elsewhere,
        "two apps must have distinct ids: got {app_id:?} twice"
    );

    // Each row: the app id and branch name the request addresses, and the entity plus
    // identifier the refusal has to name.
    let cases: Vec<(&str, String, String, &str, String)> = vec![
        (
            "unknown app",
            "d0000000000000".to_string(),
            "main".to_string(),
            "App",
            "d0000000000000".to_string(),
        ),
        (
            "unknown branch",
            app_id.clone(),
            "no-such-branch".to_string(),
            "Branch",
            "no-such-branch".to_string(),
        ),
        (
            "branch of another app",
            app_id.clone(),
            "staging".to_string(),
            "Branch",
            "staging".to_string(),
        ),
    ];

    for (case, addressed_app, addressed_branch, entity, identifier) in cases {
        for request in [json!({}), json!({ "fileMap": { "index.html": "" } })] {
            let reply =
                create_deployment(&emulator, &addressed_app, &addressed_branch, &request).await;
            assert_eq!(
                reply.status, 404,
                "case {case:?}: expected HTTP 404 for a target that names nothing, \
                 got {reply:?}"
            );
            assert_eq!(
                reply.error_code.as_deref(),
                Some("NotFoundException"),
                "case {case:?}: expected the not-found error type, got {:?}",
                reply.error_code
            );
            let expected = format!("{entity} {identifier} not found.");
            assert_eq!(
                reply.message().as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected the request-derived message {expected:?}, got {:?}",
                reply.message()
            );
        }
    }

    // A refused reservation spends nothing: the branch's next id is still the first.
    let reply = create_deployment(&emulator, &app_id, "main", &json!({})).await;
    assert_eq!(
        string_member(&reply.body, "jobId"),
        "1",
        "case \"refusals spend nothing\": a refused reservation leaves the store alone, \
         got {reply:?}"
    );
}

/// Rule 5: one branch's deployments are serialized.
#[tokio::test]
async fn a_branch_serializes_its_deployments() {
    let emulator = Emulator::new();
    let app_id = app_with_branch(&emulator, "deployment-serialization", "main").await;

    let reserved = create_deployment(&emulator, &app_id, "main", &json!({})).await;
    assert_eq!(
        reserved.status, 200,
        "case \"the first deployment\": expected HTTP 200, got {reserved:?}"
    );
    let outstanding = string_member(&reserved.body, "jobId");

    // One row per shape of the second request: the refusal is about the branch's
    // unfinished job, not about what the request asked to upload — and each refused call
    // spends the branch's next job number, so the refusal names the number IT spent,
    // which is by then the branch's last job.
    let mut spent = outstanding
        .parse::<u64>()
        .expect("a modelled jobId parses");
    for request in [
        json!({}),
        json!({ "fileMap": {} }),
        json!({ "fileMap": { "index.html": "d41d8cd98f00b204e9800998ecf8427e" } }),
    ] {
        let refused = create_deployment(&emulator, &app_id, "main", &request).await;
        assert_eq!(
            refused.status, 400,
            "case {request:?}: expected the modelled HTTP 400, got {refused:?}"
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("BadRequestException"),
            "case {request:?}: expected BadRequestException, got {:?}",
            refused.error_code
        );
        spent += 1;
        let expected = format!(
            "The last job(deployment) {spent} was not finished, please wait until it \
             finish or use StopJob operation to cancel it before creating a new deployment."
        );
        assert_eq!(
            refused.message().as_deref(),
            Some(expected.as_str()),
            "case {request:?}: expected {expected:?}, got {:?}",
            refused.message()
        );
        // The number the refusal names is a job of the branch: that is what makes the
        // message actionable, since it tells the caller which job to stop.
        let job = get_job(&emulator, &app_id, "main", &spent.to_string()).await;
        assert_eq!(
            job.status, 200,
            "case {request:?}: the job the refusal names must be readable, got {job:?}"
        );
        let summary = job.member("job").get("summary").cloned().unwrap_or(Value::Null);
        assert_eq!(
            string_member(&summary, "status"),
            JOB_STATUS_PENDING,
            "case {request:?}: a job nothing started is pending, got {summary:?}"
        );
    }

    // The cap is PER BRANCH: another branch of the same app still reserves.
    let created = emulator
        .create_branch(&app_id, &json!({ "branchName": "other" }))
        .await;
    assert_eq!(
        created.status, 200,
        "case \"another branch\": expected HTTP 200 from CreateBranch, got {created:?}"
    );
    let reply = create_deployment(&emulator, &app_id, "other", &json!({})).await;
    assert_eq!(
        reply.status, 200,
        "case \"another branch\": one branch's outstanding job does not bind another, \
         got {reply:?}"
    );

    // Stopping the branch's LAST job frees the branch — which is what the refusal told
    // the caller to do — and the identical request then succeeds. An earlier job that is
    // still unfinished does not hold the branch: only the last one does.
    let last = spent.to_string();
    stop_the_job(&emulator, &app_id, "main", &last).await;
    let again = create_deployment(&emulator, &app_id, "main", &json!({})).await;
    assert_eq!(
        again.status, 200,
        "case \"after StopJob\": a finished last job frees the branch, got {again:?}"
    );
    assert_ne!(
        string_member(&again.body, "jobId"),
        last,
        "case \"after StopJob\": the new deployment gets its own id, got {again:?}"
    );

    // STARTING a deployment does not free the branch either: its job is now running,
    // which is the other half of "was not finished". The reservation's content is
    // uploaded first, because a deployment nothing was uploaded to cannot be started at
    // all (see `deployment_upload_plane.rs`).
    let running = string_member(&again.body, "jobId");
    let upload = http::Request::builder()
        .method("PUT")
        .uri(zip_upload_url(&again))
        .body(bytes::Bytes::from_static(b"the bytes of the deployment"))
        .expect("an upload destination is a well-formed URL");
    assert_eq!(
        emulator.dispatch(upload).await.status().as_u16(),
        200,
        "case \"while the deployment runs\": the destination must accept the upload"
    );
    let started = emulator
        .call(
            "POST",
            &format!("/apps/{app_id}/branches/main/deployments/start"),
            Some(&json!({ "jobId": running })),
        )
        .await;
    assert_eq!(
        started.status, 200,
        "case \"while the deployment runs\": expected HTTP 200 from StartDeployment, \
         got {started:?}"
    );
    let while_running = create_deployment(&emulator, &app_id, "main", &json!({})).await;
    assert_eq!(
        while_running.status, 400,
        "case \"while the deployment runs\": a running job holds the branch, \
         got {while_running:?}"
    );
    let named = running
        .parse::<u64>()
        .expect("a modelled jobId parses")
        .saturating_add(1);
    assert!(
        while_running
            .message()
            .unwrap_or_default()
            .contains(&format!("The last job(deployment) {named} was not finished")),
        "case \"while the deployment runs\": the refusal must name the number it spent, \
         got {:?}",
        while_running.message()
    );

    // Once the clock has settled the branch's last job, the branch is free again: the
    // refused call above spent the last number, so it is that job's settling — the
    // closing of its deployment window — that the branch now waits for, and stopping it
    // is the caller's way out.
    stop_the_job(&emulator, &app_id, "main", &named.to_string()).await;
    let third = create_deployment(&emulator, &app_id, "main", &json!({})).await;
    assert_eq!(
        third.status, 200,
        "case \"after the last job finishes\": a finished job holds the branch no longer, \
         got {third:?}"
    );
    // The deployment that was running settles on its own clock, and it settles whatever
    // the branch did afterwards.
    let settled = settled_status(&emulator, &app_id, "main", &running).await;
    assert!(
        ["SUCCEED", "FAILED"].contains(&settled.as_str()),
        "case \"while the deployment runs\": the started deployment settles, got {settled:?}"
    );
}
