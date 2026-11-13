Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__iam__sns__sqs__sts.put-targets-target-arn-condition-restricts-to-sqs
Issue-Key: events__iam__sns__sqs__sts.put-targets-target-arn-condition-restricts-to-sqs|_until_admitted#19|Timeout|time.sleep(3)
Issue-Commit: 72bb6bfd937a489e314e0b070a9ac1619964b2b9

# events__iam__sns__sqs__sts: an events:TargetArn-conditioned Allow never admits PutTargets — the permitted SQS target is refused AccessDeniedException forever and the check exhausts its 60s budget

> _Note: this issue was identified against the code base at commit `72bb6bfd937a489e314e0b070a9ac1619964b2b9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
Cause 1 of 1: PutTargets under an assumed role whose only Allow is conditioned on the service's own request key `events:TargetArn` is refused for a target that satisfies the condition, so the conditioned Allow never grants anything. The condition evaluation is now wrong in the permissive-side direction: nothing the policy admits is ever admitted.

AWS: Smithy: eventbridge#PutTargets takes Rule (required), EventBusName and Targets (required, @length 1..100) and answers PutTargetsResponse{FailedEntryCount=0, FailedEntries}; its modeled errors are ConcurrentModificationException, InternalException, LimitExceededException, ManagedRuleException, ResourceNotFoundException — AccessDeniedException is the authorization refusal, which is a service fact the model does not carry, so for the authorization contract this is recorded behavior only. AWS authorizes PutTargets with `events:TargetArn` published into the request context, once per entry in Targets. A policy statement `Allow events:PutTargets on <rule arn>` with `Condition {"ForAnyValue:ArnLike": {"events:TargetArn": "arn:aws:sqs:*"}}` therefore MATCHES a request whose single target is an SQS queue ARN: the call is served HTTP 200 and the target appears in ListTargetsByRule. The same policy does NOT match a request whose target is an SNS topic ARN: that call is refused HTTP 400 `AccessDeniedException` with AWS's standard implicit-deny wording — fixed text `User: ` + principal + ` is not authorized to perform: ` + action + ` on resource: ` + resource + ` because no identity-based policy allows the ` + action + ` action`, whose variable parts are the caller's assumed-role session ARN (arn:aws:sts::<account>:assumed-role/<role>/<session>), the action `events:PutTargets` and the rule ARN from the request — and the rule's target list is unchanged.

Emulator: the ADMITTED leg is refused. The harness's bounded retry around the first (SQS) PutTargets re-tries only two answers — exactly that AccessDeniedException sentence, or a not-yet-valid session token — and it never stopped re-trying, so the check ran out its whole 60s budget with no verdict: `Error: Check timed out after 60.0s`. Hypothesis: the authorization gate does not publish `events:TargetArn` into the request context for PutTargets (or does not implement the ForAnyValue:/ArnLike set-and-wildcard operators over it), so the conditioned Allow matches no request and every PutTargets by that role is refused. Note this is the INVERSE symptom of the already-open 20261109T163411Z-events-iam-sns-sqs-sts-eventbridge-puttargets-is-not-gated-by-the-events.md, which recorded the condition being ignored so that a DISALLOWED SNS target was accepted 200; that issue's replay key is the 'expected the call to be refused, but it succeeded' assertion and the suite does not attribute this failure to it. The permitted call is now refused instead, which is a second, opposite defect in the same gate and must be closed with it.

Why: a client that scopes a delegated role to 'may only add SQS targets' loses the capability entirely — every PutTargets it makes, including the ones its policy explicitly allows, comes back AccessDeniedException. Least-privilege automation (CI roles, tenant-scoped rule authoring) cannot add a single target, and because the refusal carries AWS's propagation-shaped wording, a client's retry-while-the-policy-propagates loop never terminates.

Fix: the PutTargets authorization step's condition evaluation — it must put `events:TargetArn` in the request context for each entry of Targets and evaluate the ForAnyValue:/ArnLike operators against it, so a matching statement grants and only a non-matching one refuses.

Traces: events__iam__sns__sqs__sts.put-targets-target-arn-condition-restricts-to-sqs.

## Evidence
Check events__iam__sns__sqs__sts.put-targets-target-arn-condition-restricts-to-sqs, [mismatch].

Expected (AWS): with the role's only statement being Allow events:PutTargets on <rule arn> Condition ForAnyValue:ArnLike events:TargetArn = arn:aws:sqs:*, PutTargets(Targets=[{Id:'sqs', Arn:<queue arn>}]) is served 200 and ListTargetsByRule shows {'sqs': <queue arn>}; PutTargets(Targets=[{Id:'sns', Arn:<topic arn>}]) is refused AccessDeniedException 'User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: events:PutTargets on resource: <rule arn> because no identity-based policy allows the events:PutTargets action' and adds nothing.

Actual (emulator): Error: Check timed out after 60.0s. The only retry loop reached at that point retries exactly that AccessDeniedException sentence (or the not-yet-valid-token refusal) while waiting for the policy to admit the SQS PutTargets; it never became admitted, so the permitted call stays refused and no assertion is ever reached.

## Steps to Reproduce
1. CreateEventBus, PutRule on it with an EventPattern, CreateQueue (note its QueueArn) and CreateTopic.
2. CreateRole trusting arn:aws:iam::<account>:root, with one inline policy: {Effect: Allow, Action: events:PutTargets, Resource: <rule arn>, Condition: {"ForAnyValue:ArnLike": {"events:TargetArn": "arn:aws:sqs:*"}}}. AssumeRole it.
3. With the assumed-role credentials, call PutTargets(Rule=<rule>, EventBusName=<bus>, Targets=[{Id:'sqs', Arn:<queue arn>}]). AWS serves 200; the emulator refuses it AccessDeniedException indefinitely (retrying never admits it).
4. With the same credentials call PutTargets with Targets=[{Id:'sns', Arn:<topic arn>}] — this one must be the AccessDeniedException, and ListTargetsByRule must still show only the SQS target.
