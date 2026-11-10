Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__events__sqs.cfn-events-event-bus-in-place-update-retained-replacement-then-not-found
Issue-Key: cloudformation__events__sqs.cfn-events-event-bus-in-place-update-retained-replacement-then-not-found|_expect#15|AssertionError|raise AssertionError(
Issue-Commit: cd6d73564e5f4cf53934bca1327f08de31f2ee5f

# cloudformation__events__sqs: a stack's AWS::Events::Rule cannot be created on the AWS::Events::EventBus declared in the same stack — the rule fails NotFound "Event bus <name> does not exist."

> _Note: this issue was identified against the code base at commit `cd6d73564e5f4cf53934bca1327f08de31f2ee5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new root cause of service `cloudformation__events__sqs`, grounded in one failing behavioral check.

Cause 1 of 1 — the event bus a stack provisions is not visible to the sibling rule resource, so the stack rolls back on create.
AWS: recorded behavior only (resource-provisioner behavior). A template declaring `Bus: AWS::Events::EventBus {Name: <name>}` and `Rule: AWS::Events::Rule {EventBusName: {"Ref": "Bus"}, ...}` reaches CREATE_COMPLETE: `Ref` of the bus is its Name, the bus provisioner has actually created the bus in EventBridge by the time the dependent rule is provisioned, and PutRule on that bus succeeds. EventBridge's refusal for a bus that does not exist is `ResourceNotFoundException` with the message `Event bus <name> does not exist.` — fixed text with the requested bus name taken from the request.
Emulator: the create fails. The `Rule` resource lands CREATE_FAILED with `Resource handler returned message: "Event bus <bus-name> does not exist." (... HandlerErrorCode: NotFound)` and the stack ends ROLLBACK_COMPLETE with `The following resource(s) failed to create: [Rule]. ` — i.e. the AWS::Events::EventBus resource reported success without the bus existing in EventBridge (or under a name other than the one `Ref` resolves to), so the dependent rule's PutRule is refused NotFound.
Why: no stack can provision a custom event bus together with the rules on it — the whole template rolls back. Every downstream contract (Ref/GetAtt Arn of the bus, routing a put event to the queue, the in-place Description update, the retained replacement, and the post-delete ResourceNotFoundException) is unreachable.
Fix: the AWS::Events::EventBus resource provisioner's create path (it must create the bus in the EventBridge state that PutRule reads, keyed by the Name the resource's Ref publishes) and/or the AWS::Events::Rule provisioner's EventBusName resolution in the cloudformation crate.
Traces: cloudformation__events__sqs.cfn-events-event-bus-in-place-update-retained-replacement-then-not-found

## Evidence
Check cloudformation__events__sqs.cfn-events-event-bus-in-place-update-retained-replacement-then-not-found
AssertionError: stack ended 'ROLLBACK_COMPLETE' ('The following resource(s) failed to create: [Rule]. '), expected 'CREATE_COMPLETE'; failed events: [('Rule', 'CREATE_FAILED', 'Resource handler returned message: "Event bus <bus-name> does not exist." (RequestToken: ..., HandlerErrorCode: NotFound)')]
Template (abridged): Bus: AWS::Events::EventBus {Name: <bus-name>, Description: first}; Rule: AWS::Events::Rule {EventBusName: {Ref: Bus}, EventPattern: {source: [<source>]}, Targets: [{Arn: {Fn::GetAtt: [Queue, Arn]}, Id: queue}]}.

## Steps to Reproduce
1. Boot the emulator and replay cloudformation__events__sqs.cfn-events-event-bus-in-place-update-retained-replacement-then-not-found.
2. CreateStack with an AWS::Events::EventBus (explicit Name) plus an AWS::Events::Rule whose EventBusName is {"Ref": "<bus logical id>"} and whose target is a queue in the same stack.
3. Poll DescribeStacks: expect CREATE_COMPLETE, DescribeEventBus(<name>) to report the bus, and the stack's Ref/GetAtt Arn outputs to be the bus name and ARN. The emulator rolls the stack back with the Rule CREATE_FAILED NotFound.
