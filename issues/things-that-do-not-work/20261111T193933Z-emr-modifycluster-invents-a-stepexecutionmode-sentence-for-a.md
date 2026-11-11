Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr.modify-cluster-step-concurrency-level-out-of-range-rejected
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: emr.modify-cluster-step-concurrency-level-out-of-range-rejected|run#18|AssertionError|assert err["Message"] == _RANGE_MESSAGE, f"level 257: message was {err['Message']!r}"
Issue-Commit: b37a548368ed88ff8e9aed060c39391f1ed9f4cf

# EMR ModifyCluster invents a StepExecutionMode sentence for a StepConcurrencyLevel of 257..6400 instead of AWS's 'Invalid step concurrency level. Allowed level is from 1 to 256'

> _Note: this issue was identified against the code base at commit `b37a548368ed88ff8e9aed060c39391f1ed9f4cf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyCluster

## Description
Cause 1 of 1: ModifyCluster refuses an out-of-band StepConcurrencyLevel with the right error code but the wrong message for the 257..6400 class.

AWS: ModifyCluster takes `StepConcurrencyLevel` as a plain `Integer` with no @range, and the member's own documentation states the band — "The number of steps that can be executed concurrently. You can specify a minimum of 1 step and a maximum of 256 steps." (Smithy: ModifyCluster.StepConcurrencyLevel doc; errors: InvalidRequestException (client, HTTP 400), InternalServerError (HTTP 500)). The model declares no message text, so the wording is recorded behavior only: Amazon EMR answers two different messages under the single code `InvalidRequestException`, HTTP 400. A level that is below the lower bound (0 or less) or above 6400 gets the bare sentence `Invalid step concurrency level.`; a level ABOVE 256 but still within 1..6400 gets that same sentence EXTENDED with the band it may use: `Invalid step concurrency level. Allowed level is from 1 to 256`. Both messages are entirely FIXED text — there is no variable part taken from the request or from state, no echo of the rejected level, and no mention of a step execution mode. The two bounds quoted in the second sentence (1 and 256) are the service's documented band, not values from a particular run.

Emulator: for a level in 257..6400 the emulator returns InvalidRequestException with a self-invented, templated sentence instead: `StepConcurrencyLevel 257 exceeds the maximum (256) for StepExecutionMode STANDARD. Set StepExecutionMode to DISTRIBUTED for up to 6400 concurrent steps.` It echoes the rejected level into the message and names a `StepExecutionMode` concept (STANDARD/DISTRIBUTED, ceiling 6400) that this API surface does not expose at all — no operation in the EMR Smithy model carries a step-execution-mode member. The HTTP status (400) and the error code (`InvalidRequestException`) are correct, and the bare-sentence classes (level 0 and level 6401) are already correct, as is the in-range positive control (level 4 is applied and read back by DescribeCluster), so the divergence is confined to the message of the above-256-but-servable class.

Why: a client that matches on the refusal text — a retry/branching rule, a log assertion, a user-facing message, or a test that distinguishes EMR's two step-concurrency refusals — sees a sentence AWS never emits. Worse, the invented text advertises a remedy that does not exist on this API ("Set StepExecutionMode to DISTRIBUTED"), so a caller is steered toward a non-existent request member instead of being told the real band (1..256) it must stay inside.

Fix: the message producer in this service's shared validation state, `crate::state::step_concurrency_level_violation` and the helper it delegates the above-256 class to, `step_concurrency_level_exceeded_message`. The above-256-but-servable class must answer the bare sentence extended with the fixed band clause rather than a formatted template; the step_execution_mode constants backing that template describe no member this model declares. Because every operation that accepts a StepConcurrencyLevel judges the member through that one function, the fix belongs there and not in the ModifyCluster handler, which already classifies and routes correctly.

Traces: emr.modify-cluster-step-concurrency-level-out-of-range-rejected (asserts, after a successful level-0 refusal and a successful in-range change to 4, that level 257 is refused with the range message; the emulator's message is the StepExecutionMode template).

## Evidence
Check: emr.modify-cluster-step-concurrency-level-out-of-range-rejected [mismatch]

Assertion that failed (check_emr_modify_cluster_step_concurrency_level_out_of_range_rejected.py, line 71):
  assert err["Message"] == _RANGE_MESSAGE, f"level 257: message was {err['Message']!r}"
  AssertionError: level 257: message was 'StepConcurrencyLevel 257 exceeds the maximum (256) for StepExecutionMode STANDARD. Set StepExecutionMode to DISTRIBUTED for up to 6400 concurrent steps.'

Request: ModifyCluster(ClusterId=<a live cluster launched with StepConcurrencyLevel=1>, StepConcurrencyLevel=257)

Expected (AWS, recorded behavior):
  HTTP 400
  Error.Code    = "InvalidRequestException"
  Error.Message = "Invalid step concurrency level. Allowed level is from 1 to 256"
    (all fixed text; no part is taken from the request or from state)

Actual (emulator):
  HTTP 400
  Error.Code    = "InvalidRequestException"   (correct)
  Error.Message = "StepConcurrencyLevel 257 exceeds the maximum (256) for StepExecutionMode STANDARD. Set StepExecutionMode to DISTRIBUTED for up to 6400 concurrent steps."
    fixed text of the emulator's template: "StepConcurrencyLevel ", " exceeds the maximum (", ") for StepExecutionMode ", ". Set StepExecutionMode to ", " for up to ", " concurrent steps."
    values substituted in: 257 (the level from the request), 256 (MAX_STEP_CONCURRENCY_LEVEL), STANDARD / DISTRIBUTED / 6400 (emulator-local step_execution_mode constants)

Classes that DO agree in the same check (so the divergence is isolated to this one class):
  StepConcurrencyLevel=0    -> InvalidRequestException "Invalid step concurrency level."  (passes)
  StepConcurrencyLevel=4    -> accepted, echoed in the response, read back by DescribeCluster (passes)
  StepConcurrencyLevel=6401 -> not reached, but is routed to the same bare sentence

Sources in the checkout:
  services/service-com-amazonaws-emr-elasticmapreduce/src/ops/modify_cluster.rs:181-191 — handler calls crate::state::step_concurrency_level_violation(level) and wraps the returned String in InvalidRequestException
  services/service-com-amazonaws-emr-elasticmapreduce/src/state.rs:1366-1376 — step_concurrency_level_violation: returns invalid_step_concurrency_level_message() outside 1..=6400, and step_concurrency_level_exceeded_message(level) for level > 256
  services/service-com-amazonaws-emr-elasticmapreduce/src/state.rs:1346-1355 — step_concurrency_level_exceeded_message: the invented StepExecutionMode format! template
  services/service-com-amazonaws-emr-elasticmapreduce/src/state.rs:1325-1334 — step_execution_mode constants (STANDARD/DISTRIBUTED/6400), a concept no member of this Smithy model exposes

Smithy (services/service-com-amazonaws-emr-elasticmapreduce/smithy-model/com-amazonaws-emr-elasticmapreduce.json):
  ModifyCluster input  = { ClusterId: String (required), StepConcurrencyLevel: Integer, ExtendedSupport: BooleanObject }
  ModifyCluster errors = InvalidRequestException (client, httpError 400), InternalServerError (server, 500)
  StepConcurrencyLevel carries no @range trait; the band 1..256 is stated only in the member's documentation

## Steps to Reproduce
1. Create an EMR service role and an EC2 instance profile (AmazonElasticMapReduceRole / AmazonElasticMapReduceforEC2Role) and a subnet.
2. RunJobFlow with ReleaseLabel=emr-7.1.0, one MASTER instance group (m5.xlarge, count 1), KeepJobFlowAliveWhenNoSteps=true, StepConcurrencyLevel=1.
3. Poll DescribeCluster until Status.State is one of STARTING/BOOTSTRAPPING/RUNNING/WAITING and confirm Cluster.StepConcurrencyLevel == 1.
4. Call ModifyCluster(ClusterId=<id>, StepConcurrencyLevel=0) — observe InvalidRequestException "Invalid step concurrency level." (agrees with AWS).
5. Call ModifyCluster(ClusterId=<id>, StepConcurrencyLevel=4) — observe it is applied and echoed (agrees with AWS).
6. Call ModifyCluster(ClusterId=<id>, StepConcurrencyLevel=257).
   Expected: HTTP 400 InvalidRequestException, Message exactly "Invalid step concurrency level. Allowed level is from 1 to 256".
   Actual:   HTTP 400 InvalidRequestException, Message "StepConcurrencyLevel 257 exceeds the maximum (256) for StepExecutionMode STANDARD. Set StepExecutionMode to DISTRIBUTED for up to 6400 concurrent steps."
7. Re-replay with: emr.modify-cluster-step-concurrency-level-out-of-range-rejected
