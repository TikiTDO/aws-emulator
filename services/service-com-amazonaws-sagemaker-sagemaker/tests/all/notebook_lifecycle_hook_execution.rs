// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for a notebook instance's LIFECYCLE CONFIGURATION HOOKS: the
//! scripts the platform runs for an instance that names a configuration, and the CloudWatch
//! Logs streams their output is published to.
//!
//! SageMaker and CloudWatch Logs are mounted on ONE shared engine, the way the production
//! router mounts every service, and every call goes over the wire (`POST /` with
//! `X-Amz-Target`, `awsJson1_1`) through the same `MultiServiceRouter`, so what these tests
//! exercise is what a real SDK client gets: the hook's output is read back through
//! CloudWatch Logs' own `GetLogEvents`.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call:
//!
//! * the lines a hook's script writes are published, in the order it wrote them, to log
//!   group `/aws/sagemaker/NotebookInstances` in log stream
//!   `[notebook-instance-name]/[LifecycleConfigHook]` — the group and the stream the hook
//!   shape's own doc names — for each of the configuration's `OnCreate` and `OnStart`
//!   scripts, and the output is there by the time the instance reports `InService`;
//! * a notebook instance created without a lifecycle configuration, and one whose
//!   configuration's script writes nothing, publishes no stream at all.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every call is signed for.
const REGION: &str = "us-east-1";

/// The account id the emulator mints its ARNs under.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// Long enough to pass `state::NOTEBOOK_INSTANCE_IN_SERVICE_AFTER` (300 ms), so the clock
/// owes the instance its `Pending` -> `InService` transition.
const PAST_IN_SERVICE_DUE: Duration = Duration::from_millis(600);

/// The log group a notebook instance's lifecycle-configuration hooks write to, from the
/// hook shape's own doc (smithy_query query="doc" shape="NotebookInstanceLifecycleHook").
const LOG_GROUP: &str = "/aws/sagemaker/NotebookInstances";

/// A SigV4 credential scope naming `service`: it is what routes a request to that service.
/// The key id and signature are not credentials — this harness turns request authentication
/// off, so nothing is verified.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host;x-amz-target, Signature=not-verified"
    )
}

/// `script` as the base64 a hook's `Content` carries ("a base64-encoded string that
/// contains a shell script", smithy_query query="doc"
/// shape="NotebookInstanceLifecycleHook" member="Content").
fn encoded(script: &str) -> String {
    ::base64::Engine::encode(&::base64::engine::general_purpose::STANDARD, script)
}

