Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.transformer-rewrites-ingested-events-preserving-the-raw-message logs.delete-transformer-then-log-events-no-longer-transformed logs.log-fields-and-record-pointer-errors-rejected logs.get-log-object-access-denied-enforced logs.get-storage-tier-policy-access-denied-without-permission logs.delete-metric-filter-stops-future-metric-emission
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.transformer-rewrites-ingested-events-preserving-the-raw-message|run#24|AssertionError|assert _RENAMED_KEY in xform_record, (
Issue-Key: logs.delete-transformer-then-log-events-no-longer-transformed|run#14|AssertionError|assert log_record1.get("action") == "login", log_record1
Issue-Key: logs.log-fields-and-record-pointer-errors-rejected|run#36|AssertionError|assert code_msg == expected, f"expected message {expected!r}, got {code_msg!r}"
Issue-Key: logs.get-log-object-access-denied-enforced|run#17|AssertionError|assert code == "InvalidParameterException", (
Issue-Key: logs.get-storage-tier-policy-access-denied-without-permission|run#16|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: logs.delete-metric-filter-stops-future-metric-emission|run#31|Timeout|time.sleep(15.0)
Issue-Commit: 6e1ef7d8ebea1acbe69ee2691b3319db9dddeedf

# logs: 5 net-new causes — ingest transformers are never applied, GetLogObject's invalid-pointer and no-permission refusals diverge, GetStorageTierPolicy's denial ARN is malformed, and metric-filter emission never settles

