Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecr__ecs__logs.cli-log-group-retention-set-and-verified
Issue-Key: cloudwatch__ecr__ecs__logs.cli-log-group-retention-set-and-verified|_poll_until#29|Timeout|return last
Issue-Commit: dff6e291e5e6489707888b0049fa49b367909ca3

# A CLI-set log-group retention plus an ECS Fargate task's awslogs delivery never completes: check exhausts its budget

> _Note: this issue was identified against the code base at commit `dff6e291e5e6489707888b0049fa49b367909ca3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRetentionPolicy

## Description
Cause 1 of 1: the round trip "create a log group, set its retention with the CLI, run a Fargate task whose container writes a marker, read the marker back from that group" never reaches a graded state; the check is killed at its 60 s budget.

AWS: recorded behavior only. The recording shows PutRetentionPolicy on the log group being honored and DescribeLogGroups continuing to report the configured retentionInDays after ingestion, and the Fargate task's container stdout arriving as log events in that same group (awslogs driver), so a poll for the marker terminates.

Emulator: the check runs out of budget with no result ("Error: Check timed out after 60.0s"). Nothing in the sequence ever satisfies the poll — consistent with the task's container never producing log events into the retention-configured group (the task does not run to the point of delivering stdout), so the read-back of the marker never succeeds.

Why: a client that uses CloudWatch Logs as the only evidence a container ran — the normal ECS/Fargate observability contract — gets an empty log group forever and a hung wait. Retention assertions and log-based test gates cannot pass against the emulator.

Fix: the ECS task runtime's awslogs log-driver delivery path (the component that creates the log stream and puts the container's stdout events into the configured group), plus the logs service's retention bookkeeping so a PutRetentionPolicy set before ingestion survives it.

Traces: cloudwatch__ecr__ecs__logs.cli-log-group-retention-set-and-verified. The open issue 20261104T060239Z-cloudwatch-ecr-ecs-logs-tasks-never-settle-and.md covers other checks in this service but does not carry this check's replay key.

## Evidence
Expected (AWS): DescribeLogGroups reports the retentionInDays set via `aws logs put-retention-policy` both before and after the task ingests, and the task's container marker line is readable with GetLogEvents/FilterLogEvents from that group within the budget.
Actual (emulator): the check never reaches an assertion — 'Error: Check timed out after 60.0s'; no log events for the task appear in the group.

## Steps to Reproduce
1. CreateLogGroup '/sim2real/retention-<id>' and `aws logs put-retention-policy --retention-in-days 1` on it.
2. CreateCluster, RegisterTaskDefinition (FARGATE/awsvpc) with logConfiguration logDriver=awslogs pointing at that group, container echoing a unique marker.
3. RunTask and poll DescribeLogGroups for retentionInDays and FilterLogEvents for the marker.
4. AWS: both resolve; emulator: the marker never appears and the check is killed at 60 s.
