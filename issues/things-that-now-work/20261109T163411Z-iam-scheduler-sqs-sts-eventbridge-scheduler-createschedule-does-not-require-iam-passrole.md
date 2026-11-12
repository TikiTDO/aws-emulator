Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__scheduler__sqs__sts.create-schedule-caller-without-passrole-denied
Issue-Key: iam__scheduler__sqs__sts.create-schedule-caller-without-passrole-denied|run#15|AssertionError|assert error is not None, "expected CreateSchedule to be denied without iam:PassRole"
Issue-Commit: 37e3f599a06cc415cb3d05dd7e4df6099b0b715c

# EventBridge Scheduler CreateSchedule does not require iam:PassRole on the target role

> _Note: this issue was identified against the code base at commit `37e3f599a06cc415cb3d05dd7e4df6099b0b715c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchedule

## Description
Cause 1 of 1: CreateSchedule does not charge iam:PassRole against the Target.RoleArn, so a caller that may create schedules can hand Scheduler any role.

AWS: CreateSchedule passes Target.RoleArn to EventBridge Scheduler, so the caller must hold iam:PassRole for that role. A principal allowed scheduler:CreateSchedule but not iam:PassRole is refused with `AccessDeniedException` carrying AWS's standard no-policy wording for the passed role — `User: <principal arn> is not authorized to perform: iam:PassRole on resource: <role arn> because no identity-based policy allows the iam:PassRole action` — with principal and role ARN taken from the request. Smithy: scheduler#Target.RoleArn is a required member constrained to an IAM role ARN pattern and CreateSchedule declares the service's access-denied error; the PassRole requirement is a service fact — recorded behavior only, and the recording shows the call refused.

Emulator: CreateSchedule succeeds; no error is raised.

Why: iam:PassRole is the control that stops a low-privilege principal from escalating by making a service act as a more powerful role. Without it, anyone who may create a schedule can have Scheduler assume any role in the account and invoke targets with its permissions — a privilege-escalation path, and a guardrail the client tested as effective that is in fact absent.

Fix: the CreateSchedule (and UpdateSchedule) handler's authorization step must charge iam:PassRole for Target.RoleArn before creating the schedule.

Traces: iam__scheduler__sqs__sts.create-schedule-caller-without-passrole-denied.

## Evidence
Expected: CreateSchedule denied for a caller without iam:PassRole on Target.RoleArn.
Actual: AssertionError: expected CreateSchedule to be denied without iam:PassRole

## Steps to Reproduce
1. Create a target queue and a role Scheduler can assume to send to it.
2. Create a principal allowed scheduler:CreateSchedule but not iam:PassRole; AssumeRole.
3. Call CreateSchedule with Target.RoleArn set to the Scheduler role; observe success.
