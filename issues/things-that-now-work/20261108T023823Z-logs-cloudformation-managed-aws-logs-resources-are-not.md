Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.deploy-subscription-filter-cfn-then-forwards-to-destination logs.deploy-log-group-cfn-rename-replaces-group-and-drops-events logs.deploy-log-group-cfn-data-protection-policy-masks-events
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.deploy-subscription-filter-cfn-then-forwards-to-destination|run#16|AssertionError|assert len(sub_filters) == 1, (
Issue-Key: logs.deploy-log-group-cfn-rename-replaces-group-and-drops-events|run#19|AssertionError|assert status == "UPDATE_COMPLETE", f"rename update ended {status!r}: {_failed_events(cfn, stack_id)!r}"
Issue-Key: logs.deploy-log-group-cfn-data-protection-policy-masks-events|run#15|AssertionError|assert status == "CREATE_COMPLETE", f"stack ended {status!r}"
Issue-Commit: 43c7d03c349635b4ef0fa0dfa9d9ce4b91e8a4c8

# logs: CloudFormation-managed AWS::Logs::* resources are not materialized, not replaceable, and a DataProtectionPolicy on a declared log group fails the stack (3 net-new causes)

> _Note: this issue was identified against the code base at commit `43c7d03c349635b4ef0fa0dfa9d9ce4b91e8a4c8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutSubscriptionFilter

## Description
Three net-new root causes, all in how the emulator provisions the CloudWatch Logs resource types from a CloudFormation template. All three are the only failures under the suite's `### logs` heading that are not marked ALREADY FILED or OWNED.

