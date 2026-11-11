// SPDX-License-Identifier: CC0-1.0

//! What the OUTCOME of a revision's build is, driven over the same router the emulator
//! boots with Amazon S3 mounted beside this service on one engine — exactly as the
//! assembled router mounts them.
//!
//! RULE: the asynchronous build `CreateWebFunction` / `CreateWebFunctionRevision` starts
//! fetches the deployment package the revision's `buildConfig.codeConfig.s3Object` names
//! and settles the revision `Active` ("ready to serve traffic") only when that object is
//! an archive the build can read; when the object store answers that it will not serve the
//! object, or serves bytes that are no zip archive, the build FAILS — the revision settles
//! `Failed` ("the revision build failed") carrying the build's own reason. An endpoint
//! whose `autoDeploymentMode` is `LatestRevision` then serves the newest revision that
//! BUILT, so a revision that failed never takes traffic.
//!
//! Each test states that rule over a SET of inputs: the rows differ in WHAT the object is
//! (an archive, an archive with several entries, bytes that are no archive, no bytes at
//! all, a truncated archive, an object the store does not hold), so the recorded case — an
//! object whose body is `b"not a zip archive"` — is one row of the class.

use std::io::Write as _;
use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_lambdaweb_lambdaweb::state::code_package::NOT_A_ZIP_ARCHIVE_REASON;
use service_com_amazonaws_lambdaweb_lambdaweb::state::PROVISIONING_DELAY;

use super::web_function_lifecycle::{build_config, role_arn, Emulator, Reply};

/// The region every request here signs with.
const REGION: &str = "us-east-1";

/// The bucket the packages below are staged in.
const BUCKET: &str = "lambda-web-packages";

/// How long a test waits before expecting a build to have settled: the service's own span
/// plus slack for a loaded test machine. An endpoint settles one span later again, so a
/// wait that covers the endpoint covers the revision too.
fn settled() -> Duration {
    PROVISIONING_DELAY + PROVISIONING_DELAY + Duration::from_millis(400)
}

/// An Amazon S3 client over this emulator's own in-process callback — the seam a build
/// fetches its package through, so an object staged here is the object the build reads.
fn s3(emulator: &Emulator) -> aws_sdk_s3::Client {
    let callback = smithy_server_runtime::EmbeddedEmulator::from_router(emulator.router().clone());
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
        .region(aws_sdk_s3::config::Region::new(REGION))
        .endpoint_url(format!("https://s3.{REGION}.amazonaws.com"))
        .force_path_style(true)
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    aws_sdk_s3::Client::from_conf(config)
}

/// Create the bucket every package below is staged in.
async fn create_bucket(emulator: &Emulator) {
    // No `CreateBucketConfiguration`: `us-east-1` is the one Region a create must NOT
    // state a location constraint for, which Amazon S3 refuses as
    // `InvalidLocationConstraint`.
    let created = s3(emulator).create_bucket().bucket(BUCKET).send().await;
    assert!(
        created.is_ok(),
        "staging {BUCKET}: the bucket must be created, got {created:?}"
    );
}

/// Stage one object through Amazon S3's OWN operations: what a build fetches is what that
/// service really holds.
async fn stage_object(emulator: &Emulator, key: &str, body: Vec<u8>) {
    let put = s3(emulator)
        .put_object()
        .bucket(BUCKET)
        .key(key)
        .body(aws_sdk_s3::primitives::ByteStream::from(body))
        .send()
        .await;
    assert!(
        put.is_ok(),
        "staging s3://{BUCKET}/{key}: the object must be stored, got {put:?}"
    );
}

/// A .zip archive carrying `entries`, written with the maintained zip writer — a real
/// archive, not bytes that happen to pass.
fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, contents) in entries {
        writer
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .expect("the entry starts");
        writer.write_all(contents).expect("the entry is written");
    }
    writer
        .finish()
        .expect("the archive is finished")
        .into_inner()
}

