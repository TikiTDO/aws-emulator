Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.cdk-nested-stack-is-tracked-and-cascades-on-delete
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.cdk-nested-stack-is-tracked-and-cascades-on-delete|_teardown#13|AssertionError|raise AssertionError("teardown left residue: " + "; ".join(errors))
Issue-Commit: b94f7af2da1ac92db0383ac2b193617da0b34cf0

# DeleteStack on a root stack that owns a nested AWS::CloudFormation::Stack ends DELETE_FAILED instead of cascading to DELETE_COMPLETE

> _Note: this issue was identified against the code base at commit `b94f7af2da1ac92db0383ac2b193617da0b34cf0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteStack

## Description
Cause 1 of 1: deleting a root stack that contains an AWS::CloudFormation::Stack (nested-stack) resource does not cascade — the root stack settles in DELETE_FAILED and stays in the account, so nothing downstream of the delete can be observed and the deploy's toolkit stack and staging bucket cannot be reclaimed either. Failing check id: cloudformation.cdk-nested-stack-is-tracked-and-cascades-on-delete.

AWS: recorded behavior only for the cascade itself (the Smithy model gives DeleteStack only StackName plus RetainResources/RoleARN/ClientRequestToken/DeletionMode and models TokenAlreadyExistsException; it says nothing about nested-stack ordering). What the recording requires: DeleteStack on the root deletes the nested stack first and then the parent; the parent reaches DELETE_COMPLETE and is thereafter not-found by name — DescribeStacks(StackName=<parent name>) is refused HTTP 400 ValidationError with the message "Stack with id <name> does not exist" (fixed text "Stack with id " … " does not exist", the name taken from the request) — while DescribeStacks by the nested stack's stack id still answers, with StackStatus DELETE_COMPLETE. The resource the nested stack created (here an SSM parameter named by the bubbled-up output) must not outlive the cascade: GetParameter on it is refused ParameterNotFound. No status in the delete path is DELETE_FAILED when no resource refuses deletion and no termination protection is set (the check clears it before deleting).

Emulator: the parent stack's delete settles in DELETE_FAILED. The suite's bounded poller (DescribeStacks every 5 s until a terminal status) reads DELETE_FAILED and the teardown reports the stack, the toolkit stack and the staging bucket all still present: 'teardown left residue: app stack <name>: AssertionError("stack <name> reached DELETE_FAILED during teardown"); toolkit stack <name> and staging bucket kept: app stack not deleted'. Because the parent never leaves DELETE_FAILED, the three observations AWS makes after the cascade — parent not-found, nested stack DELETE_COMPLETE, nested resource gone — cannot be reached at all.

Why: a client that deletes a parent stack is told the delete failed while the stack, its nested child and the child's resources all remain. CDK `destroy`, any stack-set or pipeline teardown, and every test harness that deletes what it deployed leak resources and then fail to re-deploy, because the stack name is still taken and the retained staging bucket/toolkit stack cannot be removed. A DELETE_FAILED verdict also drives callers into the force-delete/continue-rollback paths for a delete that should simply have completed.

Fix: the DeleteStack path of the CloudFormation state machine — specifically the handling of an AWS::CloudFormation::Stack resource during parent deletion (delete the child stack, wait for its DELETE_COMPLETE, then finish the parent), and the resource-level delete result that is currently being turned into a stack-level DELETE_FAILED. Related open issues cover the nested-stack representation rather than this cascade and do not subsume it: 20261110T124641Z-cloudformation-a-nested-stack-s-parent-returns-outputs.md (parent Outputs carry no OutputValue), 20261107T060626Z-cloudformation-s3-cloudformation-nested-stacks-no-nested-change-set-tree.md (no nested change-set tree) and 20261111T133150Z-cloudformation-ec2-ssm-2-net-new-causes-a.md (an SSM::Association stack delete ending DELETE_FAILED, a different resource type and stack shape).

Traces: behavioral check cloudformation.cdk-nested-stack-is-tracked-and-cascades-on-delete (a `cdk deploy` of a NestedStack app, then UpdateTerminationProtection checks, then the parent delete).

## Evidence
From this behavioral replay, check cloudformation.cdk-nested-stack-is-tracked-and-cascades-on-delete:

  File "check_cloudformation_cdk_nested_stack_is_tracked_and_cascades_on_delete.py", line 176, in run
    _teardown(ctx, account, parent_stack, staging_bucket, ...)
  File "check_cloudformation_cdk_nested_stack_is_tracked_and_cascades_on_delete.py", line 358, in _teardown
    raise AssertionError("teardown left residue: " + "; ".join(errors))
  AssertionError: teardown left residue: app stack <parent stack name>: AssertionError("stack '<parent stack name>' reached DELETE_FAILED during teardown"); toolkit stack <toolkit stack name> and staging bucket kept: app stack not deleted

(The stack names in the message are this run's generated names and are not part of the requirement.)

Expected (AWS): DeleteStack(StackName=<parent>) → the parent polls to DELETE_COMPLETE; DescribeStacks(StackName=<parent>) is then refused HTTP 400 ValidationError "Stack with id <parent> does not exist"; DescribeStacks(StackName=<nested stack id>) reports StackStatus DELETE_COMPLETE; GetParameter on the parameter the nested stack created is refused ParameterNotFound.
Actual (emulator): the parent stack reaches DELETE_FAILED and remains, together with the nested stack, the toolkit stack and the staging bucket.

## Steps to Reproduce
1. `cdk bootstrap` a per-run qualifier, then `cdk deploy` the nested-stack app (a parent stack whose template declares one AWS::CloudFormation::Stack child; the child creates an SSM parameter and exports its name, which the parent re-exports as ChildParamName).
2. Confirm the parent is CREATE_COMPLETE and that DescribeStackResources on it lists exactly one AWS::CloudFormation::Stack whose PhysicalResourceId is the child's stack id.
3. Ensure termination protection is off on the parent (UpdateTerminationProtection(EnableTerminationProtection=False)).
4. Call DeleteStack(StackName=<parent>) and poll DescribeStacks until a terminal status.
5. Observe DELETE_FAILED; AWS reaches DELETE_COMPLETE, after which the parent is not-found, the child reads DELETE_COMPLETE by stack id, and the child's SSM parameter is gone.

Re-replay: cloudformation.cdk-nested-stack-is-tracked-and-cascades-on-delete.
