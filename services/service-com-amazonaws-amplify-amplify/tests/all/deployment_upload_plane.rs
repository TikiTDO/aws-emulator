// SPDX-License-Identifier: CC0-1.0

//! The manual-deployment UPLOAD PLANE: the destinations `CreateDeployment` hands back
//! are served by this emulator, and what is uploaded to them is what the deployment
//! `StartDeployment` starts from that reservation deploys.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` — and every
//! upload FOLLOWS the URL the service published, with the method and URL it published
//! and no credential of any kind, exactly as a client follows an upload URL.
//!
//! The rules under test, each over a TABLE spanning the class it generalises over
//! rather than over one recorded request:
//!
//! 1. A destination a reservation returns ACCEPTS the upload: an unsigned `PUT` to it
//!    answers HTTP 200, whatever the reservation asked to upload (one zipped
//!    deployment, or one file per name) and whatever the content is.
//! 2. What a reserved deployment deploys is what was UPLOADED to those destinations:
//!    the job settles to `SUCCEED` and publishes the files the upload carries when
//!    they make up a servable site, and to `FAILED` with the reason when they do not —
//!    an archive these bytes are not, an upload carrying no root document. A
//!    reservation nothing was uploaded to cannot be STARTED at all: that call is
//!    refused with the modelled 400 and its job stays the created one.
//! 3. A destination is written by ONE `PUT`: another method against it is refused, the
//!    last write is what the deployment deploys, and a URL this emulator never handed
//!    out resolves to nothing.
//! 4. A destination stops being usable when its reservation's window closes — the 8
//!    hours documented between `CreateDeployment` and `StartDeployment` — and a write
//!    after that instant is refused rather than stored.
//! 5. What this store RETAINS of deployment content is bounded as a whole, and the bytes
//!    a branch held are released when that branch is deleted.

use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;
use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_amplify_amplify::ops::start_deployment::{
    DEPLOYMENT_FILE_NOT_UPLOADED, MAX_ARCHIVE_BYTES, ROOT_DOCUMENT,
};
use service_com_amazonaws_amplify_amplify::state::{
    ComAmazonawsAmplifyAmplifyState, JOB_SETTLE_MILLIS, MAX_STORED_DEPLOYMENT_BYTES,
};

use crate::app_branch_lifecycle::{list_member, string_member, Emulator, Reply, REGION};

/// `UploadUrl`'s modelled maximum length, which every destination has to fit in
/// (`smithy_query(query="shape",shape="UploadUrl")`: `length 0..1000`).
const UPLOAD_URL_MAX: usize = 1000;

/// The terminal status of a deployment whose files were read and published.
const SUCCEED: &str = "SUCCEED";

/// The terminal status of a deployment whose files could not be published.
const FAILED: &str = "FAILED";

/// The status of the job of a deployment nothing has started, which is where a refused
/// start leaves it.
const PENDING: &str = "PENDING";

/// A declared `MD5Hash` for a file a reservation names: the member is modelled
/// `length 0..32` with no further constraint
/// (`smithy_query(query="shape",shape="FileMap")`), and nothing in this rule reads it.
const DECLARED_HASH: &str = "d41d8cd98f00b204e9800998ecf8427e";

/// How long a case waits before expecting the clock-owed status change: the span the
/// service itself publishes, plus a margin for a slow machine.
fn past_the_settle_span() -> Duration {
    Duration::from_millis(u64::try_from(JOB_SETTLE_MILLIS).unwrap_or(500) + 500)
}

/// The emulator plus the engine it is mounted on, so a case can read the asset paths a
/// deployment published (which no modelled operation reports) after driving every call
/// that produced them over the wire.
struct Deployments {
    emulator: Emulator,
    engine: Arc<engine::InMemoryEngine>,
}

/// What following an upload destination answered: the status and the error code, which
/// is all a client of an upload URL branches on.
#[derive(Debug)]
struct Followed {
    status: u16,
    code: Option<String>,
}

impl Deployments {
    fn new() -> Self {
        let (emulator, engine) = Emulator::with_state();
        Self { emulator, engine }
    }

    /// An app with one branch on it, and the app's id. No repository: a manual
    /// deployment deploys content Amplify does not hold, and "manually deployed apps
    /// are not connected to a Git repository"
    /// (`smithy_query(query="doc",shape="CreateDeployment")`).
    async fn app_with_branch(&self, name: &str, branch: &str) -> String {
        let app_id = self.emulator.app_id_of_new_app(name).await;
        let created = self
            .emulator
            .create_branch(&app_id, &json!({ "branchName": branch }))
            .await;
        assert_eq!(
            created.status, 200,
            "app {name:?}: expected HTTP 200 from CreateBranch, got {created:?}"
        );
        app_id
    }

