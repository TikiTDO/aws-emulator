// SPDX-License-Identifier: CC0-1.0

//! The CodeDeploy on-premises instance registry, the per-application revision
//! registry and the blue/green instance-termination wait, driven over the wire.
//!
//! Every case here goes through the same front door a client uses: a signed-shape
//! `POST /` with `X-Amz-Target: CodeDeploy_20141006.<Operation>` dispatched by the
//! multi-service router, so the boundary validator, this service's refusal
//! wording and the awsJson1_1 envelope are all exercised, not bypassed.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The Region an unsigned request's ARNs carry, and the account id every
/// emulated ARN carries.
const REGION: &str = "us-east-1";
/// See [`REGION`].
const ACCOUNT: &str = "000000000000";

/// One emulator with one CodeDeploy service mounted on one shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing; routing is by `X-Amz-Target`.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_codedeploy_codedeploy_20141006::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call `operation` with `body`, answering its HTTP status and parsed body.
    async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("x-amz-target", format!("CodeDeploy_20141006.{operation}"))
            .header("content-type", "application/x-amz-json-1.1")
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Call `operation` and require success, answering the response body.
    async fn ok(&self, operation: &str, body: Value) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(status, 200, "{operation} must succeed, got {parsed:?}");
        parsed
    }

    /// Call `operation` and require a 400 refusal, answering its error code.
    async fn refused(&self, case: &str, operation: &str, body: Value) -> String {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from {operation}, got {status} {parsed:?}"
        );
        error_code(&parsed)
    }

    /// Create a deployment, waiting out the refusal a deployment group that is
    /// already running one answers with.
    ///
    /// A deployment group runs ONE deployment at a time, so a client deploying to
    /// the same group twice is refused with `DeploymentLimitExceededException`
    /// until the first deployment is terminal — which this waits out exactly as a
    /// client would. The loop is this suite's own because this suite keeps its own
    /// harness: the shared one in `codedeploy_lifecycle` is written against that
    /// suite's `Emulator`, and the rule both wait on is stated there.
    async fn deploy_when_free(&self, body: Value) -> Value {
        let deadline_at = Instant::now() + Duration::from_secs(15);
        loop {
            let (status, parsed) = self.call("CreateDeployment", body.clone()).await;
            if status == 200 {
                return parsed;
            }
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "DeploymentLimitExceededException"),
                "CreateDeployment must succeed or wait for its deployment group, got {status} \
                 {parsed:?}"
            );
            assert!(
                Instant::now() < deadline_at,
                "expected the deployment group to be free, got {parsed:?}"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
}

/// The wire error code of a failure body: awsJson carries it in `__type`, either
/// bare or namespaced.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// An `s3Location` revision, the form the AWS CLI's `--s3-location` sends.
fn s3_revision(bucket: &str, key: &str) -> Value {
    json!({"revisionType": "S3", "s3Location": {"bucket": bucket, "key": key, "bundleType": "zip"}})
}

/// Create an application, answering its name.
async fn application(emulator: &Emulator, name: &str) -> String {
    emulator
        .ok("CreateApplication", json!({"applicationName": name}))
        .await;
    name.to_string()
}

/// The tag a deployment group of this suite selects its targets by, and the
/// instance [`seeded_group`] registers carrying it.
///
/// A deployment ends `Succeeded` only if its group resolved a target, so every
/// group here is given one selector and one registered instance that satisfies
/// it; which status a group's targets imply is covered in
/// `codedeploy_deployment_outcome`.
const TARGET_TAG_KEY: &str = "Fleet";
/// See [`TARGET_TAG_KEY`].
const TARGET_TAG_VALUE: &str = "deployable";

/// Create an application and a deployment group on it, answering the group name.
/// `style` and `blue_green` are added to the group only when they are not null.
///
/// The group selects on-premises instances by [`TARGET_TAG_KEY`], and one
/// instance carrying that tag is registered for it, so its deployments have a
/// target to reach.
async fn seeded_group(
    emulator: &Emulator,
    application_name: &str,
    style: Value,
    blue_green: Value,
) -> String {
    application(emulator, application_name).await;
    let group = format!("{application_name}-dg");
    let mut request = json!({
        "applicationName": application_name,
        "deploymentGroupName": group,
        "serviceRoleArn": "arn:aws:iam::000000000000:role/CodeDeployRole",
        "onPremisesInstanceTagFilters": [
            {"Key": TARGET_TAG_KEY, "Value": TARGET_TAG_VALUE, "Type": "KEY_AND_VALUE"},
        ],
    });
    if !style.is_null() {
        request["deploymentStyle"] = style;
    }
    if !blue_green.is_null() {
        request["blueGreenDeploymentConfiguration"] = blue_green;
    }
    emulator.ok("CreateDeploymentGroup", request).await;

    let instance = format!("{application_name}-host");
    emulator
        .ok(
            "RegisterOnPremisesInstance",
            json!({
                "instanceName": instance,
                "iamUserArn": format!("arn:aws:iam::{ACCOUNT}:user/{instance}"),
            }),
        )
        .await;
    emulator
        .ok(
            "AddTagsToOnPremisesInstances",
            json!({
                "instanceNames": [instance],
                "tags": [{"Key": TARGET_TAG_KEY, "Value": TARGET_TAG_VALUE}],
            }),
        )
        .await;
    group
}

/// The `deploymentStyle` of a blue/green deployment group.
fn blue_green_style() -> Value {
    json!({"deploymentType": "BLUE_GREEN", "deploymentOption": "WITH_TRAFFIC_CONTROL"})
}

