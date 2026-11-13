Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: f4204ce556c91dff64e4ed4bfbe851c5f3483f56

# Executing a budget action never applies its Definition (IAM policy attach / SCP attach / SSM document run), yet reports EXECUTION_SUCCESS

_Filed: 2026-10-24T08:54:40Z_
_Operation: ExecuteBudgetAction | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `f4204ce556c91dff64e4ed4bfbe851c5f3483f56`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A budget action exists to APPLY something outside Budgets: `ActionType` is one of `APPLY_IAM_POLICY`, `APPLY_SCP_POLICY`, `RUN_SSM_DOCUMENTS`, and `Definition` names exactly what to apply and to whom (`IamActionDefinition{PolicyArn, Roles, Groups, Users}`, `ScpActionDefinition{PolicyId, TargetIds}`, `SsmActionDefinition{ActionSubType, Region, InstanceIds}`).

Expected (AWS): `ExecuteBudgetAction` with `ExecutionType: APPROVE_BUDGET_ACTION` on an `APPLY_IAM_POLICY` action assumes `ExecutionRoleArn` and ATTACHES `IamActionDefinition.PolicyArn` to every role/group/user named in the definition; the attachment is observable in IAM (e.g. `ListAttachedRolePolicies` on the named role now reports the policy). `REVERSE_BUDGET_ACTION` detaches it again. The action's `Status` becomes `EXECUTION_SUCCESS` only because that work succeeded.

Actual (emulator): the handler moves the stored `Action.status` to `EXECUTION_IN_PROGRESS` and, after the emulator's own 500 ms settle, to `EXECUTION_SUCCESS` — while nothing at all is applied anywhere. No IAM policy is attached, no SCP is attached, no SSM document is run, and `REVERSE_BUDGET_ACTION` likewise only flips the status back. So a client that executes an action and then asks IAM whether the deny-policy landed on the role sees an un-protected role while Budgets claims the action executed successfully. The same hole makes the whole point of a budget action (the guardrail it enforces) unobservable in the emulator.

The emulator does have the other side of this wire: the IAM service crate implements `AttachRolePolicy` / `ListAttachedRolePolicies` / `AttachUserPolicy` (only the *group* variants are still stubs), so the cross-service effect is implementable and verifiable within one engine.

## Evidence

Spec/well-known semantics: Smithy model — `ActionType` values `APPLY_IAM_POLICY | APPLY_SCP_POLICY | RUN_SSM_DOCUMENTS`; `IamActionDefinition$PolicyArn` doc "The Amazon Resource Name (ARN) of the policy to be attached"; `IamActionDefinition$Roles` doc "A list of roles to be attached"; `ScpActionDefinition$TargetIds` "A list of target IDs"; `Action$ExecutionRoleArn` "The role passed for action execution and reversion". Applying the definition is the documented purpose of executing the action, and `ActionStatus$EXECUTION_SUCCESS` asserts that it happened.
Handler: services/service-com-amazonaws-budgets-awsbudgetservicegateway/src/ops/execute_budget_action.rs:196-262 — resolves the budget and action, refuses an in-flight action, picks the transition via `crate::state::action_execution`, then calls `action.start(execution, now)` and returns. Nothing reads `action.action.definition` and nothing touches any other service's state.
src/state.rs:64-69 states the omission explicitly: "Nothing in this emulator applies an action's `Definition` to IAM, Organizations or Systems Manager — that work belongs to other services and no request here runs it." src/state.rs:176-183 (`ACTION_SETTLES_AFTER = 500ms`) then settles the status to `EXECUTION_SUCCESS` regardless.
Peer service: services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/attach_role_policy.rs and list_attached_role_policies.rs are implemented (not scaffolds), so the attachment is observable from the same engine.
Tests: tests/all/budget_action_execution.rs covers only the status machine (`an_execution_moves_the_action_only_where_its_status_leaves_work`, `the_clock_settles_an_execution_with_no_further_call`); no test asserts any effect outside the budgets store.

## Steps to Reproduce

1. Mount both the budgets and iam emulated services on one engine. In IAM, create a role `BudgetActionRole` and a role `DevRole`, plus (or reference) a managed policy ARN `arn:aws:iam::123456789012:policy/DenyAll`.
2. CreateBudget {"AccountId":"123456789012","Budget":{"BudgetName":"guardrail","TimeUnit":"MONTHLY","BudgetType":"COST","BudgetLimit":{"Amount":"100","Unit":"USD"}}} → 200.
3. CreateBudgetAction {"AccountId":"123456789012","BudgetName":"guardrail","NotificationType":"ACTUAL","ActionType":"APPLY_IAM_POLICY","ActionThreshold":{"ActionThresholdValue":80,"ActionThresholdType":"PERCENTAGE"},"Definition":{"IamActionDefinition":{"PolicyArn":"arn:aws:iam::123456789012:policy/DenyAll","Roles":["DevRole"]}},"ExecutionRoleArn":"arn:aws:iam::123456789012:role/BudgetActionRole","ApprovalModel":"AUTOMATIC","Subscribers":[{"SubscriptionType":"EMAIL","Address":"team@example.com"}]} → note the ActionId.
4. ExecuteBudgetAction {"AccountId":"123456789012","BudgetName":"guardrail","ActionId":"<id>","ExecutionType":"APPROVE_BUDGET_ACTION"} → 200.
5. Wait for the settle, then DescribeBudgetAction → `Action.Status` is `EXECUTION_SUCCESS`.
6. Call IAM ListAttachedRolePolicies for `DevRole`: the emulator reports no attached policies, while AWS would report `DenyAll` attached by the executed action.
7. ExecuteBudgetAction with "ExecutionType":"REVERSE_BUDGET_ACTION" → status returns to `REVERSE_SUCCESS` with no corresponding detach, confirming neither direction of the cross-service effect is wired.