    /// `CreateDeployment` over the wire.
    async fn create_deployment(&self, app_id: &str, branch: &str, request: &Value) -> Reply {
        self.emulator
            .call(
                "POST",
                &format!("/apps/{app_id}/branches/{branch}/deployments"),
                Some(request),
            )
            .await
    }

    /// `StartDeployment` over the wire.
    async fn start_deployment(&self, app_id: &str, branch: &str, request: &Value) -> Reply {
        self.emulator
            .call(
                "POST",
                &format!("/apps/{app_id}/branches/{branch}/deployments/start"),
                Some(request),
            )
            .await
    }

    /// `GetJob` over the wire.
    async fn get_job(&self, app_id: &str, branch: &str, job_id: &str) -> Reply {
        self.emulator
            .call(
                "GET",
                &format!("/apps/{app_id}/branches/{branch}/jobs/{job_id}"),
                None,
            )
            .await
    }

    /// Follow an upload destination exactly as a client does: `method` against the URL
    /// the service published, and with NO credential of any kind.
    async fn follow(&self, method: &str, url: &str, body: &[u8]) -> Followed {
        let request = http::Request::builder()
            .method(method)
            .uri(url)
            .header("content-type", "application/zip")
            .body(bytes::Bytes::copy_from_slice(body))
            .expect("an upload destination is a well-formed URL");
        let response = self.emulator.dispatch(request).await;
        Followed {
            status: response.status().as_u16(),
            code: response
                .headers()
                .get("x-amzn-errortype")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string),
        }
    }

    /// Follow a destination with the `PUT` the plane publishes, asserting it is
    /// accepted — rule 1 — and answering nothing else.
    async fn upload(&self, case: &str, url: &str, body: &[u8]) {
        let followed = self.follow("PUT", url, body).await;
        assert_eq!(
            followed.status, 200,
            "case {case:?}: the destination {url:?} must accept the upload, got {followed:?}"
        );
    }

    /// The zip destination of a fresh reservation on `branch`.
    async fn zip_destination_on(&self, app_id: &str, branch: &str) -> String {
        let reserved = self.create_deployment(app_id, branch, &json!({})).await;
        assert_eq!(
            reserved.status, 200,
            "a reservation on {branch:?}: expected HTTP 200 from CreateDeployment, \
             got {reserved:?}"
        );
        string_member(&reserved.body, "zipUploadUrl")
    }

    /// The asset paths one job published, as this service persists them — the content
    /// the deployment put on the branch, which no modelled read reports.
    fn deployed_assets(&self, app_id: &str, branch: &str, job_id: &str) -> BTreeSet<String> {
        let state = self.engine.state::<ComAmazonawsAmplifyAmplifyState>();
        let store = state.lock();
        store
            .apps
            .get(app_id)
            .and_then(|app| app.branches.get(branch))
            .and_then(|branch| branch.jobs.get(job_id))
            .map(|job| job.deployed_assets.clone())
            .unwrap_or_default()
    }

    /// Close the window of the reservation `job_id` names, as the clock does after the
    /// documented 8 hours: the instant is the one the service itself stores, so a case
    /// about a closed window does not have to wait for it.
    ///
    /// Both records the window governs are moved: the reservation, which stops being
    /// writable, and the deployment's JOB, which the closing window fails (the documented
    /// "if the duration exceeds 8 hours, the StartDeployment call and the associated Job
    /// will fail").
    fn close_the_window_of(&self, app_id: &str, branch: &str, job_id: &str) {
        let state = self.engine.state::<ComAmazonawsAmplifyAmplifyState>();
        let mut store = state.lock();
        let now = service_com_amazonaws_amplify_amplify::state::now_millis();
        let branch = store
            .apps
            .get_mut(app_id)
            .and_then(|app| app.branches.get_mut(branch))
            .expect("the branch just reserved on is in the store");
        branch
            .deployments
            .get_mut(job_id)
            .expect("the reservation just made is in the store")
            .expires_at = now;
        branch
            .jobs
            .get_mut(job_id)
            .expect("the deployment just created has a job")
            .settles_at = Some(now);
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
        std::io::Write::write_all(&mut writer, b"deployed")
            .expect("an archive entry is written");
    }
    writer.finish().expect("the archive closes");
    buffer.into_inner()
}

