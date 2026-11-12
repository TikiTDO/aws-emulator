Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sns__sqs__sts.change-set-notification-arns-deliver-every-stack-event-to-the-topic
Issue-Key: cloudformation__sns__sqs__sts.change-set-notification-arns-deliver-every-stack-event-to-the-topic|run#23|AssertionError|assert [k for k, _ in fields] == _expected_keys(event), f"message keys: {[k for k, _ in fields]!r}"
Issue-Commit: 3fc0780eeb295abac98eb24007dd895893c6797a

# cloudformation__sns__sqs__sts: a stack-event SNS notification body omits the DetailedStatus line, so the message's field set is one key short of AWS's

> _Note: this issue was identified against the code base at commit `3fc0780eeb295abac98eb24007dd895893c6797a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChangeSet/ExecuteChangeSet NotificationARNs (stack-event SNS notification body)

## Description
This issue carries 1 net-new root cause.

Cause 1 of 1: every CloudFormation stack-event notification the emulator publishes to a stack's NotificationARNs topic is missing its `DetailedStatus` line, so the key='value' field set a subscriber parses is one key short of AWS's and AWS's documented field ordering is broken at that position.
Failing check ids for this cause: cloudformation__sns__sqs__sts.change-set-notification-arns-deliver-every-stack-event-to-the-topic.

AWS: recorded behavior only for the notification envelope — the SNS message body CloudFormation publishes for NotificationARNs is not described by the CloudFormation Smithy model; the model does, however, model the field itself: Smithy StackEvent.DetailedStatus (enum DetailedStatus, optional, documented "An optional field that contains information about the detailed status of the stack event", values including CONFIGURATION_COMPLETE and VALIDATION_FAILED). The recording shows CloudFormation publishing one SNS message per StackEvent of the executed change set, Type=Notification, Subject='AWS CloudFormation Notification', body a sequence of `Key='value'` lines in a FIXED order: StackId, Timestamp, EventId, LogicalResourceId, Namespace, then PhysicalResourceId once the event carries one, then PrincipalId only on events whose ResourceType is AWS::CloudFormation::Stack, then ResourceProperties, ResourceStatus, ResourceStatusReason, DetailedStatus, ResourceType, StackName, ClientRequestToken. The variable parts are taken from the event and the request: StackId/StackName/Namespace (the account) from the stack, LogicalResourceId/ResourceStatus/ResourceType/PhysicalResourceId/EventId/Timestamp from the StackEvent, ResourceStatusReason and DetailedStatus the event's values or the empty string when the event has none, ClientRequestToken from the operation. The fixed text is the key names themselves and the `Key='value'` line form.

Emulator: the published body carries the lines ['StackId', 'Timestamp', 'EventId', 'LogicalResourceId', 'Namespace', 'PhysicalResourceId', 'PrincipalId', 'ResourceProperties', 'ResourceStatus', 'ResourceStatusReason', 'ResourceType', 'StackName', 'ClientRequestToken'] — `DetailedStatus` is absent entirely, rather than being emitted as DetailedStatus='' for an event with no detailed status. Delivery itself, the envelope (Type/TopicArn/Subject), the per-event fan-out and the surviving line order are all correct; only that one key is dropped, and the check fails at the message-key assertion before the value assertions are reached.

Why: a client that parses the notification body positionally or by full field set — the documented, stable `Key='value'` contract CloudFormation's own console and operator tooling rely on — sees a message whose schema differs from AWS's. Consumers keyed on DetailedStatus (the only push signal for CONFIGURATION_COMPLETE and for VALIDATION_FAILED on a failed resource create) never observe it, and strict parsers that require the full key list reject every notification the emulator sends.

Fix: the CloudFormation stack-event notification serializer — the component that renders a StackEvent into the SNS message body for each ARN in the stack's NotificationARNs (the stack-operation event publisher in the cloudformation service crate). It must emit a DetailedStatus line for every event, between ResourceStatusReason and ResourceType, carrying the event's DetailedStatus or the empty string when the event has none.

Related open issues, neither of which carries this cause or this check's replay key: 20261107T184737Z-cloudformation-sns-sqs-2-net-new-causes-a.md (notifications not delivered AT ALL for a CreateStack/UpdateStack NotificationARNs stack — here they ARE delivered, and the defect is the body's field set) and 20261108T003043Z-cloudformation-iam-lambda-logs-a-stack-whose-resource.md (DetailedStatus missing from the DescribeStackEvents API read, not from the SNS notification body).

Traces: the check id in trace_ids.

## Evidence
Expected (AWS, from the recording): for a stack event of the executed change set whose ResourceType is AWS::CloudFormation::Stack and which carries a PhysicalResourceId, the notification message lines are, in order: StackId, Timestamp, EventId, LogicalResourceId, Namespace, PhysicalResourceId, PrincipalId, ResourceProperties, ResourceStatus, ResourceStatusReason, DetailedStatus, ResourceType, StackName, ClientRequestToken.

Actual (emulator): AssertionError: message keys: ['StackId', 'Timestamp', 'EventId', 'LogicalResourceId', 'Namespace', 'PhysicalResourceId', 'PrincipalId', 'ResourceProperties', 'ResourceStatus', 'ResourceStatusReason', 'ResourceType', 'StackName', 'ClientRequestToken'] — i.e. the identical list with the DetailedStatus line dropped. No DetailedStatus='' line is emitted for an event without a detailed status either.

Smithy cited: StackEvent.DetailedStatus (target DetailedStatus, enum, optional) in services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json.

## Steps to Reproduce
1. Create an SNS topic and an SQS queue, give the queue a policy allowing sns.amazonaws.com to SendMessage from that topic, and subscribe the queue to the topic (non-raw delivery).
2. CreateChangeSet with ChangeSetType=CREATE, NotificationARNs=[topic] over a template containing one AWS::CloudFormation::WaitConditionHandle and an Output joining Ref AWS::NotificationARNs; wait for CREATE_COMPLETE, then ExecuteChangeSet and wait for the stack to reach CREATE_COMPLETE.
3. DescribeStackEvents for the stack, then drain the queue until every EventId has arrived and split each message's Message body on the `Key='value'` lines.
4. Compare the ordered key list of each delivered message with AWS's: the DetailedStatus key is missing from every message.
Or replay the check id in trace_ids.
