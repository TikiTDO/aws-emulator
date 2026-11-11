Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__logs.awslogs-exec-role-scoped-to-one-group-is-denied-on-another ecs__logs.awslogs-without-create-log-stream-delivers-nothing ecs__logs.log-workload-producers-awslogs-create-group-denied-without-create-log-group ecs__logs.log-workload-producers-failed-start-delivers-no-log-events-unlike-an-unlogged-run ecs__logs.ecs-readonly-mount-point-write-rejected-while-the-writer-sibling-still-writes ecs__logs.ecs-runtime-platform-arm64-task-reports-aarch64-to-its-own-process ecs__logs.log-workload-producers-awslogs-one-event-per-line-verbatim ecs__logs.log-workload-producers-awslogs-long-line-split-at-event-size-limit ecs__logs.ecs-task-awslogs-stream-name-and-stdout
Issue-Key: ecs__logs.awslogs-exec-role-scoped-to-one-group-is-denied-on-another|run#7|AssertionError|assert bad.get("stopCode") == "TaskFailedToStart", (
Issue-Key: ecs__logs.awslogs-without-create-log-stream-delivers-nothing|run#11|AssertionError|assert bad_task.get("stopCode") == "TaskFailedToStart", (
Issue-Key: ecs__logs.log-workload-producers-awslogs-create-group-denied-without-create-log-group|run#24|AssertionError|assert no.get("stopCode") == "TaskFailedToStart", f"denied task stopCode={no.get('stopCode')!r}"
Issue-Key: ecs__logs.log-workload-producers-failed-start-delivers-no-log-events-unlike-an-unlogged-run|run#20|AssertionError|assert bad.get("stopCode") == "TaskFailedToStart", f"bad-image stopCode={bad.get('stopCode')!r}"
Issue-Key: ecs__logs.ecs-readonly-mount-point-write-rejected-while-the-writer-sibling-still-writes|run#10|AssertionError|assert writer.get("exitCode") == 0, (
Issue-Key: ecs__logs.ecs-runtime-platform-arm64-task-reports-aarch64-to-its-own-process|run#16|AssertionError|assert arm_container.get("exitCode") == 0 and x86_container.get("exitCode") == 0, (
Issue-Key: ecs__logs.log-workload-producers-awslogs-one-event-per-line-verbatim|run#17|AssertionError|assert sorted(msgs) == sorted(stdout_lines + [stderr_line]), (
Issue-Key: ecs__logs.log-workload-producers-awslogs-long-line-split-at-event-size-limit|run#23|AssertionError|assert i < len(msgs), f"ran out of events reassembling the {n}-byte line"
Issue-Key: ecs__logs.ecs-task-awslogs-stream-name-and-stdout|_poll_until#20|Timeout|time.sleep(interval_s)
Issue-Commit: 6e1ef7d8ebea1acbe69ee2691b3319db9dddeedf

# ecs__logs: 6 net-new causes — execution-role logs authorization, image-pull failure, container execution, awslogs line framing, awslogs size-limit splitting and awslogs delivery are all wrong