/// The `fileUploadUrls` of a reply, keyed by the file name the request named.
fn file_upload_urls(reply: &Reply) -> Vec<(String, String)> {
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

/// The one execution step a settled job carries, as a client reads it.
fn settled_step(job: &Value) -> Value {
    let steps = list_member(job, "steps");
    assert_eq!(
        steps.len(),
        1,
        "a settled job carries one execution step, got {steps:?}"
    );
    steps[0].clone()
}

/// One row of the upload table: what the reservation asked to upload, what is uploaded
/// to each destination it hands back, and what the deployment started from it then
/// publishes.
struct UploadCase {
    case: &'static str,
    /// The `CreateDeployment` request.
    request: Value,
    /// The bytes uploaded to the zip destination, when the row uploads one.
    zip: Option<Vec<u8>>,
    /// The file names whose own destinations the row uploads to.
    uploaded_files: &'static [&'static str],
    /// The status the deployment's job is in once the clock has had its span: the
    /// terminal one it settles to, or the created one a refused start leaves it in.
    status: &'static str,
    /// The asset paths it publishes.
    assets: &'static [&'static str],
    /// A fragment the FAILED job's reason has to carry, for a row that fails.
    reason_names: Option<&'static str>,
}

/// The table: one row per way a reservation's uploads can make up (or fail to make up)
/// a deployment.
fn upload_cases() -> Vec<UploadCase> {
    vec![
        UploadCase {
            case: "a zipped deployment carrying a site",
            request: json!({}),
            zip: Some(zip_of(&["index.html", "assets/app.js", "assets/styles/main.css"])),
            uploaded_files: &[],
            status: SUCCEED,
            assets: &["assets/app.js", "assets/styles/main.css", "index.html"],
            reason_names: None,
        },
        UploadCase {
            case: "a zipped deployment carrying one file",
            request: json!({}),
            zip: Some(zip_of(&["index.html"])),
            uploaded_files: &[],
            status: SUCCEED,
            assets: &["index.html"],
            reason_names: None,
        },
        UploadCase {
            case: "a zipped deployment whose root document is nested",
            request: json!({}),
            zip: Some(zip_of(&["site/index.html"])),
            uploaded_files: &[],
            status: FAILED,
            assets: &[],
            reason_names: Some(ROOT_DOCUMENT),
        },
        UploadCase {
            case: "an upload that is not an archive",
            request: json!({}),
            zip: Some(b"<html>not a zip</html>".to_vec()),
            uploaded_files: &[],
            status: FAILED,
            assets: &[],
            reason_names: Some("archive"),
        },
        UploadCase {
            case: "a zip destination nothing was uploaded to",
            request: json!({}),
            zip: None,
            uploaded_files: &[],
            status: PENDING,
            assets: &[],
            reason_names: None,
        },
        UploadCase {
            case: "one named file, uploaded",
            request: json!({ "fileMap": { "index.html": DECLARED_HASH } }),
            zip: None,
            uploaded_files: &["index.html"],
            status: SUCCEED,
            assets: &["index.html"],
            reason_names: None,
        },
        UploadCase {
            case: "three named files, all uploaded",
            request: json!({ "fileMap": {
                "index.html": DECLARED_HASH,
                "assets/app.js": DECLARED_HASH,
                "assets/styles/main.css": DECLARED_HASH,
            }}),
            zip: None,
            uploaded_files: &["index.html", "assets/app.js", "assets/styles/main.css"],
            status: SUCCEED,
            assets: &["assets/app.js", "assets/styles/main.css", "index.html"],
            reason_names: None,
        },
        UploadCase {
            case: "three named files, two uploaded",
            request: json!({ "fileMap": {
                "index.html": DECLARED_HASH,
                "assets/app.js": DECLARED_HASH,
                "assets/styles/main.css": DECLARED_HASH,
            }}),
            zip: None,
            uploaded_files: &["index.html", "assets/app.js"],
            status: SUCCEED,
            assets: &["assets/app.js", "index.html"],
            reason_names: None,
        },
        UploadCase {
            case: "named files whose root document was never uploaded",
            request: json!({ "fileMap": {
                "index.html": DECLARED_HASH,
                "assets/app.js": DECLARED_HASH,
            }}),
            zip: None,
            uploaded_files: &["assets/app.js"],
            status: FAILED,
            assets: &[],
            reason_names: Some(ROOT_DOCUMENT),
        },
        UploadCase {
            case: "named files, none uploaded",
            request: json!({ "fileMap": { "index.html": DECLARED_HASH } }),
            zip: None,
            uploaded_files: &[],
            status: PENDING,
            assets: &[],
            reason_names: None,
        },
    ]
}

