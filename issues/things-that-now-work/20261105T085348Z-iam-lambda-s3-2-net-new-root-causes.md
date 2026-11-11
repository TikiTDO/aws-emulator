Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__lambda__s3.bucket-policy-deleted-role-principal-rejected iam__lambda__s3.e2e-csv-to-xlsx-pipeline-lifecycle
Issue-Key: iam__lambda__s3.bucket-policy-deleted-role-principal-rejected|_invoke_payload#10|AssertionError|assert "error" not in result, f"handler faulted instead of returning a result: {result!r}"
Issue-Key: iam__lambda__s3.e2e-csv-to-xlsx-pipeline-lifecycle|phase#9|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: 8f45cbde1fc786a42a670207326215689e25a831

# iam__lambda__s3: 2 net-new root causes — the Lambda python runtime ships no AWS SDK, and its `os` module is incomplete (no os.stat_result)

> _Note: this issue was identified against the code base at commit `8f45cbde1fc786a42a670207326215689e25a831`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Two root causes, both in the in-process python runtime that `lambda:Invoke` executes the deployment package in.

Cause 1 of 2: no AWS SDK on the function's import path.
AWS: the managed python3.x Lambda runtimes bundle boto3/botocore (AWS publishes the bundled version per runtime), so a handler that does `import boto3` and calls S3 under its execution role runs and returns its own result, or the service's own refusal, as its payload. Smithy (lambda Invoke): `StatusCode` is `@httpResponseCode` (200 for `RequestResponse`), `FunctionError` is bound to the `X-Amz-Function-Error` header and is present only when the handler itself faulted, `Payload` is the `@httpPayload` blob. AWS models no error for a missing SDK, because on AWS there is none.
Emulator: Invoke answers 200 with `X-Amz-Function-Error: Unhandled` and the payload `{"errorMessage":"Unable to import module '<handler-module>': No module named 'boto3'","errorType":"Runtime.ImportModuleError","requestId":"<per-invocation id>","stackTrace":[]}`. Fixed text: `Unable to import module '<module>': No module named 'boto3'`, `errorType` `Runtime.ImportModuleError`; the module name is taken from the function's `Handler` and the request id is per-invocation (not part of the requirement).
Why: the bucket-policy principal semantics the check exists to prove are asserted from inside the function. With no SDK the allowed and the refused halves are indistinguishable, so a bucket policy naming a deleted role's principal could be accepted or refused and the client would see the same fault either way.
Fix: the Invoke execution path's python runtime environment — boto3/botocore must be importable inside the runtime, with a credential/endpoint environment that resolves to the emulator under the function's execution role.
Traces: bucket-policy-deleted-role-principal-rejected.

Cause 2 of 2: the runtime's `os` module is a partial stand-in, not CPython's — `os.stat_result` does not exist, so importing a handler that (directly or through the standard library) touches it fails at import time.
AWS: the managed python3.x runtimes provide the complete CPython standard library, so a handler module importing stdlib machinery that references `os.stat_result` (zipfile, shutil, pathlib and anything layered on them — the spreadsheet writer this pipeline uses among them) imports cleanly, and Invoke returns the handler's result with no function error.
Emulator: Invoke answers with `FunctionError` and the payload `{"errorMessage":"Unable to import module '<handler-module>': module 'os' has no attribute 'stat_result'","errorType":"Runtime.ImportModuleError","requestId":"<per-invocation id>","stackTrace":[]}`. Fixed text: `Unable to import module '<module>': module 'os' has no attribute 'stat_result'`, `errorType` `Runtime.ImportModuleError`; the module name comes from the function's `Handler`. This is a SEPARATE gap from cause 1: it is reached by a handler that does not need the SDK, and fixing the SDK import would not fix it.
Why: a client cannot run any function that uses ordinary file handling from the standard library. The failure surfaces as the function's own import error, so an operator is sent to debug their package instead of the runtime, and the whole CSV→XLSX pipeline the check drives never produces output.
Fix: the in-process python runtime's standard-library surface — the `os` module it exposes must carry CPython's attributes (`os.stat_result` included) that stdlib modules rely on.
Traces: e2e-csv-to-xlsx-pipeline-lifecycle (phase 4/7, `solve`).

## Evidence
Cause 1 — expected: `invoke` returns 200 with no `FunctionError` and the handler's own result dict.
Actual: `AssertionError: handler faulted instead of returning a result: {'status_code': 200, 'error': 'Unhandled', 'payload': {'errorMessage': "Unable to import module 'lambda_function': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError', 'requestId': '...', 'stackTrace': []}}` (iam__lambda__s3.bucket-policy-deleted-role-principal-rejected).

Cause 2 — expected: the backfill invocation for the uploaded CSV returns 200 with the converted-object result.
Actual: `AssertionError: [phase 4/7 solve] the backfill invocation for raw/orders_2026_01.csv failed: Unhandled {"errorMessage":"Unable to import module 'lambda_function': module 'os' has no attribute 'stat_result'","errorType":"Runtime.ImportModuleError","requestId":"...","stackTrace":[]}` (iam__lambda__s3.e2e-csv-to-xlsx-pipeline-lifecycle).

## Steps to Reproduce
Cause 1: create a function whose handler module does `import boto3` and calls `s3:GetObject`, then `lambda:Invoke` it — the 200 response sets `X-Amz-Function-Error: Unhandled` and the payload is `Runtime.ImportModuleError` / `No module named 'boto3'`.
Cause 2: create a function whose handler module imports standard-library file machinery that references `os.stat_result` (e.g. a zip/spreadsheet writer) and invoke it — the payload is `Runtime.ImportModuleError` / `module 'os' has no attribute 'stat_result'`.
