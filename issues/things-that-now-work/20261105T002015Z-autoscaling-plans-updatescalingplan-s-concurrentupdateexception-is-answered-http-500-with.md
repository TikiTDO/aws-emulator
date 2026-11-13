Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling-plans.asp-concurrent-update-is-rejected
Issue-Shape-Id: com.amazonaws.autoscalingplans#AnyScaleScalingPlannerFrontendService
Issue-Key: autoscaling-plans.asp-concurrent-update-is-rejected|run#23|AssertionError|assert status == 400, f"expected status 400, got {status!r}"
Issue-Commit: 0a8b28aea83ccc3190789fc779439d6e199d02f9

# UpdateScalingPlan's ConcurrentUpdateException is answered HTTP 500 with emulator-authored wording instead of AWS's HTTP 400 "You already have a pending update to the scaling plan"

> _Note: this issue was identified against the code base at commit `0a8b28aea83ccc3190789fc779439d6e199d02f9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateScalingPlan

## Description
The emulator DOES serialise mutations on a scaling plan and DOES raise ConcurrentUpdateException when an UpdateScalingPlan overlaps a still-pending transition — the error code is right. Two things about that refusal diverge from AWS. This issue has 2 causes.

Cause 1 of 2: the overlapping-update refusal is answered with HTTP status 500 instead of AWS's 400.
AWS: recorded behavior only for the status. Smithy (services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/smithy-model/com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice.json): UpdateScalingPlan binds ConcurrentUpdateException, which the model classifies @error("server") with httpError 500 and awsQueryError httpResponseCode 500 (doc: "Concurrent updates caused an exception, for example, if you request an update to a scaling plan that already has a pending update."). The recording shows the live service does NOT follow that classification for this refusal: an UpdateScalingPlan issued while the previous update is still pending comes back with error code ConcurrentUpdateException at HTTP status 400 — i.e. AWS treats it as a client error the caller caused, and the modelled 500 is a model-vs-service discrepancy the emulator must resolve in favour of the recording.
Emulator: error code ConcurrentUpdateException (correct) but HTTP status 500, because the refusal is minted as the modelled server-side error and the serializer takes the status straight from the model's httpError.
Why: 500 is the retriable-server-fault class. The AWS SDKs' default retry policy retries a 500 with backoff and surfaces it as a service-side failure, whereas a 400 ConcurrentUpdateException is returned to the caller immediately as a terminal client error. A client that serialises its own plan mutations against AWS's 400 (back off, re-read the plan status, retry its own write) instead sees the emulator's call silently retried by the SDK and then reported as an AWS outage; error-class-based branching (is_client_error / 4xx vs 5xx) takes the wrong branch, and anything that pages on 5xx rates fires on ordinary contention.
Fix: UpdateScalingPlan's error channel / the status the ConcurrentUpdateException refusal is serialized with — the handler's `concurrent_update` helper and the modelled-error-to-status mapping it feeds must answer 400 for this refusal rather than inheriting the model's httpError 500. The same refusal is raised from the shared admission rule, so every mutating operation that mints ConcurrentUpdateException from it (DeleteScalingPlan as well as UpdateScalingPlan) needs the same status.
Traces: autoscaling-plans.asp-concurrent-update-is-rejected (assertion "expected status 400, got 500").

Cause 2 of 2: the refusal message is emulator-authored prose, not AWS's sentence for this error class.
AWS: recorded behavior only. For an update that overlaps a pending update, AWS's ConcurrentUpdateException message is the fixed sentence "You already have a pending update to the scaling plan" — the whole sentence is fixed text and interpolates nothing: no plan name, no status code, no verb.
Emulator: the message is built from a template that interpolates request and state values — "The scaling plan '<ScalingPlanName>' is in status <StatusCode> and cannot be <verb>" (crate::state::plan_in_progress_message), e.g. for an update blocked by a pending update it reads "The scaling plan '<name>' is in status UpdateInProgress and cannot be updated". The plan name comes from the request and the status code from the stored record; neither belongs in AWS's text.
Why: the message is part of the contract for an error class clients routinely discriminate on. A caller or test suite matching AWS's sentence finds no match, and an operator reading the emulator's message is shown wording AWS never emits. Because Cause 1 fails the status assertion first, this wrong text is reached by the same check as soon as the status is corrected, so fixing only the status leaves the check failing.
Fix: the message source the mutating handlers raise their in-progress refusal from (crate::state::plan_in_progress_message, reached via admit_plan_mutation / PlanMutationRefusal::InProgress) — replace the name/status/verb template with AWS's fixed sentence for this refusal.
Traces: autoscaling-plans.asp-concurrent-update-is-rejected (assertion "unexpected message ...", immediately after the status assertion).

Not asserted as requirements anywhere above: the generated plan and table names in the check, and any request id or clock value in the recorded envelope.

No open issue owns this. The open autoscaling-plans issues concern other code paths: 20261022T175157Z (plan lifecycle has no Application Auto Scaling side effects), 20261029T193125Z (empty-ScalingInstructions wording, derived PolicyName, update version), 20261030T033853Z x2 (scalingPlanName pattern + NextToken wording; LastMutatingRequestTime + policy names), 20261102T001453Z and 20261102T033919Z (DescribeScalingPlanResources' ScalingPlanVersion gate). None of them touches the ConcurrentUpdateException status or its message.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator booted from this checkout: 2/10 passed; all 8 failures in autoscaling-plans, 7 of them already OWNED by open issues. The one surviving failure:

[mismatch] autoscaling-plans.asp-concurrent-update-is-rejected
  File "check_autoscaling_plans_asp_concurrent_update_is_rejected.py", line 148, in run
    assert status == 400, f"expected status 400, got {status!r}"
  AssertionError: expected status 400, got 500

The check's preceding assertion — `code == "ConcurrentUpdateException"` — PASSED, so the emulator does detect the overlap and does pick the right error shape; only the HTTP status it is serialized with is wrong (Cause 1). The positive control also passed (at least one UpdateScalingPlan in the loop was accepted), so the refusal was a genuine concurrency refusal and not a blanket failure.

Cause 1, expected vs actual:
  error code: ConcurrentUpdateException vs ConcurrentUpdateException (match)
  HTTP status: expected 400, got 500
Model evidence: UpdateScalingPlan errors = ConcurrentUpdateException(server, httpError 500), InternalServiceException(server, 500), ObjectNotFoundException(client, 400), ValidationException(client, 400). The emulator inherits the modelled 500; the recording shows AWS answers 400.
Emulator source (services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/ops/update_scaling_plan.rs): lines 221-226 map PlanMutationRefusal::InProgress to `concurrent_update(message)`; lines 271-278 define that helper as the modelled ConcurrentUpdateException, documented in-file as "(HTTP 500)" (also lines 95-96 of the module header).

Cause 2, expected vs actual message for the same refusal:
  expected: 'You already have a pending update to the scaling plan' (fixed text in full; no variable parts)
  actual:   'The scaling plan '<ScalingPlanName>' is in status UpdateInProgress and cannot be updated' — fixed parts "The scaling plan '", "' is in status ", " and cannot be "; variable parts the request's own ScalingPlanName, the stored record's status_code, and the mutation verb.
Emulator source: services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/state.rs lines 1789-1794 (`plan_in_progress_message`), raised from `admit_plan_mutation` at lines 626-636. The check asserts the AWS sentence at its line 149 (source line 602 of the quoted listing), reached as soon as Cause 1 is fixed.

## Steps to Reproduce
1. Boot the emulator from this checkout and point an autoscaling-plans client (plus dynamodb and application-autoscaling clients) at it.
2. Create a provisioned DynamoDB table tagged so a scaling plan can select it, and wait for it to exist.
3. CreateScalingPlan with that tag filter as ApplicationSource and one target-tracking instruction on dynamodb:table:ReadCapacityUnits; poll DescribeScalingPlans until StatusCode == 'Active'.
4. Issue two UpdateScalingPlan calls (ScalingPlanVersion=1, one instruction, a changed TargetValue) back to back with no wait between them, so the second overlaps the first's still-pending UpdateInProgress transition. Repeat the pair until one call is refused.
5. Observe the refusal: Error.Code == 'ConcurrentUpdateException' (correct) but ResponseMetadata.HTTPStatusCode == 500; expected 400 (Cause 1).
6. Read Error.Message on that refusal: it is "The scaling plan '<plan>' is in status UpdateInProgress and cannot be updated"; expected the fixed sentence "You already have a pending update to the scaling plan" (Cause 2).
7. Confirm the refusal did not damage the plan (it should settle Active at version 1 with 1 instruction) — that part already holds.
8. Re-replay autoscaling-plans.asp-concurrent-update-is-rejected; both causes must pass before this issue is closed.
