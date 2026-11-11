Issue-Source: trace-sweep-divergence
Issue-Trace: arc-zonal-shift-practice-run-configuration-validation-errors
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: arc-zonal-shift-practice-run-configuration-validation-errors
Issue-Shape-Id: com.amazonaws.arczonalshift#PercDataPlane
Issue-Key: arc-zonal-shift|CreatePracticeRunConfiguration|4xx->4xx|$.Message
Issue-Commit: 92964a3563e046b76198eacfa7b4e785275ebde9

# arc-zonal-shift CreatePracticeRunConfiguration 403 omits the not-a-managed-resource Message

> _Note: this issue was identified against the code base at commit `92964a3563e046b76198eacfa7b4e785275ebde9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePracticeRunConfiguration

## Description
One net-new divergence on the arc-zonal-shift CreatePracticeRunConfiguration access-denied path. The HTTP status is correct (403), but the emulator's error document does not carry the message member the recorded service populates, so the reason for the refusal is lost. This bundle has 1 cause; it must be closed.

Cause 1 of 1: CreatePracticeRunConfiguration 403 body carries no Message member (fixture arc-zonal-shift-practice-run-configuration-validation-errors, step task#2).
AWS: Smithy: CreatePracticeRunConfiguration (POST /configuration, success code 201) binds AccessDeniedException (errorKind client, httpError 403), a structure whose only modeled member is `message` (doc: "You do not have sufficient access to perform this action."). Its required input member is `resourceIdentifier` (target ResourceIdentifier, @length min 8 max 1024). Recorded behavior only for the wire casing and the text: when the resource named by `resourceIdentifier` is not registered as an ARC managed resource for the calling account, the service answers 403 with the text at top-level member `Message` (capital M). The message is templated: its FIXED text is "Resource " + " is not a managed resource for account ID " + ".", and its VARIABLE parts are (a) the resource ARN taken verbatim from the request's `resourceIdentifier` and (b) the caller's AWS account ID taken from the emulator's credential/account state. The ELBv2 ARN and the account id 000000000000 in the recording are one run's values filling that template, not part of the requirement.
Emulator: answers 403 but with no `Message` member at all (replay: "$.Message: missing (expected 'Resource arn:aws:elasticloadbalancing:us-west-2:000000000000:loadbalancer/app/<name>/<id> is not a managed resource for account ID 000000000000.', got None)"). The refusal text, if emitted at all, is not under the member the service populates.
Why: a client that reads the access-denied reason out of the field the service actually fills (`Message`) gets nothing - the failure surfaces as a bare 403 with no explanation - so an operator cannot tell "this load balancer is not an ARC managed resource" from a genuine IAM authorization denial, and retry/remediation logic keyed on that text never fires.
Fix: the arc-zonal-shift CreatePracticeRunConfiguration handler's managed-resource precondition check and the AccessDeniedException error serializer it returns through, so the 403 document carries the templated text under the recorded member name.

Related but NOT covering this cause: open issue 20261029T170702Z-arc-zonal-shift-accessdeniedexception-403-body-does-not.md covers the 403 bodies of StartZonalShift/CancelZonalShift/UpdateZonalShift and the different fixed text "Requested resource is not available."; open issue 20261029T193235Z-arc-zonal-shift-accessdenied-message-on-unknown-shift.md covers the unknown-shift-id AccessDenied message. Neither names CreatePracticeRunConfiguration or this not-a-managed-resource message, and neither carries this fixture/step as its replay key (this cluster's owned_by is empty).

Traces: arc-zonal-shift-practice-run-configuration-validation-errors#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator_up=true, score_pct 55.9, 11 divergence clusters).

Cluster: service=arc-zonal-shift|CreatePracticeRunConfiguration|status=403->403|keys=$.Message
  fixture_id: arc-zonal-shift-practice-run-configuration-validation-errors
  step_key:   task#2
  verdict:    mismatch
  expected_status 403, actual_status 403
  detail: $.Message: missing (expected 'Resource arn:aws:elasticloadbalancing:us-west-2:000000000000:loadbalancer/app/sim2real-probe-unmanage-f3ffe7bc/1111111111111111 is not a managed resource for account ID 000000000000.', got None)
  affected_count: 1
  stub_path: (none) - the operation IS implemented
  owned_by: (none) - no open issue carries this replay key

Requirement decomposition of the recorded message: fixed text = "Resource <ARN> is not a managed resource for account ID <accountId>."; variable part 1 = the ARN echoed from the request member `resourceIdentifier` (the generated load-balancer name and id in the recording are per-run values); variable part 2 = the caller's account id from emulator state.

Model evidence (services/service-com-amazonaws-arczonalshift-percdataplane/smithy-model/com-amazonaws-arczonalshift-percdataplane.json): CreatePracticeRunConfiguration POST /configuration code 201, errors include AccessDeniedException (client, httpError 403); AccessDeniedException is a client error with httpError 403 and the single member `message`.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
2. Cause 1: drive CreatePracticeRunConfiguration (POST /configuration) with a `resourceIdentifier` naming an ELBv2 load balancer that is NOT registered as an ARC managed resource for the calling account - fixture arc-zonal-shift-practice-run-configuration-validation-errors, step task#2.
3. Observe the response: status 403 is correct, but the JSON body has no top-level `Message` member.
4. Expected: a 403 whose body carries, at top-level member `Message`, "Resource <the resourceIdentifier ARN from the request> is not a managed resource for account ID <the caller's account id>."
5. Re-replay that fixture step; it must match for this issue to be closed.
