Issue-Source: trace-sweep-divergence
Issue-Trace: vpclattice-listener-rule-lifecycle
Issue-Step: task#14
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: vpclattice-listener-rule-lifecycle vpclattice-target-group-lifecycle
Issue-Shape-Id: com.amazonaws.vpclattice#MercuryControlPlane
Issue-Key: vpc-lattice|GetRule|4xx->2xx|$.message
Issue-Key: vpc-lattice|GetRule|4xx->2xx|$.resourceId
Issue-Key: vpc-lattice|GetRule|4xx->2xx|$.resourceType
Issue-Key: vpc-lattice|GetRule|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: vpc-lattice|GetRule|4xx->2xx|status
Issue-Key: vpc-lattice|GetTargetGroup|4xx->2xx|$
Issue-Key: vpc-lattice|GetTargetGroup|4xx->2xx|$.message
Issue-Key: vpc-lattice|GetTargetGroup|4xx->2xx|$.resourceId
Issue-Key: vpc-lattice|GetTargetGroup|4xx->2xx|$.resourceType
Issue-Key: vpc-lattice|GetTargetGroup|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: vpc-lattice|GetTargetGroup|4xx->2xx|status
Issue-Commit: aa6eac2cd77e70e1001710733620f31a870d8297

# vpc-lattice deleted rules and target groups stay readable instead of answering 404

> _Note: this issue was identified against the code base at commit `aa6eac2cd77e70e1001710733620f31a870d8297`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRule

## Description
BUNDLE OF 2 CAUSES, one shared fault: a delete leaves the resource in the store, so every later read still succeeds where AWS answers 404 ResourceNotFoundException. Both must be closed before submitting.

Shared AWS reference: ResourceNotFoundException (Smithy: httpError 404, members `message`, `resourceId`, `resourceType`, ALL required; doc "The request references a resource that does not exist.") is modeled on both GetRule and GetTargetGroup. The error is returned with the x-amzn-errortype header naming it, and its message template is "<ResourceName> <id> cannot be found." — the fixed text is the resource-kind word plus " cannot be found.", and the variable part is the identifier taken from the request; `resourceId` repeats that identifier and `resourceType` is the service's wire token for the kind ("RULE", "TARGET_GROUP"). This template is RECORDED BEHAVIOR for the two recordings below; the model states only the member set and the 404.

Shared Fix: the delete handlers must remove the record from the service's state store (and the read handlers must then report the modeled 404 through the existing not-found construction, which they already do for an unknown identifier). Both DeleteRule and DeleteTargetGroup are still unimplemented scaffolds that answer 501 and mutate nothing; the 501 itself is owned by each operation's own unimplemented-stub issue, while THESE clusters are the observable read divergences the stubs leave behind and no stub issue carries their replay keys (both clusters report stub_path empty and owned_by empty).

Cause 1 of 2 — GetRule after DeleteRule, fixture vpclattice-listener-rule-lifecycle step task#14 (expected HTTP 404, actual HTTP 200).
AWS: once DeleteRule (Smithy: DELETE /services/{serviceIdentifier}/listeners/{listenerIdentifier}/rules/{ruleIdentifier}, success 204) has removed a rule, GetRule on that rule id answers 404 ResourceNotFoundException with message "Rule <ruleId> cannot be found." (fixed text "Rule " + " cannot be found."; <ruleId> is the identifier from the request path), resourceId = that rule id, resourceType = "RULE", and an x-amzn-errortype header naming ResourceNotFoundException.
Emulator: answers HTTP 200 with the deleted rule's body — status, message, resourceId, resourceType and the x-amzn-errortype header are all absent (replay: "status: expected 404, got 200"; "$.message: missing (expected 'Rule rule-birmvby5npf2496f1 cannot be found.', got None)"; "$.resourceId: missing"; "$.resourceType: missing (expected 'RULE')"; "$headers.x-amzn-errortype: missing"). DeleteRule never removed anything.
Why: teardown is unobservable. A client that deletes a rule and polls GetRule to confirm removal never sees it go away, so cleanup loops hang or report failure; a rule a caller believes deleted keeps matching traffic in the emulated listener, and rule-name/priority reuse after delete collides with a record that should be gone.
Fix: DeleteRule's handler (remove the rule from the store, release its name/priority keys for its listener, answer 204) and the rule store in the state module; GetRule's 404 path needs no change.
Traces: vpclattice-listener-rule-lifecycle#task#14; code: services/service-com-amazonaws-vpclattice-mercurycontrolplane/src/ops/delete_rule.rs:173 returns Scaffolded::not_implemented("DeleteRule").

Cause 2 of 2 — GetTargetGroup after DeleteTargetGroup, fixture vpclattice-target-group-lifecycle step task#9 (expected HTTP 404, actual HTTP 200, polled).
AWS: DeleteTargetGroup (Smithy: DELETE /targetgroups/{targetGroupIdentifier}, success 200, output id/arn/status; doc "Deletes a target group. You can't delete a target group if it is used in a listener rule or if the target group creation is in progress.") removes the target group — deletion is asynchronous, so GetTargetGroup first reports status DELETE_IN_PROGRESS and then answers 404 ResourceNotFoundException with message "TargetGroup <id> cannot be found." (fixed text "TargetGroup " + " cannot be found."; <id> from the request), resourceId = that id, resourceType = "TARGET_GROUP", plus the x-amzn-errortype header. The fixture polls GetTargetGroup for the 404 to allow for that transition.
Emulator: answers HTTP 200 with the live target group for all 61 poll attempts over the 60s cap (replay: "$: poll: status 200 not in [404] after 61 attempt(s)", body still {id, arn, name, type: LAMBDA, config, createdAt, lastUpdatedAt, status: ACTIVE, serviceArns: []}), and the error members and x-amzn-errortype header are absent.
Why: worse than a slow delete — the resource never disappears, so a delete-then-recreate or an await-deletion loop never terminates, the account-unique target-group name stays permanently taken, and a test asserting the 404 waits out its whole timeout.
Fix: DeleteTargetGroup's handler (reject deletion while the group is in use by a listener rule or still creating, mark it DELETE_IN_PROGRESS, then remove it and release its name index entry) and the target-group store in the state module; GetTargetGroup's 404 path needs no change.
Traces: vpclattice-target-group-lifecycle#task#9; code: src/ops/delete_target_group.rs:173 returns Scaffolded::not_implemented("DeleteTargetGroup").