/// A `blueGreenDeploymentConfiguration` that takes `action` on the original
/// instances after `minutes` of waiting.
fn blue_green_config(action: &str, minutes: i32) -> Value {
    json!({
        "terminateBlueInstancesOnDeploymentSuccess": {
            "action": action,
            "terminationWaitTimeInMinutes": minutes,
        }
    })
}

/// Poll `GetDeployment` until `ready` accepts the stored `deploymentInfo`,
/// answering it. The transitions are owed by the CLOCK, so this waits for them
/// rather than asking for them.
async fn poll_deployment(
    emulator: &Emulator,
    case: &str,
    deployment_id: &str,
    ready: impl Fn(&Value) -> bool,
) -> Value {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let read = emulator
            .ok("GetDeployment", json!({"deploymentId": deployment_id}))
            .await;
        let info = read["deploymentInfo"].clone();
        if ready(&info) {
            return info;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the deployment never reached the expected state, last read {info:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// ---------------------------------------------------------------------------
// On-premises instances
// ---------------------------------------------------------------------------

/// RULE: an on-premises instance is answered from the record its registration
/// stored — its name, the one IAM ARN it was registered with, the ARN this
/// service minted for it and the instant it was registered — and the same record
/// answers a batch read.
#[tokio::test]
async fn an_on_premises_instance_reads_back_as_it_was_registered() {
    let emulator = Emulator::new();
    // Rows differ along the IAM identity (the model supports exactly one of the
    // two) and along the form and length of the instance name, which carries no
    // modelled pattern.
    let cases: &[(&str, &str, &str)] = &[
        (
            "AssetTag12010298EX",
            "iamUserArn",
            "arn:aws:iam::000000000000:user/CodeDeployUser",
        ),
        (
            "host-1.example.com",
            "iamSessionArn",
            "arn:aws:sts::000000000000:assumed-role/CodeDeployRole/host-1",
        ),
        ("a", "iamUserArn", "arn:aws:iam::000000000000:user/short"),
        (
            &"n".repeat(100),
            "iamSessionArn",
            "arn:aws:sts::000000000000:assumed-role/CodeDeployRole/long",
        ),
    ];

    for (name, arn_member, arn) in cases {
        emulator
            .ok(
                "RegisterOnPremisesInstance",
                json!({"instanceName": name, *arn_member: arn}),
            )
            .await;
        let read = emulator
            .ok("GetOnPremisesInstance", json!({"instanceName": name}))
            .await;
        let info = &read["instanceInfo"];
        assert_eq!(
            info["instanceName"].as_str(),
            Some(*name),
            "case {name:?}: expected the registered instanceName, got {info:?}"
        );
        // The ARN names this REGISTRATION: the host name, then the registration
        // id the service minted for it.
        let arn_prefix = format!("arn:aws:codedeploy:{REGION}:{ACCOUNT}:instance/{name}_");
        let reported_arn = info["instanceArn"].as_str().unwrap_or_default();
        assert!(
            reported_arn
                .strip_prefix(arn_prefix.as_str())
                .is_some_and(|registration| !registration.is_empty()),
            "case {name:?}: expected the minted instanceArn to start {arn_prefix:?} and carry a \
             registration id, got {info:?}"
        );
        assert_eq!(
            info[*arn_member].as_str(),
            Some(*arn),
            "case {name:?}: expected {arn_member} {arn}, got {info:?}"
        );
        let other = if *arn_member == "iamUserArn" {
            "iamSessionArn"
        } else {
            "iamUserArn"
        };
        assert!(
            info[other].is_null(),
            "case {name:?}: expected no {other}, got {info:?}"
        );
        assert!(
            info["registerTime"].as_f64().unwrap_or_default() > 0.0,
            "case {name:?}: expected a registerTime, got {info:?}"
        );
        assert!(
            info["deregisterTime"].is_null(),
            "case {name:?}: expected no deregisterTime, got {info:?}"
        );
    }

    // A batch read answers the same records, for the names it holds one for, in
    // the order the request listed them.
    let names: Vec<&str> = cases.iter().map(|(name, _, _)| *name).collect();
    let mut requested = names.clone();
    requested.push("never-registered");
    let batch = emulator
        .ok(
            "BatchGetOnPremisesInstances",
            json!({"instanceNames": requested}),
        )
        .await;
    let reported: Vec<String> = batch["instanceInfos"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|info| info["instanceName"].as_str().map(str::to_string))
        .collect();
    assert_eq!(
        reported,
        names.iter().map(|name| name.to_string()).collect::<Vec<_>>(),
        "case batch of every registered name plus one unknown: expected the registered names in \
         request order, got {reported:?}"
    );
}

/// RULE: a name that holds no registration is not an instance — reading it is
/// refused with `InstanceNotRegisteredException` — and a name a request did not
/// give at all is refused with `InstanceNameRequiredException`.
#[tokio::test]
async fn reading_an_unregistered_on_premises_instance_is_refused() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "RegisterOnPremisesInstance",
            json!({
                "instanceName": "registered-one",
                "iamUserArn": "arn:aws:iam::000000000000:user/CodeDeployUser",
            }),
        )
        .await;

    // Rows differ along the form of a name nothing registered, including one
    // that differs from a registered name only in case.
    for name in [
        "never-registered-name",
        "REGISTERED-ONE",
        "registered-one.2",
    ] {
        let code = emulator
            .refused(name, "GetOnPremisesInstance", json!({"instanceName": name}))
            .await;
        assert_eq!(
            code, "InstanceNotRegisteredException",
            "case {name:?}: expected InstanceNotRegisteredException, got {code}"
        );
    }

    // A name the request did not give, and a blank one, are the SAME refusal:
    // the operation was given no name to look up. A name outside the form
    // CodeDeploy accepts is the other refusal: it is not a name at all.
    for (case, request, expected) in [
        ("absent instanceName", json!({}), "InstanceNameRequiredException"),
        (
            "empty instanceName",
            json!({"instanceName": ""}),
            "InstanceNameRequiredException",
        ),
        (
            "blank instanceName",
            json!({"instanceName": "   "}),
            "InstanceNameRequiredException",
        ),
        (
            "a name carrying a space",
            json!({"instanceName": "registered-one "}),
            "InvalidInstanceNameException",
        ),
        (
            "a name longer than the service's name length",
            json!({"instanceName": "n".repeat(101)}),
            "InvalidInstanceNameException",
        ),
        (
            // The EC2-instance-id form is RESERVED, so it is refused as a name
            // rather than reported as a host nothing registered.
            "a name in the reserved EC2 instance id form",
            json!({"instanceName": "i-0123456789abcdef0"}),
            "InvalidInstanceNameException",
        ),
    ] {
        let code = emulator
            .refused(case, "GetOnPremisesInstance", request)
            .await;
        assert_eq!(
            code, expected,
            "case {case:?}: expected {expected}, got {code}"
        );
    }
}

/// RULE: deregistering an instance RETIRES its record with a `deregisterTime`
/// instead of dropping it — so the batch read still reports it while the
/// single-instance read refuses it as not registered — deregistering a name
/// twice or deregistering one that was never registered is a success, and the
/// name can be registered again.
#[tokio::test]
async fn deregistering_an_on_premises_instance_retires_its_record() {
    let emulator = Emulator::new();
    let user_arn = "arn:aws:iam::000000000000:user/CodeDeployUser";
    emulator
        .ok(
            "RegisterOnPremisesInstance",
            json!({"instanceName": "retired", "iamUserArn": user_arn}),
        )
        .await;

    // Rows: the name holds a registration, the name was already deregistered,
    // and the name never held one at all. Every one of them succeeds.
    for case in ["registered", "already deregistered", "never registered"] {
        let name = if case == "never registered" {
            "no-such-instance"
        } else {
            "retired"
        };
        let (status, body) = emulator
            .call("DeregisterOnPremisesInstance", json!({"instanceName": name}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeregisterOnPremisesInstance to succeed, got {status} {body:?}"
        );
    }

    // The registration is gone, so the single-instance reader refuses the name.
    let code = emulator
        .refused(
            "a deregistered instance",
            "GetOnPremisesInstance",
            json!({"instanceName": "retired"}),
        )
        .await;
    assert_eq!(
        code, "InstanceNotRegisteredException",
        "case a deregistered instance: expected InstanceNotRegisteredException, got {code}"
    );

    // The retired RECORD is still there: the batch read reports it, with the
    // instant it was retired at.
    let read = emulator
        .ok(
            "BatchGetOnPremisesInstances",
            json!({"instanceNames": ["retired"]}),
        )
        .await;
    let info = &read["instanceInfos"][0];
    let deregister_time = info["deregisterTime"].as_f64().unwrap_or_default();
    assert!(
        deregister_time > 0.0,
        "case retired record: expected a deregisterTime, got {info:?}"
    );
    assert!(
        info["registerTime"].as_f64().unwrap_or_default() <= deregister_time,
        "case retired record: expected registerTime <= deregisterTime, got {info:?}"
    );

    // Deregistering twice does not move the instant it was retired at.
    let again = emulator
        .ok(
            "BatchGetOnPremisesInstances",
            json!({"instanceNames": ["retired"]}),
        )
        .await;
    assert_eq!(
        again, read,
        "case retired record read twice: expected a repeatable read, got {again:?}"
    );

    // The name, and the IAM ARN, are free again.
    emulator
        .ok(
            "RegisterOnPremisesInstance",
            json!({"instanceName": "retired", "iamUserArn": user_arn}),
        )
        .await;
    let reregistered = emulator
        .ok("GetOnPremisesInstance", json!({"instanceName": "retired"}))
        .await;
    assert!(
        reregistered["instanceInfo"]["deregisterTime"].is_null(),
        "case re-registered: expected no deregisterTime, got {:?}",
        reregistered["instanceInfo"]
    );
}

/// RULE: a batch read carries at most the number of names
/// `BatchLimitExceededException` states as the request-size quota (100) and at
/// least one, and a batch outside that range is refused as a client error rather
/// than served.
#[tokio::test]
async fn a_batch_of_on_premises_instances_is_held_to_the_documented_limit() {
    let emulator = Emulator::new();
    let names: Vec<String> = (0..101).map(|index| format!("batched-{index:03}")).collect();
    for name in names.iter().take(100) {
        emulator
            .ok(
                "RegisterOnPremisesInstance",
                json!({
                    "instanceName": name,
                    "iamUserArn": format!("arn:aws:iam::000000000000:user/{name}"),
                }),
            )
            .await;
    }

    // Rows: an empty batch, sizes inside the quota (including the 25 the
    // operation's own prose names and one past it), the quota itself, and one
    // past it.
    let cases: &[(usize, Option<&str>)] = &[
        (0, Some("InstanceNameRequiredException")),
        (1, None),
        (25, None),
        (26, None),
        (100, None),
        (101, Some("BatchLimitExceededException")),
    ];

    for (size, expected) in cases {
        let batch: Vec<String> = names.iter().take(*size).cloned().collect();
        let request = json!({"instanceNames": batch});
        match expected {
            Some(code) => {
                let actual = emulator
                    .refused(
                        &format!("{size} names"),
                        "BatchGetOnPremisesInstances",
                        request,
                    )
                    .await;
                assert_eq!(
                    actual, *code,
                    "case {size} names: expected {code}, got {actual}"
                );
            }
            None => {
                let answered = emulator
                    .ok("BatchGetOnPremisesInstances", request)
                    .await;
                let count = answered["instanceInfos"]
                    .as_array()
                    .map(Vec::len)
                    .unwrap_or_default();
                assert_eq!(
                    count, *size,
                    "case {size} names: expected {size} instanceInfos, got {answered:?}"
                );
            }
        }
    }

    // A blank name inside an otherwise legal batch is the same refusal as an
    // absent `instanceNames`.
    let code = emulator
        .refused(
            "a blank name in the batch",
            "BatchGetOnPremisesInstances",
            json!({"instanceNames": ["batched-000", ""]}),
        )
        .await;
    assert_eq!(
        code, "InstanceNameRequiredException",
        "case a blank name in the batch: expected InstanceNameRequiredException, got {code}"
    );
}

// ---------------------------------------------------------------------------
// Application revisions
// ---------------------------------------------------------------------------
/// RULE: a revision is answered from the record its registration stored — the
/// location verbatim, the description it was registered with and the instant it
/// was registered — and a revision is identified by WHICH artifact it names, not
/// by how that artifact is packaged.
#[tokio::test]
async fn an_application_revision_reads_back_as_it_was_registered() {
    let emulator = Emulator::new();
    let name = application(&emulator, "revisioned").await;
    // Rows differ along `revisionType`: every form the model's
    // `RevisionLocation` carries a location for.
    let cases: &[(&str, Value)] = &[
        ("S3", s3_revision("artifacts", "prod/app.zip")),
        (
            "S3 with a version and an ETag",
            json!({
                "revisionType": "S3",
                "s3Location": {
                    "bucket": "artifacts",
                    "key": "prod/app.zip",
                    "bundleType": "zip",
                    "version": "3sL4kqtJlcpXroDTDmJ+rmSpXd3dIbrHY+MTRCxf3vjVBH40Nr8X8gdRQBpUMLUo",
                    "eTag": "\"9b2cf535f27731c974343645a3985328\"",
                },
            }),
        ),
        (
            "GitHub",
            json!({
                "revisionType": "GitHub",
                "gitHubLocation": {
                    "repository": "my-account/my-app",
                    "commitId": "f3e4a1b2c3d4e5f60718293a4b5c6d7e8f901234",
                },
            }),
        ),
        (
            "String",
            json!({
                "revisionType": "String",
                "string": {"content": "version: 0.0\nos: linux\n", "sha256": "abc123"},
            }),
        ),
        (
            "AppSpecContent",
            json!({
                "revisionType": "AppSpecContent",
                "appSpecContent": {"content": "{\"version\":\"0.0\"}", "sha256": "def456"},
            }),
        ),
    ];

    for (case, revision) in cases {
        emulator
            .ok(
                "RegisterApplicationRevision",
                json!({
                    "applicationName": name,
                    "revision": revision,
                    "description": format!("registered for {case}"),
                }),
            )
            .await;
        let read = emulator
            .ok(
                "GetApplicationRevision",
                json!({"applicationName": name, "revision": revision}),
            )
            .await;
        assert_eq!(
            read["applicationName"].as_str(),
            Some(name.as_str()),
            "case {case:?}: expected the application name back, got {read:?}"
        );
        assert_eq!(
            read["revision"], *revision,
            "case {case:?}: expected the revision verbatim, got {:?}",
            read["revision"]
        );
        assert_eq!(
            read["revisionInfo"]["description"].as_str(),
            Some(format!("registered for {case}").as_str()),
            "case {case:?}: expected the registered description, got {:?}",
            read["revisionInfo"]
        );
        assert!(
            read["revisionInfo"]["registerTime"]
                .as_f64()
                .unwrap_or_default()
                > 0.0,
            "case {case:?}: expected a registerTime, got {:?}",
            read["revisionInfo"]
        );
        assert!(
            read["revisionInfo"]["firstUsedTime"].is_null()
                && read["revisionInfo"]["lastUsedTime"].is_null(),
            "case {case:?}: expected no use times for a revision no deployment used, got {:?}",
            read["revisionInfo"]
        );
    }

    // `bundleType` says how to unpack the object, not which object it is, so the
    // same location under another bundle type is the SAME revision.
    let repackaged = json!({
        "revisionType": "S3",
        "s3Location": {"bucket": "artifacts", "key": "prod/app.zip", "bundleType": "tgz"},
    });
    let read = emulator
        .ok(
            "GetApplicationRevision",
            json!({"applicationName": name, "revision": repackaged}),
        )
        .await;
    assert_eq!(
        read["revision"]["s3Location"]["bundleType"].as_str(),
        Some("zip"),
        "case another bundleType: expected the registered revision, got {read:?}"
    );
}

/// RULE: CodeDeploy knows a revision by REGISTRATION, so a revision no
/// registration and no deployment ever named does not exist for the application
/// — whatever sits at that location — and an application that does not exist has
/// no revisions to look in.
#[tokio::test]
async fn an_unregistered_application_revision_does_not_exist() {
    let emulator = Emulator::new();
    let name = application(&emulator, "sparse").await;
    emulator
        .ok(
            "RegisterApplicationRevision",
            json!({
                "applicationName": name,
                "revision": s3_revision("artifacts", "prod/app.zip"),
            }),
        )
        .await;

    // Rows differ along WHICH part of the identity the request got wrong.
    let cases: &[(&str, Value)] = &[
        (
            "another bucket",
            s3_revision("other-bucket", "prod/app.zip"),
        ),
        ("another key", s3_revision("artifacts", "prod/other.zip")),
        (
            "a version the registration did not carry",
            json!({
                "revisionType": "S3",
                "s3Location": {
                    "bucket": "artifacts",
                    "key": "prod/app.zip",
                    "bundleType": "zip",
                    "version": "AnotherObjectVersion",
                },
            }),
        ),
        (
            "a GitHub commit",
            json!({
                "revisionType": "GitHub",
                "gitHubLocation": {
                    "repository": "my-account/my-app",
                    "commitId": "0000000000000000000000000000000000000000",
                },
            }),
        ),
        (
            "inline content nobody registered",
            json!({
                "revisionType": "AppSpecContent",
                "appSpecContent": {"content": "{\"version\":\"0.0\"}"},
            }),
        ),
    ];

    for (case, revision) in cases {
        let code = emulator
            .refused(
                case,
                "GetApplicationRevision",
                json!({"applicationName": name, "revision": revision}),
            )
            .await;
        assert_eq!(
            code, "RevisionDoesNotExistException",
            "case {case:?}: expected RevisionDoesNotExistException, got {code}"
        );
    }

    let code = emulator
        .refused(
            "an application that does not exist",
            "GetApplicationRevision",
            json!({
                "applicationName": "no-such-application",
                "revision": s3_revision("artifacts", "prod/app.zip"),
            }),
        )
        .await;
    assert_eq!(
        code, "ApplicationDoesNotExistException",
        "case an application that does not exist: expected ApplicationDoesNotExistException, got \
         {code}"
    );

    // A revision that names no location at all identifies nothing to look up.
    for (case, revision) in [
        ("an empty revision", json!({})),
        ("a revisionType with no location", json!({"revisionType": "S3"})),
    ] {
        let code = emulator
            .refused(
                case,
                "GetApplicationRevision",
                json!({"applicationName": name, "revision": revision}),
            )
            .await;
        assert_eq!(
            code, "InvalidRevisionException",
            "case {case:?}: expected InvalidRevisionException, got {code}"
        );
    }
}

/// RULE: deploying a revision is also how CodeDeploy learns it, so a deployment
/// registers the revision it names, dates its first and last use, and makes it
/// the target revision of the deployment group it deployed to.
#[tokio::test]
async fn deploying_a_revision_registers_it_and_names_the_group() {
    let emulator = Emulator::new();
    let group = seeded_group(&emulator, "deployed-from", Value::Null, Value::Null).await;
    let revision = s3_revision("artifacts", "prod/deployed.zip");

    // Nothing is known about the revision before the deployment.
    let code = emulator
        .refused(
            "before the deployment",
            "GetApplicationRevision",
            json!({"applicationName": "deployed-from", "revision": revision}),
        )
        .await;
    assert_eq!(
        code, "RevisionDoesNotExistException",
        "case before the deployment: expected RevisionDoesNotExistException, got {code}"
    );

    emulator
        .deploy_when_free(json!({
            "applicationName": "deployed-from",
            "deploymentGroupName": group,
            "revision": revision,
        }))
        .await;

    let read = emulator
        .ok(
            "GetApplicationRevision",
            json!({"applicationName": "deployed-from", "revision": revision}),
        )
        .await;
    let info = &read["revisionInfo"];
    let first_used = info["firstUsedTime"].as_f64().unwrap_or_default();
    let last_used = info["lastUsedTime"].as_f64().unwrap_or_default();
    assert!(
        first_used > 0.0 && (first_used - last_used).abs() < f64::EPSILON,
        "case one deployment: expected firstUsedTime == lastUsedTime, got {info:?}"
    );
    assert_eq!(
        info["deploymentGroups"].as_array().map(|groups| groups
            .iter()
            .filter_map(|group| group.as_str().map(str::to_string))
            .collect::<Vec<_>>()),
        Some(vec![group.clone()]),
        "case one deployment: expected the deployment group it targets, got {info:?}"
    );

    // A second deployment of the SAME revision moves only its last use.
    emulator
        .deploy_when_free(json!({
            "applicationName": "deployed-from",
            "deploymentGroupName": group,
            "revision": revision,
        }))
        .await;
    let again = emulator
        .ok(
            "GetApplicationRevision",
            json!({"applicationName": "deployed-from", "revision": revision}),
        )
        .await;
    let info = &again["revisionInfo"];
    assert_eq!(
        info["firstUsedTime"].as_f64(),
        Some(first_used),
        "case a second deployment: expected firstUsedTime to stay put, got {info:?}"
    );
    assert!(
        info["lastUsedTime"].as_f64().unwrap_or_default() >= last_used,
        "case a second deployment: expected lastUsedTime to move forward, got {info:?}"
    );

    // Deploying another revision moves the group's target revision to it, so the
    // first revision is no longer any group's target.
    let newer = s3_revision("artifacts", "prod/newer.zip");
    emulator
        .deploy_when_free(json!({
            "applicationName": "deployed-from",
            "deploymentGroupName": group,
            "revision": newer,
        }))
        .await;
    let superseded = emulator
        .ok(
            "GetApplicationRevision",
            json!({"applicationName": "deployed-from", "revision": revision}),
        )
        .await;
    // The member is still REPORTED, as the empty list: the revision has a set of
    // groups targeting it and that set is now empty.
    assert_eq!(
        superseded["revisionInfo"]["deploymentGroups"],
        json!([]),
        "case a superseded revision: expected deploymentGroups [], got {:?}",
        superseded["revisionInfo"]
    );
}

/// The bucket-and-key identity of each revision a listing answered, so a page
/// can be compared as data.
fn listed_keys(page: &Value) -> Vec<String> {
    page["revisions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|revision| {
            let s3 = &revision["s3Location"];
            match (s3["bucket"].as_str(), s3["key"].as_str()) {
                (Some(bucket), Some(key)) => format!("{bucket}/{key}"),
                _ => revision["gitHubLocation"]["commitId"]
                    .as_str()
                    .unwrap_or("<neither S3 nor GitHub>")
                    .to_string(),
            }
        })
        .collect()
}

/// RULE: a revision listing answers the application's registered revisions,
/// narrowed by the filters the request named — an S3 key prefix only ever
/// narrows WITHIN a bucket, so it is refused without one — and ordered by the
/// column and direction it asked for.
#[tokio::test]
async fn listing_application_revisions_filters_and_sorts() {
    let emulator = Emulator::new();
    let group = seeded_group(&emulator, "listed", Value::Null, Value::Null).await;
    let revisions = [
        s3_revision("bucket-a", "prod/one.zip"),
        s3_revision("bucket-a", "stage/two.zip"),
        s3_revision("bucket-b", "prod/three.zip"),
        json!({
            "revisionType": "GitHub",
            "gitHubLocation": {"repository": "acct/app", "commitId": "cafebabe"},
        }),
    ];
    for revision in &revisions {
        emulator
            .ok(
                "RegisterApplicationRevision",
                json!({"applicationName": "listed", "revision": revision}),
            )
            .await;
    }
    // One of them becomes the group's target revision, which is what the
    // `deployed` filter selects on.
    emulator
        .ok(
            "CreateDeployment",
            json!({
                "applicationName": "listed",
                "deploymentGroupName": group,
                "revision": revisions[0],
            }),
        )
        .await;

    // Rows differ along the filter and the ordering the request asked for.
    let cases: &[(&str, Value, Vec<&str>)] = &[
        (
            "no filter",
            json!({"applicationName": "listed"}),
            vec![
                "bucket-a/prod/one.zip",
                "bucket-a/stage/two.zip",
                "bucket-b/prod/three.zip",
                "cafebabe",
            ],
        ),
        (
            "a bucket filter",
            json!({"applicationName": "listed", "s3Bucket": "bucket-a"}),
            vec!["bucket-a/prod/one.zip", "bucket-a/stage/two.zip"],
        ),
        (
            "a bucket and a key prefix",
            json!({"applicationName": "listed", "s3Bucket": "bucket-a", "s3KeyPrefix": "prod/"}),
            vec!["bucket-a/prod/one.zip"],
        ),
        (
            "a bucket and a key prefix that matches nothing",
            json!({"applicationName": "listed", "s3Bucket": "bucket-a", "s3KeyPrefix": "none/"}),
            vec![],
        ),
        (
            "deployed=include",
            json!({"applicationName": "listed", "deployed": "include"}),
            vec!["bucket-a/prod/one.zip"],
        ),
        (
            "deployed=exclude",
            json!({"applicationName": "listed", "deployed": "exclude"}),
            vec![
                "bucket-a/stage/two.zip",
                "bucket-b/prod/three.zip",
                "cafebabe",
            ],
        ),
        (
            "deployed=ignore",
            json!({"applicationName": "listed", "deployed": "ignore"}),
            vec![
                "bucket-a/prod/one.zip",
                "bucket-a/stage/two.zip",
                "bucket-b/prod/three.zip",
                "cafebabe",
            ],
        ),
        (
            "sortBy=registerTime descending",
            json!({
                "applicationName": "listed",
                "sortBy": "registerTime",
                "sortOrder": "descending",
            }),
            vec![
                "cafebabe",
                "bucket-b/prod/three.zip",
                "bucket-a/stage/two.zip",
                "bucket-a/prod/one.zip",
            ],
        ),
        (
            "sortBy=lastUsedTime descending",
            json!({
                "applicationName": "listed",
                "sortBy": "lastUsedTime",
                "sortOrder": "descending",
            }),
            vec![
                "bucket-a/prod/one.zip",
                "cafebabe",
                "bucket-b/prod/three.zip",
                "bucket-a/stage/two.zip",
            ],
        ),
    ];

    for (case, request, expected) in cases {
        let page = emulator.ok("ListApplicationRevisions", request.clone()).await;
        assert_eq!(
            listed_keys(&page),
            expected
                .iter()
                .map(|key| key.to_string())
                .collect::<Vec<_>>(),
            "case {case:?}: expected {expected:?}, got {page:?}"
        );
    }

    // A key prefix with no bucket to narrow, and a bucket filter that names no
    // bucket, are refused with the codes this operation models for them.
    let refusals: &[(&str, Value, &str)] = &[
        (
            "a key prefix with no bucket",
            json!({"applicationName": "listed", "s3KeyPrefix": "prod/"}),
            "BucketNameFilterRequiredException",
        ),
        (
            "an empty bucket filter",
            json!({"applicationName": "listed", "s3Bucket": ""}),
            "InvalidBucketNameFilterException",
        ),
        (
            "an application that does not exist",
            json!({"applicationName": "no-such-application"}),
            "ApplicationDoesNotExistException",
        ),
        (
            "a sort column the model does not carry",
            json!({"applicationName": "listed", "sortBy": "whenever"}),
            "InvalidSortByException",
        ),
        (
            "a sort order the model does not carry",
            json!({"applicationName": "listed", "sortOrder": "sideways"}),
            "InvalidSortOrderException",
        ),
        (
            "a deployed-state filter the model does not carry",
            json!({"applicationName": "listed", "deployed": "maybe"}),
            "InvalidDeployedStateFilterException",
        ),
    ];
    for (case, request, expected) in refusals {
        let code = emulator
            .refused(case, "ListApplicationRevisions", request.clone())
            .await;
        assert_eq!(
            code, *expected,
            "case {case:?}: expected {expected}, got {code}"
        );
    }
}

/// RULE: `ListApplicationRevisions` is `@paginated` on `nextToken` alone, so a
/// token CONTINUES the listing — every registered revision is reported exactly
/// once across the pages — and a token this service did not issue is refused.
#[tokio::test]
async fn listing_application_revisions_pages_without_repeating() {
    let emulator = Emulator::new();
    let name = application(&emulator, "paged").await;
    let total = 150;
    for index in 0..total {
        emulator
            .ok(
                "RegisterApplicationRevision",
                json!({
                    "applicationName": name,
                    "revision": s3_revision("artifacts", &format!("app-{index:03}.zip")),
                }),
            )
            .await;
    }

    let mut collected: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut request = json!({"applicationName": name});
        if let Some(token) = &token {
            request["nextToken"] = json!(token);
        }
        let page = emulator.ok("ListApplicationRevisions", request).await;
        let keys = listed_keys(&page);
        assert!(
            !keys.is_empty(),
            "case page {pages}: expected a non-empty page, got {page:?}"
        );
        collected.extend(keys);
        pages += 1;
        assert!(pages <= 5, "case page {pages}: paging did not terminate");
        token = page["nextToken"].as_str().map(str::to_string);
        if token.is_none() {
            break;
        }
    }

    assert!(
        pages > 1,
        "case {total} revisions: expected more than one page, got {pages}"
    );
    let mut distinct = collected.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        (collected.len(), distinct.len()),
        (total, total),
        "case {total} revisions: expected every revision exactly once, got {} of which {} distinct",
        collected.len(),
        distinct.len()
    );

    for bogus in ["not-a-token", "p", "pzzz-", ""] {
        let code = emulator
            .refused(
                bogus,
                "ListApplicationRevisions",
                json!({"applicationName": name, "nextToken": bogus}),
            )
            .await;
        assert_eq!(
            code, "InvalidNextTokenException",
            "case token {bogus:?}: expected InvalidNextTokenException, got {code}"
        );
    }
}