> _Note: this issue was identified against the code base at commit `6e1ef7d8ebea1acbe69ee2691b3319db9dddeedf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Six net-new root causes across nine checks. None of the nine is marked ALREADY FILED or OWNED. The open issues 20261105T143800Z-ecs-logs-4-net-new-causes-awslogs-delivery.md and 20261104T062603Z-ecs-logs-the-awslogs-driver-never-delivers-awsvpc.md describe this family, but they carry timeout-shaped replay keys for these checks; the checks now fail with concrete assertions on stopCode/exitCode/delivered events, so those issues do not cover them.

Cause 1 of 6: an ECS task whose execution role may not write to its awslogs destination is not failed at start.
AWS: awslogs is initialized before any container runs. If the execution role's logs grant does not cover the named group (a resource-scoped grant naming only another group, a grant missing logs:CreateLogStream, or a missing logs:CreateLogGroup for a group that does not exist), DescribeTasks reports lastStatus STOPPED, stopCode TaskFailedToStart and a stoppedReason with the fixed prefix `ResourceInitializationError: failed to validate logger args: create stream has been retried N times: failed to create Cloudwatch log stream:` followed by the Logs/IAM refusal naming the execution-role session arn, the denied action (logs:CreateLogStream / logs:CreateLogGroup) and the log-group resource arn; the destination group stays empty, and widening the grant lets the identical task start. The arns, action and retry count are request/state values; the prefix is fixed text. Smithy: ecs TaskStopCode enum = TaskFailedToStart | EssentialContainerExited | UserInitiated | ServiceSchedulerInitiated | SpotInterruption | TerminationNotice | InfrastructureHealth; the ResourceInitializationError wording is AWS-documented stopped-task behavior.
Emulator: every such task is admitted and stops with stopCode EssentialContainerExited / stoppedReason 'Essential container in task exited'. The resource scope, the missing CreateLogStream and the missing CreateLogGroup are all unobservable.
Why: a client cannot tell a task ECS refused to start from a task that ran and exited, so an execution role that cannot log is reported as a successful run with no logs, and automation keyed on stopCode TaskFailedToStart never fires.
Fix: the ECS task-start path must authorize the awslogs destination against the execution role (per log-group resource and per action) before launching containers and report TaskFailedToStart + the ResourceInitializationError stoppedReason.
Traces: ecs__logs.awslogs-exec-role-scoped-to-one-group-is-denied-on-another, ecs__logs.awslogs-without-create-log-stream-delivers-nothing, ecs__logs.log-workload-producers-awslogs-create-group-denied-without-create-log-group.

Cause 2 of 6: a task whose image cannot be pulled is not failed at start either.
AWS: a task referencing a nonexistent image or tag never starts: DescribeTasks reports stopCode TaskFailedToStart with a stoppedReason whose fixed prefix is `CannotPullContainerError:` followed by the registry's message naming the image (e.g. `... manifest for <image> not found: manifest unknown`), and the log group receives no events — which is what distinguishes it from a task that ran without logging.
Emulator: the bad-image task stops with stopCode 'EssentialContainerExited', so the failed-start case and the ran-but-unlogged case are identical.
Why: image-pull failure is the single most common ECS deployment failure. A client gets no distinguishable signal and no reason text to act on.
Fix: the ECS task-start path must attempt image resolution before start and report stopCode TaskFailedToStart with the CannotPullContainerError stoppedReason when it fails.
Traces: ecs__logs.log-workload-producers-failed-start-delivers-no-log-events-unlike-an-unlogged-run.

Cause 3 of 6: the container's command never executes — every container exits 127.
AWS: a container started from a published image runs its command; a shell script that writes to a mounted volume, or that reports `uname -m`, succeeds and exits with the script's own code, and the architecture a runtimePlatform arm64 task reports to its own process is aarch64 (x86_64 for X86_64).
Emulator: DescribeTasks reports the container STOPPED with exitCode 127 ('command not found') and stoppedReason 'Essential container in task exited' for both the read-write/read-only mount pair and for both halves of the arm64/x86_64 pair. Because even the read-write sibling and both architectures fail, the readOnly refusal and the architecture report prove nothing.
Why: nothing a client puts in a container actually runs, so every data-plane guarantee built on it (mount enforcement, architecture selection, exit codes) is unverifiable and silently wrong.
Fix: the ECS task-execution path must run the container's entrypoint/command for the task's runtimePlatform and report the process's real exit code.
Traces: ecs__logs.ecs-readonly-mount-point-write-rejected-while-the-writer-sibling-still-writes, ecs__logs.ecs-runtime-platform-arm64-task-reports-aarch64-to-its-own-process.

Cause 4 of 6: the awslogs driver does not frame one log event per output line verbatim.
AWS: the awslogs driver emits one CloudWatch Logs event per line of the container's output, byte-for-byte: leading/trailing whitespace and tabs are preserved, a lone CR before the newline is kept in the message, a final unterminated line is flushed when the stream closes, and stderr lines are delivered to the same stream as stdout.
Emulator: of seven produced lines only three arrive — delivered ['<token>-first', 'u: é ✓ 日本', '<token>-last'] against expected ['<token>-first', '  lead\tand trail  ', 'u: é ✓ 日本', '<token>-unterminated-then-closed', 'cr-before-newline\r', '<token>-last', '<token>-on-stderr']. The tokens are per-run; the requirement is that whitespace-only-decorated lines, the CR-terminated line, the unterminated final line and stderr all appear, each as its own event, unmodified.
Why: a client reading container logs loses lines entirely — including the last line a crashing process writes and everything on stderr, which is where errors go. Log-based alerting and post-mortems silently miss data.
Fix: the awslogs delivery path's line framing and flushing (preserve bytes, flush the unterminated tail, include stderr).
Traces: ecs__logs.log-workload-producers-awslogs-one-event-per-line-verbatim.

Cause 5 of 6: a line longer than the per-event size limit is not split into reassemblable events.
AWS: the awslogs driver splits an output line that exceeds the CloudWatch Logs event size limit into consecutive events in the same stream whose concatenation reproduces the original line exactly.
Emulator: the check runs out of events while reassembling a 40000-byte line (`AssertionError: ran out of events reassembling the 40000-byte line`) — the tail of the line is dropped rather than continued in further events. The 40000-byte length is the check's input, not a requirement; the requirement is that the split preserves the whole line.
Why: long structured lines (stack traces, JSON payloads) are truncated with no marker, so a client reading them back gets corrupt data and cannot tell it was truncated.
Fix: the awslogs delivery path's oversize-line splitting.
Traces: ecs__logs.log-workload-producers-awslogs-long-line-split-at-event-size-limit.

Cause 6 of 6: the awslogs stream for a task is never created / never carries the container's stdout.
AWS: a task using awslogs with awslogs-stream-prefix P creates the stream `P/<container-name>/<task-id>` in the configured group and delivers the container's stdout to it.
Emulator: the check is killed with `Error: Check timed out after 60.0s` while waiting for that stream and its content.
Why: the default way to read ECS container logs returns nothing, with no error anywhere.
Fix: the awslogs delivery path — create the prefixed stream and PutLogEvents the container's output.
Traces: ecs__logs.ecs-task-awslogs-stream-name-and-stdout.

## Evidence
Cause 1 — Expected stopCode 'TaskFailedToStart' with a 'ResourceInitializationError:' stoppedReason. Actual: `AssertionError: the task logging to the unpermitted group stopped with stopCode 'EssentialContainerExited' (reason 'Essential container in task exited'), expected 'TaskFailedToStart'`; `AssertionError: task whose execution role cannot create a log stream was not failed at start: stopCode=EssentialContainerExited reason=Essential container in task exited`; `AssertionError: denied task stopCode='EssentialContainerExited'`.
Cause 2 — Expected stopCode 'TaskFailedToStart' with a 'CannotPullContainerError:' stoppedReason. Actual: `AssertionError: bad-image stopCode='EssentialContainerExited'`.
Cause 3 — Expected the container command to run and exit with its own code. Actual: `AssertionError: the read-write sibling failed, so the read-only refusal proves nothing: {... 'name': 'writer', 'lastStatus': 'STOPPED', 'exitCode': 127, ...} stoppedReason='Essential container in task exited'` and `AssertionError: one of the two architectures failed to run public.ecr.aws/docker/library/busybox:1.36: arm={... 'exitCode': 127 ...} x86={... 'exitCode': 127 ...}` (both halves exit 127).
Cause 4 — Expected seven events, one per produced line, verbatim. Actual: `AssertionError: delivered events ['frame-<tok>-first', 'u: é ✓ 日本', 'frame-<tok>-last'] != expected lines ['frame-<tok>-first', '  lead\tand trail  ', 'u: é ✓ 日本', 'frame-<tok>-unterminated-then-closed', 'cr-before-newline\r', 'frame-<tok>-last', 'frame-<tok>-on-stderr']`.
Cause 5 — Expected the split events to concatenate back to the whole line. Actual: `AssertionError: ran out of events reassembling the 40000-byte line`.
Cause 6 — Expected the '<prefix>/<container>/<task-id>' stream to exist and hold the container's stdout. Actual: `Error: Check timed out after 60.0s`.

## Steps to Reproduce
Cause 1: create two log groups; grant the execution role logs:CreateLogStream/logs:PutLogEvents on group A only; RunTask with awslogs→A, then the identical task with awslogs→B; also run variants with logs:CreateLogStream omitted and with logs:CreateLogGroup omitted for a nonexistent group. Poll DescribeTasks for stopCode/stoppedReason.
Cause 2: RunTask a task definition naming an image tag that was never pushed; poll DescribeTasks.
Cause 3: RunTask a task with one volume mounted read-write into one container and read-only into another, and separately a pair of tasks with runtimePlatform cpuArchitecture ARM64 and X86_64 running `uname -m`; read exitCode and the containers' output.
Cause 4: RunTask a container that prints a whitespace-padded line, a CR-terminated line, an unterminated final line and a line on stderr; read the awslogs events.
Cause 5: RunTask a container that prints one 40000-byte line; reassemble the delivered events.
Cause 6: RunTask with awslogs-stream-prefix P and poll DescribeLogStreams/GetLogEvents for 'P/<container>/<task-id>'.