/// Rules 1 and 2: every destination a reservation hands back accepts its upload, and
/// the deployment started from that reservation deploys exactly what was uploaded.
#[tokio::test]
async fn an_upload_destination_accepts_the_upload_and_the_deployment_deploys_it() {
    let deployments = Deployments::new();
    let app_id = deployments.app_with_branch("manual-deploy-uploads", "main").await;

    // Every destination of every reservation is its own: no two uploads of this suite
    // are handed the same URL.
    let mut seen: HashSet<String> = HashSet::new();

    for row in upload_cases() {
        let reserved = deployments
            .create_deployment(&app_id, "main", &row.request)
            .await;
        assert_eq!(
            reserved.status, 200,
            "case {:?}: expected HTTP 200 from CreateDeployment, got {reserved:?}",
            row.case
        );
        let job_id = string_member(&reserved.body, "jobId");
        let zip_url = reserved
            .body
            .get("zipUploadUrl")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let per_file = file_upload_urls(&reserved);

        // Every destination addresses THIS emulator — the endpoint the reserving call
        // was made to — and is inside the modelled bound.
        for url in per_file
            .iter()
            .map(|(_, url)| url)
            .chain(if zip_url.is_empty() { None } else { Some(&zip_url) })
        {
            assert!(
                url.starts_with(&format!("https://amplify.{REGION}.amazonaws.com/"))
                    && url.len() <= UPLOAD_URL_MAX,
                "case {:?}: a destination must address the endpoint the caller reached, \
                 in at most {UPLOAD_URL_MAX} characters, got {url:?}",
                row.case
            );
            assert!(
                seen.insert(url.clone()),
                "case {:?}: every upload has its OWN destination, got the repeated {url:?}",
                row.case
            );
        }

        if let Some(body) = row.zip.as_deref() {
            deployments.upload(row.case, &zip_url, body).await;
        }
        for file_name in row.uploaded_files {
            let url = per_file
                .iter()
                .find(|(name, _)| name == file_name)
                .map(|(_, url)| url.clone())
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: the reservation names no destination for {file_name:?}: \
                         {per_file:?}",
                        row.case
                    )
                });
            deployments
                .upload(row.case, &url, format!("the bytes of {file_name}").as_bytes())
                .await;
        }

        let started = deployments
            .start_deployment(&app_id, "main", &json!({ "jobId": job_id }))
            .await;
        // A row that uploaded NOTHING has no deployment file behind its reservation, so
        // the start is refused with the modelled 400 and AWS's fixed sentence (pinned
        // literally in `manual_deployment_preconditions.rs`) rather than starting a job
        // with no content; every other row starts.
        let refused = row.zip.is_none() && row.uploaded_files.is_empty();
        if refused {
            assert_eq!(
                started.status, 400,
                "case {:?}: a reservation nothing was uploaded to cannot be started, \
                 got {started:?}",
                row.case
            );
            assert_eq!(
                started.error_code.as_deref(),
                Some("BadRequestException"),
                "case {:?}: expected the modelled BadRequestException, got {:?}",
                row.case,
                started.error_code
            );
            assert_eq!(
                started.message().as_deref(),
                Some(DEPLOYMENT_FILE_NOT_UPLOADED),
                "case {:?}: expected {DEPLOYMENT_FILE_NOT_UPLOADED:?}, got {:?}",
                row.case,
                started.message()
            );
        } else {
            assert_eq!(
                started.status, 200,
                "case {:?}: expected HTTP 200 from StartDeployment, got {started:?}",
                row.case
            );
        }

        sleep(past_the_settle_span());
        let read_back = deployments.get_job(&app_id, "main", &job_id).await;
        assert_eq!(
            read_back.status, 200,
            "case {:?}: the started deployment's job is readable, got {read_back:?}",
            row.case
        );
        let job = read_back.member("job");
        let settled = job.get("summary").cloned().unwrap_or(Value::Null);
        assert_eq!(
            string_member(&settled, "status"),
            row.status,
            "case {:?}: expected the job to be {} after the span, got {settled:?}",
            row.case,
            row.status
        );
        let published = deployments.deployed_assets(&app_id, "main", &job_id);
        let expected: BTreeSet<String> = row.assets.iter().map(|path| (*path).to_string()).collect();
        assert_eq!(
            published, expected,
            "case {:?}: expected the deployment to publish {expected:?}, got {published:?}",
            row.case
        );
        if let Some(fragment) = row.reason_names {
            let reason = settled_step(&job)
                .get("statusReason")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            assert!(
                reason.contains(fragment),
                "case {:?}: the failed job's reason must name {fragment:?}, got {reason:?}",
                row.case
            );
        }

        // A refused start leaves the reservation's job unfinished, and a branch
        // SERIALIZES its deployments, so it is stopped before the next row reserves one.
        // Every other row's job reached a terminal status in the span above.
        if refused {
            let stopped = deployments
                .emulator
                .call(
                    "DELETE",
                    &format!("/apps/{app_id}/branches/main/jobs/{job_id}/stop"),
                    None,
                )
                .await;
            assert_eq!(
                stopped.status, 200,
                "case {:?}: expected HTTP 200 from StopJob, got {stopped:?}",
                row.case
            );
            sleep(past_the_settle_span());
        }
    }
}