> _Note: this issue was identified against the code base at commit `6e1ef7d8ebea1acbe69ee2691b3319db9dddeedf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTransformer

## Description
Five net-new root causes across six checks. None of the six is marked ALREADY FILED or OWNED. Open issues name adjacent members of some of these families — 20261029T182816Z-logs-transformers-are-not-applied-on-ingest-data.md (cause 1), 20261109T143515Z-logs-5-net-new-causes-getlogobject-pointer-refusal.md (causes 2 and 3), 20261024T150703Z-logs-cloudwatch-logs-metric-filters-emit-no-cloudwatch-metric.md (cause 5) — but none of them carries the replay keys below.

Cause 1 of 5: a transformer installed with PutTransformer is never applied to events on ingest.
AWS: after PutTransformer on a log group, every event ingested by PutLogEvents is passed through transformerConfig before it is stored: a parseKeyValue/parseJSON processor's extracted fields and a renameKeys processor's renamed field are present on the stored record (readable through Insights/FilterLogEvents), while @message keeps the raw original. DeleteTransformer then stops that, and subsequent events are stored untransformed. Smithy: logs PutTransformer(logGroupIdentifier, transformerConfig: Processors, 1..20) and DeleteTransformer(logGroupIdentifier) are both modelled; GetTransformer echoes the installed config.
Emulator: PutTransformer is accepted and GetTransformer echoes the config, but ingested records carry only @message/@timestamp/@ptr and the raw payload — the renamed field is absent (`the transformer was installed and echoed by GetTransformer, but the ingested event has no 'renamed_field' field`), and the delete-then-ingest check cannot even establish its transformed baseline (the record has no parsed `action` field at all, only `@message: 'action=login&user=...'`).
Why: a client that relies on a transformer to structure its logs at ingest gets unstructured records with no error, so every downstream query, metric filter and subscription that keys on the transformed fields silently matches nothing — and DeleteTransformer cannot be shown to change anything.
Fix: the PutLogEvents ingest path must apply the log group's (and the account policy's) transformer to each event before storing it, and stop applying it after DeleteTransformer.
Traces: logs.transformer-rewrites-ingested-events-preserving-the-raw-message, logs.delete-transformer-then-log-events-no-longer-transformed.

Cause 2 of 5: GetLogObject's malformed-pointer refusal uses the wrong message text.
AWS: GetLogObject with a logObjectPointer that is not a valid pointer is refused with the fixed message `Invalid log object pointer format.` Smithy: logs GetLogObject declares AccessDeniedException, InvalidOperationException, InvalidParameterException, LimitExceededException and ResourceNotFoundException, all client errors at HTTP 400; logObjectPointer is required with length 1..512.
Emulator: the message is `The specified logObjectPointer is not valid.` — entirely emulator-authored wording for a message whose text is fixed (no variable part at all).
Why: the message is the only thing distinguishing a malformed pointer from the other 400s GetLogObject can return; a client matching on it (or showing it to an operator) gets text AWS never emits.
Fix: the GetLogObject handler's invalid-pointer refusal message.
Traces: logs.log-fields-and-record-pointer-errors-rejected.

Cause 3 of 5: GetLogObject prefers an authorization refusal over the parameter refusal for a bogus pointer.
AWS: GetLogObject called with no permissions AND an unparseable pointer is refused with InvalidParameterException — the pointer is rejected before authorization is evaluated, so the caller learns the request was malformed rather than that it was denied.
Emulator: the call is refused with AccessDeniedException instead.
Why: the two codes drive different client behaviour — AccessDeniedException sends a client off to repair IAM for a request that could never have succeeded, and hides a malformed pointer indefinitely.
Fix: the GetLogObject handler's ordering of pointer validation against the authorization check.
Traces: logs.get-log-object-access-denied-enforced.

Cause 4 of 5: the GetStorageTierPolicy denial message renders a malformed resource ARN.
AWS: a caller with no policy allowing logs:GetStorageTierPolicy is refused AccessDeniedException with the message `User: <session arn> is not authorized to perform: logs:GetStorageTierPolicy on resource: <resource arn> because no identity-based policy allows the logs:GetStorageTierPolicy action`. The fixed text is `User: `, ` is not authorized to perform: `, ` on resource: ` and ` because no identity-based policy allows the <action> action`; the variable parts are the caller's session arn, the action and the resource arn — which for this account-level operation is the wildcard log-group ARN `arn:aws:logs:<region>:<account>:log-group:*`. Smithy: logs GetStorageTierPolicy takes no input members and declares AccessDeniedException among its errors.
Emulator: the resource is rendered as `arn:aws:logs:<region>:<account>:log-group::log-stream:` — an empty log-group name plus a stray log-stream segment.
Why: the resource ARN in a denial is what an operator or a policy-repair tool copies into the Resource element of the fix. This one names no resource at all, so the repair cannot be derived from the message.
Fix: the CloudWatch Logs authorization layer's resource-ARN rendering for account-level operations (GetStorageTierPolicy must denote 'log-group:*', not an empty group with a log-stream segment).
Traces: logs.get-storage-tier-policy-access-denied-without-permission.

Cause 5 of 5: metric-filter metric emission never settles, so DeleteMetricFilter cannot be shown to stop it.
AWS: while a metric filter exists, each matching ingested event increments its CloudWatch metric (GetMetricStatistics/GetMetricData over the filter's metricTransformations namespace/name returns datapoints); after DeleteMetricFilter, further matching events produce no new datapoints.
Emulator: the check is killed with `Error: Check timed out after 60.0s` while waiting for the before-delete datapoints, so the stop-after-delete contract is never reached.
Why: a metric filter is how log content becomes an alarm. A client gets a filter that reports as created but publishes nothing, so every alarm built on it stays in INSUFFICIENT_DATA — and deleting the filter changes nothing observable either.
Fix: the PutLogEvents ingest path's metric-filter evaluation and CloudWatch metric publication, and its removal on DeleteMetricFilter.
Traces: logs.delete-metric-filter-stops-future-metric-emission.

## Evidence
Cause 1 — Expected: the ingested record carries the transformer's output fields. Actual: `AssertionError: the transformer was installed and echoed by GetTransformer, but the ingested event has no 'renamed_field' field: {'@message': '{"orig": "..."}', 'orig': '...', ...}` and `AssertionError: {'@message': 'action=login&user=...', '@log': ..., '@logStream': ..., '@ptr': ...}` for `log_record1.get("action") == "login"`.
Cause 2 — Expected message `Invalid log object pointer format.` Actual: `The specified logObjectPointer is not valid.`
Cause 3 — Expected error code InvalidParameterException. Actual: `AssertionError: expected InvalidParameterException for GetLogObject with no permissions and a bogus pointer, got 'AccessDeniedException'`.
Cause 4 — Expected `... on resource: arn:aws:logs:<region>:<account>:log-group:* because no identity-based policy allows the logs:GetStorageTierPolicy action`. Actual: `... on resource: arn:aws:logs:us-west-2:000000000000:log-group::log-stream: because no identity-based policy allows the logs:GetStorageTierPolicy action` (region/account/role are per-run; the malformed resource ARN is the defect).
Cause 5 — Expected datapoints before DeleteMetricFilter and none after. Actual: `Error: Check timed out after 60.0s`.

## Steps to Reproduce
Cause 1: PutTransformer on a log group with a parse+renameKeys config, PutLogEvents a matching event, and read it back via Insights/FilterLogEvents; then DeleteTransformer, ingest again and read back.
Cause 2: GetLogObject(logObjectPointer='<not a pointer>') and compare the message text.
Cause 3: assume a role with no logs permissions and call GetLogObject with a bogus pointer; compare the error code.
Cause 4: assume a role with no logs permissions and call GetStorageTierPolicy; read the resource ARN out of the AccessDeniedException message.
Cause 5: PutMetricFilter on a group, ingest matching events, poll cloudwatch GetMetricStatistics for the transformed metric, then DeleteMetricFilter and ingest more matching events.
