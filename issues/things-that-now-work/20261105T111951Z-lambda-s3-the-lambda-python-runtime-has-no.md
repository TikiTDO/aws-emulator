Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3.async-invoke-returns-accepted-and-converges-to-the-same-object lambda__s3.bucket-policy-alone-grants-lambda-role-object-read lambda__s3.bucket-policy-deny-overrides-lambda-identity-allow lambda__s3.csv-to-xlsx-handler-escapes-xml-metacharacters lambda__s3.e2e-check-lambda-layers-s-code-lifecycle lambda__s3.function-sdk-put-outside-granted-prefix-denied-as-function-error lambda__s3.function-sdk-reads-what-the-caller-wrote lambda__s3.handler-guard-rejects-key-outside-the-raw-prefix lambda__s3.invoke-reports-handler-exception-through-function-error lambda__s3.output-key-is-the-basename-of-a-nested-source-key
Issue-Key: lambda__s3.async-invoke-returns-accepted-and-converges-to-the-same-object|run#11|KeyError|assert json.loads(sync_body)["written"] == [_out_key(_OUTPUT_PREFIX, sync_key)], (
Issue-Key: lambda__s3.bucket-policy-alone-grants-lambda-role-object-read|_invoke_payload#12|AssertionError|assert "error" not in result, f"handler faulted instead of returning a result: {result!r}"
Issue-Key: lambda__s3.bucket-policy-deny-overrides-lambda-identity-allow|_invoke_payload#9|AssertionError|assert "error" not in result, f"handler faulted instead of returning a result: {result!r}"
Issue-Key: lambda__s3.csv-to-xlsx-handler-escapes-xml-metacharacters|run#8|AssertionError|assert resp.get("FunctionError") is None, (
Issue-Key: lambda__s3.e2e-check-lambda-layers-s-code-lifecycle|phase#6|_PhaseError|raise _PhaseError(
Issue-Key: lambda__s3.function-sdk-put-outside-granted-prefix-denied-as-function-error|run#8|AssertionError|assert err is None, f"the permitted PutObject failed the invocation: {err!r} {out!r}"
Issue-Key: lambda__s3.function-sdk-reads-what-the-caller-wrote|run#12|AssertionError|assert err is None, f"the function's reads failed: {err!r} {out!r}"
Issue-Key: lambda__s3.handler-guard-rejects-key-outside-the-raw-prefix|run#9|AssertionError|assert resp.get("FunctionError") is None, (
Issue-Key: lambda__s3.invoke-reports-handler-exception-through-function-error|run#9|AssertionError|assert detail["errorType"] == "KeyError", (
Issue-Key: lambda__s3.output-key-is-the-basename-of-a-nested-source-key|run#10|AssertionError|assert resp.get("FunctionError") is None, (
Issue-Commit: a0ca0e6d405fa8f821a988e0d013d6063af395f8

# lambda__s3: the Lambda python runtime has no AWS SDK, so no handler can read or write S3

> _Note: this issue was identified against the code base at commit `a0ca0e6d405fa8f821a988e0d013d6063af395f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1 — the managed python runtime ships no boto3/botocore, so every handler in this group faults at init instead of calling S3.
AWS: the python3.x managed runtimes bundle boto3/botocore; a handler may `import boto3` and call s3:GetObject/PutObject/ListObjectsV2 under its execution role, returning its own result — or the service's AccessDenied code, surfaced as a handled payload, when an identity policy, a prefix condition or a bucket policy denies it; a raising handler is reported through FunctionError with its own errorType.
Emulator: Invoke answers HTTP 200 with FunctionError='Unhandled' and payload {"errorMessage":"Unable to import module '<module>': No module named 'boto3'","errorType":"Runtime.ImportModuleError"}, so the handler never runs and every assertion about its result, its guard logic or its denial is unobservable.
Why: the CSV->XLSX conversion pipeline, the prefix-scoping guard, the bucket-policy grant and deny controls and the handler-exception reporting all fail for one unrelated reason; a client gets an init error where AWS returns the handler's output. Open issues 20261103T031416Z-lambda-s3-5-net-new-causes-in-s3.md and 20261103T031416Z-lambda-s3-6-net-new-causes-in-s3.md own other checks in this group, not these.
Fix: the python runtime's bundled site-packages / module search path in the lambda runtime component.
Traces: all check ids in trace_ids.

## Evidence
The checks fail where they read the handler's result; the invoke response is HTTP 200 with FunctionError='Unhandled' and payload {"errorMessage":"Unable to import module '<module>': No module named 'boto3'","errorType":"Runtime.ImportModuleError"}; one check reports "invoking <fn> with {...} never satisfied the expected condition within 60s; last observed response was {...same payload...}". Expected: the handler's own result, or its caught AccessDenied code.

## Steps to Reproduce
CreateFunction (python3.12) whose handler does `import boto3` and calls s3.get_object/put_object, then Invoke it; add a bucket policy Allow/Deny or a prefix condition for the negative legs.
