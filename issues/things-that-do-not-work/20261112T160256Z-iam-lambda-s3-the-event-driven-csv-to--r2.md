Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__lambda__s3.e2e-csv-to-xlsx-pipeline-lifecycle
Issue-Key: iam__lambda__s3.e2e-csv-to-xlsx-pipeline-lifecycle|_watch_absent#9|Timeout|return found
Issue-Commit: 2aaa5a298b102bf0987c01ba9c9df4294351381c

# iam__lambda__s3: the event-driven CSV-to-xlsx pipeline still never completes a phase — an S3 ObjectCreated notification never invokes the Lambda, so no workbook is produced and the filter-rejected key proves nothing

> _Note: this issue was identified against the code base at commit `2aaa5a298b102bf0987c01ba9c9df4294351381c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
REOPENS 20261110T153718Z-iam-lambda-s3-the-event-driven-csv-to.md: that issue is marked fixed, but the same check still reaches no assertion at all — it is cut off with the identical "Error: Check timed out after 60.0s", so the fix did not make the pipeline run. 1 net-new root cause for this heading (the only failing check under it that is neither ALREADY FILED nor OWNED).

Cause 1 of 1: an S3 bucket notification whose LambdaFunctionConfigurations name a function never results in that function being invoked for a matching ObjectCreated event, so the whole event-driven pipeline (role -> Lambda -> notification -> converted object) makes no progress and every assertion downstream of the first upload is unreachable. Failing check: iam__lambda__s3.e2e-csv-to-xlsx-pipeline-lifecycle.

AWS: Smithy (service-com-amazonaws-s3-amazons3): PutBucketNotificationConfiguration PUT /{Bucket}?notification, 200 with an empty body; input NotificationConfiguration.LambdaFunctionConfigurations is a list of LambdaFunctionConfiguration whose LambdaFunctionArn ("the ARN of the Lambda function that Amazon S3 invokes when the specified event type occurs") and Events are @required, with an optional Filter (prefix/suffix key rules). Beyond the wire contract this is recorded behavior only: once the execution role, the function and the notification are in place, a PutObject of a key matching the filter causes S3 to invoke the function asynchronously with an s3:ObjectCreated:* record — with no further API call from the client — and the object the handler writes (here an OPC .xlsx carrying the source CSV rows, Content-Type application/vnd.openxmlformats-officedocument.spreadsheetml.sheet) becomes readable from the bucket; a key the Filter rejects causes no invocation and therefore no output object; and the pipeline can then be torn down (bucket emptied, notification/function/role/bucket deleted) leaving nothing behind. The recorded run also shows the normal eventual-consistency window for a freshly-written notification configuration (the check budgets 540 s of re-upload rounds for it) and the post-CreateRole window in which lambda:CreateFunction still answers InvalidParameterValueException "The role defined for the function cannot be assumed by Lambda" (bounded retry, 120 s) — on AWS both windows close and the pipeline then converts every new upload.

Emulator: the check is reported [mismatch] with "Error: Check timed out after 60.0s" and no assertion reached. The emulator has no real propagation delay to wait out, so the only way to still be spinning at the harness cap is that the arming/conversion loop never converges: no converted object ever appears for an uploaded raw/*.csv key, so the probe loop re-uploads until it is cut off. Nothing after the first upload is ever compared — not the workbook's existence, not its sheet cells against the source CSV, not the required absence of output for a filter-rejected key, not the teardown.

Why: a client that wires S3 -> Lambda the documented way gets a pipeline that silently produces nothing. There is no error to react to: PutBucketNotificationConfiguration answers 200, the function exists and is Active, and only the derived objects are missing, so an operator sees an apparently healthy estate with an empty output prefix. Worse, because the positive effect never appears, every negative control built on it is vacuous — an emulator that ignored Filter entirely, or that invoked the wrong function, is indistinguishable from this one.

Fix: the S3 notification dispatcher's Lambda destination — the component that, on an ObjectCreated event, evaluates the stored LambdaFunctionConfigurations (Events plus the prefix/suffix Filter) and performs the asynchronous lambda:Invoke against the named function ARN — together with the function runtime that must actually execute the handler and let its PutObject land in the bucket. If arming the configuration is deliberately delayed, that delay must be short enough that a newly uploaded matching key is converted, since the configuration is already readable.

Traces: iam__lambda__s3.e2e-csv-to-xlsx-pipeline-lifecycle (the only example; it exercises CreateRole/AttachRolePolicy/PutRolePolicy, lambda CreateFunction/GetFunction/AddPermission, s3 PutBucketNotificationConfiguration/ListObjectsV2/PutObject and lambda Invoke for the backfill).

## Evidence
Failing check iam__lambda__s3.e2e-csv-to-xlsx-pipeline-lifecycle, verdict [mismatch]:
  Error: Check timed out after 60.0s
with no assertion reached — the suite report lists no other detail for it, and the check carries neither an ALREADY FILED nor an OWNED marker, so no open issue holds its replay key.

Expected (from the check's own docstring and its recorded trajectory): "Builds an event-driven CSV-to-xlsx pipeline (role, Lambda, S3 notification) and proves a CSV uploaded under raw/ yields a real OPC workbook holding the source rows with no further API call, while a filter-rejected key yields nothing; then tears the pipeline down." Concretely, for every raw/<stem>.csv present or newly uploaded, an object <OutputPrefix><stem>.xlsx appears whose xl/worksheets/sheet1.xml inline-string cells equal the rows of the source CSV read back at run time, stamped Content-Type application/vnd.openxmlformats-officedocument.spreadsheetml.sheet; for a key the notification Filter rejects, no object appears.

Actual: no phase of the lifecycle completes inside the run budget, so none of those comparisons happened. The identical symptom was already reported and closed as 20261110T153718Z-iam-lambda-s3-the-event-driven-csv-to.md (Issue-Key iam__lambda__s3.e2e-csv-to-xlsx-pipeline-lifecycle|_watch_absent#9|Timeout|return found); this replay shows the behavior unchanged.

Per-run values (bucket name, function and role names carrying the run token, object keys, timestamps, request ids) are not part of the requirement; the requirement is that a matching upload is converted at all.

## Steps to Reproduce
1. Create an S3 bucket and seed a few objects under raw/ with .csv keys (e.g. raw/orders_2026_01.csv holding a header row plus data rows).
2. iam CreateRole with a trust policy for lambda.amazonaws.com, AttachRolePolicy AWSLambdaBasicExecutionRole, and PutRolePolicy granting s3:GetObject/s3:PutObject on the bucket.
3. lambda CreateFunction (python3.12, in-process zip) whose handler reads record['s3']['object']['key'], fetches the CSV and PutObject's an .xlsx built from its rows under converted/; retry CreateFunction while it answers InvalidParameterValueException "The role defined for the function cannot be assumed by Lambda", then poll GetFunction to State=Active.
4. lambda AddPermission: principal s3.amazonaws.com, action lambda:InvokeFunction, SourceArn the bucket ARN.
5. s3 PutBucketNotificationConfiguration with one LambdaFunctionConfiguration: Events ['s3:ObjectCreated:*'], LambdaFunctionArn the function, Filter prefix 'raw/' and suffix '.csv'.
6. PutObject a NEW key raw/probe-<n>.csv and poll ListObjectsV2/GetObject under converted/ for the derived <stem>.xlsx. Expected: the workbook appears (and parses as an OPC package whose sheet cells equal the CSV rows) with no further API call. Observed: it never appears, and the check is cut off at the 60 s harness cap having completed no phase.
7. PutObject a key the filter rejects (e.g. raw/ignored.txt) and confirm no output object appears — unreachable today because step 6 never succeeds.
8. Re-replay iam__lambda__s3.e2e-csv-to-xlsx-pipeline-lifecycle to confirm the fix.
