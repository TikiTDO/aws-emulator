Issue-Source: trace-sweep-divergence
Issue-Trace: compute-optimizer-automation-get-automation-rule-malformed-arn-access-denied
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: compute-optimizer-automation-get-automation-rule-malformed-arn-access-denied
Issue-Shape-Id: com.amazonaws.computeoptimizerautomation#ComputeOptimizerAutomationService
Issue-Key: compute-optimizer-automation|GetAutomationRule|4xx->4xx|$.message
Issue-Commit: 2d11c019ca89f17fce399c4a7a8c59c866e2a978

# GetAutomationRule access-denied message is a canned sentence instead of the IAM denial built from caller, action and resource

> _Note: this issue was identified against the code base at commit `2d11c019ca89f17fce399c4a7a8c59c866e2a978`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAutomationRule

## Description
Cause 1 of 1: GetAutomationRule called with a ruleArn that does not match the automation-rule ARN pattern is rejected 403 AccessDeniedException with the correct code and status, but the emulator serves a fixed, canned sentence in $.message where AWS serves the standard IAM denial message assembled from the request.

AWS: Smithy (services/service-com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice/smithy-model/com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice.json): GetAutomationRule input member ruleArn is required with pattern ^arn:aws:compute-optimizer::[0-9]{12}:automation-rule/[a-zA-Z0-9_-]+$; GetAutomationRule models AccessDeniedException (client, httpError 403) alongside InvalidParameterValueException (400) and ResourceNotFoundException (404). Recorded behavior: a ruleArn that does not match that pattern at all fails authorization/resource-pattern matching before any resource lookup and is rejected 403 AccessDeniedException (not InvalidParameterValueException) with the standard IAM denial message. That message's FIXED text is 'User: ' / ' is not authorized to perform: ' / ' on resource: '; its VARIABLE parts are (a) the caller's principal ARN taken from the signing credential, (b) the operation's IAM action name for this service, aco-automation:GetAutomationRule, and (c) the ruleArn exactly as the request sent it. In the recorded step those variables resolved to the emulator's default principal, the action aco-automation:GetAutomationRule and the resource string the request carried — those three concrete values are per-request/per-environment fill-ins, not part of the requirement; the template and the action name are.

Emulator: returns HTTP 403 with AccessDeniedException (both correct) but $.message is the constant 'You do not have sufficient permissions to perform this action.' — it names neither the caller principal, nor the action aco-automation:GetAutomationRule, nor the offending resource value from the request. Replay verdict: mismatch, expected_status 403, actual_status 403, differing key $.message.

Why: clients and operators distinguish a malformed/unmatched resource identifier from a genuine missing IAM permission by reading the action and resource embedded in the denial message, and policy-debugging and remediation tooling parses the 'is not authorized to perform: <action> on resource: <resource>' form. Against the emulator there is nothing to parse and no way to learn which resource was denied, so sim-vs-real behaviour of any caller that inspects denial text diverges even though the status and error code agree.

Fix: GetAutomationRule's handler and its authorization-rejection path (services/service-com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice/src/ops/get_automation_rule.rs and the shared access-denied error helper it calls) must build the AccessDeniedException message from the signing principal, this operation's IAM action name and the supplied ruleArn, instead of emitting the shared canned access-denied constant.

Related open issue: 20261023T134046Z-compute-optimizer-automation-error-messages-not-built-from.md describes the same canned-access-denied text from the BEHAVIORAL corpus (check compute-optimizer-automation.get-automation-rule-malformed-arn-rejected). It carries no wire replay key, so it does not gate this wire fixture step; this issue supplies that binding and should be fixed together with it.

Traces: fixture compute-optimizer-automation-get-automation-rule-malformed-arn-access-denied, step task#2.

## Evidence
Replay of simparity-wire-aws-v1 (25 steps, 24 passed, 1 mismatched, score 96.0%).

[mismatch] fixture compute-optimizer-automation-get-automation-rule-malformed-arn-access-denied step task#2
  service=compute-optimizer-automation operation=GetAutomationRule
  expected_status 403, actual_status 403
  detail: $.message: value_mismatch
    Expected (AWS, built from the request): 'User: <caller principal ARN> is not authorized to perform: aco-automation:GetAutomationRule on resource: <ruleArn exactly as sent>'
      (recorded fill-ins: principal arn:aws:iam::000000000000:user/emulator, resource not-an-arn — environment/request values, not requirements)
    Actual (emulator): 'You do not have sufficient permissions to perform this action.'
  Fixed text that must match AWS: 'User: ', ' is not authorized to perform: ', ' on resource: '.
  Variable parts: caller principal from the signing credential, IAM action aco-automation:GetAutomationRule, ruleArn from the request.

Smithy evidence: GetAutomationRule.ruleArn required, pattern ^arn:aws:compute-optimizer::[0-9]{12}:automation-rule/[a-zA-Z0-9_-]+$; modeled error AccessDeniedException (client, httpError 403).

## Steps to Reproduce
1. Boot the emulator and point a Compute Optimizer Automation client at it.
2. Call GetAutomationRule with a ruleArn that does not match ^arn:aws:compute-optimizer::[0-9]{12}:automation-rule/[a-zA-Z0-9_-]+$ (the recorded fixture sends a plain non-ARN string).
3. Observe: HTTP 403, AccessDeniedException, message 'You do not have sufficient permissions to perform this action.'
   Expected: HTTP 403, AccessDeniedException, message 'User: <caller principal ARN> is not authorized to perform: aco-automation:GetAutomationRule on resource: <the ruleArn as sent>'.
4. Replay to confirm: fixture compute-optimizer-automation-get-automation-rule-malformed-arn-access-denied, step task#2.
