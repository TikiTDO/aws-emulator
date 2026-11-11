Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda.function-sdk-cloudwatch-get-otel-enrichment-answers
Issue-Key: cloudwatch__lambda.function-sdk-cloudwatch-get-otel-enrichment-answers|_invoke_granted#5|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Commit: 4315fe29349a8ccf086f6caac897ebb4dc04700d

# A Lambda function's SDK call to cloudwatch GetOTelEnrichment is refused NotImplemented, although the same operation answers the external caller (1 cause)

> _Note: this issue was identified against the code base at commit `4315fe29349a8ccf086f6caac897ebb4dc04700d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetOTelEnrichment

## Description
REOPENS 20261025T020208Z-cloudwatch-getotelenrichment-stub.md: its fix left the call refused. The fix made the operation answer an external caller — in this same check the harness's own cw.get_otel_enrichment() succeeds before and after the invoke — but the identical call made by an SDK client inside a Lambda function is still refused NotImplemented, so GetOTelEnrichment is unreachable from function code.

Cause 1 of 1 — the in-function SDK call is dispatched under the wrong operation name and refused.
AWS: Smithy: cloudwatch GetOTelEnrichment takes an empty input and returns GetOTelEnrichmentOutput{Status (required, OTelEnrichmentStatus: Running|Stopped), IncludeFilters, ExcludeFilters (0..100 selectors), CreatedAt, UpdatedAt}, with no modelled errors; it is account-scoped and read-only, authorized by cloudwatch:GetOTelEnrichment. AWS serves the same 200 response to a caller inside a Lambda function as to one outside it: boto3's get_otel_enrichment() returns a dict whose Status is a string, one of Running or Stopped, matching what the account's own GetOTelEnrichment reads around the invoke.
Emulator: the function's boto3 client raises NotImplemented — error code "NotImplemented", message "operation GetOtelEnrichment is not implemented in this emulator" — surfacing as an Unhandled function error. The operation named in that message, "GetOtelEnrichment", is not the modelled operation name "GetOTelEnrichment": the in-runtime SDK shim derives the wire name from the snake_case method by title-casing each segment (get_otel_enrichment -> GetOtelEnrichment), and that name resolves to nothing, so the call is refused before reaching the implemented handler (services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/ops/get_o_tel_enrichment.rs, which does serve Status/filters/CreatedAt/UpdatedAt from state). The function's deployment package even ships a GetOTelEnrichment model slice referenced by AWS_DATA_PATH, which the shim also ignores.
Why: a client cannot call this operation from function code at all, and gets a non-AWS error code for it. Any operation whose name contains an acronym boundary that snake_case cannot round-trip (OTel, DB, ARN-style names) is unreachable the same way, so function code that works on AWS fails inside the emulator with an error AWS never returns, and the function's own failure mode (Unhandled) is attributed to the handler rather than to the SDK path.
Fix: the Lambda runtime's bundled SDK/client shim — the component that maps a client method name to the wire operation must resolve it from the service model (honouring AWS_DATA_PATH when the deployment package ships a model slice) instead of title-casing snake_case segments, so GetOTelEnrichment reaches the implemented cloudwatch handler and returns its modelled output; an unresolvable operation must not be reported with the non-AWS code NotImplemented.

Traces: cloudwatch__lambda.function-sdk-cloudwatch-get-otel-enrichment-answers.

## Evidence
Check cloudwatch__lambda.function-sdk-cloudwatch-get-otel-enrichment-answers [mismatch]:

AssertionError: the function failed: 'Unhandled' {'errorMessage': 'An error occurred (NotImplemented) when calling the GetOtelEnrichment operation: operation GetOtelEnrichment is not implemented in this emulator', 'errorType': 'NotImplemented', 'stackTrace': ['  File "index.py", line 5, in handler\n    out = boto3.client("cloudwatch").get_otel_enrichment()\n', '  File "client.py", line 210, in method\n    return self._call(operation_name, parameters)\n']}

Expected: the invoke returns {"status": <Running|Stopped>, "status_type": "str", "keys": [... "Status" ...]} — the same Status the external caller's GetOTelEnrichment reads on one side of the invoke.
Actual: the in-function call is refused with code NotImplemented naming operation "GetOtelEnrichment" (note the capitalization: the modelled name is GetOTelEnrichment), while the harness's external cw.get_otel_enrichment() calls immediately before and after the invoke both succeed. The requestId in the recorded payload is a per-run value.

## Steps to Reproduce
1. Create an execution role allowing cloudwatch:GetOTelEnrichment on *.
2. Create a python3.12 function whose handler is: out = boto3.client("cloudwatch").get_otel_enrichment(); return {"status": out.get("Status"), "keys": sorted(k for k in out if k != "ResponseMetadata")}. Ship a GetOTelEnrichment model slice in the package and set AWS_DATA_PATH=/var/task/models.
3. Call cloudwatch GetOTelEnrichment directly — it answers 200 with a Status.
4. Invoke the function: the emulator returns FunctionError Unhandled with errorType NotImplemented for operation "GetOtelEnrichment"; AWS returns the account's Status.