/// SageMaker and CloudWatch Logs on ONE shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_sagemaker_sagemaker::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(
            service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// One `awsJson1_1` call to the service `target_prefix` names.
    async fn call(&self, target_prefix: &str, service: &str, target: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{target_prefix}.{target}"))
            .header("authorization", authorization(service))
            .body(bytes::Bytes::from(body.to_string()))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// One SageMaker call that must be served.
    async fn sagemaker_ok(&self, target: &str, body: &Value) -> Value {
        let (status, answered) = self.call("SageMaker", "sagemaker", target, body).await;
        assert_eq!(
            status, 200,
            "case {target} {body}: expected HTTP 200, got {status} with {answered}"
        );
        answered
    }

    /// One CloudWatch Logs call.
    async fn logs(&self, target: &str, body: &Value) -> (u16, Value) {
        self.call("Logs_20140328", "logs", target, body).await
    }

    /// The messages the log stream `stream` of [`LOG_GROUP`] holds, in the order
    /// `GetLogEvents` reports them, or `None` when CloudWatch Logs holds no such stream.
    async fn stream_messages(&self, stream: &str) -> Option<Vec<String>> {
        let (status, answered) = self
            .logs(
                "GetLogEvents",
                &json!({ "logGroupName": LOG_GROUP, "logStreamName": stream }),
            )
            .await;
        if status != 200 {
            return None;
        }
        Some(
            answered
                .get("events")
                .and_then(Value::as_array)
                .unwrap_or(&Vec::new())
                .iter()
                .map(|event| {
                    event
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect(),
        )
    }
}

/// Create a lifecycle configuration whose `OnCreate` and `OnStart` hooks carry the scripts
/// given, each as the base64 the member takes.
async fn given_lifecycle_config(
    emulator: &Emulator,
    name: &str,
    on_create: Option<&str>,
    on_start: Option<&str>,
) {
    let mut request = json!({ "NotebookInstanceLifecycleConfigName": name });
    if let Some(script) = on_create {
        request["OnCreate"] = json!([{ "Content": encoded(script) }]);
    }
    if let Some(script) = on_start {
        request["OnStart"] = json!([{ "Content": encoded(script) }]);
    }
    emulator
        .sagemaker_ok("CreateNotebookInstanceLifecycleConfig", &request)
        .await;
}

/// Create a notebook instance, naming `config` as its lifecycle configuration when one is
/// given, and wait for the clock to owe it `InService`.
async fn given_notebook_instance(emulator: &Emulator, name: &str, config: Option<&str>) {
    let mut request = json!({
        "NotebookInstanceName": name,
        "InstanceType": "ml.t3.medium",
        "RoleArn": format!("arn:aws:iam::{ACCOUNT}:role/service-role/AmazonSageMaker-ExecutionRole"),
    });
    if let Some(config) = config {
        request["LifecycleConfigName"] = json!(config);
    }
    emulator
        .sagemaker_ok("CreateNotebookInstance", &request)
        .await;
    tokio::time::sleep(PAST_IN_SERVICE_DUE).await;
    let described = emulator
        .sagemaker_ok("DescribeNotebookInstance", &json!({ "NotebookInstanceName": name }))
        .await;
    let status = described
        .get("NotebookInstanceStatus")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert_eq!(
        status, "InService",
        "case {name}: the instance must come up, got {described}"
    );
}

/// RULE: the lines a lifecycle hook's script writes are published, in order, to the hook's
/// own log stream of the notebook-instance log group.
#[tokio::test]
async fn a_lifecycle_hook_publishes_what_its_script_wrote() {
    /// One script a configuration can carry, and the lines running it writes.
    struct Row {
        case: &'static str,
        script: &'static str,
        published: Vec<&'static str>,
    }

    // Rows differ along what the script is made of: one line, several, quoting, a variable
    // the script sets and expands, a command this session does not provide beside an echo,
    // and an `exit` that ends the script early.
    let rows = vec![
        Row {
            case: "one echo",
            script: "#!/bin/bash\necho sim-marker-one\n",
            published: vec!["sim-marker-one"],
        },
        Row {
            case: "several echoes in order",
            script: "echo first\necho second\necho third\n",
            published: vec!["first", "second", "third"],
        },
        Row {
            case: "quoted words and several arguments",
            script: "echo \"hello there\" friend\necho 'single quoted'\n",
            published: vec!["hello there friend", "single quoted"],
        },
        Row {
            case: "a variable the script sets and expands",
            script: "MARKER=sim-marker-two\nexport SUFFIX=done\necho $MARKER\necho ${MARKER}-${SUFFIX}\n",
            published: vec!["sim-marker-two", "sim-marker-two-done"],
        },
        Row {
            case: "a command this environment does not provide beside an echo",
            script: "yum install -y nothing\necho after-the-unknown-command\n",
            published: vec!["after-the-unknown-command"],
        },
        Row {
            case: "an exit that ends the script",
            script: "echo before-exit\nexit 0\necho after-exit\n",
            published: vec!["before-exit"],
        },
        Row {
            case: "comments and blank lines carry no output",
            script: "# a comment\n\n   \necho only-line\n",
            published: vec!["only-line"],
        },
    ];

    for row in rows {
        let emulator = Emulator::new();
        let config = "sim-lifecycle-config";
        // The same script is given to BOTH hooks, so each hook's own stream is checked
        // against the same expectation.
        given_lifecycle_config(&emulator, config, Some(row.script), Some(row.script)).await;
        given_notebook_instance(&emulator, "sim-notebook", Some(config)).await;

        for hook in ["LifecycleConfigOnCreate", "LifecycleConfigOnStart"] {
            let stream = format!("sim-notebook/{hook}");
            let messages = emulator
                .stream_messages(&stream)
                .await
                .unwrap_or_else(|| panic!("case {}: expected the stream {stream}", row.case));
            assert_eq!(
                messages,
                row.published
                    .iter()
                    .map(|line| (*line).to_string())
                    .collect::<Vec<String>>(),
                "case {} ({hook}): expected the lines the script wrote, got {messages:?}",
                row.case
            );
        }
    }
}

/// RULE: nothing is published for an instance with no lifecycle configuration, and nothing
/// for a configuration whose script writes no line — a stream opened for either would
/// report output where there was none.
#[tokio::test]
async fn nothing_is_published_where_no_script_wrote_anything() {
    /// One instance, and the configuration (if any) it names.
    struct Row {
        case: &'static str,
        on_create: Option<&'static str>,
    }

    // Rows differ along why there is no output: no configuration at all, a configuration
    // whose script writes nothing, and one whose only command this session does not provide.
    let rows = [
        Row {
            case: "no lifecycle configuration",
            on_create: None,
        },
        Row {
            case: "a script of comments alone",
            on_create: Some("#!/bin/bash\n# nothing to do\n"),
        },
        Row {
            case: "a script whose only command needs a host program",
            on_create: Some("pip install awscli\n"),
        },
    ];

    for row in rows {
        let emulator = Emulator::new();
        let config = match row.on_create {
            Some(script) => {
                given_lifecycle_config(&emulator, "sim-quiet-config", Some(script), None).await;
                Some("sim-quiet-config")
            }
            None => None,
        };
        given_notebook_instance(&emulator, "sim-quiet-notebook", config).await;

        let messages = emulator
            .stream_messages("sim-quiet-notebook/LifecycleConfigOnCreate")
            .await;
        assert!(
            messages.is_none_or(|held| held.is_empty()),
            "case {}: no hook output was written, so nothing is published",
            row.case
        );
    }
}
