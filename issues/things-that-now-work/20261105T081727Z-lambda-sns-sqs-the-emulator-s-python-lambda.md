Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sns__sqs.cfn-deploy-iam-role-trust-lambda-invokes-sqs-positive lambda__sns__sqs.cfn-deploy-lambda-execution-role-missing-sqs-permission-negative lambda__sns__sqs.lambda-sdk-change-message-visibility-sqs lambda__sns__sqs.lambda-sdk-delete-message-from-sqs lambda__sns__sqs.lambda-sdk-list-subscriptions-by-topic lambda__sns__sqs.lambda-sdk-receive-message-from-sqs lambda__sns__sqs.lambda-sdk-receive-message-missing-permission-negative lambda__sns__sqs.lambda-sdk-receive-message-missing-permission-rejected lambda__sns__sqs.lambda-sdk-subscribe-missing-permission-negative lambda__sns__sqs.lambda-sdk-subscribe-missing-permission-rejected lambda__sns__sqs.lambda-sdk-subscribe-to-sns-topic lambda__sns__sqs.lambda-sns-sdk-missing-permission lambda__sns__sqs.lambda-sns-sdk-publish lambda__sns__sqs.lambda-sqs-sdk-missing-permission lambda__sns__sqs.lambda-sqs-sdk-send-message
Issue-Key: lambda__sns__sqs.cfn-deploy-iam-role-trust-lambda-invokes-sqs-positive|run#15|AssertionError|assert "FunctionError" not in resp, f"lambda invocation failed: {resp}"
Issue-Key: lambda__sns__sqs.cfn-deploy-lambda-execution-role-missing-sqs-permission-negative|_poll_until#10|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: lambda__sns__sqs.lambda-sdk-change-message-visibility-sqs|_invoke#12|AssertionError|assert "FunctionError" not in resp, f"unexpected FunctionError: {resp.get('FunctionError')}, body={body}"
Issue-Key: lambda__sns__sqs.lambda-sdk-delete-message-from-sqs|_invoke#7|AssertionError|assert "FunctionError" not in resp, f"unexpected FunctionError: {resp.get('FunctionError')}, body={body}"
Issue-Key: lambda__sns__sqs.lambda-sdk-list-subscriptions-by-topic|_invoke_json_until#16|AssertionError|assert resp.get("FunctionError") is None, resp.get("FunctionError")
Issue-Key: lambda__sns__sqs.lambda-sdk-receive-message-from-sqs|_invoke#7|AssertionError|assert "FunctionError" not in resp, f"unexpected FunctionError: {resp.get('FunctionError')}, body={body}"
Issue-Key: lambda__sns__sqs.lambda-sdk-receive-message-missing-permission-negative|_invoke_json_until#14|AssertionError|assert resp.get("FunctionError") is None, resp.get("FunctionError")
Issue-Key: lambda__sns__sqs.lambda-sdk-receive-message-missing-permission-rejected|_invoke#7|AssertionError|assert "FunctionError" not in resp, f"unexpected FunctionError: {resp.get('FunctionError')}, body={body}"
Issue-Key: lambda__sns__sqs.lambda-sdk-subscribe-missing-permission-negative|_invoke_json_until#14|AssertionError|assert resp.get("FunctionError") is None, resp.get("FunctionError")
Issue-Key: lambda__sns__sqs.lambda-sdk-subscribe-missing-permission-rejected|_invoke#6|AssertionError|assert "FunctionError" not in resp, f"unexpected FunctionError: {resp.get('FunctionError')}, body={body}"
Issue-Key: lambda__sns__sqs.lambda-sdk-subscribe-to-sns-topic|_invoke#9|AssertionError|assert "FunctionError" not in resp, f"unexpected FunctionError: {resp.get('FunctionError')}, body={body}"
Issue-Key: lambda__sns__sqs.lambda-sns-sdk-missing-permission|run#8|AssertionError|assert allowed_payload.get("ok") is True, (
Issue-Key: lambda__sns__sqs.lambda-sns-sdk-publish|_wait_for_messages#12|Timeout|resp = sqs.receive_message(
Issue-Key: lambda__sns__sqs.lambda-sqs-sdk-missing-permission|run#6|AssertionError|assert denied_result.get("error") is None, (
Issue-Key: lambda__sns__sqs.lambda-sqs-sdk-send-message|_wait_for_messages#5|AssertionError|raise AssertionError(
Issue-Commit: ce1e7fa07219b3a065b602a90626dda6f8aa6c34

# lambda__sns__sqs: the emulator's Python Lambda runtime ships no AWS SDK, so every handler that imports boto3 fails with Runtime.ImportModuleError and no in-function SNS/SQS call ever happens

