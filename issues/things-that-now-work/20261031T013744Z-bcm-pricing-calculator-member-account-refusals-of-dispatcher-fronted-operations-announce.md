Issue-Source: trace-sweep-divergence
Issue-Trace: bcm-pricing-calculator-bill-scenario-estimate-member-account-restricted
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bcm-pricing-calculator-bill-scenario-estimate-member-account-restricted
Issue-Shape-Id: com.amazonaws.bcmpricingcalculator#AWSBCMPricingCalculator
Issue-Key: bcm-pricing-calculator|CreateBillScenario|4xx->4xx|$headers.content-type
Issue-Key: bcm-pricing-calculator|GetBillScenario|4xx->4xx|$headers.content-type
Issue-Key: bcm-pricing-calculator|DeleteBillScenario|4xx->4xx|$headers.content-type
Issue-Key: bcm-pricing-calculator|ListBillScenarios|4xx->4xx|$headers.content-type
Issue-Key: bcm-pricing-calculator|ListBillScenarioUsageModifications|4xx->4xx|$headers.content-type
Issue-Key: bcm-pricing-calculator|UpdatePreferences|4xx->4xx|$headers.content-type
Issue-Key: bcm-pricing-calculator|BatchCreateBillScenarioUsageModification|4xx->4xx|$headers.content-type
Issue-Commit: 63c612bdac710916216a1ca8dd5dbf0b76505045

# Member-account refusals of dispatcher-fronted operations announce content-type application/x-amz-json-1.0 instead of 1.1