/// `PUT /web-functions` with an initial revision built from the object `key`.
async fn create_function(emulator: &Emulator, name: &str, key: &str) -> Reply {
    emulator
        .send(
            "PUT",
            "/web-functions",
            REGION,
            Some(json!({
                "functionName": name,
                "revisionConfig": {
                    "buildConfig": build_config("nodejs24.x", BUCKET, key, None),
                    "serviceConfig": { "executionRoleArn": role_arn(name) },
                },
            })),
        )
        .await
}

/// `POST /web-functions/{name}/revisions` built from the object `key`.
async fn create_revision(emulator: &Emulator, name: &str, key: &str) -> Reply {
    emulator
        .send(
            "POST",
            &format!("/web-functions/{name}/revisions"),
            REGION,
            Some(json!({
                "buildConfig": build_config("nodejs24.x", BUCKET, key, None),
                "serviceConfig": { "executionRoleArn": role_arn(name) },
            })),
        )
        .await
}

/// `GET /web-functions/{name}/revisions/{revisionId}`.
async fn get_revision(emulator: &Emulator, name: &str, revision_id: &str) -> Value {
    let read = emulator
        .send(
            "GET",
            &format!("/web-functions/{name}/revisions/{revision_id}"),
            REGION,
            None,
        )
        .await;
    assert_eq!(
        read.status, 200,
        "GetWebFunctionRevision {name}/{revision_id}: expected 200, got {} {:?}",
        read.status, read.body
    );
    read.body
}

/// A member of a JSON object as a string, or the empty string when it is absent.
fn text(object: &Value, name: &str) -> String {
    object
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One row: the case, the object key, the bytes staged under it (`None` stages nothing at
/// all), and the `stateReason` the build settles `Failed` with (`None` for a build that
/// settles `Active`).
struct Row {
    case: &'static str,
    key: &'static str,
    staged: Option<Vec<u8>>,
    failure: Option<&'static str>,
}

/// The rows every test below drives: the class of deployment package, spanning what the
/// rule generalises over — whether there is an object at all, and whether its bytes are an
/// archive a build can read.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an archive with one entry",
            key: "one-entry.zip",
            staged: Some(archive(&[("index.mjs", b"export const handler = () => 1;")])),
            failure: None,
        },
        Row {
            case: "an archive with several entries, one of them nested",
            key: "several-entries.zip",
            staged: Some(archive(&[
                ("index.mjs", b"export const handler = () => 1;"),
                ("src/app.mjs", b"export const app = 1;"),
                ("package.json", b"{}"),
            ])),
            failure: None,
        },
        Row {
            case: "an archive with an empty entry",
            key: "empty-entry.zip",
            staged: Some(archive(&[("index.mjs", b"")])),
            failure: None,
        },
        Row {
            // The recorded case: an object whose body is plain text.
            case: "bytes that are no archive",
            key: "code-b.zip",
            staged: Some(b"not a zip archive".to_vec()),
            failure: Some(NOT_A_ZIP_ARCHIVE_REASON),
        },
        Row {
            case: "an object with no bytes at all",
            key: "empty.zip",
            staged: Some(Vec::new()),
            failure: Some(NOT_A_ZIP_ARCHIVE_REASON),
        },
        Row {
            case: "an archive whose central directory was cut off",
            key: "truncated.zip",
            staged: Some({
                let mut built = archive(&[("index.mjs", b"export const handler = () => 1;")]);
                built.truncate(built.len() / 2);
                built
            }),
            failure: Some(NOT_A_ZIP_ARCHIVE_REASON),
        },
        Row {
            case: "a key the bucket holds nothing under",
            key: "absent.zip",
            staged: None,
            failure: Some(
                "Error occurred while GetObject. S3 Error Code: NoSuchKey. S3 Error Message: \
                 The specified key does not exist.",
            ),
        },
    ]
}

