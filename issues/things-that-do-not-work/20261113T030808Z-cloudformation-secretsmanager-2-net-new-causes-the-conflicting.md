Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__secretsmanager.deploy-secretsmanager-rotationschedule-conflicting-rules-rejected-and-rolled-back
Issue-Key: cloudformation__secretsmanager.deploy-secretsmanager-rotationschedule-conflicting-rules-rejected-and-rolled-back|run#9|AssertionError|assert reason == _REJECTED, (
Issue-Commit: 9de0dc732b013e7abf09d4b752737b31a8a239ee

# cloudformation__secretsmanager: 2 net-new causes — the conflicting-RotationRules refusal uses the wrong message text, and the CloudFormation resource-handler reason omits the service/status/request-id suffix

> _Note: this issue was identified against the code base at commit `9de0dc732b013e7abf09d4b752737b31a8a239ee`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RotateSecret

## Description
A CloudFormation stack containing an AWS::SecretsManager::RotationSchedule whose RotationRules set BOTH AutomaticallyAfterDays and ScheduleExpression is correctly refused and rolled back by the emulator, but the failure reason the stack event carries diverges from AWS in two independent ways. 2 net-new causes follow; both are observed on the one failing check listed under Traces.

Cause 1 of 2: the refusal message for conflicting RotationRules is not AWS's wording.
AWS: Smithy (secretsmanager) RotationRulesType.AutomaticallyAfterDays documents the rule — "In RotateSecret, you can set the rotation schedule in RotationRules with AutomaticallyAfterDays or ScheduleExpression, but not both." The recorded refusal is an HTTP 400 whose message is the fixed text "You cannot specify both rotation frequency and schedule expression together." — that sentence is entirely fixed text, it interpolates nothing from the request. Surfaced through the AWS::SecretsManager::RotationSchedule handler it reaches CloudFormation as HandlerErrorCode InvalidRequest.
Emulator: the same template is refused with HandlerErrorCode InvalidRequest but the message "You can specify either AutomaticallyAfterDays or ScheduleExpression in RotationRules, but not both." — different fixed text for the same error class.
Why: a client (or a CDK/CloudFormation consumer, or any test) that matches Secrets Manager's documented refusal text for a both-rules-set rotation schedule sees an unrecognised reason; the condition is reported, but not as the error AWS names, so error-text assertions and operator runbooks keyed to AWS's sentence fail.
Fix: the RotationRules validation behind RotateSecret in the secretsmanager service (the message it emits for AutomaticallyAfterDays + ScheduleExpression specified together).
Traces: cloudformation__secretsmanager.deploy-secretsmanager-rotationschedule-conflicting-rules-rejected-and-rolled-back.

Cause 2 of 2: the resource-handler ResourceStatusReason drops the AWS SDK error suffix.
AWS: recorded behavior only — when a registry resource handler surfaces a downstream service refusal, the CREATE_FAILED ResourceStatusReason wraps the SDK exception text, which carries the suffix "(Service: SecretsManager, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)" after the service's own sentence. The fixed parts are "(Service: SecretsManager, Status Code: 400, Request ID: " / ") (SDK Attempt Count: " / ")"; the request id and the attempt count are per-call values (the check masks them), and the service name and status code come from the refusing service and its HTTP status.
Emulator: the reason contains only the bare message, with no "(Service: ..., Status Code: ..., Request ID: ...) (SDK Attempt Count: ...)" segment at all, so the recorded reason and the emulator's reason differ in shape as well as in text.
Why: tooling that parses a CREATE_FAILED/UPDATE_FAILED reason to recover which service refused and with what HTTP status (a very common pattern in deployment pipelines and in CDK error reporting) gets nothing to parse — the downstream service identity and status code are lost from stack events.
Fix: the ResourceStatusReason formatting in the AWS::SecretsManager::RotationSchedule path of the CloudFormation resource-handler layer — the wrapper that renders a downstream service error into a handler reason.
Traces: cloudformation__secretsmanager.deploy-secretsmanager-rotationschedule-conflicting-rules-rejected-and-rolled-back.

Not covered by the open issue 20261109T223435Z-cloudformation-secretsmanager-4-net-new-causes-describesecret-still.md (DescribeSecret after stack delete, ResourcePolicy Ref, GetAtt on write-only SecretString) nor by 20261112T222923Z-cloudformation-iam-lambda-secretsmanager-2-net-new-causes-104be0.md (rate()-derived AutomaticallyAfterDays): neither concerns the conflicting-rules refusal text or the handler-reason envelope.

## Evidence
Check cloudformation__secretsmanager.deploy-secretsmanager-rotationschedule-conflicting-rules-rejected-and-rolled-back, [mismatch]:

AssertionError: expected the RotationSchedule failure reason ('You cannot specify both rotation frequency and schedule expression together. (Service: SecretsManager, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)', 'InvalidRequest'), got ('You can specify either AutomaticallyAfterDays or ScheduleExpression in RotationRules, but not both.', 'InvalidRequest')

Expected (AWS, request id / attempt count masked by the check):
  message = "You cannot specify both rotation frequency and schedule expression together. (Service: SecretsManager, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)"
  HandlerErrorCode = "InvalidRequest"
Actual (emulator):
  message = "You can specify either AutomaticallyAfterDays or ScheduleExpression in RotationRules, but not both."
  HandlerErrorCode = "InvalidRequest"

The HandlerErrorCode and the ROLLBACK_COMPLETE stack status both match; only the reason text diverges (wrong sentence, and no SDK error suffix).

Smithy (secretsmanager) RotationRulesType.AutomaticallyAfterDays: "In RotateSecret, you can set the rotation schedule in RotationRules with AutomaticallyAfterDays or ScheduleExpression, but not both."

## Steps to Reproduce
1. CreateStack (Capabilities=[CAPABILITY_IAM]) with a template containing an IAM role, a rotation Lambda, a Lambda permission for secretsmanager.amazonaws.com, an AWS::SecretsManager::Secret with GenerateSecretString: {}, and an AWS::SecretsManager::RotationSchedule with RotationRules {"AutomaticallyAfterDays": 30, "ScheduleExpression": "rate(10 days)"} and RotateImmediatelyOnUpdate: false.
2. Poll DescribeStacks until terminal — the stack reaches ROLLBACK_COMPLETE (correct).
3. Read DescribeStackEvents and take the single CREATE_FAILED row for logical id RotationSchedule.
4. Parse its ResourceStatusReason as 'Resource handler returned message: "<message>" (RequestToken: ..., HandlerErrorCode: <code>)'.
5. Observe <message> is "You can specify either AutomaticallyAfterDays or ScheduleExpression in RotationRules, but not both." instead of AWS's "You cannot specify both rotation frequency and schedule expression together. (Service: SecretsManager, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)".
