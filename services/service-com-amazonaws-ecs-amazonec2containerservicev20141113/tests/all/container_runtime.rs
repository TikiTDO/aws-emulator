// SPDX-License-Identifier: CC0-1.0

//! The container DATA PLANE and the scheduler's STEADY STATE, driven end to end over HTTP.
//!
//! Three rules are under test here, each over a table of inputs:
//!
//! 1. A container's `command` is RUN: what the program writes on its standard output is
//!    delivered to the log group its `logConfiguration` names, one log event per line, in a
//!    stream named `<awslogs-stream-prefix>/<container name>/<task id>`; and the status the
//!    program ends with is the container's `exitCode`.
//! 2. `StopTask` sends the container the stop signal ("a stop signal value ... will default
//!    to SIGTERM"), so a container that handles it exits with the status its own handler
//!    chose and one that does not is ended by the signal (status 128 + 15).
//! 3. A service "runs and maintains your desired number of tasks": a stopped task of a
//!    service is REPLACED by a new task of the same task definition, while an identical
//!    standalone `RunTask` task — which no scheduler maintains a count for — is never
//!    relaunched. A service's task reports `group` `service:<service name>`; a standalone
//!    task does not.
//!
//! CloudWatch Logs is mounted BESIDE Amazon ECS on the SAME engine, so a container's output
//! travels the real in-process callback into the real log service and is read back with that
//! service's own `GetLogEvents`: nothing here reads the producer's own buffer.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    cluster_with_capacity, list, ok, present_count, string, SIGNED_REGION,
};

/// The log group every container below delivers to, created through CloudWatch Logs' own
/// API before any task is launched — the way a client sets one up.
const LOG_GROUP: &str = "/emulator/ecs/containers";

/// The `awslogs-stream-prefix` the task definitions below configure, and so the first
/// segment of every stream name they deliver to.
const STREAM_PREFIX: &str = "run";

/// The one container of every task definition below.
const CONTAINER: &str = "app";

/// The image the containers start from; irrelevant to these rules, so one value.
const IMAGE: &str = "public.ecr.aws/nginx/nginx:latest";

/// How often a poll re-reads the emulator.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// How long a poll keeps trying before it gives up: comfortably past the clock-owed
/// transitions a task takes to start and to stop (`state::TRANSITION_SPAN` is 500 ms), with
/// margin for a loaded test machine.
const POLL_BUDGET: Duration = Duration::from_secs(10);

/// How long a row waits before asserting that something did NOT happen: long enough that it
/// would have by now.
const QUIET_BUDGET: Duration = Duration::from_secs(3);

/// A router carrying Amazon ECS and CloudWatch Logs over ONE shared engine — the two
/// services a container's output travels through, mounted the way the production router
/// mounts them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(engine),
    );
    router
}

/// One CloudWatch Logs call, the way a client makes it: `POST /` with
/// `X-Amz-Target: Logs_20140328.<operation>` and a credential scoped to `logs`, which is
/// what the router picks the service from.
async fn logs(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("logs.{SIGNED_REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("Logs_20140328.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDECSTEST/20260101/{SIGNED_REGION}\
                 /logs/aws4_request, SignedHeaders=host;x-amz-target, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&payload)
            .unwrap_or_else(|_| json!({ "__unparsed": String::from_utf8_lossy(&payload) }))
    };
    (status, parsed)
}

