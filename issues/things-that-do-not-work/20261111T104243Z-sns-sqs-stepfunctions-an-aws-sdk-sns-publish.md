Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns__sqs__stepfunctions.aws-sdk-sns-publish-task-delivers-and-missing-topic-is-caught
Issue-Key: sns__sqs__stepfunctions.aws-sdk-sns-publish-task-delivers-and-missing-topic-is-caught|run#16|AssertionError|assert bad["status"] == "SUCCEEDED", (
Issue-Commit: 56af4a38801370870cefc18c3ef22e7084182017

# sns__sqs__stepfunctions: an aws-sdk:sns:publish task's missing-topic failure is named 'SNS.NotFoundException' (AWS: 'Sns.NotFoundException') and its Cause uses the legacy SDK v1 envelope, so a Catch on the AWS error name never matches and the execution FAILS

> _Note: this issue was identified against the code base at commit `56af4a38801370870cefc18c3ef22e7084182017`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
TWO net-new root causes, both grounded in the one failing check of this suite's `sns__sqs__stepfunctions` service that the replay report marks neither ALREADY FILED nor OWNED. A fixer must close BOTH before the check passes.

Related OPEN issues named for context only (they do not carry this check's replay key, so they do not cover it): 20261111T004529Z-sns-sqs-stepfunctions-2-net-new-causes-the.md and 20261111T062437Z-sns-sqs-stepfunctions-an-sns-publish-task-s.md.

Cause 1 of 2: the error NAME a failed AWS SDK service integration reports.
Check id: sns__sqs__stepfunctions.aws-sdk-sns-publish-task-delivers-and-missing-topic-is-caught
AWS: for the optimized/AWS-SDK integration `arn:aws:states:::aws-sdk:sns:publish`, Step Functions names a service-side failure `<ServiceName>.<ErrorShapeName>` using the SDK service name in PascalCase — for SNS that is `Sns`, so a Publish against a well-formed ARN naming no topic surfaces as the ASL error name `Sns.NotFoundException` (recorded behavior; the underlying refusal is SNS Publish's modelled NotFoundException, awsQueryError code `NotFound`, httpError 404). A state whose Catch lists ErrorEquals ["Sns.NotFoundException"] therefore handles it and the execution ends SUCCEEDED.
Emulator: the task fails with error name `SNS.NotFoundException` (service token upper-cased, not PascalCase), which matches no ErrorEquals entry, so the Catch is skipped and the execution ends FAILED.
Why: every Catch/Retry written against AWS's documented SDK-integration error names silently stops matching; a state machine that AWS drives down its handled branch dies on the emulator, and the failure is reported under an error name no AWS client ever sees.
Fix: the Step Functions task-execution error mapper for `aws-sdk:*` resources (the sfn service crate, service-com-amazonaws-sfn-awsstepfunctions) — derive the prefix from the target service's PascalCase SDK name, not its upper-cased token.

Cause 2 of 2: the Cause STRING carried with that failure.
Check id: sns__sqs__stepfunctions.aws-sdk-sns-publish-task-delivers-and-missing-topic-is-caught
AWS: the Cause is the modern AWS SDK's error envelope — fixed text "Topic does not exist (Service: Sns, Status Code: 404, Request ID: <id>) (SDK Attempt Count: 1)": comma-separated fields, the PascalCase service name `Sns`, no "Error Code"/"Proxy" fields, and a trailing "(SDK Attempt Count: N)". The variable parts are the SNS message for the refusal, the status code, the per-call request id and the attempt count; the surrounding text is fixed.
Emulator: the Cause is the legacy SDK v1 envelope "Topic does not exist (Service: AmazonSNS; Status Code: 404; Error Code: NotFound; Request ID: <id>; Proxy: null)" — semicolon-separated, service `AmazonSNS`, extra Error Code and Proxy fields, no SDK Attempt Count.
Why: the caught error object a Catch writes to its ResultPath is what handlers log, pattern-match and forward; a client parsing Cause for the service, status or attempt count reads fields AWS does not emit and misses the ones it does.
Fix: same mapper/serializer as Cause 1 — build the Cause with the modern SDK envelope for aws-sdk integration failures.

Traces: both causes are observed in the single failing check above; the first (positive) leg of the same check — publish to a real topic, MessageId returned and the message delivered to the subscribed queue — passed, so only the failure path diverges.

## Evidence
Failing check (suite service heading `### sns__sqs__stepfunctions`): sns__sqs__stepfunctions.aws-sdk-sns-publish-task-delivers-and-missing-topic-is-caught

AssertionError: "the Catch on 'Sns.NotFoundException' must handle the failure and end SUCCEEDED, got 'FAILED': error='SNS.NotFoundException' cause='Topic does not exist (Service: AmazonSNS; Status Code: 404; Error Code: NotFound; Request ID: <per-run id>; Proxy: null)'".

State machine under test: single Task, Resource `arn:aws:states:::aws-sdk:sns:publish`, Parameters {TopicArn.$: $.topic, Message.$: $.message}, Catch [{ErrorEquals: ["Sns.NotFoundException"], ResultPath: "$.err", Next: "Missing"}].

Expected: execution status SUCCEEDED, output.handled == "missing-topic", and the caught err = {Error: "Sns.NotFoundException", Cause: "Topic does not exist (Service: Sns, Status Code: 404, Request ID: <id>) (SDK Attempt Count: 1)"} (request id masked by the check).
Actual: execution status FAILED with error "SNS.NotFoundException" and the legacy-envelope cause above; the Missing state never ran.

## Steps to Reproduce
1. sns:CreateTopic; sqs:CreateQueue with a policy admitting the topic; sns:Subscribe(Protocol=sqs).
2. iam:CreateRole for states.amazonaws.com allowing sns:Publish on both <topic> and <topic>-absent.
3. states:CreateStateMachine with: StartAt "Publish"; "Publish" = Task, Resource "arn:aws:states:::aws-sdk:sns:publish", Parameters {"TopicArn.$":"$.topic","Message.$":"$.message"}, Catch [{"ErrorEquals":["Sns.NotFoundException"],"ResultPath":"$.err","Next":"Missing"}], End true; "Missing" = Pass emitting {"handled":"missing-topic"}.
4. states:StartExecution with {"topic": "<real topic arn>", "message": "<marker>"} — succeeds, MessageId returned, message delivered (passes today).
5. states:StartExecution with {"topic": "<topic arn>-absent", "message": "never"} — EXPECT status SUCCEEDED via the Catch, with err.Error "Sns.NotFoundException" and err.Cause "Topic does not exist (Service: Sns, Status Code: 404, Request ID: <id>) (SDK Attempt Count: 1)"; OBSERVE status FAILED, error "SNS.NotFoundException", cause "Topic does not exist (Service: AmazonSNS; Status Code: 404; Error Code: NotFound; Request ID: <id>; Proxy: null)".
Replay: sns__sqs__stepfunctions.aws-sdk-sns-publish-task-delivers-and-missing-topic-is-caught.
