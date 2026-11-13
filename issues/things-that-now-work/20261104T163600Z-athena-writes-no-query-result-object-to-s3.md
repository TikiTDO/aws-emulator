Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena.athena-workgroup-default-output-location-used-by-query athena.workgroup-default-output-location-used-for-query athena.workgroup-enforced-configuration-overrides-client-output-location athena.server-side-enforced-workgroup-config-overrides-client-request
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena|GetObject|NoSuchKey
Issue-Key: athena.workgroup-enforced-configuration-overrides-client-output-location|run#16|AssertionError|assert _object_count(s3, bucket, "wg-output/") > 0, (
Issue-Key: athena.server-side-enforced-workgroup-config-overrides-client-request|run#28|AssertionError|assert honored_count > 0, (
Issue-Commit: 1b83187822ca52f98e00143d7b0b9c4d0812f404

# Athena writes no query-result object to S3 and does not apply workgroup output-location resolution or EnforceWorkGroupConfiguration (3 causes)

> _Note: this issue was identified against the code base at commit `1b83187822ca52f98e00143d7b0b9c4d0812f404`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQueryExecution

## Description
A completed Athena query produces no bytes in S3. The emulator reports a terminal query state but never writes the `<QueryExecutionId>.csv` result object (nor its `.csv.metadata` sibling) under the resolved output prefix, and it does not implement the workgroup-vs-client output-location resolution rules that decide which prefix is used. 3 causes.

Cause 1 of 3: no result object is written under a workgroup's default output location.
AWS: when StartQueryExecution omits ResultConfiguration.OutputLocation and the workgroup's Configuration.ResultConfiguration.OutputLocation is set, Athena writes the results there; a GetObject on `<workgroup prefix>/<QueryExecutionId>.csv` succeeds after the execution reaches SUCCEEDED (the object key is derived from the QueryExecutionId, the prefix from the workgroup configuration).
Emulator: the S3 GetObject for the result object fails with NoSuchKey ('The specified key does not exist.') — no object was ever written under the workgroup prefix.
Why: every client that reads Athena results from S3 (rather than through GetQueryResults) gets nothing; downstream jobs, manifests and ETL steps that consume the result object see an empty location.
Fix: the athena crate's query-execution result writer — it must materialise the CSV result object (and its metadata sibling) in the S3 store at the resolved output location — and the output-location resolution that falls back to the workgroup's ResultConfiguration.
Traces: athena.athena-workgroup-default-output-location-used-by-query, athena.workgroup-default-output-location-used-for-query.

Cause 2 of 3: with EnforceWorkGroupConfiguration=true the workgroup prefix is not used.
AWS: when the workgroup sets EnforceWorkGroupConfiguration=true, a client-supplied ResultConfiguration.OutputLocation is ignored and the result object lands under the workgroup's OutputLocation; an object listing of the workgroup prefix is non-empty and the client prefix stays empty.
Emulator: the workgroup prefix contains no objects at all after the execution reaches a terminal state, so the override cannot even be observed.
Why: a tenant relying on EnforceWorkGroupConfiguration to pin result bytes into a governed bucket gets no bytes anywhere, and the governance control it depends on is unverifiable.
Fix: the athena crate's output-location resolution (EnforceWorkGroupConfiguration precedence) together with the result writer from cause 1.
Traces: athena.workgroup-enforced-configuration-overrides-client-output-location.

Cause 3 of 3: with EnforceWorkGroupConfiguration=false the client-supplied prefix is not used either.
AWS: with enforcement off, the client's ResultConfiguration.OutputLocation wins and the result object is written under the client prefix.
Emulator: nothing is written under the client-supplied prefix.
Why: the two halves of the enforcement contract are indistinguishable — a client cannot control where its results land under either setting.
Fix: same resolution path and result writer as causes 1-2, on the enforcement-off branch.
Traces: athena.server-side-enforced-workgroup-config-overrides-client-request.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout.

Cause 1 — athena.athena-workgroup-default-output-location-used-by-query and athena.workgroup-default-output-location-used-for-query both fail [error] with botocore.errorfactory.NoSuchKey: An error occurred (NoSuchKey) when calling the GetObject operation: The specified key does not exist. — the GetObject targets the result object under the workgroup's configured prefix after the query reached a terminal state.
Cause 2 — athena.workgroup-enforced-configuration-overrides-client-output-location: assertion `_object_count(s3, bucket, "wg-output/") > 0` fails — 'no result object was actually written under the enforced workgroup prefix'. The workgroup was created with Configuration.ResultConfiguration.OutputLocation='s3://<bucket>/wg-output/' and EnforceWorkGroupConfiguration=True, and StartQueryExecution(QueryString='SELECT 1', WorkGroup=<wg>, ResultConfiguration.OutputLocation='s3://<bucket>/client-output/').
Cause 3 — athena.server-side-enforced-workgroup-config-overrides-client-request: 'with EnforceWorkGroupConfiguration=False the query result must actually be written under the client-supplied prefix'.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay suite simparity-behavioral-aws-v1 and inspect the four athena checks listed in trace_ids.
3. Minimal manual repro:
   a. CreateBucket B. CreateWorkGroup(Name=W, Configuration.ResultConfiguration.OutputLocation='s3://B/wg-output/'). StartQueryExecution(QueryString='SELECT 1', WorkGroup=W) with no ResultConfiguration. Poll GetQueryExecution to SUCCEEDED, then S3 GetObject 's3://B/wg-output/<QueryExecutionId>.csv' — expect 200; observe NoSuchKey.
   b. Set EnforceWorkGroupConfiguration=true on W and submit with ResultConfiguration.OutputLocation='s3://B/client-output/' — after SUCCEEDED, ListObjectsV2 on 'wg-output/' must be non-empty and 'client-output/' empty; observe both empty.
   c. Set EnforceWorkGroupConfiguration=false and repeat — 'client-output/' must be non-empty; observe empty.