> _Note: this issue was identified against the code base at commit `63c612bdac710916216a1ca8dd5dbf0b76505045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBillScenario

## Description
Replaying simparity-wire-aws-v1 produces 10 divergence clusters, all on bcm-pricing-calculator. SEVEN of them are this one fault, reached through seven different operations: when a RESTRICTED MEMBER ACCOUNT calls a dispatcher-fronted operation, the 400 refusal body is byte-for-byte right (status, error code and message all match the recording) but the response is announced under the WRONG media type. A fixer must close all seven causes below; they share one seam, so one scoped change closes them, and each must re-replay clean.

The general rule, stated once and then enumerated per cause:

AWS: the model declares aws.protocols#awsJson1_0 (smithy_query query="service_info" on services/service-com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator/smithy-model/com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator.json -> service AWSBCMPricingCalculator, version 2024-06-19, protocols [awsJson1_0], sigv4Name bcm-pricing-calculator), which IMPLIES application/x-amz-json-1.0. The media type actually announced on this refusal class is recorded behavior only: every recorded member-account refusal of a workload-estimate / bill-scenario / bill-estimate / preferences operation carries `content-type: application/x-amz-json-1.1`, i.e. the account-class refusal is announced by the same front door that announces the successes and the state errors of those operations, NOT by the request validator in front of it. The corpus is explicit that this is a per-refusal-CLASS fact and not a blanket flip: in the same corpus the boundary field-validation refusals of bcm-pricing-calculator-validation-error-paths (task#1/#2/#3) are recorded under application/x-amz-json-1.0 and now MATCH the emulator on that header, and the resource-tagging family is recorded under 1.0 throughout. Whatever is changed here must leave those at 1.0.

Emulator: every one of these seven refusals is announced `application/x-amz-json-1.0`. The cause is visible in the checkout: services/service-com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator/src/service.rs:164 builds DISPATCHER = ResponseMediaTypeOperation::announcing(DISPATCHER_RESPONSE_MEDIA_TYPE, not_a_validation_refusal), and services/common/src/response_media_type.rs:218-248 defines that predicate as "4xx whose awsJson envelope names ValidationException in __type is the validator's". The account-class gate (the member-account restriction, whose reason value src/service.rs:141-145 describes as rendered by crate::state::gate_reason) answers as ValidationException too, so the predicate buckets a FRONT-DOOR refusal with the VALIDATOR's refusals and leaves the protocol-implied 1.0 in place. The error code in __type is the wrong discriminator for this service: the two announcers are told apart by WHICH refusal it is (a modelled input-constraint violation vs. an account-class refusal of an otherwise well-formed request), not by the shape name both of them use.

Why: a client, a recording proxy and SDK middleware all select the JSON protocol decoder off the response content-type. On this refusal class the emulator describes its body as a protocol other than the one AWS announced, so strict clients and protocol-conformance tooling mis-decode or reject a refusal they handle fine against AWS, and a parity test that diffs response headers fails on every member-account path. It is systemic: it reaches all 33 dispatcher-fronted operations, not just the seven replayed.

Fix: the service's announcer classification, not the status/code/body. In services/service-com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator/src/service.rs (the DISPATCHER predicate at line 164-165) and src/service/response_media_type.rs (which states which responses the dispatcher owns): the predicate must put only the MODELLED INPUT-CONSTRAINT refusal — the one this crate's own ModelledValidation path renders, reason fieldValidationFailed — in the validator's 1.0 bucket, and leave the account-class gate's refusal in the dispatcher's 1.1 bucket. services/common/src/response_media_type.rs::not_a_validation_refusal reads only __type, so it cannot express that distinction; either this service states its own predicate or the shared one is given the distinction. No status, code, message or body byte changes. Two RESOLVED issues bracket this seam and neither covers it: 20261023T091713Z-bcm-pricing-calculator-responses-carry-content-type-application-x-amz-json and 20261029T170742Z-bcm-pricing-calculator-announces-awsjson1-1-on-every (which installed the current all-ValidationException-is-1.0 rule). Do not resolve this by flipping the service back to a single announcer — that re-opens the fixed one.

Cause 1 of 7: CreateBillScenario — fixture bcm-pricing-calculator-bill-scenario-estimate-member-account-restricted, step task#1. 400 refusal announced application/x-amz-json-1.0; recording announces application/x-amz-json-1.1.
Cause 2 of 7: GetBillScenario — same fixture, step task#3. Same divergence.
Cause 3 of 7: DeleteBillScenario — same fixture, step task#7. Same divergence.
Cause 4 of 7: ListBillScenarios — same fixture, step task#9. Same divergence.
Cause 5 of 7: ListBillScenarioUsageModifications — same fixture, step task#12. Same divergence.
Cause 6 of 7: UpdatePreferences — same fixture, step task#17. Same divergence.
Cause 7 of 7: BatchCreateBillScenarioUsageModification — same fixture, step task#18. Same divergence.

Traces (examples of the one defect, one per cause): bcm-pricing-calculator-bill-scenario-estimate-member-account-restricted#task#1, #task#3, #task#7, #task#9, #task#12, #task#17, #task#18.

## Evidence
Replay summary: trace_set_id simparity-wire-aws-v1, emulator_up true, bootstrap_signal false, steps_total 64, steps_passed 38, steps_mismatched 26, steps_errored 0, steps_blocked 0, score_pct 59.4, divergence_count 10 — all 10 on service bcm-pricing-calculator, verdict mismatch, every one with empty stub_path and empty owned_by.

The seven clusters of this fault, each expected_status 400 == actual_status 400, body matching, one diverging key:
  $headers.content-type: value_mismatch (expected 'application/x-amz-json-1.1', got 'application/x-amz-json-1.0')
with divergence_key / fixture#step:
  service=bcm-pricing-calculator|CreateBillScenario|status=400->400|keys=$headers.content-type — bcm-pricing-calculator-bill-scenario-estimate-member-account-restricted#task#1
  service=bcm-pricing-calculator|GetBillScenario|status=400->400|keys=$headers.content-type — ...#task#3
  service=bcm-pricing-calculator|DeleteBillScenario|status=400->400|keys=$headers.content-type — ...#task#7
  service=bcm-pricing-calculator|ListBillScenarios|status=400->400|keys=$headers.content-type — ...#task#9
  service=bcm-pricing-calculator|ListBillScenarioUsageModifications|status=400->400|keys=$headers.content-type — ...#task#12
  service=bcm-pricing-calculator|UpdatePreferences|status=400->400|keys=$headers.content-type — ...#task#17
  service=bcm-pricing-calculator|BatchCreateBillScenarioUsageModification|status=400->400|keys=$headers.content-type — ...#task#18

Scoping evidence (why this is not a blanket flip): the other three clusters in the same replay — bcm-pricing-calculator-validation-error-paths#task#1/#task#2/#task#3 — diverge ONLY on $.fieldList[0].message and report NO content-type key, i.e. those boundary field-validation refusals are recorded under application/x-amz-json-1.0 and the emulator already matches. Tagging-operation steps pass outright.

Model grounding (smithy_query query="service_info", model_path services/service-com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator/smithy-model/com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator.json): protocols [awsJson1_0]; common_errors AccessDeniedException, InternalServerException, ThrottlingException, ValidationException; 36 bound operations.

Code grounding: src/service.rs:164-165 (DISPATCHER = ResponseMediaTypeOperation::announcing(DISPATCHER_RESPONSE_MEDIA_TYPE, not_a_validation_refusal)); src/service/response_media_type.rs:70-83 (the 1.0->1.1 pair and the dispatcher-fronted operation list — all 33 non-tagging operations); services/common/src/response_media_type.rs:218-248 (is_validation_refusal reads 4xx + __type == ValidationException; not_a_validation_refusal is its negation). src/service.rs:141-145 records that the account-class gate is a producer distinct from the modelled-constraint layer.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Replay fixture bcm-pricing-calculator-bill-scenario-estimate-member-account-restricted and inspect the response headers of steps task#1 (CreateBillScenario), task#3 (GetBillScenario), task#7 (DeleteBillScenario), task#9 (ListBillScenarios), task#12 (ListBillScenarioUsageModifications), task#17 (UpdatePreferences) and task#18 (BatchCreateBillScenarioUsageModification): each is refused 400 with the recorded body, but content-type reads application/x-amz-json-1.0 where the recording has application/x-amz-json-1.1.
4. Equivalently: call any dispatcher-fronted operation (any operation except ListTagsForResource/TagResource/UntagResource) as a member account the account-class gate restricts, and read the content-type of the 400.
5. Regression control, which must keep announcing 1.0: replay bcm-pricing-calculator-validation-error-paths task#1/#2/#3 (modelled @pattern refusals) and the bcm-pricing-calculator-workload-estimate-tagging steps.
6. After fixing, re-replay bcm-pricing-calculator-bill-scenario-estimate-member-account-restricted (all seven steps) AND bcm-pricing-calculator-validation-error-paths plus the tagging fixture, so the scoped change is confirmed not to have flipped the 1.0 announcers.