/// Rule 3: a destination is written by ONE `PUT`, the last write is what is deployed,
/// and a URL this emulator never handed out resolves to nothing.
#[tokio::test]
async fn a_destination_is_written_by_one_put_and_an_unknown_one_resolves_to_nothing() {
    let deployments = Deployments::new();
    let app_id = deployments.app_with_branch("upload-destination-writes", "main").await;

    let reserved = deployments
        .create_deployment(&app_id, "main", &json!({}))
        .await;
    let job_id = string_member(&reserved.body, "jobId");
    let zip_url = string_member(&reserved.body, "zipUploadUrl");

    // Every method other than the one `PUT` the destination publishes is refused, and
    // refusing one stores nothing.
    for method in ["GET", "POST", "DELETE", "HEAD"] {
        let followed = deployments.follow(method, &zip_url, b"not an upload").await;
        assert_eq!(
            followed.status, 405,
            "case {method:?}: a destination is written by a single PUT, got {followed:?}"
        );
    }

    // A URL this emulator never handed out: the same shape with another token, another
    // job id, and another app id. Each resolves to nothing rather than to some other
    // reservation's upload.
    let unknown: Vec<String> = vec![
        zip_url.replace(&format!("/{job_id}/"), "/999999/"),
        format!("https://amplify.{REGION}.amazonaws.com/apps/{app_id}/deployments/{job_id}/uploads/0000000000000000"),
        format!(
            "https://amplify.{REGION}.amazonaws.com/apps/dnosuchapp000/deployments/{job_id}/uploads/0000000000000000"
        ),
    ];
    for url in &unknown {
        let followed = deployments.follow("PUT", url, b"uploaded").await;
        assert_eq!(
            followed.status, 404,
            "case {url:?}: a destination this emulator never handed out resolves to \
             nothing, got {followed:?}"
        );
    }

    // The last write is what the deployment deploys: the first upload carries no root
    // document, the second does, and the job succeeds with the second one's files.
    deployments
        .upload("first write", &zip_url, &zip_of(&["site/index.html"]))
        .await;
    deployments
        .upload("second write", &zip_url, &zip_of(&["index.html", "app.js"]))
        .await;
    let started = deployments
        .start_deployment(&app_id, "main", &json!({ "jobId": job_id }))
        .await;
    assert_eq!(
        started.status, 200,
        "case \"re-written destination\": expected HTTP 200 from StartDeployment, got {started:?}"
    );
    sleep(past_the_settle_span());
    let job = deployments
        .get_job(&app_id, "main", &job_id)
        .await
        .member("job");
    assert_eq!(
        string_member(&job.get("summary").cloned().unwrap_or(Value::Null), "status"),
        SUCCEED,
        "case \"re-written destination\": the last write is what is deployed, got {job:?}"
    );
    assert_eq!(
        deployments.deployed_assets(&app_id, "main", &job_id),
        ["app.js".to_string(), "index.html".to_string()]
            .into_iter()
            .collect::<BTreeSet<String>>(),
        "case \"re-written destination\": the deployment publishes the last upload's files"
    );
}

