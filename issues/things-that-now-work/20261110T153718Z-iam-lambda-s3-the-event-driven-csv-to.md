Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__lambda__s3.e2e-csv-to-xlsx-pipeline-lifecycle
Issue-Key: iam__lambda__s3.e2e-csv-to-xlsx-pipeline-lifecycle|_watch_absent#9|Timeout|return found
Issue-Commit: 76dff93be94f91562bba6a25a3aecfc1e2578190

# iam__lambda__s3: the event-driven CSV-to-xlsx pipeline never completes a phase — the S3 notification never converts an upload and the filter-rejected key cannot be shown to yield nothing

> _Note: this issue was identified against the code base at commit `76dff93be94f91562bba6a25a3aecfc1e2578190`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration (ObjectCreated -> Lambda) / Invoke

## Description
One net-new root cause in the iam__lambda__s3 suite; this heading has no open issue carrying this check's replay key.

Cause 1 of 1: the pipeline the check builds — execution role, Lambda, and an S3 ObjectCreated notification with a key filter — never converts an uploaded CSV, so neither the produced workbook nor the filter-rejected non-effect nor the teardown is observable.
AWS: recorded behavior only for the end-to-end effect. Once the role, function and bucket notification are in place, a CSV uploaded under raw/ triggers the function with no further API call and a real OPC (xlsx) workbook holding the source rows appears at the output key; a key the notification's filter rejects produces nothing; and tearing the pipeline down (emptying the bucket, deleting the notification, function, role and bucket) leaves nothing behind.
Emulator: the check is reported "Error: Check timed out after 60.0s" with no assertion reached — no phase of the lifecycle completes, so neither the workbook's existence and contents, nor the required absence for the filtered key, nor the clean teardown was ever compared.
Why: an event-driven S3 -> Lambda data pipeline produces nothing. Every derived artifact an operator would inspect is missing, and because the positive effect never appears the negative (filter-rejected key yields nothing) is vacuous — a notification configuration that ignored its filter entirely would look the same.
Fix: the S3 notification dispatcher's Lambda destination (an ObjectCreated event matching the configured prefix/suffix filter must asynchronously invoke the function) together with the function runtime that must actually execute the handler and write its output object.
Traces: iam__lambda__s3.e2e-csv-to-xlsx-pipeline-lifecycle.

## Evidence
Check iam__lambda__s3.e2e-csv-to-xlsx-pipeline-lifecycle, verdict [mismatch]:
  Error: Check timed out after 60.0s
with no assertion reached.

The awaited requirement, from the check's own docstring: "Builds an event-driven CSV-to-xlsx pipeline (role, Lambda, S3 notification) and proves a CSV uploaded under raw/ yields a real OPC workbook holding the source rows with no further API call, while a filter-rejected key yields nothing; then tears the pipeline down." The check notes that the reference stack's BucketDeployment becomes its `setup` phase and autoDeleteObjects becomes its `cleanup` phase's paginated delete.

Expected: after PutObject of a CSV under raw/, an xlsx object appears whose sheet holds the source rows, with no further API call; a filter-rejected key yields no object; teardown removes everything. Actual: no phase completes inside the run budget, so nothing was compared. Bucket names, function names, roles and keys are per-run values.

## Steps to Reproduce
1. Create a bucket and an execution role; create a Lambda whose handler reads the uploaded CSV and writes an xlsx workbook back to the bucket.
2. AddPermission for s3.amazonaws.com, then PutBucketNotificationConfiguration with a LambdaFunctionConfiguration on s3:ObjectCreated:* filtered to the raw/ prefix and the CSV suffix.
3. PutObject a CSV under raw/ and poll for the output object; open it as an OPC workbook and compare the rows to the source.
4. PutObject a key the filter rejects and confirm no output object appears.
5. Empty and delete the bucket, function and role. The emulator completes none of this. Re-replay iam__lambda__s3.e2e-csv-to-xlsx-pipeline-lifecycle to confirm the fix.