/// Create the log group every container of a suite delivers to.
async fn create_log_group(router: &MultiServiceRouter, case: &str) {
    let (status, body) = logs(
        router,
        "CreateLogGroup",
        json!({ "logGroupName": LOG_GROUP }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateLogGroup to answer 200, got {status} with {body}"
    );
}

/// The messages CloudWatch Logs holds in `stream`, oldest first — the events the container's
/// own output became. An unknown stream reads as no messages, which is what a stream nothing
/// was delivered to holds.
async fn delivered(router: &MultiServiceRouter, stream: &str) -> Vec<String> {
    let (status, body) = logs(
        router,
        "GetLogEvents",
        json!({
            "logGroupName": LOG_GROUP,
            "logStreamName": stream,
            "startFromHead": true,
        }),
    )
    .await;
    if status != 200 {
        return Vec::new();
    }
    body["events"]
        .as_array()
        .map(|events| {
            events
                .iter()
                .filter_map(|event| event["message"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The stream the `awslogs` driver delivers one container of one task to:
/// `<prefix>/<container name>/<task id>`, the naming the driver's own
/// `awslogs-stream-prefix` documentation states. Spelled out here rather than imported, so
/// a wrong naming in the implementation cannot make a test pass.
fn stream_of(task_arn: &str) -> String {
    let task_id = task_arn.rsplit('/').next().unwrap_or_default();
    format!("{STREAM_PREFIX}/{CONTAINER}/{task_id}")
}

/// One container definition that RUNS `command`, delivering its output to [`LOG_GROUP`]
/// through the `awslogs` driver, with `environment` in its process's environment.
fn container(command: &[&str], environment: &[(&str, &str)]) -> Value {
    json!({
        "name": CONTAINER,
        "image": IMAGE,
        "memory": 128,
        "command": command,
        "environment": environment
            .iter()
            .map(|(name, value)| json!({ "name": name, "value": value }))
            .collect::<Vec<Value>>(),
        "logConfiguration": {
            "logDriver": "awslogs",
            "options": {
                "awslogs-group": LOG_GROUP,
                "awslogs-region": SIGNED_REGION,
                "awslogs-stream-prefix": STREAM_PREFIX,
            },
        },
    })
}

/// Register one revision of `family` whose single container runs `command`, and answer its
/// ARN.
async fn register(
    router: &MultiServiceRouter,
    family: &str,
    command: &[&str],
    environment: &[(&str, &str)],
    case: &str,
) -> String {
    let body = ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "containerDefinitions": [container(command, environment)],
        }),
        case,
    )
    .await;
    string(&body["taskDefinition"], "taskDefinitionArn", case)
}

/// Launch one standalone task of `task_definition` in `cluster` and answer its ARN.
async fn run_task(
    router: &MultiServiceRouter,
    cluster: &str,
    task_definition: &str,
    case: &str,
) -> String {
    let body = ok(
        router,
        "RunTask",
        json!({
            "cluster": cluster,
            "taskDefinition": task_definition,
            "launchType": "EC2",
        }),
        case,
    )
    .await;
    let tasks = list(&body, "tasks", case);
    assert_eq!(
        tasks.len(),
        1,
        "case {case:?}: expected RunTask to place one task, got {body}"
    );
    string(&tasks[0], "taskArn", case)
}

/// The task `task_arn` as `DescribeTasks` reports it now.
async fn describe(router: &MultiServiceRouter, cluster: &str, task_arn: &str, case: &str) -> Value {
    let body = ok(
        router,
        "DescribeTasks",
        json!({ "cluster": cluster, "tasks": [task_arn] }),
        case,
    )
    .await;
    let tasks = list(&body, "tasks", case);
    assert_eq!(
        tasks.len(),
        1,
        "case {case:?}: expected DescribeTasks to describe {task_arn}, got {body}"
    );
    tasks[0].clone()
}

/// The ARNs of every task of `cluster`, whatever its status.
async fn all_task_arns(router: &MultiServiceRouter, cluster: &str, case: &str) -> Vec<String> {
    let mut arns = Vec::new();
    for status in ["RUNNING", "PENDING", "STOPPED"] {
        let body = ok(
            router,
            "ListTasks",
            json!({ "cluster": cluster, "desiredStatus": status }),
            case,
        )
        .await;
        for arn in list(&body, "taskArns", case) {
            if let Some(arn) = arn.as_str() {
                arns.push(arn.to_string());
            }
        }
    }
    arns.sort();
    arns.dedup();
    arns
}

/// Poll `probe` until it answers `Some`, or the budget runs out — for the transitions the
/// clock owes and the deliveries a container's own run makes.
async fn poll_until<T, F, Fut>(probe: F, description: &str) -> T
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    let deadline = Instant::now() + POLL_BUDGET;
    loop {
        if let Some(answer) = probe().await {
            return answer;
        }
        assert!(
            Instant::now() < deadline,
            "{description}: not observed within {POLL_BUDGET:?}"
        );
        ::tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// RULE 1: what a container writes is delivered to its own `awslogs` stream, one event per
/// line, and the status its program ends with is the container's `exitCode`.
///
/// The rows span what the rule generalises over: how many lines the program writes, the
/// status it ends with, where the text comes from (literal, the container's environment),
/// whether a write ends its line at all, which command FORM produced it (a simple command,
/// an and-or list, a loop) and whether the launch named a shell program or an argument
/// vector the image exec's directly — including a vector naming a utility the container has
/// no program for, which is the status a shell reports for one.
#[tokio::test]
async fn a_containers_output_and_exit_status_are_its_programs_own() {
    let router = emulator();
    let cluster = "runtime";
    cluster_with_capacity(&router, cluster, "container runtime").await;
    create_log_group(&router, "container runtime").await;

    struct Case {
        name: &'static str,
        command: &'static [&'static str],
        environment: &'static [(&'static str, &'static str)],
        delivered: &'static [&'static str],
        exit_code: i64,
    }

    let cases = [
        Case {
            name: "one line per echo",
            command: &["sh", "-c", "echo one; echo two"],
            environment: &[],
            delivered: &["one", "two"],
            exit_code: 0,
        },
        Case {
            name: "the status exit names",
            command: &["sh", "-c", "echo leaving; exit 7"],
            environment: &[],
            delivered: &["leaving"],
            exit_code: 7,
        },
        Case {
            name: "a status past one byte wraps as a shell wraps it",
            command: &["sh", "-c", "exit 300"],
            environment: &[],
            delivered: &[],
            exit_code: 44,
        },
        Case {
            name: "the environment the container was given",
            command: &["sh", "-c", "echo $GREETING ${MISSING:-default}"],
            environment: &[("GREETING", "hello")],
            delivered: &["hello default"],
            exit_code: 0,
        },
        Case {
            // The two default-value forms differ on a variable that is SET but empty:
            // `${NAME-WORD}` takes the default only when the name is unset, `${NAME:-WORD}`
            // also when it is empty.
            name: "the two default-value forms on an empty-but-set variable",
            command: &[
                "sh",
                "-c",
                "echo \"${BLANK-unset-only}\" \"${BLANK:-unset-or-empty}\"",
            ],
            environment: &[("BLANK", "")],
            delivered: &[" unset-or-empty"],
            exit_code: 0,
        },
        Case {
            name: "a write that does not end its line",
            command: &["sh", "-c", "echo -n half; echo rest"],
            environment: &[],
            delivered: &["halfrest"],
            exit_code: 0,
        },
        Case {
            name: "an and-or list",
            command: &["sh", "-c", "false || echo fallback && echo both"],
            environment: &[],
            delivered: &["fallback", "both"],
            exit_code: 0,
        },
        Case {
            name: "a loop whose condition is false at once",
            command: &["sh", "-c", "echo start; while false; do echo never; done; exit 3"],
            environment: &[],
            delivered: &["start"],
            exit_code: 3,
        },
        Case {
            name: "an argument vector the image exec's directly",
            command: &["echo", "direct"],
            environment: &[],
            delivered: &["direct"],
            exit_code: 0,
        },
        Case {
            name: "a vector naming a utility the container has no program for",
            command: &["definitely-not-a-utility"],
            environment: &[],
            delivered: &[],
            exit_code: 127,
        },
        // The three rows below are the CONFINEMENT: this container has no filesystem, no
        // process to start and no host to reach, so each of these forms fails closed with
        // the status a shell reports for a command it could not run — nothing is read,
        // written or started, and nothing is delivered.
        Case {
            name: "a utility of the host's filesystem",
            command: &["sh", "-c", "cat /etc/passwd"],
            environment: &[],
            delivered: &[],
            exit_code: 127,
        },
        Case {
            name: "a redirection into a host path",
            command: &["sh", "-c", "echo secret > /tmp/emulator-escape"],
            environment: &[],
            delivered: &[],
            exit_code: 127,
        },
        Case {
            name: "a command substitution, which would have to run a command",
            command: &["sh", "-c", "echo $(echo inner)"],
            environment: &[],
            delivered: &[],
            exit_code: 127,
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let name = case.name;
        let family = format!("runtime-{index}");
        let definition = register(
            &router,
            &family,
            case.command,
            case.environment,
            name,
        )
        .await;
        let task_arn = run_task(&router, cluster, &definition, name).await;
        let stream = stream_of(&task_arn);

        // The container's program ends on its own, and the status it ended with is what the
        // container reports.
        let exit_code = poll_until(
            || async {
                let task = describe(&router, cluster, &task_arn, name).await;
                let containers = list(&task, "containers", name);
                present_count(&containers[0], "exitCode", name)
            },
            &format!("case {name:?}: the container's exitCode"),
        )
        .await;
        assert_eq!(
            exit_code, case.exit_code,
            "case {name:?}: expected exitCode {}, got {exit_code}",
            case.exit_code
        );

        if case.delivered.is_empty() {
            // Nothing was written, so nothing is delivered — asserted after the status
            // arrived, which is after everything the program wrote would have.
            let events = delivered(&router, &stream).await;
            assert!(
                events.is_empty(),
                "case {name:?}: expected no log events in {stream}, got {events:?}"
            );
            continue;
        }
        let expected: Vec<String> = case.delivered.iter().map(|line| line.to_string()).collect();
        let events = poll_until(
            || async {
                let events = delivered(&router, &stream).await;
                (events.len() >= expected.len()).then_some(events)
            },
            &format!("case {name:?}: {} line(s) in {stream}", expected.len()),
        )
        .await;
        assert_eq!(
            events, expected,
            "case {name:?}: expected {expected:?} in {stream}, got {events:?}"
        );
    }
}

/// RULE 2: `StopTask` sends the container the stop signal, so a container that HANDLES it
/// exits with the status its own handler chose and one that does not is ended by the signal.
///
/// The rows span what the rule generalises over: whether the program installed a handler at
/// all, which status the handler exits with, and whether it writes anything before it does.
#[tokio::test]
async fn a_stop_sends_the_container_the_signal_its_program_answers() {
    let router = emulator();
    let cluster = "signals";
    cluster_with_capacity(&router, cluster, "stop signal").await;
    create_log_group(&router, "stop signal").await;

    struct Case {
        name: &'static str,
        /// The program, which must stay running until the signal arrives.
        command: &'static [&'static str],
        /// What the handler writes before it exits, if anything.
        farewell: Option<&'static str>,
        /// The status the container is expected to report.
        exit_code: i64,
    }

    let cases = [
        Case {
            name: "a handler that writes and exits with its own status",
            command: &[
                "sh",
                "-c",
                "echo up; trap 'echo down; exit 42' TERM; while true; do sleep 1; done",
            ],
            farewell: Some("down"),
            exit_code: 42,
        },
        Case {
            name: "a handler spelled with the signal's SIG name",
            command: &[
                "sh",
                "-c",
                "echo up; trap 'exit 9' SIGTERM; sleep infinity",
            ],
            farewell: None,
            exit_code: 9,
        },
        Case {
            name: "no handler at all, so the signal ends the program",
            command: &["sh", "-c", "echo up; sleep 3600"],
            farewell: None,
            // "the exit status shall be 128 plus the signal number": SIGTERM is 15.
            exit_code: 143,
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let name = case.name;
        let family = format!("signals-{index}");
        let definition = register(&router, &family, case.command, &[], name).await;
        let task_arn = run_task(&router, cluster, &definition, name).await;
        let stream = stream_of(&task_arn);

        // Every program writes its boot line first, which is what says the container is
        // really running before anything is stopped.
        poll_until(
            || async {
                let events = delivered(&router, &stream).await;
                (events.first().map(String::as_str) == Some("up")).then_some(())
            },
            &format!("case {name:?}: the boot line in {stream}"),
        )
        .await;

        ok(
            &router,
            "StopTask",
            json!({ "cluster": cluster, "task": task_arn }),
            name,
        )
        .await;

        let exit_code = poll_until(
            || async {
                let task = describe(&router, cluster, &task_arn, name).await;
                let containers = list(&task, "containers", name);
                present_count(&containers[0], "exitCode", name)
            },
            &format!("case {name:?}: the stopped container's exitCode"),
        )
        .await;
        assert_eq!(
            exit_code, case.exit_code,
            "case {name:?}: expected exitCode {}, got {exit_code}",
            case.exit_code
        );

        let events = delivered(&router, &stream).await;
        match case.farewell {
            Some(farewell) => assert_eq!(
                events.last().map(String::as_str),
                Some(farewell),
                "case {name:?}: expected the handler's line {farewell:?} last in {stream}, \
                 got {events:?}"
            ),
            None => assert_eq!(
                events,
                vec!["up".to_string()],
                "case {name:?}: expected only the boot line in {stream}, got {events:?}"
            ),
        }
    }
}

/// The BOUND on a container's run: a program that never ends is given up on, and nothing is
/// claimed about the container it was running.
///
/// The run's step budget is this emulator's, not the service's, so a program written as an
/// endless loop cannot hold the emulator for as long as the process lives: it is ABANDONED
/// once the budget is spent, which reports no `exitCode` — no status was produced — and
/// leaves the emulator serving every other call.
///
/// The rows span what the bound has to hold for: a loop whose commands this container
/// PERFORMS, and loops whose condition and body are forms it DECLINES (so they perform no
/// command at all) — in both polarities, `until` on a declined form and `while` on a
/// negated one.
#[tokio::test]
async fn a_program_that_never_ends_is_bounded_and_claims_no_exit_status() {
    let router = emulator();
    let cluster = "bounded";
    cluster_with_capacity(&router, cluster, "endless program").await;
    create_log_group(&router, "endless program").await;

    let cases: [(&str, &str); 3] = [
        (
            "a loop of commands the container performs",
            "echo up; while true; do :; done",
        ),
        (
            "an until loop whose condition and body are forms it declines",
            "echo up; until case x in esac; do case x in esac; done",
        ),
        (
            "a while loop whose condition is a declined form negated",
            "echo up; while ! case x in esac; do case x in esac; done",
        ),
    ];

    for (index, (name, script)) in cases.iter().enumerate() {
        let family = format!("bounded-{index}");
        let definition = register(&router, &family, &["sh", "-c", script], &[], name).await;
        let task_arn = run_task(&router, cluster, &definition, name).await;
        let stream = stream_of(&task_arn);

        // The program really started: its first line is delivered before the loop.
        poll_until(
            || async {
                let events = delivered(&router, &stream).await;
                (events == vec!["up".to_string()]).then_some(())
            },
            &format!("case {name:?}: the boot line in {stream}"),
        )
        .await;

        // Long enough for the budget to be spent, and for an exit status to have been
        // recorded if one were ever going to be.
        ::tokio::time::sleep(QUIET_BUDGET).await;
        let task = describe(&router, cluster, &task_arn, name).await;
        let containers = list(&task, "containers", name);
        assert_eq!(
            present_count(&containers[0], "exitCode", name),
            None,
            "case {name:?}: a run that was given up on claims no exit status, got {task}"
        );
        // The emulator is still serving: the call above answered, and so does this one.
        let events = delivered(&router, &stream).await;
        assert_eq!(
            events,
            vec!["up".to_string()],
            "case {name:?}: the abandoned run delivered nothing after its boot line, got \
             {events:?}"
        );
    }
}

/// RULE 3: a service maintains its desired count, so a stopped task of a service is
/// REPLACED, while an identical standalone task is never relaunched — and the two are told
/// apart on the record by the `group` each reports.
///
/// Both halves run on the SAME task definition in the SAME cluster, so the only difference
/// between them is which of the two launched the task: the service's scheduler, or
/// `RunTask`. Each container writes its boot line and its farewell line to its OWN stream
/// and exits with its handler's status, which is how a client observes that the replacement
/// is a container that really ran.
#[tokio::test]
async fn a_stopped_service_task_is_replaced_while_a_standalone_task_is_not() {
    let case = "service replacement";
    let router = emulator();
    let cluster = "steady";
    cluster_with_capacity(&router, cluster, case).await;
    create_log_group(&router, case).await;

    let command = [
        "sh",
        "-c",
        "echo booted; trap 'echo farewell; exit 42' TERM; while true; do sleep 1; done",
    ];
    let definition = register(&router, "steady", &command, &[], case).await;

    let service_name = "steady-service";
    ok(
        &router,
        "CreateService",
        json!({
            "cluster": cluster,
            "serviceName": service_name,
            "taskDefinition": definition,
            "desiredCount": 1,
            "launchType": "EC2",
        }),
        case,
    )
    .await;

    // The scheduler's own task, once it is running.
    let service_task = poll_until(
        || async {
            let body = ok(
                &router,
                "ListTasks",
                json!({ "cluster": cluster, "serviceName": service_name }),
                case,
            )
            .await;
            let arns = list(&body, "taskArns", case);
            let arn = arns.first()?.as_str()?.to_string();
            let task = describe(&router, cluster, &arn, case).await;
            (string(&task, "lastStatus", case) == "RUNNING").then_some(arn)
        },
        "the service's first task reaching RUNNING",
    )
    .await;
    let service_group = string(&describe(&router, cluster, &service_task, case).await, "group", case);
    assert_eq!(
        service_group,
        format!("service:{service_name}"),
        "the task a service placed reports the service's own group, got {service_group:?}"
    );

    // The identical standalone task, which no scheduler maintains a count for.
    let standalone = run_task(&router, cluster, &definition, case).await;
    poll_until(
        || async {
            let task = describe(&router, cluster, &standalone, case).await;
            (string(&task, "lastStatus", case) == "RUNNING").then_some(())
        },
        "the standalone task reaching RUNNING",
    )
    .await;
    let standalone_group = string(&describe(&router, cluster, &standalone, case).await, "group", case);
    assert!(
        !standalone_group.starts_with("service:"),
        "a standalone task reports no service group, got {standalone_group:?}"
    );

    // Both containers are really running: each wrote its boot line into its own stream.
    for (which, task_arn) in [("service", &service_task), ("standalone", &standalone)] {
        let stream = stream_of(task_arn);
        poll_until(
            || async {
                let events = delivered(&router, &stream).await;
                (events == vec!["booted".to_string()]).then_some(())
            },
            &format!("the {which} container's boot line in {stream}"),
        )
        .await;
    }

    for task_arn in [&service_task, &standalone] {
        ok(
            &router,
            "StopTask",
            json!({ "cluster": cluster, "task": task_arn }),
            case,
        )
        .await;
    }

    // The service's task is REPLACED: a task that is not the stopped one reaches RUNNING on
    // the same task definition, under the same group.
    let replacement = poll_until(
        || async {
            let body = ok(
                &router,
                "ListTasks",
                json!({
                    "cluster": cluster,
                    "serviceName": service_name,
                    "desiredStatus": "RUNNING",
                }),
                case,
            )
            .await;
            for arn in list(&body, "taskArns", case) {
                let arn = arn.as_str()?.to_string();
                if arn == service_task {
                    continue;
                }
                let task = describe(&router, cluster, &arn, case).await;
                if string(&task, "lastStatus", case) == "RUNNING" {
                    return Some((arn, task));
                }
            }
            None
        },
        "a replacement task of the service reaching RUNNING",
    )
    .await;
    assert_eq!(
        string(&replacement.1, "taskDefinitionArn", case),
        definition,
        "the replacement runs the same task definition, got {}",
        replacement.1
    );
    assert_eq!(
        string(&replacement.1, "group", case),
        format!("service:{service_name}"),
        "the replacement is the service's own task, got {}",
        replacement.1
    );
    // It is a container that really ran: its own stream carries its own boot line.
    let replacement_stream = stream_of(&replacement.0);
    poll_until(
        || async {
            let events = delivered(&router, &replacement_stream).await;
            (events == vec!["booted".to_string()]).then_some(())
        },
        &format!("the replacement container's boot line in {replacement_stream}"),
    )
    .await;

    // Both stopped containers handled the signal: each wrote its farewell line and exited
    // with its handler's status.
    for (which, task_arn) in [("service", &service_task), ("standalone", &standalone)] {
        let exit_code = poll_until(
            || async {
                let task = describe(&router, cluster, task_arn, case).await;
                let containers = list(&task, "containers", case);
                present_count(&containers[0], "exitCode", case)
            },
            &format!("the {which} container's exitCode"),
        )
        .await;
        assert_eq!(
            exit_code, 42,
            "the {which} container exits with its handler's status, got {exit_code}"
        );
        let stream = stream_of(task_arn);
        let events = delivered(&router, &stream).await;
        assert_eq!(
            events,
            vec!["booted".to_string(), "farewell".to_string()],
            "the {which} container's stream carries its boot and farewell lines, got {events:?}"
        );
    }

    // The standalone task is NOT relaunched: after long enough for a replacement to have
    // appeared, the cluster holds exactly the tasks the service placed plus the one
    // `RunTask` placed.
    ::tokio::time::sleep(QUIET_BUDGET).await;
    let arns = all_task_arns(&router, cluster, case).await;
    let standalone_tasks: Vec<String> = {
        let mut standalone_tasks = Vec::new();
        for arn in &arns {
            let task = describe(&router, cluster, arn, case).await;
            if !string(&task, "group", case).starts_with("service:") {
                standalone_tasks.push(arn.clone());
            }
        }
        standalone_tasks
    };
    assert_eq!(
        standalone_tasks,
        vec![standalone.clone()],
        "no standalone task is relaunched, so the only one is the task RunTask placed; \
         the cluster holds {arns:?}"
    );
}