/// Rule 5: what this store retains of deployment content is BOUNDED as a whole, and the
/// bytes a branch held are released when that branch is deleted.
///
/// The bound is the service's own
/// (`service_com_amazonaws_amplify_amplify::state::MAX_STORED_DEPLOYMENT_BYTES`), so the
/// case is written against that constant rather than against a number: it fills the
/// store with uploads that are each inside the per-deployment clamp, then shows that the
/// write which would pass the whole-store ceiling is refused.
#[tokio::test]
async fn the_bytes_this_store_retains_are_bounded_and_released() {
    let deployments = Deployments::new();
    let app_id = deployments.app_with_branch("upload-budget", "main").await;

    // Each upload is at the per-deployment clamp, so filling the store takes exactly as
    // many reservations as the ceiling is multiples of that clamp. One reservation per
    // BRANCH, because a branch serializes its deployments (a new one is refused while
    // the branch's last job is unfinished) and the bound under test is the whole store's,
    // not one branch's.
    let per_upload = MAX_ARCHIVE_BYTES;
    let fills = MAX_STORED_DEPLOYMENT_BYTES / per_upload;
    for index in 0..fills {
        let case = format!("filling upload {index}");
        let branch = format!("fill-{index}");
        let created = deployments
            .emulator
            .create_branch(&app_id, &json!({ "branchName": branch }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from CreateBranch, got {created:?}"
        );
        let url = deployments.zip_destination_on(&app_id, &branch).await;
        deployments.upload(&case, &url, &vec![0u8; per_upload]).await;
    }

    // One more byte, on a reservation of its own, would take the store past the ceiling.
    let url = deployments.zip_destination_on(&app_id, "main").await;
    let refused = deployments.follow("PUT", &url, b"!").await;
    assert_eq!(
        refused.status, 413,
        "case \"past the store's budget\": the write that would pass the ceiling is \
         refused, got {refused:?}"
    );
    assert_eq!(
        refused.code.as_deref(),
        Some("EntityTooLarge"),
        "case \"past the store's budget\": expected the plane's refusal code, got {refused:?}"
    );

    // Deleting the branches releases everything they held, so the same write is accepted
    // on a branch of its own afterwards.
    for index in 0..fills {
        let branch = format!("fill-{index}");
        let deleted = deployments.emulator.delete_branch(&app_id, &branch).await;
        assert_eq!(
            deleted.status, 200,
            "case \"after DeleteBranch\": expected HTTP 200 from DeleteBranch on \
             {branch:?}, got {deleted:?}"
        );
    }
    let created = deployments
        .emulator
        .create_branch(&app_id, &json!({ "branchName": "next" }))
        .await;
    assert_eq!(
        created.status, 200,
        "case \"after DeleteBranch\": expected HTTP 200 from CreateBranch, got {created:?}"
    );
    let url = deployments.zip_destination_on(&app_id, "next").await;
    deployments.upload("after DeleteBranch", &url, b"!").await;
}

/// Rule 4: a destination stops being usable when its reservation's window closes.
#[tokio::test]
async fn a_write_after_the_reservations_window_is_refused() {
    let deployments = Deployments::new();
    let app_id = deployments.app_with_branch("upload-window", "main").await;

    // One row per shape of reservation: the window belongs to the RESERVATION, so it
    // closes over the single zip destination and over a named file's alike.
    let rows: Vec<(&str, Value)> = vec![
        ("the zipped deployment", json!({})),
        (
            "a named file",
            json!({ "fileMap": { "index.html": DECLARED_HASH } }),
        ),
    ];

    for (case, request) in rows {
        let reserved = deployments
            .create_deployment(&app_id, "main", &request)
            .await;
        let job_id = string_member(&reserved.body, "jobId");
        let url = match file_upload_urls(&reserved).first() {
            Some((_, url)) => url.clone(),
            None => string_member(&reserved.body, "zipUploadUrl"),
        };

        // Inside the window the destination accepts the upload.
        deployments.upload(case, &url, &zip_of(&["index.html"])).await;

        deployments.close_the_window_of(&app_id, "main", &job_id);
        let followed = deployments.follow("PUT", &url, &zip_of(&["index.html"])).await;
        assert_eq!(
            followed.status, 403,
            "case {case:?}: a write after the window closes is refused, got {followed:?}"
        );
        assert_eq!(
            followed.code.as_deref(),
            Some("AccessDenied"),
            "case {case:?}: expected the plane's refusal code, got {followed:?}"
        );
    }
}
