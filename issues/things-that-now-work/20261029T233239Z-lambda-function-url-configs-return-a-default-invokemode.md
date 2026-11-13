Issue-Source: trace-sweep-divergence
Issue-Trace: lam-page-list-function-url-configs
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lam-page-list-function-url-configs lambda-function-url-config-lifecycle
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|CreateFunctionUrlConfig|2xx->2xx|$.CreationTime
Issue-Key: lambda|CreateFunctionUrlConfig|2xx->2xx|$.InvokeMode
Issue-Key: lambda|ListFunctionUrlConfigs|2xx->2xx|$.FunctionUrlConfigs[*].CreationTime
Issue-Key: lambda|ListFunctionUrlConfigs|2xx->2xx|$.FunctionUrlConfigs[*].LastModifiedTime
Issue-Key: lambda|ListFunctionUrlConfigs|2xx->2xx|$.NextMarker
Issue-Key: lambda|CreateFunctionUrlConfig|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: lambda|UpdateFunctionUrlConfig|2xx->2xx|$.InvokeMode
Issue-Commit: 6fcb2038be76f91f904ab31cfab5a8d60c9030f7

# Lambda function URL configs return a default InvokeMode, wrong timestamp form and a bogus NextMarker

> _Note: this issue was identified against the code base at commit `6fcb2038be76f91f904ab31cfab5a8d60c9030f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFunctionUrlConfig

## Description
Shared fault: the function-URL serializer fills in an InvokeMode the caller never set, renders its timestamps in the wrong ISO-8601 variant, and ListFunctionUrlConfigs emits a version qualifier as its pagination token. Six net-new clusters.

AWS: Smithy CreateFunctionUrlConfigResponse declares `CreationTime` (required) and an optional `InvokeMode` enum whose documentation names BUFFERED as the behavioural default. The recorded responses do NOT carry `InvokeMode` when the request omitted it — the member is absent, not defaulted. Lambda renders these ISO-8601 timestamps with a numeric UTC offset (the recorded FunctionConfiguration LastModified in this corpus is of the form `2026-08-07T16:27:08.713+0000`), not a `Z` suffix. ListFunctionUrlConfigs' `NextMarker` is an opaque pagination token.

Emulator: `InvokeMode: "BUFFERED"` is added to Create/Update responses that recorded no such member; CreationTime/LastModifiedTime are emitted as `...Z`, which does not match the recorded timestamp form; and ListFunctionUrlConfigs returns the literal `$LATEST` — a function version qualifier — in `NextMarker`.

Why: a client that treats the presence of InvokeMode as "explicitly configured" mis-reads an unset config; a client parsing the timestamp with AWS's documented offset format fails; and a client that pages by feeding NextMarker back gets `$LATEST`, which is not a marker, so pagination loops or errors.

Fix: the function-URL serializer (omit unset InvokeMode, render timestamps in the recorded offset form) and ListFunctionUrlConfigs' pagination (emit a real marker, or none for a complete page).

Cause 1 of 6: CreateFunctionUrlConfig, 201 — fixture lam-page-list-function-url-configs step task#5; `InvokeMode` added and `CreationTime` in the wrong form.
Cause 2 of 6: ListFunctionUrlConfigs, 200 — fixture lam-page-list-function-url-configs step task#7; entry CreationTime/LastModifiedTime in the wrong form and `NextMarker` returned as `$LATEST`.
Cause 3 of 6: ListFunctionUrlConfigs, 200 — fixture lam-page-list-function-url-configs step task#8; entry timestamps in the wrong form.
Cause 4 of 6: CreateFunctionUrlConfig, 400 — fixture lam-page-list-function-url-configs step task#9; the `x-amzn-errortype` header the emulator emits (`InvalidParameterValueException`) does not match the recorded header's shape.
Cause 5 of 6: CreateFunctionUrlConfig, 201 — fixture lambda-function-url-config-lifecycle step task#8; `InvokeMode` added where the recording has none.
Cause 6 of 6: UpdateFunctionUrlConfig, 200 — fixture lambda-function-url-config-lifecycle step task#11; `InvokeMode` added where the recording has none.

## Evidence
Cluster details: `$.InvokeMode: type_mismatch (expected None, got 'BUFFERED')` on three clusters; `$.CreationTime: value_mismatch (expected '<shaped-not-asserted>', got '2026-10-29T23:17:07.899Z')` and the same for LastModifiedTime; `$.NextMarker: value_mismatch (expected '<shaped-not-asserted>', got '$LATEST')`; task#9 `$headers.x-amzn-errortype: shape_mismatch`. Expected status == actual status in all six. Smithy: CreateFunctionUrlConfigResponse.InvokeMode is optional with BUFFERED documented as the default behaviour.

## Steps to Reproduce
Replay the corpus. Fixture lambda-function-url-config-lifecycle step task#8: CreateFunctionUrlConfig without InvokeMode and read the 201 body — it contains `InvokeMode: BUFFERED`. Fixture lam-page-list-function-url-configs step task#7: ListFunctionUrlConfigs and read NextMarker.
