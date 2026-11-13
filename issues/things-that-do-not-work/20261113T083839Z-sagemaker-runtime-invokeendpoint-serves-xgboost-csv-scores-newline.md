Issue-Source: trace-sweep-divergence
Issue-Trace: sagemaker-train-host-and-invoke-xgboost-model
Issue-Step: task#12
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sagemaker-train-host-and-invoke-xgboost-model
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker-runtime|InvokeEndpoint|2xx->2xx|$
Issue-Key: sagemaker-runtime|InvokeEndpoint|2xx->2xx|$headers.content-type
Issue-Commit: fae38c34e99cb924099b47e9ab649e154c215f8c

# sagemaker-runtime InvokeEndpoint serves XGBoost CSV scores newline-separated and rounded, with a Content-Type lacking charset

> _Note: this issue was identified against the code base at commit `fae38c34e99cb924099b47e9ab649e154c215f8c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeEndpoint

## Description
ONE cause.

Cause 1 of 1: the CSV inference response body and its Content-Type header do not follow the SageMaker CSV serialization contract.
AWS: InvokeEndpoint returns the container's response as the httpPayload Body blob and relays the container's Content-Type in the response header (Smithy: InvokeEndpointOutput.Body -> BodyBlob @httpPayload, ContentType -> Header @httpHeader('Content-Type'); http 200 on /endpoints/{EndpointName}/invocations). For a CSV-scoring built-in XGBoost endpoint the recorded response is a SINGLE line of comma-separated predictions with one field per input record and the container's full float precision preserved (recording: '16.488086700439453,2.494762659072876' for a two-record request), served with Content-Type 'text/csv; charset=utf-8'. The requirement is the FORM: one line, ',' as the record separator, no per-record newline, no rounding of the container's float output, and a Content-Type that carries the charset parameter. The literal score values are per-model and are not part of the requirement.
Emulator: returns the two predictions newline-separated and rounded to one decimal ('16.5\n2.5' where the recording carries full-precision comma-separated values) and sets $headers.content-type to 'text/csv' with no '; charset=utf-8' parameter.
Why: a client that parses a CSV inference response splits on ',' for a single-row result, so it reads ONE field containing an embedded newline instead of N predictions — batch scoring silently collapses to a single mis-parsed value. The rounding destroys inference precision, so numeric assertions on scores and any downstream thresholding are wrong. A client that keys on the exact Content-Type (including charset) to pick its decoder falls through to a byte/other branch.
Fix: the InvokeEndpoint response path in the sagemaker-runtime crate — the CSV serializer for the hosted container's predictions (join records with ',' on one line, emit the container's float values unrounded) and the response-header construction that sets Content-Type.
Traces: sagemaker-train-host-and-invoke-xgboost-model#task#12.

Related open issue that does NOT cover this cause: '20261113T002132Z-sagemaker-3-net-new-causes-invokeendpoint-scores-a.md' concerns InvokeEndpoint accepting an unsupported ContentType instead of refusing with ModelError/424 — a request-validation cause, not this response serialization and Content-Type defect.

## Evidence
sagemaker-train-host-and-invoke-xgboost-model#task#12, InvokeEndpoint, status 200->200:
  $: value_mismatch — expected '16.488086700439453,2.494762659072876' (one line, comma-separated, full float precision), got '16.5\n2.5' (newline-separated, rounded to one decimal).
  $headers.content-type: value_mismatch — expected 'text/csv; charset=utf-8', got 'text/csv'.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Replay fixture sagemaker-train-host-and-invoke-xgboost-model through step task#12: train the XGBoost job, create the model and endpoint, then POST /endpoints/{EndpointName}/invocations with a two-record text/csv Body.
3. Inspect the response body separator and float precision, and the response Content-Type header: the body comes back newline-separated and rounded and the header omits '; charset=utf-8'.
