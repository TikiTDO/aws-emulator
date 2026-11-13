Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sqs__stepfunctions.cli-detach-lambda-invoke-policy-breaks-sfn-task-negative lambda__sqs__stepfunctions.lambda-send-sqs-missing-permission-negative lambda__sqs__stepfunctions.lambda-sqs-send-invalid-queue-url-negative
Issue-Key: lambda__sqs__stepfunctions.cli-detach-lambda-invoke-policy-breaks-sfn-task-negative|run#51|AssertionError|assert cause.startswith("User: arn:aws:sts::"), f"expected an authorization failure, got: {cause}"
Issue-Key: lambda__sqs__stepfunctions.lambda-send-sqs-missing-permission-negative|run#11|AssertionError|assert payload.get("ok") is False, f"Expected failure, got ok=True in: {payload}"
Issue-Key: lambda__sqs__stepfunctions.lambda-sqs-send-invalid-queue-url-negative|run#12|AssertionError|assert error_code == expected_code, (
Issue-Commit: 2b63d043efdd2fa9855b978e8c81153e1c6b6b87

# lambda__sqs__stepfunctions: an AccessDenied names the role ARN instead of the assumed-role session ARN, an unpermitted in-function send succeeds, and the SQS error code is the shape name (3 causes)

> _Note: this issue was identified against the code base at commit `2b63d043efdd2fa9855b978e8c81153e1c6b6b87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendMessage

## Description
Three net-new root causes. The open issues on this suite (20261105T081644Z-lambda-sqs-stepfunctions-no-aws-sdk-in-the.md, 20261029T183117Z-lambda-sfn-sqs-nine-net-new-causes-sqs.md, 20261024T043150Z-sfn-5-of-the-6-interpreter-causes-remain-open.md) cover the runtime's missing SDK clients and the SFN interpreter; none carries these replay keys and none describes the principal rendering or the error-code substitution below.

Cause 1 of 3: the IAM implicit-deny message names the ROLE ARN, not the assumed-role SESSION ARN of the caller.
AWS: when a Step Functions execution assumes the state machine's role and that role may not invoke the function, the Task fails with error Lambda.AWSLambdaException and a cause whose message is AWS's implicit-deny sentence, and the principal in it is the SESSION identity of the assumed role: "User: arn:aws:sts::<account>:assumed-role/<role name>/<session name> is not authorized to perform: lambda:InvokeFunction on resource: <function ARN> because no identity-based policy allows the lambda:InvokeFunction action". The fixed text is everything outside the brackets, including the `arn:aws:sts::` / `assumed-role/` form; <account>, <role name>, <session name> and <function ARN> come from the request and from IAM/SFN state. The `sts` service and `assumed-role` resource type are what distinguishes an assumed-role caller from an IAM principal; AWS never renders this principal as `arn:aws:iam::...:role/...`.
Emulator: the cause reads "User: arn:aws:iam::<account>:role/<role name> is not authorized to perform: lambda:InvokeFunction on resource: <function ARN> because no identity-based policy allows the lambda:InvokeFunction action (Service: AWSLambda; Status Code: 400; Error Code: AccessDeniedException; Proxy: null)" — the right sentence with the wrong principal form: the role ARN in the `iam` namespace instead of the `sts` assumed-role session ARN.
Why: operators and automation parse this principal to identify the session that was denied (CloudTrail correlation, policy-condition debugging, assertions in deployment gates). A role ARN where AWS writes an assumed-role session ARN breaks that parsing and hides which session was acting.
Fix: the principal rendering in the authorization layer's access-denied message builder — when the caller is an assumed role, render its session ARN (arn:aws:sts::<account>:assumed-role/<role>/<session>).
Traces: lambda__sqs__stepfunctions.cli-detach-lambda-invoke-policy-breaks-sfn-task-negative.

Cause 2 of 3: an in-function sqs:SendMessage is served although the execution role has no grant.
AWS: the handler's call is signed with the execution role and authorized by IAM; without the Allow it is refused HTTP 403 `AccessDenied` with "User: <assumed-role session ARN> is not authorized to perform: sqs:SendMessage on resource: <queue ARN> because no identity-based policy allows the sqs:SendMessage action". SQS's Smithy model does not enumerate the authorization error, so this is AWS's documented IAM behavior.
Emulator: "Expected failure, got ok=True in: {'ok': True, 'error': None}" — the send succeeds and the handler reports no error.
Why: least-privilege roles are unverifiable and a message AWS would reject is actually enqueued, so downstream assertions rest on a write that cannot happen in production.
Fix: the SQS SendMessage handler's authorization check plus the execution-role caller-identity plumbing for in-process handler calls.
Traces: lambda__sqs__stepfunctions.lambda-send-sqs-missing-permission-negative.

Cause 3 of 3: a refused SQS call inside a handler reports the error SHAPE name instead of the wire error code.
AWS: SQS serializes QueueDoesNotExist with the query-compatible code `AWS.SimpleQueueService.NonExistentQueue` at HTTP 400 and the message "The specified queue does not exist." (Smithy: com.amazonaws.sqs#QueueDoesNotExist, errorKind client, httpError 400, awsQueryError {code: "AWS.SimpleQueueService.NonExistentQueue", httpResponseCode: 400}). An SDK therefore exposes Error.Code == "AWS.SimpleQueueService.NonExistentQueue"; `QueueDoesNotExist` is an internal shape identifier that never appears on the wire.
Emulator: the handler sees {'ok': False, 'error_code': 'QueueDoesNotExist'}.
Why: handler code and alarms branch on the code string; code written against AWS takes the wrong branch and error classification keyed on the documented code stops matching.
Fix: the error-deserialization path of the in-process Lambda runtime's AWS SDK client — surface the awsQueryError code, not the modeled shape name.
Traces: lambda__sqs__stepfunctions.lambda-sqs-send-invalid-queue-url-negative.

## Evidence
Cause 1 — expected the cause to begin "User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: lambda:InvokeFunction on resource: <function ARN> because no identity-based policy allows the lambda:InvokeFunction action". Actual: "User: arn:aws:iam::000000000000:role/sim2real-detach-srole-... is not authorized to perform: lambda:InvokeFunction on resource: arn:aws:lambda:us-west-2:000000000000:function:sim2real-detach-fn-... because no identity-based policy allows the lambda:InvokeFunction action (Service: AWSLambda; Status Code: 400; Error Code: AccessDeniedException; Proxy: null)" — account/role/function names are per-run values; the divergence is the `arn:aws:iam::...:role/...` principal form where AWS writes `arn:aws:sts::...:assumed-role/.../...`.

Cause 2 — expected: HTTP 403 AccessDenied for the un-granted SendMessage. Actual: "AssertionError: Expected failure, got ok=True in: {'ok': True, 'error': None}".

Cause 3 — expected error code "AWS.SimpleQueueService.NonExistentQueue" (independently confirmed against real SQS for the same nonexistent queue name). Actual: {'ok': False, 'error_code': 'QueueDoesNotExist'}.

## Steps to Reproduce
1. Cause 1: deploy a state machine with a Lambda Task, grant its role lambda:InvokeFunction, start an execution (succeeds), then detach-role-policy and start another. Read the execution's cause — the principal is the role ARN, not the assumed-role session ARN.
2. Cause 2: invoke a function whose role has no sqs:SendMessage for the target queue and whose handler sends to it; it returns ok=True.
3. Cause 3: have the handler send to a nonexistent queue URL and return the caught error code; it is 'QueueDoesNotExist'.
4. Replay the three check ids in trace_ids.
