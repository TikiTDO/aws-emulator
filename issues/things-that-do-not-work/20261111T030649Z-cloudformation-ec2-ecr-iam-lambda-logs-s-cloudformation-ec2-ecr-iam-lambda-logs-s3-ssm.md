Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__ecr__iam__lambda__logs__s3__ssm.cdk-bootstrap-session-tags-unlock-lookup-deploy-and-asset-publishing
Issue-Key: cloudformation__ec2__ecr__iam__lambda__logs__s3__ssm|GetLifecyclePolicy|LifecyclePolicyNotFoundException
Issue-Commit: 6e1ef7d8ebea1acbe69ee2691b3319db9dddeedf

# cloudformation__ec2__ecr__iam__lambda__logs__s3__ssm: a stack-declared ECR repository LifecyclePolicy is never stored, so GetLifecyclePolicy answers LifecyclePolicyNotFoundException and the CDK bootstrap flow dies

> _Note: this issue was identified against the code base at commit `6e1ef7d8ebea1acbe69ee2691b3319db9dddeedf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetLifecyclePolicy

## Description
One net-new root cause. This check is not marked ALREADY FILED and is not OWNED. The open issue 20261109T141609Z-ecr-ecs-iam-the-ecr-access-e2e-deploy.md describes the same shape for a different service family but does not carry this check's replay key.

Cause 1 of 1: an AWS::ECR::Repository whose template declares a LifecyclePolicy is created without that lifecycle policy, so ECR has none to return.
AWS: when a stack creates an AWS::ECR::Repository carrying the LifecyclePolicy property (the CDK bootstrap stack's container-assets repository does exactly this), the repository's lifecycle policy is set at create time. ecr GetLifecyclePolicy(repositoryName=<repo>) then returns registryId, repositoryName and lifecyclePolicyText; LifecyclePolicyNotFoundException is reserved for a repository that genuinely has no policy. Smithy: ecr GetLifecyclePolicy errors include LifecyclePolicyNotFoundException (client, HTTP 400) alongside RepositoryNotFoundException; its message is the fixed text "Lifecycle policy does not exist for the repository with name '<repositoryName>' in the registry with id '<registryId>'" — repositoryName and registryId are values from the request/account, the rest is fixed.
Emulator: the bootstrap deploy succeeds, but GetLifecyclePolicy on the bootstrap container-assets repository raises LifecyclePolicyNotFoundException: "Lifecycle policy does not exist for the repository with name 'cdk-<qualifier>-container-assets-<account>-<region>' in the registry with id '<account>'". The repository name and registry id are per-run values; the defect is that the policy the template declared was never stored, so a correct refusal is returned for a repository that should have one. The check cannot get past its bootstrap phase.
Why: any client whose stack declares a repository lifecycle policy — every CDK app that uses the standard bootstrap stack — sees a repository with no retention rules at all, and code that reads the policy back (to assert it, or to merge rules into it) is refused. Images are never expired and the stack's declared intent is silently dropped.
Fix: the CloudFormation AWS::ECR::Repository resource handler must apply the LifecyclePolicy property to ECR state (the same state PutLifecyclePolicy/GetLifecyclePolicy read and write) on create and on update.
Traces: cloudformation__ec2__ecr__iam__lambda__logs__s3__ssm.cdk-bootstrap-session-tags-unlock-lookup-deploy-and-asset-publishing.

## Evidence
Cause 1 — Expected: after the bootstrap stack creates the container-assets repository with a declared LifecyclePolicy, GetLifecyclePolicy(repositoryName=<that repo>) returns the policy text. Actual: `botocore.errorfactory.LifecyclePolicyNotFoundException: An error occurred (LifecyclePolicyNotFoundException) when calling the GetLifecyclePolicy operation: Lifecycle policy does not exist for the repository with name 'cdk-q666765547-container-assets-000000000000-us-west-2' in the registry with id '000000000000'` — fixed text as AWS words it, with a per-run repository name and the account id as its variable parts.

## Steps to Reproduce
1. Deploy the CDK bootstrap stack (CloudFormation CreateStack) whose AWS::ECR::Repository declares a LifecyclePolicy for the container-assets repository; wait for CREATE_COMPLETE.
2. Call ecr GetLifecyclePolicy(repositoryName='cdk-<qualifier>-container-assets-<account>-<region>').
Observed: LifecyclePolicyNotFoundException. Expected: the lifecyclePolicyText the template declared, with registryId and repositoryName echoed.
