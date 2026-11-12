Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: arc-region-switch.missing-permission-start-plan-execution-denied arc-region-switch.active-execution-blocks-start-and-delete-rejected arc-region-switch.create-plan-invalid-region-count-rejected arc-region-switch.start-plan-execution-post-recovery-missing-id-rejected arc-region-switch.update-plan-nonexistent-not-found arc-region-switch.decline-approval-step-cancels-execution arc-region-switch.start-plan-execution-manual-approval-completes arc-region-switch.get-plan-evaluation-status-flags-missing-config arc-region-switch.eventbridge-execution-started-event-delivered arc-region-switch.list-route53-health-checks-empty-when-none-configured
Issue-Shape-Id: com.amazonaws.arcregionswitch#ArcRegionSwitch
Issue-Key: arc-region-switch.missing-permission-start-plan-execution-denied|run#23|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException for the policy-less caller, got {code!r}"
Issue-Key: arc-region-switch.active-execution-blocks-start-and-delete-rejected|run#21|AssertionError|assert got2 == ("IllegalStateException", active_msg), got2
Issue-Key: arc-region-switch.create-plan-invalid-region-count-rejected|run#8|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: arc-region-switch.start-plan-execution-post-recovery-missing-id-rejected|run#13|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: arc-region-switch.update-plan-nonexistent-not-found|run#13|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: arc-region-switch.decline-approval-step-cancels-execution|run#17|AssertionError|assert final["stepStates"][0]["status"] == "failed", (
Issue-Key: arc-region-switch.start-plan-execution-manual-approval-completes|run#26|AssertionError|assert "executionStarted" in event_types, f"executionStarted missing from {event_types!r}"
Issue-Key: arc-region-switch.get-plan-evaluation-status-flags-missing-config|run#12|AssertionError|assert bad["evaluationState"] == "actionRequired", (
Issue-Key: arc-region-switch.eventbridge-execution-started-event-delivered|_poll_for_matching_events#23|Timeout|resp = sqs.receive_message(
Issue-Key: arc-region-switch.list-route53-health-checks-empty-when-none-configured|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Commit: 24dcbcf55b45bb9d0573a38e9c06262a825c087d

# arc-region-switch: 10 net-new causes — StartPlanExecution skips identity authorization, refusal messages are emulator-authored, declined-step status, missing executionStarted event, evaluation never reports actionRequired, no EventBridge emission, no Route 53 health-check provisioning