// ---------------------------------------------------------------------------
// The blue/green instance-termination wait
// ---------------------------------------------------------------------------

/// RULE: a blue/green deployment whose group terminates the original instances
/// holds the deployment open for the `terminationWaitTimeInMinutes` that group
/// configured — reporting the wait as started once traffic routing is complete —
/// and `SkipWaitTimeForInstanceTermination` overrides that wait so the
/// deployment completes immediately after the routing.
#[tokio::test]
async fn skipping_the_wait_terminates_a_blue_green_deployment_immediately() {
    // Rows differ along the configured wait: a documented value, and the maximum
    // the model documents for the member (2880 minutes).
    for minutes in [60, 2880] {
        let case = format!("a {minutes} minute termination wait");
        let emulator = Emulator::new();
        let application_name = format!("waiting-{minutes}");
        let group = seeded_group(
            &emulator,
            &application_name,
            blue_green_style(),
            blue_green_config("TERMINATE", minutes),
        )
        .await;
        let created = emulator
            .ok(
                "CreateDeployment",
                json!({
                    "applicationName": application_name,
                    "deploymentGroupName": group,
                    "revision": s3_revision("artifacts", "blue-green.zip"),
                }),
            )
            .await;
        let deployment_id = created["deploymentId"]
            .as_str()
            .expect("a deployment id")
            .to_string();

        // The wait starts once the routing is complete, and the deployment stays
        // non-terminal while it runs.
        let waiting = poll_deployment(&emulator, &case, &deployment_id, |info| {
            info["instanceTerminationWaitTimeStarted"].as_bool() == Some(true)
        })
        .await;
        assert_eq!(
            waiting["status"].as_str(),
            Some("InProgress"),
            "case {case:?}: expected the deployment to still be running while it waits, got \
             {waiting:?}"
        );
        assert!(
            waiting["completeTime"].is_null(),
            "case {case:?}: expected no completeTime while it waits, got {waiting:?}"
        );

        emulator
            .ok(
                "SkipWaitTimeForInstanceTermination",
                json!({"deploymentId": deployment_id}),
            )
            .await;

        let terminal = poll_deployment(&emulator, &case, &deployment_id, |info| {
            info["status"].as_str() == Some("Succeeded")
        })
        .await;
        let create_time = terminal["createTime"].as_f64().unwrap_or_default();
        let complete_time = terminal["completeTime"].as_f64().unwrap_or_default();
        assert!(
            complete_time >= create_time,
            "case {case:?}: expected createTime <= completeTime, got {terminal:?}"
        );
        assert!(
            complete_time - create_time < f64::from(minutes) * 60.0,
            "case {case:?}: expected the wait to have been skipped, got {terminal:?}"
        );
        assert_eq!(
            terminal["instanceTerminationWaitTimeStarted"].as_bool(),
            Some(true),
            "case {case:?}: expected the wait to still read as started, got {terminal:?}"
        );

        // And skipping it again is refused: there is no wait left on a
        // deployment that is already complete.
        let code = emulator
            .refused(
                &case,
                "SkipWaitTimeForInstanceTermination",
                json!({"deploymentId": deployment_id}),
            )
            .await;
        assert_eq!(
            code, "DeploymentAlreadyCompletedException",
            "case {case:?}: expected DeploymentAlreadyCompletedException, got {code}"
        );
    }
}