Cause 1 of 3: a CloudFormation-created AWS::Logs::SubscriptionFilter is reported CREATE_COMPLETE but no subscription filter exists on the log group.
AWS: Smithy (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/smithy-model/com-amazonaws-cloudwatchlogs-logs-20140328.json) — PutSubscriptionFilter takes logGroupName, filterName, filterPattern and destinationArn (all @required) plus roleArn, and DescribeSubscriptionFilters returns `subscriptionFilters` for the group, filtered by `filterNamePrefix`. CloudFormation's AWS::Logs::SubscriptionFilter resource is a thin wrapper over PutSubscriptionFilter: once the stack reports CREATE_COMPLETE, DescribeSubscriptionFilters(logGroupName=<the declared group>, filterNamePrefix=<the declared FilterName>) returns exactly one filter whose `filterPattern` equals the template's FilterPattern and whose `destinationArn` equals the resolved Fn::GetAtt of the destination (here an AWS::Kinesis::Stream ARN), and matching events ingested into the group are forwarded to that destination as the gzip'd CWL envelope (messageType DATA_MESSAGE, logGroup = the group name, subscriptionFilters naming the filter).
Emulator: the stack containing AWS::Logs::LogGroup + AWS::Logs::LogStream + AWS::Kinesis::Stream + AWS::IAM::Role + AWS::Logs::SubscriptionFilter reaches CREATE_COMPLETE (the check asserts that first and it passed), but DescribeSubscriptionFilters for that group and prefix answers 200 with an EMPTY `subscriptionFilters` list: `AssertionError: expected exactly one subscription filter named '<filter-name>', got []`. The filter was never created at all, so the forwarding assertions downstream are unreachable.
Why: a client that wires a log pipeline declaratively gets a stack that says the filter is live while the group has no filter: DescribeSubscriptionFilters shows nothing to reconcile against, no event is ever forwarded to Kinesis, and a drift/verification step reads the topology as "filter missing" immediately after a successful deploy. This is distinct from the already-open 20261108T001650Z-cloudwatch-lambda-logs-a-cloudformation-deployed-aws-logs.md, which records that a CFN-deployed filter with a LAMBDA destination does not DELIVER (that check's filter is assumed created); here the resource is not created in the first place, with a Kinesis destination.
Fix: the CloudFormation resource provider for AWS::Logs::SubscriptionFilter must, on create, actually call the Logs PutSubscriptionFilter path (resolving LogGroupName/DestinationArn/RoleArn intrinsics) and register the filter in the log group's state instead of reporting CREATE_COMPLETE for a no-op.
Traces: logs.deploy-subscription-filter-cfn-then-forwards-to-destination.

Cause 2 of 3: replacing a CloudFormation-managed log group rolls the update back because the emulator refuses to update an AWS::Logs::LogStream in place instead of replacing it.
AWS: recorded/documented CloudFormation behavior (the Logs Smithy model says nothing about CFN update semantics). AWS::Logs::LogGroup$LogGroupName and AWS::Logs::LogStream$LogGroupName / $LogStreamName all have "Update requires: Replacement": changing LogGroupName replaces the group (new physical id, new ARN) and, because the stream's LogGroupName resolves through Ref to the replaced group, the stream is replaced too. The stack reaches UPDATE_COMPLETE; the old group is deleted (reading it then fails ResourceNotFoundException with message "The specified log group does not exist."), the replacement group carries only the recreated stream with no carried-over events, and it is writable.
Emulator: the update terminates UPDATE_ROLLBACK_COMPLETE. The resource event recorded is `('LogStream', 'UPDATE_FAILED', 'NotImplemented: this emulator does not change the properties of an existing AWS::Logs::LogStream in place; the resource was left as it is.')` and the stack event `('<stack>', 'UPDATE_FAILED', 'The following resource(s) failed to update: [LogStream]. ')`. The fixed text here is the refusal itself; `LogStream` is the template's logical id and the stack name is the request's. AWS never emits a NotImplemented handler error for this change — it performs a replacement.
Why: any template change that renames a log group (or otherwise forces replacement of a declared log stream) is unapplicable: the stack rolls back, the rename never happens, and a client cannot evolve a logging topology through CloudFormation at all. Worse, the rollback hides the real AWS outcome (old group gone, new group empty and writable), so tests of replacement semantics cannot run.
Fix: the CloudFormation resource provider for AWS::Logs::LogStream (and the group provider for LogGroupName changes) must implement replacement-on-update for the replacement-only properties — create the new stream under the new group, repoint the logical id's physical id, and delete the old one — rather than returning a NotImplemented handler error.
Traces: logs.deploy-log-group-cfn-rename-replaces-group-and-drops-events.

Cause 3 of 3: a stack declaring an AWS::Logs::LogGroup with a DataProtectionPolicy never creates — it ends ROLLBACK_COMPLETE.
AWS: Smithy — PutDataProtectionPolicy(logGroupIdentifier, policyDocument) attaches a data-protection policy to a log group and GetDataProtectionPolicy(logGroupIdentifier) returns that `policyDocument`; the modeled client errors are InvalidParameterException / LimitExceededException / OperationAbortedException / ResourceNotFoundException. CloudFormation's AWS::Logs::LogGroup exposes the same thing as the DataProtectionPolicy property, so a template declaring a group with a well-formed policy (Version 2021-06-01, an Audit statement with a FindingsDestination and a Deidentify/MaskConfig statement over arn:aws:dataprotection::aws:data-identifier/EmailAddress), RetentionInDays, Tags and sibling AWS::Logs::LogStream resources reaches CREATE_COMPLETE; GetDataProtectionPolicy then returns that document, matched identifiers are masked at ingest (only the matched span is replaced, the rest of the message is verbatim), get_log_events(unmask=True) recovers the original, and a sibling group in the same stack with no policy stays verbatim.
Emulator: the stack terminates `ROLLBACK_COMPLETE` instead of CREATE_COMPLETE — `AssertionError: stack ended 'ROLLBACK_COMPLETE'` — so none of the policy behavior is reachable. Hypothesis: the AWS::Logs::LogGroup resource provider rejects or fails on the DataProtectionPolicy property (the only property this template adds over the templates in causes 1 and 2, whose LogGroup+LogStream resources do reach CREATE_COMPLETE). The check asserts on status before collecting DescribeStackEvents, so the per-resource reason is not in the replay output; a fixer should read the stack's FAILED events to confirm which resource failed.
Why: a client cannot declare a data-protected log group at all — the whole stack rolls back, taking unrelated resources with it, instead of producing a group whose sensitive fields are masked on ingest. Sensitive data then flows unmasked in any topology that falls back to creating the group without the policy.
Fix: the CloudFormation resource provider for AWS::Logs::LogGroup must accept the DataProtectionPolicy property and apply it through the PutDataProtectionPolicy path at create time (and the Logs ingest path must honour it), rather than failing the resource and rolling the stack back.
Traces: logs.deploy-log-group-cfn-data-protection-policy-masks-events.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, overall 471/902. Service heading `### logs`: 307/444 passed, 137 failed; 134 of those are marked ALREADY FILED or OWNED by an existing issue, and these three are the only net-new ones.

1) [mismatch] logs.deploy-subscription-filter-cfn-then-forwards-to-destination
   check_logs_deploy_subscription_filter_cfn_then_forwards_to_destination.py, line 126:
   `AssertionError: expected exactly one subscription filter named 'sim2real-behav-logs-sf-filter-<suffix>', got []`
   Order of events: create_stack (Capabilities CAPABILITY_IAM) with AWS::Logs::LogGroup(RetentionInDays 1), AWS::Logs::LogStream(Ref LogGroup), AWS::Kinesis::Stream(ShardCount 1), AWS::IAM::Role trusting logs.amazonaws.com with kinesis:PutRecord on the stream, and AWS::Logs::SubscriptionFilter{LogGroupName: Ref LogGroup, FilterName, FilterPattern "ERROR", DestinationArn: Fn::GetAtt DestStream.Arn, RoleArn: Fn::GetAtt CWLRole.Arn} -> stack reaches CREATE_COMPLETE (asserted and passed) -> describe_subscription_filters(logGroupName=<group>, filterNamePrefix=<filter>) returns subscriptionFilters=[].
   Expected: one filter with filterPattern 'ERROR' and destinationArn naming the Kinesis stream, then ERROR-matching events ingested into the group arriving on the stream shard as a gzip'd DATA_MESSAGE envelope whose logGroup is the group and whose subscriptionFilters names the filter. Observed: no filter at all.

