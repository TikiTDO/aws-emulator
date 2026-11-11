Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ecs__logs.deep-ecs-1-guestdepth-firelens-fluentbit-routes-app-output ec2__ecs__logs.deep-ecs-1-guestdepth-pidmode-task
Issue-Key: ec2__ecs__logs.deep-ecs-1-guestdepth-firelens-fluentbit-routes-app-output|_poll_until#16|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Key: ec2__ecs__logs.deep-ecs-1-guestdepth-pidmode-task|run#14|AssertionError|assert _version_tuple(pv) >= (1, 4, 0), (
Issue-Commit: 6e1ef7d8ebea1acbe69ee2691b3319db9dddeedf

# ec2__ecs__logs: 2 net-new causes — a FireLens/fluentbit sidecar routes no application output to CloudWatch Logs, and DescribeTasks reports platformVersion 'LATEST' instead of the resolved Fargate platform version

> _Note: this issue was identified against the code base at commit `6e1ef7d8ebea1acbe69ee2691b3319db9dddeedf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTasks

## Description
Two net-new root causes. Neither check is marked ALREADY FILED nor OWNED; the open issue 20261104T062603Z-ec2-ecs-logs-runtask-accepts-a-configuredatlaunch-volume.md covers other checks of this family but neither of these replay keys.

Cause 1 of 2: a FireLens (awsfirelens / fluentbit) log router never delivers the application container's output.
AWS: a task whose application container uses logConfiguration.logDriver = awsfirelens with an aws-for-fluent-bit sidecar declared firelensConfiguration.type = fluentbit gets its stdout routed by the sidecar to the configured cloudwatch_logs destination: the log stream named by the firelens options (log_stream_prefix/log_stream_name) is created in the configured log group and carries the application's lines. Recorded behavior only for the delivery itself; Smithy models the plumbing (ecs FirelensConfiguration, LogConfiguration.logDriver enum including awsfirelens).
Emulator: the log stream is never created. The check fails with `TimeoutError: log stream <stream> in <log-group>: the run budget ended first; not graded` — the stream and log-group names are per-run values; the requirement is that FireLens-routed output reaches the configured group at all.
Why: a client that routes container logs through FireLens — the supported way to send ECS logs anywhere other than plain awslogs — gets no logs and no error. The task looks healthy, the sidecar looks attached, and the destination stays empty.
Fix: the ECS task-execution log-routing path must honour logDriver awsfirelens plus a firelensConfiguration sidecar and deliver the application container's stdout to the sidecar's configured destination (creating the log stream and putting events).
Traces: ec2__ecs__logs.deep-ecs-1-guestdepth-firelens-fluentbit-routes-app-output.

Cause 2 of 2: the Fargate platform version is echoed as the literal string 'LATEST' instead of the concrete version the task actually ran on.
AWS: RunTask on Fargate without an explicit platformVersion uses the LATEST platform version, and the task record returned by RunTask/DescribeTasks reports the RESOLVED concrete version (e.g. '1.4.0'), not the alias. That matters because task-definition features are gated on it: pidMode=task requires platform version >= 1.4.0. Smithy: ecs Task.platformVersion is documented "The platform version where your task runs on. A platform version is only specified for tasks that use the Fargate launch type. If you didn't specify one, the LATEST platform version is used."
Emulator: DescribeTasks returns platformVersion 'LATEST' verbatim, so the check's `_version_tuple(pv) >= (1, 4, 0)` cannot be evaluated and fails with "pidMode=task task ran on Fargate platform 'LATEST'; pidMode=task needs >= 1.4.0".
Why: a client that reads platformVersion to decide whether a platform-gated feature (pidMode, ephemeral storage sizing, the 1.4.0 ENI/secret behaviours) is available, or that pins/asserts a version in CI, gets an alias it cannot compare. There is no way to learn what platform the task actually ran on.
Fix: the ECS RunTask/StartTask task-record construction (and DescribeTasks serialization) must resolve the LATEST alias to the concrete Fargate platform version it runs tasks as, and report that in Task.platformVersion.
Traces: ec2__ecs__logs.deep-ecs-1-guestdepth-pidmode-task.

## Evidence
Cause 1 — Expected: the FireLens-configured log stream appears in the configured log group and holds the application container's lines. Actual: `TimeoutError: log stream fl-e12e079043cc in sim2real-xsvc-lg-e94360f48505: the run budget ended first; not graded` (names are per-run).
Cause 2 — Expected: Task.platformVersion is a concrete version string >= 1.4.0 (e.g. '1.4.0'). Actual: `AssertionError: pidMode=task task ran on Fargate platform 'LATEST'; pidMode=task needs >= 1.4.0` — DescribeTasks returned the literal alias 'LATEST'.

## Steps to Reproduce
Cause 1: register a task definition with an app container (logDriver awsfirelens, options naming a cloudwatch_logs region/log_group_name/log_stream_prefix) plus a log_router container with firelensConfiguration.type=fluentbit; RunTask on Fargate; poll logs DescribeLogStreams/GetLogEvents on the configured group.
Cause 2: register a Fargate task definition with pidMode=task, RunTask WITHOUT platformVersion, then DescribeTasks and read tasks[0].platformVersion.