> _Note: this issue was identified against the code base at commit `ce1e7fa07219b3a065b602a90626dda6f8aa6c34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
This issue carries 1 net-new root cause. It must be closed before submitting.

Cause 1 of 1: a RequestResponse Invoke of a python3.x function whose handler does `import boto3` never runs the handler body — the import fails, so the function can make no AWS API call at all.

AWS: the managed Python runtimes (python3.9 … python3.13) bundle the AWS SDK for Python in the runtime image, so `import boto3` resolves in a deployment package that contains nothing but the handler module, and the runtime injects the execution role's credentials (AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY / AWS_SESSION_TOKEN, AWS_REGION) so a client built in the handler is signed as that role. Smithy: Invoke returns 200 for a RequestResponse invocation whose handler returned normally, with NO `x-amz-function-error` header and the handler's return value as the payload; `FunctionError`/`x-amz-function-error: Unhandled` is documented for "the error that occurred while running the function" only. The model's own InvalidRuntimeException is a 502 server error whose documentation is "The runtime or runtime version specified is not supported" — nothing in the model makes an SDK import a function-level failure. Recorded behavior for the negative members of this cluster: an in-handler SDK call made with a role that lacks the action raises a botocore ClientError the handler catches and reports inside its own JSON payload, while the Invoke itself still answers 200 with no FunctionError; the positive members show the handler's SNS Publish / SQS SendMessage actually reaching the topic/queue.

Emulator: every such Invoke answers 200 with `x-amz-function-error: Unhandled`, `FunctionError: Unhandled`, and the payload document `{"errorMessage": "Unable to import module '<module>': No module named 'boto3'", "errorType": "Runtime.ImportModuleError", "requestId": "<uuid>", "stackTrace": []}`. In that document the fixed text is `Unable to import module '` … `': No module named 'boto3'`, `Runtime.ImportModuleError` and the empty `stackTrace`; the variable parts are the handler module name taken from the request's `Handler` (`index`, `lambda_function` in these checks) and the per-invocation requestId. Where the check waits on the downstream effect instead of the payload, the effect simply never arrives: `AssertionError: Expected 1 messages, got 0 within 30s` on the queue the handler was to send to, or the check exhausting its 60s budget. The run happens in the in-process interpreter built by crate::state::execution, whose guest import surface is an allow-list of native modules with no socket/ssl layer and no boto3/botocore package on the path, so no handler can reach any AWS front door.

Why: a client relying on AWS's behavior gets a function that cannot do the one thing most Lambda code does — call another AWS service. Positive flows (publish to SNS, send/receive/delete/change-visibility on SQS, subscribe, list subscriptions) silently produce nothing, and negative flows are worse than wrong: the handler never reaches the SDK call, so the AccessDenied the missing IAM permission should produce is replaced by an import crash, and a test asserting "denied" passes for the wrong reason or hangs. Any emulated workload built on "Lambda as glue" is unusable.

Fix: the Lambda execution engine's guest environment — services/service-com-amazonaws-lambda-awsgirapiservice/src/state/execution/python.rs (the guest module allow-list and import path) and its runtime bootstrap (src/state/execution/bootstrap.rs) must provide a boto3/botocore surface for the guest that dispatches to the emulator's own service front doors under the function's execution-role identity, as the managed runtimes do.

Traces: the 15 failing checks listed in trace_ids, all of this service heading. The identical cause under other service headings is filed separately as the events__lambda__sns__sqs and events__lambda__sns issues from this sweep.

## Evidence
Expected (AWS): Invoke 200, no FunctionError, handler payload, and the handler's SNS/SQS call observable downstream.
Actual (emulator), e.g. lambda__sns__sqs.lambda-sdk-delete-message-from-sqs:
  AssertionError: unexpected FunctionError: Unhandled, body={'errorMessage': "Unable to import module 'index': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError', 'requestId': '3ed089ca-919f-4095-b45d-972dc1577216', 'stackTrace': []}
Identically for lambda-sdk-change-message-visibility-sqs, lambda-sdk-receive-message-from-sqs, lambda-sdk-receive-message-missing-permission-rejected, lambda-sdk-subscribe-missing-permission-rejected, lambda-sdk-subscribe-to-sns-topic.
lambda__sns__sqs.lambda-sns-sdk-missing-permission:
  AssertionError: positive control failed: role with sns:Publish could not publish: {'status_code': 200, 'error': 'Unhandled', 'payload': {'errorMessage': "Unable to import module 'lambda_function': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError', ...}}
lambda__sns__sqs.lambda-sqs-sdk-missing-permission:
  AssertionError: Lambda crashed instead of catching the SDK error: {'status_code': 200, 'error': 'Unhandled', 'payload': {'errorMessage': "Unable to import module 'lambda_function': No module named 'boto3'", ...}}
lambda__sns__sqs.cfn-deploy-lambda-execution-role-missing-sqs-permission-negative:
  AssertionError: denied function invocation completes not met within 30.0s. Last result: {'errorMessage': "Unable to import module 'index': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError', ...}
lambda__sns__sqs.cfn-deploy-iam-role-trust-lambda-invokes-sqs-positive:
  AssertionError: lambda invocation failed: {... 'x-amz-function-error': 'Unhandled', 'StatusCode': 200, 'FunctionError': 'Unhandled', 'ExecutedVersion': '$LATEST' ...}
lambda__sns__sqs.lambda-sqs-sdk-send-message:
  AssertionError: Expected 1 messages, got 0 within 30s
lambda__sns__sqs.lambda-sdk-list-subscriptions-by-topic / lambda-sdk-receive-message-missing-permission-negative / lambda-sdk-subscribe-missing-permission-negative:
  AssertionError: Unhandled
lambda__sns__sqs.lambda-sns-sdk-publish: Check timed out after 60.0s (the publish the handler was to make never reaches the topic).

## Steps to Reproduce
Replay the checks in trace_ids. Each creates an IAM role, zips a single-module python3.12 handler that does `import boto3`, creates the function, and calls Invoke (RequestResponse) — directly or through a CloudFormation-deployed stack. Minimal repro: CreateFunction with Runtime=python3.12, Handler=index.handler and a package containing only `index.py` whose first line is `import boto3`; then Invoke it. AWS returns 200 with the handler's payload; the emulator returns 200 with FunctionError=Unhandled and errorType Runtime.ImportModuleError.