2) [mismatch] logs.deploy-log-group-cfn-rename-replaces-group-and-drops-events
   check_logs_deploy_log_group_cfn_rename_replaces_group_and_drops_events.py, line 74:
   `AssertionError: rename update ended 'UPDATE_ROLLBACK_COMPLETE': [('<stack>', 'UPDATE_FAILED', 'The following resource(s) failed to update: [LogStream]. '), ('LogStream', 'UPDATE_FAILED', 'NotImplemented: this emulator does not change the properties of an existing AWS::Logs::LogStream in place; the resource was left as it is.')]`
   Order of events: create_stack with AWS::Logs::LogGroup(LogGroupName=<old>, RetentionInDays 1) + AWS::Logs::LogStream(Ref LogGroup) and Output GroupArn=Fn::GetAtt LogGroup.Arn -> CREATE_COMPLETE, physical id of LogGroup == <old>, put_log_events/get_log_events on <old> round-trips -> update_stack with the same template but LogGroupName=<new>.
   Expected: UPDATE_COMPLETE; LogGroup physical id == <new>; the GroupArn output equal to the old ARN with `:log-group:<old>` replaced by `:log-group:<new>`; <old> absent from DescribeLogGroups and get_log_events on it refused ResourceNotFoundException with message 'The specified log group does not exist.'; <new> described with retentionInDays 1, holding exactly the one recreated stream with zero events, and writable. Observed: the update rolled back with a NotImplemented handler error on the LogStream.
   Per-run values that are NOT requirements: the generated group/stream/stack names and the account/region in the ARN. The requirements are the UPDATE_COMPLETE outcome, the replacement physical id/ARN, and that no NotImplemented reason is emitted.

3) [mismatch] logs.deploy-log-group-cfn-data-protection-policy-masks-events
   check_logs_deploy_log_group_cfn_data_protection_policy_masks_events.py, line 66:
   `AssertionError: stack ended 'ROLLBACK_COMPLETE'`
   Order of events: create_stack with AWS::Logs::LogGroup 'Protected' {LogGroupName, RetentionInDays 1, Tags, DataProtectionPolicy: {Name, Version '2021-06-01', Statement: [{Sid audit, DataIdentifier [arn:aws:dataprotection::aws:data-identifier/EmailAddress], Operation {Audit:{FindingsDestination:{}}}}, {Sid mask, same DataIdentifier, Operation {Deidentify:{MaskConfig:{}}}}]}}, a sibling AWS::Logs::LogGroup 'Control' with no policy, and three AWS::Logs::LogStream resources.
   Expected: CREATE_COMPLETE; get_data_protection_policy(logGroupIdentifier=<protected>) returns the template's document (its Name matches); an ingested message containing an email address is served with only the address replaced by '*' characters (prefix/suffix of the message verbatim) while a benign message is unchanged; get_log_events(unmask=True) returns both originals; the Control group returns both messages verbatim. Observed: ROLLBACK_COMPLETE, so nothing after the create was exercised.

