Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift__sqs.cfn-deploy-redshift-sqs-missing-permission-negative
Issue-Key: redshift__sqs.cfn-deploy-redshift-sqs-missing-permission-negative|run#10|AssertionError|assert invoke_resp.get("FunctionError") == "Unhandled", (
Issue-Commit: 2b63d043efdd2fa9855b978e8c81153e1c6b6b87

# redshift__sqs: the CFN-deployed under-permissioned relay function is not refused — its SQS send is served although the execution role has no grant (1 cause)

> _Note: this issue was identified against the code base at commit `2b63d043efdd2fa9855b978e8c81153e1c6b6b87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendMessage

## Description
Cause 1 of 1: no IAM authorization is performed for the relay function's SQS call, so the negative arm of the stack behaves exactly like the permitted one.
AWS: the relay Lambda deployed by the stack calls SQS with its execution role's credentials and the call is authorized by IAM. With the sqs:SendMessage Allow withheld (or scoped to another queue), the call is refused — HTTP 403, error code `AccessDenied`, message "User: <principal> is not authorized to perform: sqs:SendMessage on resource: <queue ARN> because no identity-based policy allows the sqs:SendMessage action". The fixed text is everything outside the brackets; <principal> is the function's assumed-role session ARN and <queue ARN> the queue named in the request, both from the request and IAM state. The exception propagates out of the handler, so Invoke answers with FunctionError / x-amz-function-error set rather than a clean 200. SQS's Smithy model does not enumerate an authorization error for SendMessage, so the requirement here is AWS's documented IAM implicit-deny behavior.
Emulator: the invoke is a clean success — HTTP 200 with x-amz-executed-version: $LATEST and no x-amz-function-error header, i.e. "expected the under-permissioned relay to fail, got {... 'StatusCode': 200, 'ExecutedVersion': '$LATEST', ...}" — and the message is relayed.
Why: the negative arm of this deployment proves that a mis-scoped role cannot publish Redshift notifications; here it publishes them, so a permission regression ships undetected and a stack that breaks in AWS passes in the emulator.
Fix: the SQS SendMessage handler's authorization check (consult the signing principal's identity policies before enqueuing), plus the plumbing that makes an in-process Lambda handler's SDK calls carry its execution role as caller identity. Open neighbours on this suite (20261104T045922Z-redshift-sqs-no-redshift-data-statement-status-change.md, 20261103T130707Z-redshift-sqs-eventbridge-dead-letters-a-delivery-its.md) concern Redshift Data statement events and EventBridge dead-lettering, not authorization, and neither carries this replay key.
Traces: redshift__sqs.cfn-deploy-redshift-sqs-missing-permission-negative.

## Evidence
Expected: the relay invoke fails — the handler's SendMessage is refused HTTP 403 `AccessDenied` with AWS's implicit-deny message naming the principal, sqs:SendMessage and the queue ARN, so the Invoke response carries FunctionError. Actual: "AssertionError: expected the under-permissioned relay to fail, got {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/json', 'x-amz-executed-version': '$LATEST', ...}}, 'StatusCode': 200, 'ExecutedVersion': '$LATEST', 'Payload': <StreamingBody>}" — no x-amz-function-error header and no refusal.

## Steps to Reproduce
1. Deploy the stack's negative arm: an SQS queue and a relay Lambda whose execution role does NOT grant sqs:SendMessage on that queue.
2. Invoke the relay so its handler sends a message.
3. The invoke answers HTTP 200 with no FunctionError and the message is enqueued, instead of the handler failing with AccessDenied.
4. Replay redshift__sqs.cfn-deploy-redshift-sqs-missing-permission-negative.
