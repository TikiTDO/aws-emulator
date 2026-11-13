Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__lambda__s3.lambda-role-wildcard-resource-s3-access-positive
Issue-Key: cloudformation__iam__lambda__s3.lambda-role-wildcard-resource-s3-access-positive|_invoke_until#8|Timeout|last = _invoke(ctx, name, payload)
Issue-Commit: 117dc8980bd60242192cfb0322b38a4da9ec7838

# cloudformation__iam__lambda__s3: a wildcard-resource s3:* Lambda role never reaches a verdict — Invoke/in-function S3 path hangs until the check's 60 s budget expires

> _Note: this issue was identified against the code base at commit `117dc8980bd60242192cfb0322b38a4da9ec7838`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One root cause behind 1 failing check.

Cause 1 of 1: a Lambda whose execution role carries a wildcard-resource `s3:*` identity policy produces no observable outcome at all — the scenario burns its entire budget without the handler's S3 write or read ever succeeding.

AWS: `Invoke` is `POST /2015-03-31/functions/{FunctionName}/invocations` and answers HTTP 200 with the handler's JSON in the `Payload` httpPayload member (Smithy: Lambda `Invoke`, output `StatusCode` (httpResponseCode), `Payload` (httpPayload, sensitive), `FunctionError` (X-Amz-Function-Error); its modelled refusals are `ResourceNotFoundException` 404, `InvalidRequestContentException` 400, `TooManyRequestsException` 429, `ServiceException` 500 — none of them is a silent stall). A synchronous invoke of a small python3.12 handler settles in well under a second. For the authorization half the Smithy models say nothing about IAM evaluation, so: recorded behavior only — against AWS an identity policy of `{"Effect":"Allow","Action":"s3:*","Resource":"*"}` on the function's role permits `s3:PutObject` and `s3:GetObject` on ANY bucket, so the handler returns `{"ok": true}` for the put and `{"ok": true, "body": <content>}` for the get, and a subsequent control-plane `GetObject` on the same key returns the bytes the handler wrote. The grant must be honoured generically, not for one special-cased bucket ARN: both buckets in the scenario are created independently and neither appears in the policy.

Emulator: the check ends with `Error: Check timed out after 60.0s`. No HTTP status, no error code and no payload is ever produced for the verdict-bearing invoke. The handler's own waits are longer than the harness budget (`_invoke_until` polls for 90 s, `_wait_active` for 90 s, `_create_function_with_retry` for 120 s), so the stall is the emulator never returning a success from the invoke-plus-in-function-S3 path, not the check giving up early. Hypothesis: this is the same defect family as the in-process Lambda runtime having no S3 service model (the sibling checks of this service report `UnknownServiceError`, `Unknown service: 's3'`), so `r.get("ok") is True` is never satisfied and the poll loop runs to the wall clock.

Why: a client cannot tell whether a broad `s3:*` grant is honoured at all. The positive control for wildcard-resource authorization has no outcome, so neither a working grant nor a refusal can be distinguished from an emulator hang — and the same stall makes the surrounding deploy flow look indefinitely pending rather than either allowed (200 + payload) or denied (403 `AccessDenied`).

Fix: the Lambda `Invoke` execution path (its in-process Python runtime's bundled AWS SDK service-model catalog / client factory, so `boto3.client('s3')` resolves) and the IAM evaluation it drives for S3 data-plane calls. Whatever the invoke waits on must settle and return the handler payload or a modelled refusal instead of never answering.

Related already-open issue: 20261106T151148Z-cloudformation-iam-lambda-s3-in-function-s3-calls.md describes the same family and even lists this check id, but the replay suite did NOT mark this check as owned by it — the recorded key there is `...lambda-role-wildcard-resource-s3-access-positive|_invoke#8|Timeout|resp = lam.invoke(...)` whereas this run fails with a bare `Check timed out after 60.0s` (no frame), so the key has shifted and the check is not gated by that issue. Treat this as the still-live cause; it is not a new defect class.

Traces: cloudformation__iam__lambda__s3.lambda-role-wildcard-resource-s3-access-positive (listed under `### cloudformation__iam__lambda__s3`).

## Evidence
Expected (AWS): the function invoked with `{"action":"put","bucket":<bucket-a>,"key":"a.txt","content":<content-a>}` answers HTTP 200 with payload `{"ok": true}`; a control-plane `GetObject(Bucket=<bucket-a>, Key="a.txt")` then returns exactly `<content-a>`. Invoked with `{"action":"get","bucket":<bucket-b>,"key":"b.txt"}` it answers HTTP 200 with `{"ok": true, "body": <content-b>}` for an object written out-of-band by the caller. Both buckets are honoured although the policy names only `"Resource": "*"`. Each invoke settles in under a second.

Actual (emulator): `[mismatch] cloudformation__iam__lambda__s3.lambda-role-wildcard-resource-s3-access-positive` -> `Error: Check timed out after 60.0s`. No status code, no error code and no payload is returned for the verdict-bearing invoke; the `_invoke_until` predicate `r.get("ok") is True` is never satisfied within its 90 s poll window, so the 60 s harness budget expires first. Sibling checks of the same service in this same run return `{'ok': False, 'error': 'UnknownServiceError', 'message': "Unknown service: 's3'. Valid service names are: sqs, events, eventbridge, dynamodb"}` from the in-process runtime, which is the message family this stall is consistent with. (The bucket/role/function names and the `wild-a-`/`wild-b-` payload contents are per-run values from the check, not part of the requirement; the requirement is 200 + `{"ok": true}` and the round-tripped body.)

## Steps to Reproduce
1. Create two S3 buckets independently (`CreateBucket`, plus `PutBucketTagging`); neither name appears in any policy below.
2. `iam:CreateRole` with a trust policy allowing `lambda.amazonaws.com` for `sts:AssumeRole`.
3. `iam:PutRolePolicy` on that role with `{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:*","Resource":"*"}]}`.
4. `lambda:CreateFunction` (Runtime `python3.12`, Handler `lambda_function.handler`, Timeout 30) with a handler that does `boto3.client('s3').put_object(...)` / `get_object(...)` and returns `{"ok": True}` / `{"ok": True, "body": ...}`, or `{"ok": False, "error": <code>}` on exception; wait for `State == "Active"`.
5. `lambda:Invoke` with `{"action":"put","bucket":<bucket-a>,"key":"a.txt","content":<text>}`. Expected: 200 and `{"ok": true}`; then `s3:GetObject` on `<bucket-a>/a.txt` returns `<text>`.
6. `s3:PutObject` `<bucket-b>/b.txt` out of band, then `lambda:Invoke` with `{"action":"get","bucket":<bucket-b>,"key":"b.txt"}`. Expected: 200 and `{"ok": true, "body": <text>}`.
Observed: no invoke ever yields `ok: true`; the scenario produces no verdict and the check aborts with `Check timed out after 60.0s`.