Note on the comparable templates: causes 1 and 2 both deploy AWS::Logs::LogGroup + AWS::Logs::LogStream successfully to CREATE_COMPLETE, which is why the DataProtectionPolicy property is the suspected trigger in cause 3.

## Steps to Reproduce
Cause 1 (AWS::Logs::SubscriptionFilter not materialized):
1. cloudformation.create_stack with Capabilities=['CAPABILITY_IAM'] and resources: AWS::Logs::LogGroup G (RetentionInDays 1); AWS::Logs::LogStream S (LogGroupName: {Ref: G}); AWS::Kinesis::Stream D (ShardCount 1); AWS::IAM::Role R trusting logs.amazonaws.com with kinesis:PutRecord on {Fn::GetAtt: [D, Arn]}; AWS::Logs::SubscriptionFilter {LogGroupName: {Ref: G}, FilterName: 'f', FilterPattern: 'ERROR', DestinationArn: {Fn::GetAtt: [D, Arn]}, RoleArn: {Fn::GetAtt: [R, Arn]}}.
2. Poll describe_stacks until CREATE_COMPLETE.
3. logs.describe_subscription_filters(logGroupName=<G>, filterNamePrefix='f'). Expected one filter with filterPattern 'ERROR' and destinationArn = D's ARN; observed `subscriptionFilters: []`.
4. logs.put_log_events into G with a message containing 'ERROR: <marker>' and read the Kinesis shard from TRIM_HORIZON: expected a gzip'd JSON record with messageType 'DATA_MESSAGE', logGroup = G and subscriptionFilters containing 'f'.
5. Re-replay: logs.deploy-subscription-filter-cfn-then-forwards-to-destination.

Cause 2 (LogStream replacement on update):
1. create_stack with AWS::Logs::LogGroup (LogGroupName=<old>, RetentionInDays 1), AWS::Logs::LogStream (LogGroupName {Ref LogGroup}, LogStreamName=<s>) and Output GroupArn = Fn::GetAtt LogGroup.Arn; wait for CREATE_COMPLETE.
2. logs.put_log_events into <old>/<s>; confirm it is served.
3. update_stack with the identical template except LogGroupName=<new>.
4. Expected UPDATE_COMPLETE with describe_stack_resource(LogGroup).PhysicalResourceId == <new>, GroupArn output repointed to <new>, <old> deleted (get_log_events on it refused ResourceNotFoundException / 'The specified log group does not exist.'), and <new> holding stream <s> with zero events and accepting a new put. Observed: UPDATE_ROLLBACK_COMPLETE with the LogStream UPDATE_FAILED reason 'NotImplemented: this emulator does not change the properties of an existing AWS::Logs::LogStream in place; the resource was left as it is.'
5. Re-replay: logs.deploy-log-group-cfn-rename-replaces-group-and-drops-events.

Cause 3 (DataProtectionPolicy on a declared log group):
1. create_stack with an AWS::Logs::LogGroup whose Properties include RetentionInDays 1, Tags and a DataProtectionPolicy object {Name, Version '2021-06-01', Statement: [an Audit statement with FindingsDestination {} and a Deidentify statement with MaskConfig {}, both over DataIdentifier ['arn:aws:dataprotection::aws:data-identifier/EmailAddress']]}, plus a sibling policy-free AWS::Logs::LogGroup and AWS::Logs::LogStream resources in both.
2. Poll describe_stacks: expected CREATE_COMPLETE; observed ROLLBACK_COMPLETE. Call describe_stack_events and read the FAILED events to see which resource handler refused the DataProtectionPolicy property.
3. After the fix: logs.get_data_protection_policy(logGroupIdentifier=<protected group>) must return the declared document; put_log_events of 'signup from <addr> accepted' into the protected group must be served with only the address masked and the surrounding text verbatim; get_log_events(..., unmask=True) must return the original; the sibling group must return it verbatim.
4. Re-replay: logs.deploy-log-group-cfn-data-protection-policy-masks-events.