Values note: the ids in the recorded messages are one run's values; the requirement is the status 404, the ResourceNotFoundException error code with its x-amzn-errortype header, the message template with the resource-kind word and the requested identifier, and resourceId/resourceType filled from the request and the resource kind.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator_up true; 110 steps, 53 passed, 57 mismatched, 0 errored, 0 blocked; score 48.2%; 22 divergence clusters). Two net-new vpc-lattice mismatch clusters, both with stub_path empty and owned_by empty:

1. GetRule vpclattice-listener-rule-lifecycle#task#14 — expected_status 404, actual_status 200. Detail verbatim:
   status: expected 404, got 200
   $.message: missing (expected 'Rule rule-birmvby5npf2496f1 cannot be found.', got None)
   $.resourceId: missing (expected 'rule-birmvby5npf2496f1', got None)
   $.resourceType: missing (expected 'RULE', got None)
   $headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>', got None)
2. GetTargetGroup vpclattice-target-group-lifecycle#task#9 — expected_status 404, actual_status 200. Detail verbatim:
   status: expected 404, got 200
   $: poll capped at 60s (expected {'message': 'TargetGroup tg-061d6752b311751ac cannot be found.', 'resourceId': 'tg-061d6752b311751ac', 'resourceType': 'TARGET_GROUP'}, got {'id': 'tg-kgr24ej5gqisv72vd', 'arn': '...targetgroup/tg-kgr24ej5gqisv72vd', 'name': 'sim2real-probe-lat-tgl-d18e5c62', 'type': 'LAMBDA', 'config': {'lambdaEventStructureVersion': 'V1'}, 'createdAt': ..., 'lastUpdatedAt': ..., 'status': 'ACTIVE', 'serviceArns': []})
   $.message / $.resourceId / $.resourceType / $headers.x-amzn-errortype: missing
   $: poll: status 200 not in [404] after 61 attempt(s)

Expected (AWS): HTTP 404 with the ResourceNotFoundException error code (x-amzn-errortype header present), message "<Rule|TargetGroup> <requested id> cannot be found.", resourceId equal to the requested identifier and resourceType "RULE"/"TARGET_GROUP".
Actual (emulator): HTTP 200 with the full resource body; no error members and no x-amzn-errortype header. The target-group read stays 200/ACTIVE for the entire 60-second poll window.

Model facts used: ResourceNotFoundException{message,resourceId,resourceType} all required, errorKind client, httpError 404, modeled on both GetRule and GetTargetGroup; DeleteRule http {DELETE /services/{serviceIdentifier}/listeners/{listenerIdentifier}/rules/{ruleIdentifier}, 204}; DeleteTargetGroup http {DELETE /targetgroups/{targetGroupIdentifier}, 200} with output id/arn/status and TargetGroupStatus values ACTIVE|CREATE_FAILED|CREATE_IN_PROGRESS|DELETE_FAILED|DELETE_IN_PROGRESS.
Code: src/ops/delete_rule.rs:173 and src/ops/delete_target_group.rs:173 both return crate::scaffolded::Scaffolded::not_implemented(..) and touch no state (each file still carries the '//! SCAFFOLDED, NOT IMPLEMENTED' marker), which is why the reads keep succeeding. The 501 answers themselves are tracked by those operations' own stub issues; these two clusters are the read-path divergences they leave behind.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixtures vpclattice-listener-rule-lifecycle and vpclattice-target-group-lifecycle (replay_wire_corpus); representative step vpclattice-listener-rule-lifecycle#task#14, other bundled step vpclattice-target-group-lifecycle#task#9.
2. Equivalent by hand against the vpc-lattice endpoint:
   a. POST /services, POST /services/{svcId}/listeners, POST /services/{svcId}/listeners/{listenerId}/rules -> note the rule id.
   b. DELETE /services/{svcId}/listeners/{listenerId}/rules/{ruleId}: the emulator answers 501 NotImplemented instead of 204.
   c. GET /services/{svcId}/listeners/{listenerId}/rules/{ruleId} (task#14): the emulator answers 200 with the rule, where AWS answers 404 ResourceNotFoundException with message "Rule {ruleId} cannot be found.", resourceId {ruleId}, resourceType "RULE" (Cause 1).
   d. POST /targetgroups with type LAMBDA -> note the target-group id; DELETE /targetgroups/{tgId}: the emulator answers 501 instead of 200 with status DELETE_IN_PROGRESS.
   e. Poll GET /targetgroups/{tgId} (task#9): it keeps answering 200 with status ACTIVE indefinitely, where AWS answers 404 ResourceNotFoundException with message "TargetGroup {tgId} cannot be found.", resourceId {tgId}, resourceType "TARGET_GROUP" (Cause 2).
3. Confirm the fix by re-replaying vpclattice-listener-rule-lifecycle#task#14 and vpclattice-target-group-lifecycle#task#9; both clusters must come back clean.
