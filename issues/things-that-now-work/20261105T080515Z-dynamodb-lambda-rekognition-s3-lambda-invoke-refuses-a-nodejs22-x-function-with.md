Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__rekognition__s3.swallowed-conditional-put-leaves-a-clean-invocation-and-a-stale-row
Issue-Key: lambda|Invoke|InvalidRuntimeException
Issue-Commit: 349fac5dd238950f9954cc41b162705b83d96a37

# lambda Invoke refuses a nodejs22.x function with 502 InvalidRuntimeException ("not supported by this emulator, which runs the Python runtimes") after CreateFunction accepted the runtime and the function reached Active

> _Note: this issue was identified against the code base at commit `349fac5dd238950f9954cc41b162705b83d96a37`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
REOPENS 20261029T233239Z-lambda-synchronous-invoke-data-plane-answers-501-notimplemented.md: its fix left the call refused. The Invoke data plane no longer answers 501 NotImplemented, but a function whose Runtime is a Node.js runtime is still refused — now with a 502 InvalidRuntimeException instead — so the function is still never executed.

Cause 1 of 1: Invoke refuses a managed, currently-supported Node.js runtime.

AWS: Smithy com.amazonaws.lambda#Runtime is an enum whose values include `nodejs22.x` (alongside nodejs18.x/20.x/24.x and the python3.x set), so CreateFunction accepts Runtime=nodejs22.x and the function becomes Active. Invoke (POST /2015-03-31/functions/{FunctionName}/invocations, modeled success code 200) then EXECUTES the deployment package for that runtime and answers 200: the HTTP payload is the handler's serialized return value (a Node handler whose body ends in a bare `return;` serializes to the JSON literal `null`), `X-Amz-Function-Error` is ABSENT when the handler did not raise, `X-Amz-Executed-Version` names the version run, and `X-Amz-Log-Result` carries the base64 log tail when LogType=Tail. Smithy models InvalidRuntimeException as errorKind=server, httpError 502, doc "The runtime or runtime version specified is not supported." — AWS raises it only for a runtime/version it genuinely does not support, never for a value of the Runtime enum that CreateFunction just accepted. The runtime a function declares is part of the contract Lambda honours; which language runtimes exist is modeled, not an implementation choice.

Emulator: CreateFunction with Runtime="nodejs22.x" is accepted and `function_active_v2` reports the function Active, but every Invoke of it is refused HTTP 502 with error code `InvalidRuntimeException` and the message `The runtime nodejs22.x is not supported by this emulator, which runs the Python runtimes.` The fixed text of that message is emulator-authored prose that AWS never emits: it names "this emulator" and announces an implementation restriction, and the only request-derived part is the runtime token (`nodejs22.x`). Because the modeled error is a 502 server error, botocore retries it and the call ends only after "reached max retries: 3", multiplying the latency of every refusal.

Why: a client that deploys any Node.js (or, by the same message, any non-Python) Lambda sees a function that reports itself Active and then cannot be run at all. Nothing in the create path warns it: the runtime is accepted, the waiter succeeds, and the failure appears only at invoke time, as a server-class 5xx that a well-behaved SDK retries rather than surfaces as a configuration problem. Every downstream behaviour that depends on the handler running — the handler's own DynamoDB writes, its Rekognition calls, the Invoke envelope (StatusCode 200, absent FunctionError, `null` payload) and the log tail / log-group lines the handler's console.log produces — is unobservable, so whole reference-architecture flows (S3 upload -> Lambda -> Rekognition -> DynamoDB) cannot be exercised even though each individual service answers.

Fix: the Invoke handler's runtime dispatch — it must execute the package for the Runtime the function was created with rather than refusing every runtime family it does not host, and it must not synthesize InvalidRuntimeException for a Runtime enum value CreateFunction accepted. Related open issues name the same missing capability from other directions but do not cover this refusal: 20261104T160000Z-cloudformation-iam-lambda-s3-six-causes-awaiting-the-lambda-provisioner-and-an-in-process-runtime.md and 20261105T022225Z-lambda-sesv2-lambda-invoke-refuses-every-real-invocation.md.

Traces: dynamodb__lambda__rekognition__s3.swallowed-conditional-put-leaves-a-clean-invocation-and-a-stale-row.

## Evidence
Failing check dynamodb__lambda__rekognition__s3.swallowed-conditional-put-leaves-a-clean-invocation-and-a-stale-row, verdict [error]:

  botocore.errorfactory.InvalidRuntimeException: An error occurred (InvalidRuntimeException) when calling the Invoke operation (reached max retries: 3): The runtime nodejs22.x is not supported by this emulator, which runs the Python runtimes.

Expected (AWS): the check's first invocation asserts `ok["StatusCode"] == 200 and "FunctionError" not in ok`, the second asserts `again["Payload"].read().decode().strip() == "null"` for the handler's bare `return;`, and the log assertions expect the handler's `console.log(err)` text (`ConditionalCheckFailedException`) in both the LogType=Tail `LogResult` and in `/aws/lambda/<fn>` via filter_log_events. None of these were reached.

Actual (emulator): HTTP 502, code `InvalidRuntimeException`, message `The runtime nodejs22.x is not supported by this emulator, which runs the Python runtimes.` Fixed text = everything except the runtime token; variable part = `nodejs22.x`, taken from the function's Runtime in the request. The preceding CreateFunction (Runtime="nodejs22.x", Handler="index.handler", ZipFile index.js) was ACCEPTED and `lam.get_waiter("function_active_v2")` completed, so the emulator advertises the function as runnable before refusing to run it.

Smithy: com.amazonaws.lambda#Runtime enum contains `nodejs22.x`; Invoke http {POST /2015-03-31/functions/{FunctionName}/invocations, code 200} with output headers X-Amz-Function-Error / X-Amz-Log-Result / X-Amz-Executed-Version; InvalidRuntimeException errorKind=server httpError=502 doc "The runtime or runtime version specified is not supported."

## Steps to Reproduce
1. CreateBucket, CreateTable (hash key image_name:S), and an IAM role trusting lambda.amazonaws.com with logs:*, rekognition:DetectLabels, dynamodb:PutItem, s3:GetObject.
2. CreateFunction with Runtime="nodejs22.x", Handler="index.handler", Code.ZipFile = a zip containing index.js, Timeout=60, Environment BUCKET_NAME/TABLE_NAME. Observe it is accepted.
3. Wait with the function_active_v2 waiter. Observe it reports Active.
4. Invoke(FunctionName=<fn>, Payload=<S3 notification event JSON>, LogType="Tail").
5. Observe HTTP 502 InvalidRuntimeException "The runtime nodejs22.x is not supported by this emulator, which runs the Python runtimes." (retried to max retries) instead of 200 with the handler's payload, no FunctionError, and a base64 LogResult.

Replay: dynamodb__lambda__rekognition__s3.swallowed-conditional-put-leaves-a-clean-invocation-and-a-stale-row
