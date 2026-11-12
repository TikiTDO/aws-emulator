Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rum.telemetry-batch-create-metric-definitions-then-get-reflects rum.telemetry-batch-delete-definitions-removed rum.telemetry-batch-delete-metric-definitions-removes-definition rum.telemetry-update-metric-definition-persists-change rum.e2e-app-monitor-lifecycle-with-metrics-and-events rum.batch-create-metric-definition-duplicate-name-conflict rum.batch-create-metric-definition-duplicate-rejected
Issue-Shape-Id: com.amazonaws.rum#RUM
Issue-Key: rum.telemetry-batch-create-metric-definitions-then-get-reflects|run#10|AssertionError|assert len(created) == 1, diag
Issue-Key: rum.telemetry-batch-delete-definitions-removed|run#11|AssertionError|assert len(created) == 1, f"expected exactly one definition created, got {created!r}"
Issue-Key: rum.telemetry-batch-delete-metric-definitions-removes-definition|run#10|AssertionError|assert len(created) == 1 and not errs, diag
Issue-Key: rum.telemetry-update-metric-definition-persists-change|run#11|AssertionError|assert len(created) == 1 and not errs, diag
Issue-Key: rum.e2e-app-monitor-lifecycle-with-metrics-and-events|run#19|AssertionError|assert len(create_resp.get("MetricDefinitions", [])) == 1, create_resp
Issue-Key: rum.batch-create-metric-definition-duplicate-name-conflict|run#15|AssertionError|assert len(second.get("Errors", [])) == 1, f"expected one item error, got {second}"
Issue-Key: rum.batch-create-metric-definition-duplicate-rejected|_create_until_resolved#16|Timeout|time.sleep(interval_s)
Issue-Commit: c42734c699ac346ade2bea5e1d020e4f24cb85eb

# BatchCreateRumMetricDefinitions never echoes the definitions it created and silently skips a duplicate instead of reporting a per-item ConflictException