/// RULE: only a blue/green deployment that terminates its original instances has
/// a termination wait, so a deployment group that keeps them alive reports no
/// wait and completes without one.
#[tokio::test]
async fn a_group_that_keeps_the_original_instances_alive_waits_for_nothing() {
    let emulator = Emulator::new();
    // Rows differ along the group's style and termination action.
    let cases: &[(&str, Value, Value)] = &[
        (
            "KEEP_ALIVE blue/green",
            blue_green_style(),
            blue_green_config("KEEP_ALIVE", 60),
        ),
        (
            "in-place with no blue/green configuration",
            json!({"deploymentType": "IN_PLACE", "deploymentOption": "WITHOUT_TRAFFIC_CONTROL"}),
            Value::Null,
        ),
        ("no deployment style at all", Value::Null, Value::Null),
    ];

    for (index, (case, style, config)) in cases.iter().enumerate() {
        let application_name = format!("no-wait-{index}");
        let group = seeded_group(&emulator, &application_name, style.clone(), config.clone()).await;
        let created = emulator
            .ok(
                "CreateDeployment",
                json!({
                    "applicationName": application_name,
                    "deploymentGroupName": group,
                    "revision": s3_revision("artifacts", "no-wait.zip"),
                }),
            )
            .await;
        let deployment_id = created["deploymentId"]
            .as_str()
            .expect("a deployment id")
            .to_string();
        let terminal = poll_deployment(&emulator, case, &deployment_id, |info| {
            info["status"].as_str() == Some("Succeeded")
        })
        .await;
        assert_eq!(
            terminal["instanceTerminationWaitTimeStarted"].as_bool(),
            Some(false),
            "case {case:?}: expected no termination wait to be reported, got {terminal:?}"
        );
    }
}

