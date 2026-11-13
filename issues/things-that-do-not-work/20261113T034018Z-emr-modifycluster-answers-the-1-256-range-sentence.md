Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr.modify-cluster-step-concurrency-level-out-of-range-rejected
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: emr.modify-cluster-step-concurrency-level-out-of-range-rejected|run#11|AssertionError|assert err["Message"] == _BARE_MESSAGE, f"level 0: message was {err['Message']!r}"
Issue-Commit: 0b0f0ac3b235f261a04cb4a241170b0b9be79526

# EMR ModifyCluster answers the 1..256 range sentence for a StepConcurrencyLevel outside every band (0, or above 6400) instead of AWS's bare 'Invalid step concurrency level.'

> _Note: this issue was identified against the code base at commit `0b0f0ac3b235f261a04cb4a241170b0b9be79526`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyCluster

## Description
Cause 1 of 1: ModifyCluster refuses a StepConcurrencyLevel that lies outside EVERY band Amazon EMR offers (0 or below, and above 6400) with the right HTTP status and error code but with the WRONG one of EMR's two messages — it emits the band-stating sentence that belongs to the other refusal class. Failing check: emr.modify-cluster-step-concurrency-level-out-of-range-rejected.

AWS: ModifyCluster takes StepConcurrencyLevel as a plain Integer with no @range trait; the band is stated only in the member's documentation ("You can specify a minimum of 1 step and a maximum of 256 steps."). Smithy: ModifyCluster input = { ClusterId: String (required), StepConcurrencyLevel: Integer, ExtendedSupport: BooleanObject }; errors = InvalidRequestException (client, httpError 400), InternalServerError (server, 500). The model declares no message text, so the wording is recorded behavior only: Amazon EMR answers TWO distinct messages under the single code InvalidRequestException, HTTP 400, and the class a level falls into decides which. A level outside every servable band — 0 or below, and anything above 6400 — gets the BARE sentence, whose text is entirely fixed with no variable part at all: "Invalid step concurrency level." A level ABOVE 256 but still inside 1..6400 gets that same sentence EXTENDED with the band clause: "Invalid step concurrency level. Allowed level is from 1 to 256" — also entirely fixed text, where the two figures are the service's documented band (its enforced minimum and maximum), not values taken from the request or from state. Neither message echoes the rejected level, and no part of either is per-run.

Emulator: for StepConcurrencyLevel=0 the emulator returns HTTP 400 InvalidRequestException with Message "Invalid step concurrency level. Allowed level is from 1 to 256" — the extended, band-stating sentence that AWS reserves for the 257..6400 class. The same producer serves the above-6400 class, so ModifyCluster(StepConcurrencyLevel=6401) diverges identically; the check asserts that class too but never reaches it, because the level-0 assertion fails first. The bare sentence is not produced anywhere in this service: the out-of-every-band class takes the extended text, and the 257..6400 class takes a self-invented StepExecutionMode template (that second class is the distinct cause already carried by open issue 20261111T193933Z-emr-modifycluster-invents-a-stepexecutionmode-sentence-for-a.md, whose replay key is the level-257 assertion; it does not cover this level-0/above-6400 class, which is why this check is still failing and unowned). The status (400), the error code (InvalidRequestException), the in-range positive control (level 4 is applied, echoed in the response and read back by DescribeCluster) and the post-refusal invariant (the previously applied level survives a refusal) all agree with AWS, so the divergence is confined to the message of the outside-every-band class.

Why: a client that branches, retries, logs or asserts on EMR's refusal text cannot tell the two step-concurrency refusals apart, because the emulator collapses them onto the band-stating wording. A caller that asked for level 0 (or a level above 6400) is told "Allowed level is from 1 to 256", which AWS only says when the requested level was actually above 256 — so a client's own validation or user-facing message is driven by a sentence AWS never emits for that request, and any test distinguishing EMR's two messages under InvalidRequestException passes against AWS and fails against the emulator (or vice versa).

Fix: the message producer in this service's shared validation state, not the ModifyCluster handler (which classifies and routes correctly). crate::state::step_concurrency_level_violation routes a level outside MIN..=6400 to invalid_step_concurrency_level_message(), and that function formats the band clause into the sentence; it must return the bare fixed sentence for this class, leaving the band clause to the above-256-but-servable class. Because every operation that accepts a StepConcurrencyLevel judges the member through that one function, the fix belongs there so RunJobFlow and ModifyCluster stay consistent. File and line citations are under Traces.