> _Note: this issue was identified against the code base at commit `c42734c699ac346ade2bea5e1d020e4f24cb85eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchCreateRumMetricDefinitions

## Description
This issue bundles TWO distinct net-new root causes in the CloudWatch RUM BatchCreateRumMetricDefinitions handler (services/service-com-amazonaws-rum-rum/src/ops/batch_create_rum_metric_definitions.rs). BOTH must be closed before submitting; each is numbered below and carries its own failing check ids.

Note on an adjacent OPEN issue: 20261029T192752Z-rum-metric-definition-and-metrics-destination-control-plane.md lists rum.batch-create-metric-definition-duplicate-name-conflict, rum.batch-create-metric-definition-duplicate-rejected and rum.telemetry-batch-delete-definitions-removed among its trace ids, and its Cause 1 describes the duplicate path as "silently re-creates ... with a freshly minted MetricDefinitionId". The emulator no longer does that: the duplicate now mints nothing, so the symptom and the replay key have both changed, and the suite did NOT mark any of these checks OWNED by that issue. Cause 2 below is the residual half of that behaviour (the per-item ConflictException is still never produced), and Cause 1 below is an entirely different defect (the success echo) that that issue does not mention at all. Neither cause is covered by it.

Cause 1 of 2: a SUCCESSFUL BatchCreateRumMetricDefinitions answers 200 with MetricDefinitions as the empty list, so the created definitions and the MetricDefinitionIds the call minted are never returned.
AWS: Smithy BatchCreateRumMetricDefinitionsResponse has Errors (target BatchCreateRumMetricDefinitionsErrors, @required) and MetricDefinitions (target MetricDefinitions, optional), documented "An array of structures that define the extended metrics."; each MetricDefinition carries MetricDefinitionId (@required, @length 1..255), Name (@required), and the optional ValueKey/UnitLabel/DimensionKeys/EventPattern/Namespace. Recorded behavior: a create of N valid definitions answers HTTP 200 with Errors=[] and MetricDefinitions holding ONE MetricDefinition per definition created, each with the server-minted MetricDefinitionId and the request's members echoed back; the ids in that response are the handle a caller uses for UpdateRumMetricDefinition and BatchDeleteRumMetricDefinitions.
Emulator: every create answers HTTP 200 with a body of exactly {"Errors": [], "MetricDefinitions": []} (content-length 36), whatever the batch created. The definitions ARE persisted — BatchGetRumMetricDefinitions returns them afterwards — but the create response reports none of them. The handler ends with `metric_definitions: Some(Vec::new())` and a comment asserting "the create does not echo what it created, so the ids it minted are learned from BatchGetRumMetricDefinitions"; that premise is what the replay contradicts.
Why: a client cannot learn the id of a definition it just created from the call that created it. Every provisioning flow that does create -> capture MetricDefinitionId -> update/delete breaks: it either fails on an empty list, or has to guess which of the destination's definitions is its own by re-reading BatchGetRumMetricDefinitions and matching on content, which is ambiguous when two definitions share a Name. Five independent checks fail at the first assertion after the create, before they can exercise update or delete at all.
Fix: the response construction at the end of the BatchCreateRumMetricDefinitions handler — the MetricDefinitions member must carry a MetricDefinition built from each record the call inserted (minted id plus the requested Name/ValueKey/UnitLabel/DimensionKeys/EventPattern/Namespace), not an unconditional empty Vec. The records are already materialised in the `records` vector immediately before the insert loop.
Traces: rum.telemetry-batch-create-metric-definitions-then-get-reflects, rum.telemetry-batch-delete-definitions-removed, rum.telemetry-batch-delete-metric-definitions-removes-definition, rum.telemetry-update-metric-definition-persists-change, rum.e2e-app-monitor-lifecycle-with-metrics-and-events.

Cause 2 of 2: resubmitting a metric definition the destination already holds is silently skipped instead of being reported per-item as ConflictException.
AWS: Smithy models ConflictException (HTTP 409) on BatchCreateRumMetricDefinitions and the per-item BatchCreateRumMetricDefinitionsError{MetricDefinition, ErrorCode, ErrorMessage} (all three @required) inside Errors. Recorded behavior: resubmitting a definition whose CONTENT is identical (same Name + Namespace + EventPattern) for the same AppMonitorName/Destination answers HTTP 200 with MetricDefinitions EMPTY and EXACTLY ONE Errors entry whose ErrorCode is "ConflictException" and whose ErrorMessage has the fixed text "Metric definition already exists: MetricDefinitionRequest(name=, valueKey=, unitLabel=, dimensionKeys=, eventPattern=, namespace=)" with the variable parts filled from the request's MetricDefinition members and the literal `null` substituted for each member the request omitted (so an omitted ValueKey renders valueKey=null, an omitted UnitLabel unitLabel=null, an omitted DimensionKeys dimensionKeys=null). The pre-existing definition keeps its original id and content — the rejected resubmission overwrites nothing. The conflict key is the definition's CONTENT, not its Name alone: the same Name with a DIFFERENT EventPattern is not a conflict and creates a second coexisting definition (recorded behavior only). Deleting the original frees the identity and makes the identical call succeed again.
Emulator: the duplicate branch in the handler's per-definition loop (`if monitor.metric_definition_defining(...).is_some() { continue; }`) drops the definition with no entry pushed into `errors`, so the second identical call answers HTTP 200 with Errors=[] and MetricDefinitions=[] — indistinguishable from the first, successful call. No ConflictException is produced on any channel: not per-item, not as an operation error. The in-batch dedupe branch immediately below it does the same thing for a definition named twice within one request.
Why: the batch API's per-item Errors list is the only channel by which a client learns "already defined". A reconciler (CDK/Terraform-style, or any retry-on-timeout wrapper) that re-sends its desired batch gets a clean 200 with no errors, so its dedupe-by-ConflictException branch is dead code and it cannot tell a definition it just created from one that already existed. Combined with Cause 1 the response carries no information at all: a caller cannot distinguish "created 3" from "all 3 already existed". The duplicate-rejected check additionally TIMES OUT after 60s because it polls waiting for the conflict item that never arrives.
Fix: the per-definition classification loop in the BatchCreateRumMetricDefinitions handler — the already-held branch must push a BatchCreateRumMetricDefinitionsError{metric_definition: requested.clone(), error_code: "ConflictException", error_message: <the wording above>} instead of `continue`, with the message rendered by a helper in the service's metric-definition state module alongside the existing refusal-message builders. Note the handler's current doc comment frames this skip as the model's @idempotent trait; @idempotent constrains the STATE effect (nothing is stored a second time, which stays correct), not the response, which must still report the conflict per item.
Traces: rum.batch-create-metric-definition-duplicate-name-conflict, rum.batch-create-metric-definition-duplicate-rejected.

## Evidence
Suite: 51/81 passed; all 30 failures are in service `rum` (bootstrapped as service-com-amazonaws-rum-rum). 23 of the 30 were marked OWNED by an existing issue and skipped; the 7 checks below were not marked ALREADY FILED or OWNED by.

Cause 1 — the create response is always empty:
  rum.telemetry-batch-create-metric-definitions-then-get-reflects
    check_rum_telemetry_batch_create_metric_definitions_then_get_reflects.py:36
    `assert len(created) == 1, diag` -> AssertionError: n=0/0  (0 MetricDefinitions, 0 Errors)
  rum.telemetry-batch-delete-metric-definitions-removes-definition
    check_...removes_definition.py:34 `assert len(created) == 1 and not errs, diag` -> AssertionError: n=0/0
  rum.telemetry-update-metric-definition-persists-change
    check_...persists_change.py:36 `assert len(created) == 1 and not errs, diag` -> AssertionError: n=0/0
  rum.telemetry-batch-delete-definitions-removed
    check_...definitions_removed.py:42 `assert len(created) == 1` -> AssertionError: expected exactly one definition created, got []
  rum.e2e-app-monitor-lifecycle-with-metrics-and-events
    check_...with_metrics_and_events.py:60 `assert len(create_resp.get("MetricDefinitions", [])) == 1`
    actual: {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/json', 'content-length': '36', ...}}, 'Errors': [], 'MetricDefinitions': []}
    expected: 200 with MetricDefinitions=[{MetricDefinitionId: <minted id>, Name: <requested Name>, Namespace: <requested Namespace>, EventPattern: <requested EventPattern>}] and Errors=[]
  Source: services/service-com-amazonaws-rum-rum/src/ops/batch_create_rum_metric_definitions.rs:396-409 returns `metric_definitions: Some(Vec::new())` unconditionally; the records it just inserted are built at lines 377-394.

Cause 2 — the duplicate resubmission reports no error:
  rum.batch-create-metric-definition-duplicate-name-conflict
    check_rum_batch_create_metric_definition_duplicate_name_conflict.py:64
    `assert len(second.get("Errors", [])) == 1`
    actual: {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/json', 'content-length': '36', ...}}, 'Errors': [], 'MetricDefinitions': []}
    expected: 200, MetricDefinitions=[], Errors=[{ErrorCode: "ConflictException", ErrorMessage: "Metric definition already exists: MetricDefinitionRequest(name=<request Name>, valueKey=null, unitLabel=null, dimensionKeys=null, eventPattern=<request EventPattern verbatim>, namespace=<request Namespace>)"}]  — fixed text is everything outside the `name=`/`valueKey=`/`unitLabel=`/`dimensionKeys=`/`eventPattern=`/`namespace=` values; those values come from the request's MetricDefinition, with the literal `null` for each omitted member.
  rum.batch-create-metric-definition-duplicate-rejected
    Error: Check timed out after 60.0s — the check polls for the per-item ConflictException that is never emitted.
  Source: batch_create_rum_metric_definitions.rs:346-358 — both the already-held branch and the in-batch duplicate branch `continue` with nothing pushed onto `errors`.

## Steps to Reproduce
Replay the 7 checks listed in trace_ids against the emulator. Minimal manual repro per cause:

Shared setup:
  CreateAppMonitor(Name=m, Domain="example.com")
  PutRumMetricsDestination(AppMonitorName=m, Destination="CloudWatch")

Cause 1:
  BatchCreateRumMetricDefinitions(AppMonitorName=m, Destination="CloudWatch", MetricDefinitions=[{Name:"MyMetric", Namespace:"custom_ns", EventPattern:'{"event_type": ["com.amazon.rum.js_error_event"]}'}])
  Observed: 200, body {"Errors": [], "MetricDefinitions": []}.
  Expected: 200, Errors=[], MetricDefinitions=[{MetricDefinitionId:<minted id>, Name:"MyMetric", Namespace:"custom_ns", EventPattern:'{"event_type": ["com.amazon.rum.js_error_event"]}'}].
  Then BatchGetRumMetricDefinitions(AppMonitorName=m, Destination="CloudWatch") shows the definition, confirming it was persisted and only the response is wrong, and the MetricDefinitionId there must equal the one the create returned.

Cause 2:
  Issue the IDENTICAL BatchCreateRumMetricDefinitions call from Cause 1 a second time, unchanged.
  Observed: 200, Errors=[], MetricDefinitions=[].
  Expected: 200, MetricDefinitions=[], Errors=[ one item, ErrorCode "ConflictException", MetricDefinition echoing the submitted request, ErrorMessage worded as in the description ].
  Then BatchGetRumMetricDefinitions must still show exactly ONE definition with that Name, with the ORIGINAL MetricDefinitionId and the original EventPattern (the refused resubmission must not overwrite it).
  Control that must keep passing: resubmit the same Name with a DIFFERENT EventPattern -> no conflict, a second coexisting definition is created with its own id.
  Control that must keep passing: BatchDeleteRumMetricDefinitions the original id, then re-issue the identical create -> succeeds with no error.
