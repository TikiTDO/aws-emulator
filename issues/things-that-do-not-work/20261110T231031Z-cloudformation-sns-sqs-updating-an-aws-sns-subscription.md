Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sns__sqs.cfn-sns-subscription-in-place-update-retained-replacement-delivers-to-both-then-not-found cloudformation__sns__sqs.cfn-sns-topic-in-place-update-retained-replacement-then-not-found
Issue-Key: cloudformation__sns__sqs.cfn-sns-subscription-in-place-update-retained-replacement-delivers-to-both-then-not-found|_expect#29|AssertionError|raise AssertionError(
Issue-Key: cloudformation__sns__sqs.cfn-sns-topic-in-place-update-retained-replacement-then-not-found|_expect#31|AssertionError|raise AssertionError(
Issue-Commit: cd6d73564e5f4cf53934bca1327f08de31f2ee5f

# cloudformation__sns__sqs: updating an AWS::SNS::Subscription answers NotImplemented, so any stack update that touches a subscription rolls back

> _Note: this issue was identified against the code base at commit `cd6d73564e5f4cf53934bca1327f08de31f2ee5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Cause 1 of 1: the CloudFormation resource provider for AWS::SNS::Subscription has no update path at all — every UpdateStack that reaches a subscription resource fails it with NotImplemented and rolls the whole stack back. Failing checks: cloudformation__sns__sqs.cfn-sns-subscription-in-place-update-retained-replacement-delivers-to-both-then-not-found, cloudformation__sns__sqs.cfn-sns-topic-in-place-update-retained-replacement-then-not-found.

AWS: AWS::SNS::Subscription splits its properties two ways. RawMessageDelivery, FilterPolicy, FilterPolicyScope, DeliveryPolicy, RedrivePolicy and SubscriptionRoleArn are mutable, so changing one is an UPDATE with no interruption: the resource keeps its physical id (the subscription ARN) and CloudFormation applies the change through SNS SetSubscriptionAttributes. TopicArn, Protocol and Endpoint are create-only, so changing one REPLACES the resource: CloudFormation calls Subscribe to mint a new subscription, adopts the new ARN as the physical id, and then deletes the old one — unless UpdateReplacePolicy is Retain, in which case the old subscription is kept and stays live (so one publish is then delivered to both endpoints). Either way the stack ends UPDATE_COMPLETE. Replacing the referenced AWS::SNS::Topic (a TopicName change, also create-only) changes the subscription's TopicArn and therefore replaces the subscription the same way. Recorded behavior only for the CloudFormation resource semantics — CFN resource types are not Smithy shapes; the SNS contract behind them is Smithy (sns 2010-03-31) Subscribe / SetSubscriptionAttributes / Unsubscribe, all of which the emulator already serves on the SNS data plane.

Emulator: UpdateStack puts the subscription resource into UPDATE_FAILED with the reason `NotImplemented: this emulator does not change the properties of an existing AWS::SNS::Subscription in place; the resource was left as it is.` and the stack settles in UPDATE_ROLLBACK_COMPLETE with StackStatusReason `The following resource(s) failed to update: [<logical id>]. ` (the logical id is the template's, `Sub` in one check and `Subscription` in the other; the fixed text is the rest). This happens both for a mutable-property change (RawMessageDelivery true -> false) and for a create-only change that should have been a replacement (Endpoint moved to another queue; TopicArn following a replaced topic), so neither the in-place nor the replacement branch exists.

Why: a client cannot evolve any stack that owns an SNS subscription. Flipping raw message delivery, tightening a filter policy, re-pointing a subscription at a new queue or renaming the topic all take the stack through a failed update and a rollback instead of UPDATE_COMPLETE, and the deployed topology never changes. Infrastructure-as-code users read UPDATE_ROLLBACK_COMPLETE as 'my change is unsafe' and have no way to apply it; UpdateReplacePolicy: Retain, the mechanism that keeps the old subscription delivering during a cut-over, is unreachable.

Fix: the CloudFormation template processor's resource provider for AWS::SNS::Subscription — add the update handler: diff the properties, apply the mutable ones through SetSubscriptionAttributes keeping the same physical id, and treat a changed TopicArn/Protocol/Endpoint as a replacement (Subscribe for the new one, then Unsubscribe the old one or retain it when UpdateReplacePolicy is Retain), emitting the ordinary UPDATE_IN_PROGRESS/UPDATE_COMPLETE (and, for a replacement, the delete-of-old) resource events.

Traces: the two failing check ids above.

## Evidence
Expected (AWS): stack status UPDATE_COMPLETE; for the RawMessageDelivery change the subscription keeps its ARN and GetSubscriptionAttributes reports RawMessageDelivery 'false'; for the Endpoint change under UpdateReplacePolicy Retain the resource's physical id becomes a NEW subscription ARN while the old ARN still exists and still receives the publish.
Actual (emulator): `AssertionError: stack ended 'UPDATE_ROLLBACK_COMPLETE' ('The following resource(s) failed to update: [Sub]. '), expected 'UPDATE_COMPLETE'; failed events: [('Sub', 'UPDATE_FAILED', 'NotImplemented: this emulator does not change the properties of an existing AWS::SNS::Subscription in place; the resource was left as it is.')]` and the same with logical id `Subscription` in the topic-replacement check. Stack names, ARNs and suffixes in the recorded runs are per-run values; the UPDATE_FAILED reason text and the rollback are the divergence.

## Steps to Reproduce
1. CreateStack with a template containing AWS::SNS::Topic, two AWS::SQS::Queue, an AWS::SQS::QueuePolicy allowing sns.amazonaws.com, and an AWS::SNS::Subscription (Protocol sqs, Endpoint = QueueA Arn, RawMessageDelivery 'true', UpdateReplacePolicy Retain). Wait for CREATE_COMPLETE.
2. UpdateStack with the identical template except RawMessageDelivery 'false'. Observe the Sub resource event: UPDATE_FAILED with the NotImplemented reason and the stack reaching UPDATE_ROLLBACK_COMPLETE instead of UPDATE_COMPLETE.
3. UpdateStack again moving Endpoint to QueueB's Arn (a create-only change that must replace the resource and retain the old subscription). Same NotImplemented failure.
4. Equivalently: in a stack whose AWS::SNS::Subscription references the topic by Ref, change the topic's TopicName so the topic is replaced — the subscription's update fails the same way.