Traces: emr.modify-cluster-step-concurrency-level-out-of-range-rejected — after confirming the launched cluster reports StepConcurrencyLevel 1, it calls ModifyCluster(StepConcurrencyLevel=0) and asserts the bare message; the emulator answers the band sentence. Sources in the checkout: services/service-com-amazonaws-emr-elasticmapreduce/src/ops/modify_cluster.rs (wraps the returned String in InvalidRequestException); services/service-com-amazonaws-emr-elasticmapreduce/src/state.rs:1428-1438 (step_concurrency_level_violation, the class routing); services/service-com-amazonaws-emr-elasticmapreduce/src/state.rs:1372-1377 (invalid_step_concurrency_level_message, which formats MIN/MAX into the sentence); services/service-com-amazonaws-emr-elasticmapreduce/src/state.rs:1408-1417 (step_concurrency_level_exceeded_message, the other class's producer).

## Evidence
Check: emr.modify-cluster-step-concurrency-level-out-of-range-rejected [mismatch]

Assertion that failed (check_emr_modify_cluster_step_concurrency_level_out_of_range_rejected.py, line 59):
  assert err["Message"] == _BARE_MESSAGE, f"level 0: message was {err['Message']!r}"
  AssertionError: level 0: message was 'Invalid step concurrency level. Allowed level is from 1 to 256'

Request: ModifyCluster(ClusterId=<a live cluster launched with StepConcurrencyLevel=1>, StepConcurrencyLevel=0)

Expected (AWS, recorded behavior):
  HTTP 400
  Error.Code    = "InvalidRequestException"
  Error.Message = "Invalid step concurrency level."
    entirely fixed text — no part is taken from the request or from state

Actual (emulator):
  HTTP 400                                   (correct)
  Error.Code    = "InvalidRequestException"  (correct)
  Error.Message = "Invalid step concurrency level. Allowed level is from 1 to 256"
    fixed text of the emulator's template: "Invalid step concurrency level. Allowed level is from ", " to "
    values substituted in: 1 (MIN_STEP_CONCURRENCY_LEVEL) and 256 (MAX_STEP_CONCURRENCY_LEVEL) — the service's own enforced band, not values from this request
  This is the message AWS emits only for a level ABOVE 256 but inside 1..6400.

The same producer serves the above-6400 class, so ModifyCluster(StepConcurrencyLevel=6401) is expected to diverge the same way; the check asserts it at line 69 but never reaches that line.

Classes that DO agree in the same check (the divergence is isolated to the outside-every-band message):
  StepConcurrencyLevel=4 -> accepted, echoed in the ModifyCluster response, read back by DescribeCluster
  DescribeCluster still reports the last accepted level after a refusal

Related but distinct, already open: 20261111T193933Z-emr-modifycluster-invents-a-stepexecutionmode-sentence-for-a.md covers the 257..6400 class (an invented "StepExecutionMode" sentence). Its replay key is the level-257 assertion; it does not carry the level-0 / above-6400 class reported here, and the suite did not mark this check as owned by it.

Smithy (services/service-com-amazonaws-emr-elasticmapreduce/smithy-model/com-amazonaws-emr-elasticmapreduce.json):
  ModifyCluster input  = { ClusterId: String (required), StepConcurrencyLevel: Integer, ExtendedSupport: BooleanObject }
  ModifyCluster errors = InvalidRequestException (client, httpError 400), InternalServerError (server, 500)
  StepConcurrencyLevel carries no @range trait; the band 1..256 appears only in the member's documentation

## Steps to Reproduce
1. Create an EMR service role and an EC2 instance profile (AmazonElasticMapReduceRole / AmazonElasticMapReduceforEC2Role) and a subnet.
2. RunJobFlow with ReleaseLabel=emr-7.1.0, one MASTER instance group (m5.xlarge, count 1), KeepJobFlowAliveWhenNoSteps=true, StepConcurrencyLevel=1.
3. Poll DescribeCluster until Status.State is one of STARTING/BOOTSTRAPPING/RUNNING/WAITING and confirm Cluster.StepConcurrencyLevel == 1.
4. Call ModifyCluster(ClusterId=<id>, StepConcurrencyLevel=0).
   Expected: HTTP 400, Error.Code "InvalidRequestException", Error.Message exactly "Invalid step concurrency level."
   Actual:   HTTP 400, Error.Code "InvalidRequestException", Error.Message "Invalid step concurrency level. Allowed level is from 1 to 256"
5. Confirm DescribeCluster still reports StepConcurrencyLevel 1 (this part agrees).
6. Call ModifyCluster(ClusterId=<id>, StepConcurrencyLevel=4) — applied and echoed (agrees).
7. Call ModifyCluster(ClusterId=<id>, StepConcurrencyLevel=6401) — same outside-every-band class.
   Expected: HTTP 400 InvalidRequestException, Message exactly "Invalid step concurrency level."
   Actual:   the band sentence "Invalid step concurrency level. Allowed level is from 1 to 256".
8. Re-replay with: emr.modify-cluster-step-concurrency-level-out-of-range-rejected