/// RULE: skipping the termination wait acts on ONE existing blue/green
/// deployment that has started and not finished; every other request is refused
/// with the modelled code for what is wrong with it.
#[tokio::test]
async fn skipping_the_termination_wait_is_refused_where_it_does_not_apply() {
    let emulator = Emulator::new();

    // A deployment id the request did not give, or gave blank.
    for (case, request) in [
        ("absent deploymentId", json!({})),
        ("empty deploymentId", json!({"deploymentId": ""})),
        ("blank deploymentId", json!({"deploymentId": "  "})),
    ] {
        let code = emulator
            .refused(case, "SkipWaitTimeForInstanceTermination", request)
            .await;
        assert_eq!(
            code, "DeploymentIdRequiredException",
            "case {case:?}: expected DeploymentIdRequiredException, got {code}"
        );
    }

    // Rows differ along the form of an id no deployment was ever created with.
    // An id of the minted `d-XXXXXXXXX` form is answered on existence; one of
    // another form is not an id at all, and is refused on FORMAT (the gate is
    // covered in `codedeploy_deployment_outcome`).
    for id in ["d-ABCDEFGHI", "d-000000000"] {
        let code = emulator
            .refused(
                id,
                "SkipWaitTimeForInstanceTermination",
                json!({"deploymentId": id}),
            )
            .await;
        assert_eq!(
            code, "DeploymentDoesNotExistException",
            "case {id:?}: expected DeploymentDoesNotExistException, got {code}"
        );
    }
    let malformed = "not-a-deployment-id";
    let code = emulator
        .refused(
            malformed,
            "SkipWaitTimeForInstanceTermination",
            json!({"deploymentId": malformed}),
        )
        .await;
    assert_eq!(
        code, "InvalidDeploymentIdException",
        "case {malformed:?}: expected InvalidDeploymentIdException, got {code}"
    );

    // An id outside CodeDeploy's `d-` + uppercase-alphanumeric format is refused
    // for its FORMAT, before the deployment is looked up: that is what
    // `InvalidDeploymentIdException` ("At least one of the deployment IDs was
    // specified in an invalid format") distinguishes from an unknown id, and every
    // operation of this service taking a deployment id shares that one check.
    let code = emulator
        .refused(
            "malformed deploymentId",
            "SkipWaitTimeForInstanceTermination",
            json!({"deploymentId": "not-a-deployment-id"}),
        )
        .await;
    assert_eq!(
        code, "InvalidDeploymentIdException",
        "case malformed deploymentId: expected InvalidDeploymentIdException, got {code}"
    );

    // An in-place deployment has no original environment to terminate. The call
    // is made straight after the deployment is created, while it is still
    // running.
    let group = seeded_group(
        &emulator,
        "in-place",
        json!({"deploymentType": "IN_PLACE", "deploymentOption": "WITHOUT_TRAFFIC_CONTROL"}),
        Value::Null,
    )
    .await;
    let created = emulator
        .ok(
            "CreateDeployment",
            json!({
                "applicationName": "in-place",
                "deploymentGroupName": group,
                "revision": s3_revision("artifacts", "in-place.zip"),
            }),
        )
        .await;
    let in_place_id = created["deploymentId"]
        .as_str()
        .expect("a deployment id")
        .to_string();
    let code = emulator
        .refused(
            "an in-place deployment",
            "SkipWaitTimeForInstanceTermination",
            json!({"deploymentId": in_place_id}),
        )
        .await;
    assert_eq!(
        code, "UnsupportedActionForDeploymentTypeException",
        "case an in-place deployment: expected UnsupportedActionForDeploymentTypeException, got \
         {code}"
    );

    // And once that deployment has reached its terminal status, the refusal is
    // about its lifecycle instead: there is no wait left to skip.
    poll_deployment(&emulator, "a terminal deployment", &in_place_id, |info| {
        info["status"].as_str() == Some("Succeeded")
    })
    .await;
    let code = emulator
        .refused(
            "a terminal deployment",
            "SkipWaitTimeForInstanceTermination",
            json!({"deploymentId": in_place_id}),
        )
        .await;
    assert_eq!(
        code, "DeploymentAlreadyCompletedException",
        "case a terminal deployment: expected DeploymentAlreadyCompletedException, got {code}"
    );
}
