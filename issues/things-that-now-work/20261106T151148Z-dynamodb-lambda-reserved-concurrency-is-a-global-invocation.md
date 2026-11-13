Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda.conc-limits-and-fanout-reserved-concurrency-admission-transitions-rejected dynamodb__lambda.ddb-leading-keys-condition-scopes-lambda-partition-access dynamodb__lambda.ddb-resource-policy-alone-grants-lambda-role-get-item dynamodb__lambda.function-sdk-put-to-unpermitted-table-denied-as-function-error
Issue-Key: dynamodb__lambda.conc-limits-and-fanout-reserved-concurrency-admission-transitions-rejected|_assert_measured#40|AssertionError|assert len(ok) == want_ok, (
Issue-Key: dynamodb__lambda.ddb-leading-keys-condition-scopes-lambda-partition-access|run#10|AssertionError|assert payload["tenant-b"].get("get_code") == "AccessDeniedException", (
Issue-Key: dynamodb__lambda.ddb-resource-policy-alone-grants-lambda-role-get-item|run#9|AssertionError|assert before["granted"].get("code") == "AccessDeniedException", (
Issue-Key: dynamodb__lambda.function-sdk-put-to-unpermitted-table-denied-as-function-error|run#14|AssertionError|assert err == "Unhandled", f"the unpermitted PutItem gave FunctionError {err!r}: {out!r}"
Issue-Commit: 5678b2d659e23eeff4b2f39316abc675f4291395

# dynamodb__lambda: reserved concurrency is a global invocation cap, and DynamoDB IAM conditions / resource policies are ignored for in-function calls (2 causes)

> _Note: this issue was identified against the code base at commit `5678b2d659e23eeff4b2f39316abc675f4291395`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutFunctionConcurrency

## Description
Two root causes behind 4 failing checks.

Cause 1 of 2 (check: conc-limits-and-fanout-reserved-concurrency-admission-transitions-rejected): reserved concurrency is not enforced per function; what refuses the extra invocations is a global emulator-wide invocation cap with its own message.
AWS: PutFunctionConcurrency(ReservedConcurrentExecutions=N) caps that function at exactly N simultaneous executions. With N=1 and 8 simultaneous invokes, exactly 1 is admitted and the other 7 are refused with HTTP 429 and error code `TooManyRequestsException`; the message AWS returns for a reserved-concurrency rejection is `Rate Exceeded.` and the response carries `Reason: ReservedFunctionConcurrentInvocationLimitExceeded`. The numbers N and the invoke count are request values; the code, status and reason are the requirement. Model: PutFunctionConcurrency and Invoke's TooManyRequestsException are modelled in services/service-com-amazonaws-lambda-awsgirapiservice.
Emulator: with reserved=1, 4 of 8 simultaneous invokes were admitted (indices 0-3) and the remaining 4 were refused with `TooManyRequestsException` carrying the message `Too many invocations are running in this emulator. Retry the request.` — an emulator-wide cap unrelated to the function's reserved concurrency, with a message AWS never sends and no `Reason` naming the reserved-concurrency limit.
Why: a client cannot size or test reserved concurrency: throttling appears at a threshold the configuration did not set, and a caller that inspects the refusal's Reason to distinguish a reserved-concurrency throttle from an account-level one gets nothing. Capacity isolation between functions is not real.
Fix: the Lambda invoke admission control — per-function reserved-concurrency accounting — and the refusal serializer (status 429, code TooManyRequestsException, message `Rate Exceeded.`, Reason `ReservedFunctionConcurrentInvocationLimitExceeded`).

Cause 2 of 2 (3 checks: ddb-leading-keys-condition-scopes-lambda-partition-access, ddb-resource-policy-alone-grants-lambda-role-get-item, function-sdk-put-to-unpermitted-table-denied-as-function-error): DynamoDB authorization for in-function calls ignores both IAM `Condition` blocks and the table resource policy.
AWS: (a) an identity policy granting dynamodb:GetItem with `Condition: {ForAllValues:StringEquals: {dynamodb:LeadingKeys: ["<tenant>"]}}` refuses a GetItem for a different partition key with HTTP 400 `AccessDeniedException` — the Condition is evaluated, not ignored. (b) A role with no dynamodb identity grant is refused even when the table's resource policy allows it: a resource policy alone does not authorize a same-account principal that has no identity-based permission. (c) A PutItem to a table the role does not allow is refused with AccessDeniedException and the handler surfaces it as a function error. Smithy: GetItem/PutItem model their client errors; the authorization refusal is the service-common AccessDeniedException.
Emulator: (a) the out-of-condition GetItem returns the item (`{'get_payload': '<value>', 'query_count': 1}`), so the Condition block was ignored; (b) both the granted and the control (no-identity-policy) role read the item; (c) the unpermitted PutItem succeeds with FunctionError None and the item is written.
Why: multi-tenant isolation built on dynamodb:LeadingKeys silently does not hold — one tenant's function reads another tenant's partition. And a resource policy is treated as sufficient authorization, which over-grants relative to AWS.
Fix: the DynamoDB data-plane authorization path — IAM condition-key evaluation (dynamodb:LeadingKeys), the identity-policy requirement for same-account principals, and the AccessDeniedException refusal itself.
Traces: the 4 failing checks in trace_ids, under `### dynamodb__lambda`. Open issues 20261105T111951Z-dynamodb-lambda-the-lambda-python-runtime-has-no.md and 20261029T180026Z-dynamodb-lambda-dynamodb-stream-esm-record-envelope-images-never-observable.md cover other checks of this service and do not carry these replay keys.

## Evidence
Cause 1 — expected: reserved=1 admits exactly 1 of 8 simultaneous invokes; the other 7 get HTTP 429 TooManyRequestsException, message `Rate Exceeded.`, Reason `ReservedFunctionConcurrentInvocationLimitExceeded`.
Actual: AssertionError: reserved=1: 4 of 8 simultaneous invokes were admitted ([0, 1, 2, 3]), expected exactly 1; the refusals carry ('TooManyRequestsException', 'Too many invocations are running in this emulator. Retry the request.').

Cause 2 — expected: AccessDeniedException for the out-of-condition GetItem, for the identity-policy-less role regardless of the table resource policy, and for the unpermitted PutItem (surfaced as a function error).
Actual:
- ddb-leading-keys-condition-scopes-lambda-partition-access -> AssertionError: GetItem outside the LeadingKeys condition must be denied, got {'get_payload': '...', 'query_count': 1} -- returning the item means the Condition block was ignored
- ddb-resource-policy-alone-grants-lambda-role-get-item -> AssertionError: a role with no dynamodb permission must be denied before the resource policy: {'granted': {...}, 'control': {...}} (both read the item)
- function-sdk-put-to-unpermitted-table-denied-as-function-error -> AssertionError: the unpermitted PutItem gave FunctionError None (the item was written)

## Steps to Reproduce
Cause 1: PutFunctionConcurrency(ReservedConcurrentExecutions=1) on a function whose handler sleeps, then fire 8 simultaneous Invokes. Expected 1 admitted / 7 x 429 `Rate Exceeded.`; observed 4 admitted / 4 x 'Too many invocations are running in this emulator. Retry the request.'.
Cause 2: (a) grant dynamodb:GetItem with a dynamodb:LeadingKeys condition for tenant-a and GetItem a tenant-b key from the handler; (b) put a table resource policy allowing a role that has no identity policy and GetItem from it; (c) PutItem to a table the role does not allow. Expected AccessDeniedException in all three; observed success.
