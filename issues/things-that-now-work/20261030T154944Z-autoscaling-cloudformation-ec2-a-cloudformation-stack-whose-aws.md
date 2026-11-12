Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__cloudformation__ec2.cfn-rolling-update-honours-max-batch-size-then-rolls-back-on-a-failed-signal autoscaling__cloudformation__ec2.deploy-autoscaling-1-replacing-update-swaps-group-failed-signal-replacement-rejected
Issue-Key: autoscaling__cloudformation__ec2.cfn-rolling-update-honours-max-batch-size-then-rolls-back-on-a-failed-signal|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: autoscaling__cloudformation__ec2.deploy-autoscaling-1-replacing-update-swaps-group-failed-signal-replacement-rejected|_wait_all#13|Timeout|time.sleep(15)
Issue-Commit: 129b431c719155b91f6d0a41f6794ea19123abad

# autoscaling__cloudformation__ec2: a CloudFormation stack whose AWS::AutoScaling::AutoScalingGroup carries a CreationPolicy ResourceSignal (and an UpdatePolicy) never reaches a terminal stack status

> _Note: this issue was identified against the code base at commit `129b431c719155b91f6d0a41f6794ea19123abad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new root cause, grounded in the two autoscaling__cloudformation__ec2 checks below (the only failures in this suite run that are neither marked ALREADY FILED nor OWNED by an open issue).

Cause 1 of 1: a CreateStack whose AWS::AutoScaling::AutoScalingGroup declares a CreationPolicy.ResourceSignal (and, in both fixtures, an UpdatePolicy — AutoScalingRollingUpdate with WaitOnResourceSignals in one, AutoScalingReplacingUpdate/WillReplace in the other) never settles: DescribeStacks keeps answering a non-terminal StackStatus for the whole observation window, so neither check ever gets a verdict and both are killed by the harness at its 60s deadline.

AWS: a CreateStack always converges to a terminal stack status. With a CreationPolicy.ResourceSignal of Count=N and Timeout=PT15M, CloudFormation holds the Auto Scaling group resource in CREATE_IN_PROGRESS until N SignalResource calls with Status=SUCCESS arrive for that LogicalResourceId (Smithy, cloudformation: SignalResource input StackName (required, StackNameOrId), LogicalResourceId (required), UniqueId (required, @length 1..64), Status (required, enum ResourceSignalStatus — SUCCESS/FAILURE)), and then reports CREATE_COMPLETE; if the signals do not arrive inside the CreationPolicy Timeout, or a FAILURE signal arrives, the resource fails and the stack converges to CREATE_FAILED and then ROLLBACK_IN_PROGRESS/ROLLBACK_COMPLETE. The same holds on the update path: an UpdateStack that changes the group's LaunchTemplate property runs the UpdatePolicy and converges to UPDATE_COMPLETE, or — per the documented rolling-update behavior the first check cites ("If a signal isn't received, CloudFormation cancels the update. Then, CloudFormation rolls back the stack") — to UPDATE_ROLLBACK_COMPLETE. Either way a client polling DescribeStacks sees a terminal StackStatus within a bounded time; AWS never parks a stack in a non-terminal status indefinitely. The two fixtures are marked DRAFT/unblessed, so for the exact wording of the stack events there is recorded behavior only; the requirement asserted here is only the one both AWS docs and the model imply — that the stack reaches SOME terminal status.

Emulator: neither stack ever reports a terminal status. Both checks begin by creating the stack and polling DescribeStacks(StackName)["Stacks"][0]["StackStatus"] every 5s for one of the terminal values, with their own 1500s/600s budgets; the emulator produces none, so the checks are still inside that first poll loop when the replay harness terminates them: "Error: Check timed out after 60.0s" for both. No refusal, no CREATE_FAILED, no rollback — the stack simply stays non-terminal. Nothing after the create is therefore exercised at all: the rolling-update batch cap, the FAILURE-signal rollback and the restored launch-template version are never reached.

Why: a client that deploys an Auto Scaling group through CloudFormation using the standard signal-based pattern (CreationPolicy.ResourceSignal fed by `aws cloudformation signal-resource` from instance UserData, plus AutoScalingRollingUpdate/AutoScalingReplacingUpdate) hangs forever. `aws cloudformation deploy`, `cdk deploy`, a stack waiter and any CI gate that waits on stack-create-complete/stack-update-complete never return and never fail — they cannot distinguish "still working" from "will never finish", and the failure path (a FAILURE signal must cancel the update and roll back) can never be observed or tested.

Fix: the CloudFormation stack-progress engine — the AWS::AutoScaling::AutoScalingGroup provisioner and the CreationPolicy/UpdatePolicy signal accounting it feeds (signal counting from SignalResource, CreationPolicy Timeout expiry, and the UpdatePolicy WaitOnResourceSignals/PauseTime path) must drive the stack to a terminal status in every case: CREATE_COMPLETE when the required signals arrive, CREATE_FAILED + rollback when the Timeout expires or a FAILURE signal arrives, and UPDATE_COMPLETE / UPDATE_ROLLBACK_COMPLETE on the update legs.

Related open issues that do NOT cover this (none carries either check's replay key): 20261024T015148Z-cloudformation-stack-outputs-are-unusable-and-create-complete.md and 20261029T175114Z-cloudformation-stack-outputs-carry-no-outputvalue-an-asg.md (ASG stack Outputs/CREATE_COMPLETE reporting, no CreationPolicy signal path), 20261029T214321Z-cloudformation-five-behavioral-checks-exhaust-the-harness-s.md (a different set of five cloudformation checks), 20261028T172843Z-autoscaling-cloudformation-cloudwatch-ec-autoscaling-cloudformation-cloudwatch-ec2-the-cdk-l2-asg.md (CDK L2 target-tracking drift scenario).

Traces: the two failing check ids in trace_ids.

## Evidence
Failing checks (both [mismatch], both unmarked by the report — not ALREADY FILED, not OWNED):

1) autoscaling__cloudformation__ec2.cfn-rolling-update-honours-max-batch-size-then-rolls-back-on-a-failed-signal
   Error: Check timed out after 60.0s
   The check's first action is _deploy_cfn(ctx, TEMPLATE, parameters=..., timeout_s=1500.0), which calls CreateStack and then polls `cfn.describe_stacks(StackName=name)["Stacks"][0]["StackStatus"]` every 5s until a terminal status; only then does it assert CREATE_COMPLETE. Template (deployments/asg-rolling-update-signals.yaml, inlined in the check): AutoScalingGroup with CreationPolicy.ResourceSignal {Count: !Ref DesiredCapacity (3), Timeout: PT15M} and UpdatePolicy.AutoScalingRollingUpdate {MaxBatchSize: 1, MinInstancesInService: 1, MinSuccessfulInstancesPercent: 100, PauseTime: PT15M, WaitOnResourceSignals: true}.

2) autoscaling__cloudformation__ec2.deploy-autoscaling-1-replacing-update-swaps-group-failed-signal-replacement-rejected
   Error: Check timed out after 60.0s
   Same shape: an inlined template whose AutoScalingGroup carries UpdatePolicy.AutoScalingReplacingUpdate {WillReplace: true} and a CreationPolicy ResourceSignal, DESIRED=2, signalled from UserData via `aws cloudformation signal-resource`.

Expected (AWS): DescribeStacks reports a terminal StackStatus — CREATE_COMPLETE once Count signals with Status=SUCCESS arrive for LogicalResourceId AutoScalingGroup, or CREATE_FAILED followed by ROLLBACK_IN_PROGRESS/ROLLBACK_COMPLETE when the CreationPolicy Timeout expires or a FAILURE signal arrives; on the update legs UPDATE_COMPLETE, or UPDATE_ROLLBACK_COMPLETE when a replacement signals FAILURE with MinSuccessfulInstancesPercent=100.

Actual (emulator): no terminal StackStatus is ever reported for either stack; the poll loop is still running when the harness kills the check at its 60s deadline. The sibling check in the same group that does NOT use a CreationPolicy (cfn-asg-availability-zones-derived-from-subnets) fails fast instead, under the already-filed stack-outputs/CREATE_COMPLETE issue — i.e. only the signal-gated stacks hang.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Create a stack from a template whose AWS::AutoScaling::AutoScalingGroup declares:
     CreationPolicy:
       ResourceSignal: {Count: 2, Timeout: PT15M}
     UpdatePolicy:
       AutoScalingRollingUpdate: {MaxBatchSize: 1, MinInstancesInService: 1, MinSuccessfulInstancesPercent: 100, PauseTime: PT15M, WaitOnResourceSignals: true}
   (or AutoScalingReplacingUpdate: {WillReplace: true}), in a VPC/subnet/launch-template estate, e.g. the inlined deployments/asg-rolling-update-signals.yaml from the first check:
     aws cloudformation create-stack --stack-name s --template-body file://asg-rolling-update-signals.yaml --parameters ... --capabilities CAPABILITY_IAM
3. Poll `aws cloudformation describe-stacks --stack-name s --query 'Stacks[0].StackStatus'`.
   Observed: the status never becomes terminal (no CREATE_COMPLETE, no CREATE_FAILED, no ROLLBACK_*), so `aws cloudformation wait stack-create-complete` never returns.
   Expected: it settles — CREATE_COMPLETE after the required SignalResource(Status=SUCCESS) calls, or CREATE_FAILED + ROLLBACK_COMPLETE on Timeout expiry / a FAILURE signal.
4. Optional second leg: send `aws cloudformation signal-resource --stack-name s --logical-resource-id AutoScalingGroup --unique-id <id> --status SUCCESS` once per expected instance and confirm the stack still never reaches CREATE_COMPLETE; then update the stack so the group's LaunchTemplate version changes and confirm no UPDATE_COMPLETE / UPDATE_ROLLBACK_COMPLETE is ever reported.
5. Replay the two checks in trace_ids to confirm the fix.
