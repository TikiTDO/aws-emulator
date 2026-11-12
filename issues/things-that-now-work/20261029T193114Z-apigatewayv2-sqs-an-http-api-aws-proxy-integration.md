Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__sqs.aws-proxy-subtype-sqs-sendmessage-delivers-to-the-queue
Issue-Key: apigatewayv2__sqs.aws-proxy-subtype-sqs-sendmessage-delivers-to-the-queue|_poll_http#20|Timeout|time.sleep(interval_s)
Issue-Commit: 6712e9b9ed86b1379f14acf29b1637f3b8f9994b

# apigatewayv2__sqs: an HTTP API AWS_PROXY integration with subtype SQS-SendMessage never delivers to the queue

> _Note: this issue was identified against the code base at commit `6712e9b9ed86b1379f14acf29b1637f3b8f9994b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendMessage

## Description
One cause.

Cause 1 of 1: an API Gateway V2 HTTP API route whose integration is IntegrationType=AWS_PROXY with IntegrationSubtype=SQS-SendMessage never puts the message on the destination queue, so the check waits for the effect until the 60s harness budget is exhausted.
AWS: recorded behavior only for the data path (the Smithy model describes only the control plane: apigatewayv2 CreateIntegration carries IntegrationType=AWS_PROXY and IntegrationSubtype, and the subtype's RequestParameters map QueueUrl/MessageBody onto sqs:SendMessage). In AWS, invoking the route performs SendMessage against the named queue under the integration's CredentialsArn and the message becomes receivable on that queue; the route answers 200 with the SendMessage result.
Emulator: the route is created and the invoke returns, but no message ever becomes visible on the queue — the check never observes the effect and is killed at 60s ("Error: Check timed out after 60.0s"). There is no AWS_PROXY subtype dispatcher turning an HTTP API invocation into the target service call.
Why: a client that fronts a queue with an HTTP API (the standard "no-Lambda" ingestion pattern) sees successful HTTP responses while nothing is ever enqueued, so every downstream consumer silently starves and the failure is invisible at the ingress.
Fix: the apigatewayv2 data-plane invoke path — the AWS_PROXY integration handler must resolve IntegrationSubtype, render RequestParameters and perform the target service call (here sqs:SendMessage) under CredentialsArn, then serialize that call's result as the route response.
Traces: apigatewayv2__sqs.aws-proxy-subtype-sqs-sendmessage-delivers-to-the-queue

## Evidence
Expected: after invoking the HTTP API route, ReceiveMessage on the integration's QueueUrl returns the message whose body the request mapped into MessageBody.
Actual: no message is ever received; the check is terminated by the harness: `Error: Check timed out after 60.0s`.

## Steps to Reproduce
1. CreateApi (HTTP, ProtocolType=HTTP) and a queue.
2. CreateIntegration with IntegrationType=AWS_PROXY, IntegrationSubtype=SQS-SendMessage, RequestParameters={QueueUrl, MessageBody} and a CredentialsArn allowed to sqs:SendMessage.
3. CreateRoute + CreateStage(AutoDeploy) and invoke the route with a body.
4. Poll ReceiveMessage on the queue — no message ever arrives.