> _Note: this issue was identified against the code base at commit `24dcbcf55b45bb9d0573a38e9c06262a825c087d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartPlanExecution,CreatePlan,DeletePlan,UpdatePlan,ApprovePlanExecutionStep,ListPlanExecutionEvents,GetPlanEvaluationStatus,ListRoute53HealthChecks

## Description
Ten distinct root causes in the arc-region-switch emulator, all grounded in failing behavioral checks of this suite. Each cause below must be closed before submitting; they are numbered Cause 1 of 10 .. Cause 10 of 10 and each names its own failing check id. (Separately filed, NOT part of this bundle: the UpdatePlanExecution scaffold stub, which the suite marks ALREADY FILED, and the open issue 20261029T170734Z-arc-region-switch-inregion-reads-answer-404-instead.md about In-Region reads answering 404 instead of 400 — that issue does not cover any cause below.)

=== Cause 1 of 10: StartPlanExecution performs no identity-based authorization, so a caller with no policy is admitted into the operation's state logic ===
AWS: Smithy: StartPlanExecution declares AccessDeniedException (errorKind client, httpError 403, required member `message`); the shape doc is "You do not have sufficient access to perform this action. HTTP Status Code: 403". AWS evaluates IAM authorization BEFORE any resource-state precondition, so a principal that holds no identity-based policy for arc-region-switch:StartPlanExecution is refused 403 AccessDeniedException regardless of the plan's execution state. The message follows AWS's standard implicit-deny template, whose fixed text is "User: <caller-arn> is not authorized to perform: <action> on resource: <resource-arn> because no identity-based policy allows the <action> action"; the variable parts are the caller's assumed-role ARN (from the signing credentials), the action name `arc-region-switch:StartPlanExecution`, and the plan ARN from the request's `planArn`.
Emulator: the restricted caller's StartPlanExecution is not denied at all — it falls through to the concurrency precondition and returns IllegalStateException (400). The check observed code 'IllegalStateException' where AccessDeniedException (403) was required.
Why: an authorization boundary silently does not exist. A client testing least-privilege IAM (or a guardrail test that asserts an unprivileged principal cannot trigger a region switch) sees the call accepted into the state machine; whether it is refused at all depends only on whether some other execution happens to be running, and the error class/status a caller must branch on (403 vs 400) is wrong.
Fix: the request-authorization layer in front of the StartPlanExecution handler — the caller's identity must be evaluated and refused with AccessDeniedException before the plan/execution-state preconditions run.
Traces: arc-region-switch.missing-permission-start-plan-execution-denied.

=== Cause 2 of 10: DeletePlan's active-execution refusal message repeats the region inside the execution id ===
AWS: Smithy: DeletePlan declares IllegalStateException (client, httpError 400, required `message`); its doc states "You cannot delete a plan that is currently being executed." The recorded refusal message's fixed text is "Active executions found for <planArn>: " followed by one "<region>: <executionId>" pair per active execution, where <executionId> is the BARE execution id — the part after the "<region>/" prefix that ExecutionId carries on the wire — and <region> is that execution's region. Both values come from state (the plan's active executions); the plan ARN comes from the request's `arn`.
Emulator: it renders the whole wire executionId (region-prefixed) after the region, duplicating the region: for an execution whose id reads "us-west-2/<id>" it answered 'Active executions found for <planArn>: us-west-2: us-west-2/<id>' where AWS answers 'Active executions found for <planArn>: us-west-2: <id>'. Error code IllegalStateException and the preceding StartPlanExecution refusal ("There is already an execution ongoing for plan <planArn>") both matched; only this message's rendering diverges.
Why: a client that parses the refusal to learn which execution is blocking the delete (or that string-matches the pair) reads a malformed id it cannot feed back to CancelPlanExecution/GetPlanExecution without stripping a duplicated region prefix.
Fix: the DeletePlan handler's refusal-message formatting (split the stored executionId into its region and bare-id parts and render each once).
Traces: arc-region-switch.active-execution-blocks-start-and-delete-rejected.

=== Cause 3 of 10: CreatePlan's duplicate-region refusal message is emulator-authored prose ===
AWS: Smithy: CreatePlan.regions targets RegionList with @length min 2 max 2, and the two Regions must be distinct. AWS refuses a request naming the same Region twice with ValidationException whose message is entirely FIXED text with no variable parts: "There are duplicate regions defined in the plan. Please remove the duplicated regions. " (note the single trailing space). Recorded behavior confirms the code ValidationException.
Emulator: same code (ValidationException) but a different message: 'regions must name two different Amazon Web Services Regions, but us-west-2 is named twice' — emulator-authored prose that also interpolates the offending Region, which AWS's text does not do.
Why: a client or test that asserts on AWS's documented duplicate-region text (or localizes/greps it) does not match, and the message implies a per-value template where AWS emits a constant.
Fix: CreatePlan's request validation — the duplicate-region branch must emit AWS's fixed text verbatim, trailing space included.
Traces: arc-region-switch.create-plan-invalid-region-count-rejected.

=== Cause 4 of 10: StartPlanExecution's missing-recoveryExecutionId refusal message is emulator-authored prose ===
AWS: Smithy: StartPlanExecution.recoveryExecutionId targets RecoveryExecutionId with pattern ^[a-z]{2}(-[a-z]+)+-[0-9]+/[0-9a-fA-F]{16}$ (i.e. 'region/executionId'), and IllegalArgumentException (client, 400) is modeled on the operation. For action=postRecovery with no recoveryExecutionId, AWS refuses with IllegalArgumentException whose message fixed text is "The recoveryExecutionId is missing or incorrectly formatted for the <planArn>. Please provide the recoveryExecutionId in the format 'region/executionId' for Post Recovery execution." — the only variable part is the plan ARN taken from the request's `planArn`.
Emulator: same code (IllegalArgumentException) but the message is 'recoveryExecutionId is required when action is postRecovery': it names neither the plan nor the required 'region/executionId' format, and it covers only the missing case where AWS's one message covers missing-or-malformed.
Why: a client cannot recover from the refusal — AWS's message tells it the exact format to supply and which plan was addressed; the emulator's does not, and any assertion on the AWS text fails.
Fix: StartPlanExecution's postRecovery argument validation (the recoveryExecutionId presence/pattern branch and its message template).
Traces: arc-region-switch.start-plan-execution-post-recovery-missing-id-rejected.

=== Cause 5 of 10: UpdatePlan's not-found message is emulator-authored prose ===
AWS: Smithy: UpdatePlan declares ResourceNotFoundException (client, httpError 404, required `message`), doc "The specified resource was not found." For an `arn` that resolves to no plan, AWS's message fixed text is "Plan not found for ARN: " followed by the plan ARN taken verbatim from the request's `arn`. The same wording is the service-wide plan-not-found text (the GetPlan refusal in the same suite reads "Plan not found for ARN: <arn>").
Emulator: the code is correct (ResourceNotFoundException) but the message reads 'No plan found for arn <arn>' — different fixed text and different capitalization of the ARN label.
Why: a client that branches on, logs or asserts the service's plan-not-found text gets a string AWS never emits; the emulator is also internally inconsistent with the wording other plan reads use.
Fix: the UpdatePlan handler's not-found refusal (use the crate's single plan-not-found message template, "Plan not found for ARN: <arn>").
Traces: arc-region-switch.update-plan-nonexistent-not-found.

=== Cause 6 of 10: a DECLINED manual-approval step's own status is reported canceled instead of failed ===
AWS: Smithy: StepState.status targets StepStatus, whose values are notStarted, running, failed, completed, canceled, skipped, pendingApproval — so `failed` and `canceled` are distinct, modeled states. Recorded behavior: when ApprovePlanExecutionStep is called with approval="decline" on a ManualApproval step, the EXECUTION transitions to canceled while the declined STEP's own status reads `failed` (the step did not get cancelled out from under it — it reached a negative terminal outcome of its own).
Emulator: the execution state is right (canceled) but GetPlanExecution reports stepStates[0].status == 'canceled' for the declined step, collapsing the two distinct modeled states into one.
Why: a client reading stepStates to find WHICH step ended the execution, and whether it ended by operator decline (failed) or by an external cancel (canceled), cannot distinguish them — every step of a cancelled execution looks identical.
Fix: the ApprovePlanExecutionStep handler's decline path / the execution state machine that stamps step terminal status.
Traces: arc-region-switch.decline-approval-step-cancels-execution.

=== Cause 7 of 10: no executionStarted event is ever recorded, so ListPlanExecutionEvents has a hole in the timeline ===
AWS: Smithy: ExecutionEventType enumerates executionPending, executionStarted, executionSucceeded, ... stepPendingApproval, stepSucceeded, and ListPlanExecutionEvents' doc is "Lists the events that occurred during a plan execution. These events provide a detailed timeline of ...". Recorded behavior: an execution that runs pending -> pendingManualApproval -> completed yields an event stream that contains an executionStarted event whose timestamp precedes the executionSucceeded event.
Emulator: the stream contains only ['executionPending', 'stepPendingApproval', 'stepSucceeded', 'executionSucceeded'] — executionStarted is never emitted, so the execution appears to go from pending straight to a step event with no start event to anchor the timeline.
Why: a client reconstructing the execution timeline (or computing start-to-finish duration from the event stream, which is the only place a start event is exposed) finds no start event and either fails or mis-dates the execution.
Fix: the plan-execution state machine's event recorder — the pending->running transition must append an ExecutionEventType::executionStarted event (with its own timestamp, eventId and previousEventId chain) before the first step event.
Traces: arc-region-switch.start-plan-execution-manual-approval-completes.

=== Cause 8 of 10: GetPlanEvaluationStatus always reports passed — an unassumable executionRole never yields actionRequired with a warning ===
AWS: Smithy: GetPlanEvaluationStatus output carries evaluationState (EvaluationStatus enum: passed, actionRequired, pendingEvaluation, unknown) plus a paginated `warnings` list, and its doc is "Retrieves the evaluation status of a Region switch plan. The evaluation status provides information ...". Recorded behavior: a plan whose executionRole names a role that does not exist (and so cannot be assumed) evaluates to `actionRequired` with at least one warning whose warningMessage names that role ARN and whose warningStatus is `active`; an otherwise-identical plan with a working role evaluates to `passed` with no warnings.
Emulator: the broken-role plan evaluates to 'passed' — the evaluator never reports actionRequired and surfaces no warning, so the only misconfiguration signal the API has is dead.
Why: a client relying on plan evaluation as a pre-switch gate gets a green verdict for a plan that cannot execute at all; the actionRequired branch and the warnings list (which is a paginated member of this operation) are unreachable.
Fix: the plan evaluation that backs GetPlanEvaluationStatus — it must actually resolve/assume the plan's executionRole, classify the plan actionRequired on failure, and emit an active warning naming the role ARN.
Traces: arc-region-switch.get-plan-evaluation-status-flags-missing-config.

=== Cause 9 of 10: starting a plan execution emits no EventBridge event, so nothing is ever routed to a rule's target ===
AWS: recorded behavior only (no event schema in the Smithy model). StartPlanExecution emits an EventBridge event on the default bus with source "aws.arc-region-switch" and detail-type "ARC Region Switch Plan Execution", carrying the plan ARN in `resources` and the starting execution's id in detail.executionId, so a rule matching that source/detail-type delivers it to its target (here an SQS queue).
Emulator: no such event is ever delivered. The check wires an EventBridge rule (source aws.arc-region-switch, detail-type "ARC Region Switch Plan Execution") to an SQS target, starts an execution and polls the queue; no message correlating to the execution arrives and the check is still polling when the harness budget expires ('Check timed out after 60.0s'). The control-plane calls (PutRule/PutTargets/SetQueueAttributes/StartPlanExecution) all succeeded, so the gap is the missing emission, not the eventing wiring.
Why: any client driving automation off region-switch events — alarms, runbooks, audit pipelines, cross-service orchestration — observes nothing when a switch starts, and the absence is silent (no error, just no event).
Fix: the StartPlanExecution handler / execution state machine must publish the plan-execution event to EventBridge on the execution's region (source, detail-type, resources with the plan ARN, detail.executionId).
Traces: arc-region-switch.eventbridge-execution-started-event-delivered.

=== Cause 10 of 10: a plan carrying a Route53HealthCheck execution block never accrues any Route 53 health check ===
AWS: Smithy: ListRoute53HealthChecks returns a paginated `healthChecks` list of Route53HealthCheck, whose required members are hostedZoneId, recordName and region (plus healthCheckId and status), doc "List the Amazon Route 53 health checks." Recorded behavior: a plan whose workflow contains a Route53HealthCheck execution block pointing at a real hosted zone and record converges to a NON-empty healthChecks list, every entry echoing the configured hostedZoneId and recordName; a twin plan with no such block keeps reporting an empty list.
Emulator: the configured plan's list never becomes non-empty — the check polls ListRoute53HealthChecks for the configured plan and is still polling when the harness budget expires ('Check timed out after 60.0s'). CreatePlan accepted the Route53HealthCheck execution block, so the block is stored but no health check is ever provisioned or reported for it.
Why: a client cannot discover or reconcile the Route 53 health checks its plan owns (there is no way to tell a plan with a health-check block from one without), and the health-check-backed execution block is inert.
Fix: plan creation/evaluation state for Route53HealthCheck execution blocks — provisioning the health check for the block's hostedZoneId/recordName and reporting it from the ListRoute53HealthChecks handler.
Traces: arc-region-switch.list-route53-health-checks-empty-when-none-configured.

## Evidence
Suite: 9/20 passed (45.0%), 10 mismatched + 1 errored, all in arc-region-switch. The errored check (arc-region-switch.update-plan-execution-resume-wrong-state-rejected) is marked ALREADY FILED (UpdatePlanExecution scaffold stub) and is excluded from this bundle.

Cause 1 — arc-region-switch.missing-permission-start-plan-execution-denied:
  AssertionError: expected AccessDeniedException for the policy-less caller, got 'IllegalStateException'
  expected: code AccessDeniedException (403), message "User: arn:aws:sts::<account>:assumed-role/<restricted-role>/<session> is not authorized to perform: arc-region-switch:StartPlanExecution on resource: <planArn> because no identity-based policy allows the arc-region-switch:StartPlanExecution action"
  actual:   IllegalStateException (400) from the concurrency precondition; the unprivileged caller was never denied.

Cause 2 — arc-region-switch.active-execution-blocks-start-and-delete-rejected:
  AssertionError at `assert got2 == ("IllegalStateException", active_msg)`
  expected: ('IllegalStateException', 'Active executions found for <planArn>: us-west-2: <bare-execution-id>')
  actual:   ('IllegalStateException', 'Active executions found for <planArn>: us-west-2: us-west-2/<bare-execution-id>')  [region repeated inside the id]
  The earlier assertion on StartPlanExecution's refusal ('There is already an execution ongoing for plan <planArn>') passed.

Cause 3 — arc-region-switch.create-plan-invalid-region-count-rejected:
  AssertionError: expected message 'There are duplicate regions defined in the plan. Please remove the duplicated regions. ', got 'regions must name two different Amazon Web Services Regions, but us-west-2 is named twice'
  code ValidationException matched; message diverges (AWS text is fixed, trailing space included).

Cause 4 — arc-region-switch.start-plan-execution-post-recovery-missing-id-rejected:
  AssertionError: expected message "The recoveryExecutionId is missing or incorrectly formatted for the <planArn>. Please provide the recoveryExecutionId in the format 'region/executionId' for Post Recovery execution.", got 'recoveryExecutionId is required when action is postRecovery'
  code IllegalArgumentException matched.

Cause 5 — arc-region-switch.update-plan-nonexistent-not-found:
  AssertionError: expected message 'Plan not found for ARN: <planArn>', got 'No plan found for arn <planArn>'
  code ResourceNotFoundException (404) matched.

Cause 6 — arc-region-switch.decline-approval-step-cancels-execution:
  AssertionError: expected the declined step's own status to read failed, got 'canceled'
  executionState == 'canceled' matched; only stepStates[0].status diverges (StepStatus models both `failed` and `canceled`).

Cause 7 — arc-region-switch.start-plan-execution-manual-approval-completes:
  AssertionError: executionStarted missing from ['executionPending', 'stepPendingApproval', 'stepSucceeded', 'executionSucceeded']
  ExecutionEventType models executionStarted; it is never emitted.

Cause 8 — arc-region-switch.get-plan-evaluation-status-flags-missing-config:
  AssertionError: expected actionRequired for a plan whose executionRole cannot be assumed, got 'passed'
  the follow-on assertions (>=1 warning naming the role ARN, warningStatus 'active') are therefore unreachable too.

Cause 9 — arc-region-switch.eventbridge-execution-started-event-delivered:
  Error: Check timed out after 60.0s while polling the SQS target of an EventBridge rule matching source 'aws.arc-region-switch' / detail-type 'ARC Region Switch Plan Execution'. No message correlating to the started execution (detail.executionId) was ever delivered; all control-plane setup calls succeeded.

Cause 10 — arc-region-switch.list-route53-health-checks-empty-when-none-configured:
  Error: Check timed out after 60.0s while polling `list_route53_health_checks(arn=<configured plan>)['healthChecks']` for a non-empty list. CreatePlan accepted the Route53HealthCheck execution block (hostedZoneId/recordName of a real hosted zone + record) but the plan never accrues a health check.

## Steps to Reproduce
Run the behavioral suite against the emulator (run_behavioral_suite) and replay the ten arc-region-switch check ids in trace_ids. Per cause:

1. CreatePlan a plan with a ManualApproval workflow step; AssumeRole into a freshly created IAM role with NO attached policies; call StartPlanExecution(planArn, targetRegion, action="activate") with those credentials -> expect 403 AccessDeniedException with the implicit-deny message; observe 400 IllegalStateException.
2. StartPlanExecution on a plan, wait until executionState != "pending", then DeletePlan(arn) -> expect IllegalStateException 'Active executions found for <planArn>: <region>: <bare-execution-id>'; observe the region repeated before the id.
3. CreatePlan(regions=[R, R], recoveryApproach="activePassive", primaryRegion=R, ...) -> expect ValidationException with AWS's fixed duplicate-region text; observe emulator prose.
4. CreatePlan with workflows [activate, postRecovery], then StartPlanExecution(planArn, targetRegion, action="postRecovery") with no recoveryExecutionId -> expect IllegalArgumentException with the "region/executionId" guidance message naming the plan; observe the short emulator message.
5. UpdatePlan(arn=<ARN of no existing plan>, workflows=[...], executionRole=...) -> expect ResourceNotFoundException 'Plan not found for ARN: <arn>'; observe 'No plan found for arn <arn>'.
6. Start an execution of a ManualApproval plan, wait for pendingManualApproval, then ApprovePlanExecutionStep(..., approval="decline"); poll GetPlanExecution to a terminal state -> executionState canceled (correct) but stepStates[0].status must read 'failed'; observe 'canceled'.
7. Same happy-path run approved instead of declined; after it completes call ListPlanExecutionEvents(planArn, executionId) -> expect an 'executionStarted' event whose timestamp precedes 'executionSucceeded'; observe no executionStarted event at all.
8. CreatePlan with executionRole naming an IAM role that was never created, then poll GetPlanEvaluationStatus(planArn) until evaluationState != pendingEvaluation -> expect 'actionRequired' with an active warning naming that role ARN; observe 'passed' with no warnings.
9. PutRule(EventPattern source=["aws.arc-region-switch"], detail-type=["ARC Region Switch Plan Execution"]) -> PutTargets(an SQS queue whose policy allows events.amazonaws.com) -> StartPlanExecution; ReceiveMessage on the queue -> expect an event with that source/detail-type, the plan ARN in resources and detail.executionId == the new execution; observe no message ever arrives (check hits the harness timeout).
10. Create a hosted zone + A record; CreatePlan whose activate workflow has a step with executionBlockType="Route53HealthCheck" and route53HealthCheckConfig{hostedZoneId, recordName}; poll ListRoute53HealthChecks(arn=that plan) -> expect a non-empty healthChecks list echoing hostedZoneId/recordName; observe it stays empty (check hits the harness timeout) while a twin plan with no such block correctly stays empty.
