Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sns__sqs.snstrig-lambda-async-retry-exhausted-onfailure-to-sqs
Issue-Key: lambda__sns__sqs.snstrig-lambda-async-retry-exhausted-onfailure-to-sqs|_publish_until_delivered#14|AssertionError|raise AssertionError(
Issue-Commit: cd6d73564e5f4cf53934bca1327f08de31f2ee5f

# lambda__sns__sqs: an SNS-triggered function whose async retries are exhausted never writes an OnFailure destination record to SQS

> _Note: this issue was identified against the code base at commit `cd6d73564e5f4cf53934bca1327f08de31f2ee5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: a function subscribed to an SNS topic that always throws is invoked asynchronously, exhausts its MaximumRetryAttempts, and the configured OnFailure destination never receives the invocation record. Failing check: lambda__sns__sqs.snstrig-lambda-async-retry-exhausted-onfailure-to-sqs.

AWS: SNS invokes a subscribed Lambda function through the same asynchronous invoke machinery a direct Invoke(InvocationType='Event') uses. Smithy (lambda 2015-03-31) PutFunctionEventInvokeConfig declares MaximumRetryAttempts and DestinationConfig.OnFailure.Destination; with MaximumRetryAttempts=1 the event is attempted once and retried once (with roughly a minute of backoff), and when the final attempt also fails Lambda sends the failure record to the OnFailure destination — for an SQS destination, one message whose body is the invocation record (requestContext with requestId, functionArn, condition 'RetriesExhausted' and approximateInvokeCount, plus requestPayload and responsePayload carrying the handler's error). Recorded behavior only for the record's field layout (the destination record is not a Smithy shape); the documented contract is https://docs.aws.amazon.com/lambda/latest/dg/invocation-async-error-handling.html.

Emulator: nothing is ever delivered to the OnFailure queue. The check publishes 14 fresh probes over 420 s (re-publishing to rule out the single add_permission propagation race) and reads the destination queue throughout; no message of any shape arrives, so the async-failure path emits no destination record at all.

Why: the only supported way to capture asynchronously-failed events is silently inert. A client wires an OnFailure destination precisely so a poison event is not lost, sees an empty queue, and concludes its events succeeded — failures vanish with no record, no retry evidence and nothing to replay.

Fix: the Lambda asynchronous-invocation path (the async event queue/retry driver) — after the attempts allowed by the function's MaximumRetryAttempts are exhausted, build the failure invocation record (condition 'RetriesExhausted', requestContext/requestPayload/responsePayload) and deliver it to the DestinationConfig.OnFailure destination, for events originating from an SNS subscription as well as from a direct Event invoke.

Note: open issues 20261107T014900Z-lambda-sns-sqs-async-destinations-and-dead-letter.md and 20261109T183935Z-cloudwatch-lambda-4-net-new-causes-async-failure.md describe neighbouring async-destination gaps; neither carries this check's replay key, and they are named here in case one fix in the async driver covers them.

Traces: the failing check id above.

## Evidence
Expected (AWS): within the retry window one message on the OnFailure SQS queue whose body is the invocation record for the SNS-triggered failing invoke — requestContext.condition 'RetriesExhausted', approximateInvokeCount 2 for MaximumRetryAttempts=1, the function ARN, and requestPayload containing the SNS Records envelope for the published message.
Actual (emulator): `AssertionError: no OnFailure destination record delivered within 420.0s of publishing 14 probe(s)` — the destination queue stays empty for the whole budget. The probe count and 420 s budget are per-run harness values; the absence of any destination record is the divergence.

## Steps to Reproduce
1. CreateFunction with a handler that always raises, plus an execution role allowed to sqs:SendMessage to the destination queue.
2. CreateQueue for the OnFailure destination.
3. PutFunctionEventInvokeConfig(FunctionName, MaximumRetryAttempts=1, DestinationConfig={'OnFailure': {'Destination': <queue arn>}}).
4. CreateTopic, Subscribe the function (protocol 'lambda') and AddPermission so SNS may invoke it.
5. Publish to the topic and ReceiveMessage on the destination queue: no record ever arrives.
