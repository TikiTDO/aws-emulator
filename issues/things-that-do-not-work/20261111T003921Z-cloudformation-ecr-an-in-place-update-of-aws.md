Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ecr.cfn-ecr-repository-create-update-in-place-replace-retained-then-delete
Issue-Key: cloudformation__ecr.cfn-ecr-repository-create-update-in-place-replace-retained-then-delete|_update#16|AssertionError|assert stack["StackStatus"] == "UPDATE_COMPLETE", (
Issue-Commit: b26d594b442f382e51363fa18dcb67bd5cc86e1e

# cloudformation__ecr: an in-place update of AWS::ECR::Repository is refused with invented NotImplemented prose, so the stack rolls back to UPDATE_ROLLBACK_COMPLETE

> _Note: this issue was identified against the code base at commit `b26d594b442f382e51363fa18dcb67bd5cc86e1e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
One net-new root cause (Cause 1 of 1), grounded in a failing behavioral check the replay report marks neither ALREADY FILED nor OWNED.

Cause 1 of 1: a stack update that changes only a no-interruption property of an existing AWS::ECR::Repository is refused by the emulator's own provisioner instead of being applied.
AWS: ImageTagMutability is an in-place (no-interruption) update property of AWS::ECR::Repository. CloudFormation applies it through ECR's own API (Smithy ecr#PutImageTagMutability, required repositoryName and imageTagMutability with values MUTABLE|IMMUTABLE), the repository keeps its identity (same repositoryName, same repositoryArn, same PhysicalResourceId), the stack resource reports UPDATE_COMPLETE and DescribeStacks reports StackStatus UPDATE_COMPLETE. AWS emits no refusal on this path at all.
Emulator: the update ends in UPDATE_ROLLBACK_COMPLETE. The stack reason is 'The following resource(s) failed to update: [<LogicalResourceId>]. ' and the resource's ResourceStatusReason is 'NotImplemented: this emulator does not change the properties of an existing AWS::ECR::Repository in place; the resource was left as it is.' Every word of that reason is the emulator's own prose - the fixed text is invented, and 'NotImplemented' is not an error code any AWS service answers; the variable parts are the template's logical id and the resource type. The repository is left with its original ImageTagMutability.
Why: a client cannot evolve an ECR repository through its template. Flipping a repository to IMMUTABLE tags (a standard supply-chain hardening step) both fails and parks the stack in a rolled-back state, so the template and the deployed estate diverge permanently and a pipeline that retries never converges.
Fix: the AWS::ECR::Repository arm of the CloudFormation per-type provisioner's update path (services/service-com-amazonaws-cloudformation-cloudformation/src/state/provisioner.rs, Provisioner::update, whose `_ => Ok(None)` fallthrough is what services/service-com-amazonaws-cloudformation-cloudformation/src/state/provisioning.rs `reconcile` turns into this NotImplemented reason) - it must map a changed ImageTagMutability (and the other in-place properties: ImageScanningConfiguration, EncryptionConfiguration where updatable, LifecyclePolicy, RepositoryPolicyText, Tags) onto the owning ECR calls and return success.
Traces: cloudformation__ecr.cfn-ecr-repository-create-update-in-place-replace-retained-then-delete.

Related but NOT covering this: the fixed issue 20261025T121312Z-cloudformation-a-stack-update-that-changes-an-aws.md removed exactly this refusal for AWS::IAM::Role only, and the open issue 20261109T141609Z-cloudformation-ecr-8-net-new-causes-two-aws.md records other ECR causes; neither carries this check's replay key.

## Evidence
Replay output (behavioral suite, section ### cloudformation__ecr):
  [mismatch] cloudformation__ecr.cfn-ecr-repository-create-update-in-place-replace-retained-then-delete
  AssertionError: update ended 'UPDATE_ROLLBACK_COMPLETE': 'The following resource(s) failed to update: [Repo]. '; [('Repo', 'NotImplemented: this emulator does not change the properties of an existing AWS::ECR::Repository in place; the resource was left as it is.')]

Expected (AWS): StackStatus UPDATE_COMPLETE, the repository's PhysicalResourceId unchanged and ecr DescribeRepositories reporting imageTagMutability IMMUTABLE.
Actual (emulator): StackStatus UPDATE_ROLLBACK_COMPLETE; resource reason as quoted. 'Repo' is the template's logical id (a request value); the rest of the reason is fixed emulator prose that AWS never emits.

## Steps to Reproduce
1. Build and boot the emulator from this checkout (cloudformation and ecr are both bootstrapped and serving).
2. CreateStack with one AWS::ECR::Repository (RepositoryName pinned, ImageTagMutability: MUTABLE). Wait for CREATE_COMPLETE.
3. UpdateStack with the same template except ImageTagMutability: IMMUTABLE (a no-interruption property - the repository is not replaced).
4. Poll DescribeStacks. Expect UPDATE_COMPLETE with the same PhysicalResourceId and ecr DescribeRepositories reporting IMMUTABLE. Observe UPDATE_ROLLBACK_COMPLETE and, in DescribeStackEvents, the resource reason 'NotImplemented: this emulator does not change the properties of an existing AWS::ECR::Repository in place; the resource was left as it is.'
5. Equivalently: re-replay cloudformation__ecr.cfn-ecr-repository-create-update-in-place-replace-retained-then-delete.