/// RULE: the initial revision of a create settles `Active` exactly when its code object is
/// an archive the build can read, and `Failed` carrying the build's reason otherwise.
#[tokio::test]
async fn the_initial_revisions_build_settles_on_what_its_code_object_is() {
    let emulator = Emulator::with_object_store();
    create_bucket(&emulator).await;
    let rows = rows();
    for row in &rows {
        if let Some(bytes) = &row.staged {
            stage_object(&emulator, row.key, bytes.clone()).await;
        }
    }
    // Every function is created first and read after ONE wait, so the suite waits out the
    // provisioning span once rather than per row.
    let mut created = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let name = format!("build-outcome-{index}");
        let reply = create_function(&emulator, &name, row.key).await;
        assert_eq!(
            reply.status, 202,
            "case {:?}: CreateWebFunction expected 202, got {} {:?}",
            row.case, reply.status, reply.body
        );
        let revision = reply
            .field("revision")
            .cloned()
            .unwrap_or_else(|| panic!("case {:?}: the create reports its revision", row.case));
        assert_eq!(
            text(&revision, "state"),
            "Pending",
            "case {:?}: a just-created revision is still building, got {revision:?}",
            row.case
        );
        created.push((name, text(&revision, "revisionId")));
    }

    tokio::time::sleep(settled()).await;

    for (row, (name, revision_id)) in rows.iter().zip(&created) {
        let revision = get_revision(&emulator, name, revision_id).await;
        let expected_state = if row.failure.is_some() {
            "Failed"
        } else {
            "Active"
        };
        assert_eq!(
            text(&revision, "state"),
            expected_state,
            "case {:?}: expected the build to settle {expected_state}, got {revision:?}",
            row.case
        );
        if let Some(reason) = row.failure {
            assert_eq!(
                text(&revision, "stateReason"),
                reason,
                "case {:?}: expected the stateReason {reason:?}, got {revision:?}",
                row.case
            );
        }
    }
}

/// RULE, over the other create path: a revision added to an existing function settles on
/// exactly the same reading of its own code object.
#[tokio::test]
async fn a_later_revisions_build_settles_on_what_its_code_object_is() {
    let emulator = Emulator::with_object_store();
    create_bucket(&emulator).await;
    let rows = rows();
    for row in &rows {
        if let Some(bytes) = &row.staged {
            stage_object(&emulator, row.key, bytes.clone()).await;
        }
    }
    let name = "build-outcome-revisions";
    let created_function = create_function(&emulator, name, "one-entry.zip").await;
    assert_eq!(
        created_function.status, 202,
        "the function is created, got {}",
        created_function.status
    );

    let mut created = Vec::new();
    for row in &rows {
        let reply = create_revision(&emulator, name, row.key).await;
        assert_eq!(
            reply.status, 202,
            "case {:?}: CreateWebFunctionRevision expected 202, got {} {:?}",
            row.case, reply.status, reply.body
        );
        assert_eq!(
            text(&reply.body, "state"),
            "Pending",
            "case {:?}: a just-created revision is still building, got {:?}",
            row.case,
            reply.body
        );
        created.push(text(&reply.body, "revisionId"));
    }

    tokio::time::sleep(settled()).await;

    for (row, revision_id) in rows.iter().zip(&created) {
        let revision = get_revision(&emulator, name, revision_id).await;
        let expected_state = if row.failure.is_some() {
            "Failed"
        } else {
            "Active"
        };
        assert_eq!(
            text(&revision, "state"),
            expected_state,
            "case {:?}: expected the build to settle {expected_state}, got {revision:?}",
            row.case
        );
        if let Some(reason) = row.failure {
            assert_eq!(
                text(&revision, "stateReason"),
                reason,
                "case {:?}: expected the stateReason {reason:?}, got {revision:?}",
                row.case
            );
        }
    }
}

/// RULE: an endpoint whose `autoDeploymentMode` is `LatestRevision` serves the newest
/// revision that BUILT — so it switches onto a revision whose build succeeded and never
/// onto one that failed, while a `Disabled` endpoint keeps serving what it was given
/// whatever is created after it.
#[tokio::test]
async fn an_auto_deploying_endpoint_switches_onto_a_built_revision_and_never_onto_a_failed_one() {
    let emulator = Emulator::with_object_store();
    create_bucket(&emulator).await;
    stage_object(
        &emulator,
        "deploy-a.zip",
        archive(&[("index.mjs", b"export const handler = () => 'a';")]),
    )
    .await;
    stage_object(
        &emulator,
        "deploy-c.zip",
        archive(&[("index.mjs", b"export const handler = () => 'c';")]),
    )
    .await;
    stage_object(&emulator, "deploy-b.zip", b"not a zip archive".to_vec()).await;

    // (case, the endpoint's autoDeploymentMode, whether it follows the newest built
    // revision)
    let modes: [(&str, &str, bool); 2] = [
        ("auto-deploying", "LatestRevision", true),
        ("fixed", "Disabled", false),
    ];
    for (case, mode, follows) in modes {
        let name = format!("auto-deploy-{case}");
        let created = emulator
            .send(
                "PUT",
                "/web-functions",
                REGION,
                Some(json!({
                    "functionName": name,
                    "revisionConfig": {
                        "buildConfig": build_config("nodejs24.x", BUCKET, "deploy-a.zip", None),
                        "serviceConfig": { "executionRoleArn": role_arn(&name) },
                    },
                    "endpointConfig": {
                        "endpointName": "ep",
                        "endpointType": "HomeRegion",
                        "authType": "ApplicationManaged",
                        "autoDeploymentMode": mode,
                    },
                })),
            )
            .await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: CreateWebFunction expected 202, got {} {:?}",
            created.status, created.body
        );
        let revision_a = text(
            created.field("revision").unwrap_or(&Value::Null),
            "revisionId",
        );

        // A revision whose code is not an archive: its build fails, so nothing may route
        // to it.
        let failed = create_revision(&emulator, &name, "deploy-b.zip").await;
        let revision_b = text(&failed.body, "revisionId");
        tokio::time::sleep(settled()).await;
        let endpoint = get_endpoint(&emulator, &name, "ep").await;
        assert_eq!(
            served_revisions(&endpoint),
            vec![revision_a.clone()],
            "case {case:?}: a revision whose build failed must take no traffic, got \
             {endpoint:?}"
        );
        assert_eq!(
            text(&get_revision(&emulator, &name, &revision_b).await, "state"),
            "Failed",
            "case {case:?}: revision B's build must have failed"
        );

        // A revision that builds: an auto-deploying endpoint follows it, a fixed one does
        // not.
        let built = create_revision(&emulator, &name, "deploy-c.zip").await;
        let revision_c = text(&built.body, "revisionId");
        tokio::time::sleep(settled()).await;
        let endpoint = get_endpoint(&emulator, &name, "ep").await;
        let expected = if follows {
            vec![revision_c.clone()]
        } else {
            vec![revision_a.clone()]
        };
        assert_eq!(
            served_revisions(&endpoint),
            expected,
            "case {case:?}: expected the endpoint to serve {expected:?}, got {endpoint:?}"
        );
    }
}

/// `GET /web-functions/{name}/endpoints/{endpointName}`.
async fn get_endpoint(emulator: &Emulator, name: &str, endpoint_name: &str) -> Value {
    let read = emulator
        .send(
            "GET",
            &format!("/web-functions/{name}/endpoints/{endpoint_name}"),
            REGION,
            None,
        )
        .await;
    assert_eq!(
        read.status, 200,
        "GetWebFunctionEndpoint {name}/{endpoint_name}: expected 200, got {} {:?}",
        read.status, read.body
    );
    read.body
}

/// The revision ids an endpoint's `revisionWeights` route traffic to.
fn served_revisions(endpoint: &Value) -> Vec<String> {
    endpoint
        .get("revisionWeights")
        .and_then(Value::as_array)
        .map(|weights| {
            weights
                .iter()
                .map(|weight| text(weight, "revisionId"))
                .collect()
        })
        .unwrap_or_default()
}
