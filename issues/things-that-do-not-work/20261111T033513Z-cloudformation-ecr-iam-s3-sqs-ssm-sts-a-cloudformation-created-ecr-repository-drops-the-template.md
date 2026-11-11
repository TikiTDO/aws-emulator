Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ecr__iam__s3__sqs__ssm__sts.cdk-bootstrap-deploy-without-execution-role-runs-as-the-session-tagged-deploy-role
Issue-Key: cloudformation__ecr__iam__s3__sqs__ssm__sts|GetLifecyclePolicy|LifecyclePolicyNotFoundException
Issue-Commit: a5271544d134d49a9bef1d276b891cd8aa450a13

# A CloudFormation-created ECR repository drops the template's LifecyclePolicy: GetLifecyclePolicy refuses with LifecyclePolicyNotFoundException

> _Note: this issue was identified against the code base at commit `a5271544d134d49a9bef1d276b891cd8aa450a13`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetLifecyclePolicy

## Description
One net-new root cause for this service heading, reached by bootstrapping the CDK toolkit and reading its container-assets repository's lifecycle policy back through ECR.

Cause 1 of 1: an AWS::ECR::Repository's LifecyclePolicy property is never applied, so the created repository has no lifecycle policy at all and the read of it is refused.
AWS:      Smithy (ecr) GetLifecyclePolicy -> registryId, repositoryName, lifecyclePolicyText, lastEvaluatedAt, with modeled error LifecyclePolicyNotFoundException (client, HTTP 400) documented as "The lifecycle policy could not be found, and no policy is set to the repository." - raised only when no policy was ever put. CloudFormation's AWS::ECR::Repository applies LifecyclePolicy.LifecyclePolicyText with ecr:PutLifecyclePolicy at create, so a repository declared with a policy answers HTTP 200 with that policy text.
Emulator: HTTP 400 LifecyclePolicyNotFoundException with the message "Lifecycle policy does not exist for the repository with name '<repositoryName>' in the registry with id '<registryId>'" - the fixed text is everything but the two quoted values, which are taken from the request and from state. The refusal is well-formed; the defect is that the repository was created WITHOUT the template's lifecycle policy, which aborts the deploy flow at its asset-publishing step.
Why:      a client that declares image expiry in its template believes untagged images are reaped; nothing is reaped and every read-back or drift check of the policy is refused instead of answered.
Fix:      the CloudFormation AWS::ECR::Repository resource handler's LifecyclePolicy property mapping onto ECR's lifecycle-policy state, not ECR's GetLifecyclePolicy handler.
Traces:   cloudformation__ecr__iam__s3__sqs__ssm__sts.cdk-bootstrap-deploy-without-execution-role-runs-as-the-session-tagged-deploy-role.

Related open issues that do NOT carry this check's replay key: 20261109T141609Z-ecr-a-cloudformation-aws-ecr-repository-that-declares.md and 20261110T232635Z-cloudformation-ecr-iam-s3-ssm-cdk-bootstrap-toolkit-fidelity-ecr-lifecyclepolicy-dropped-defaulted.md (other service headings). The open issue under THIS heading, 20261110T232635Z-cloudformation-ecr-iam-s3-sqs-ssm-sts-cdk-bootstrap-staging-bucket-reports-aes256-default-encryption.md, is about S3 bucket encryption and does not cover this cause.

## Evidence
Check cloudformation__ecr__iam__s3__sqs__ssm__sts.cdk-bootstrap-deploy-without-execution-role-runs-as-the-session-tagged-deploy-role [error]:

botocore.errorfactory.LifecyclePolicyNotFoundException: An error occurred (LifecyclePolicyNotFoundException) when calling the GetLifecyclePolicy operation: Lifecycle policy does not exist for the repository with name 'cdk-<qualifier>-container-assets-<account>-<region>' in the registry with id '<account>'

Expected (AWS): HTTP 200 with lifecyclePolicyText equal to the policy the toolkit template declared (one rule, rulePriority 1, selection tagStatus untagged / countType sinceImagePushed, action expire).
Actual (emulator): HTTP 400 LifecyclePolicyNotFoundException; the repository name and registry id in the message are values from the request/state, the qualifier/account/region in the recorded run are per-environment values.

## Steps to Reproduce
1. Run `cdk bootstrap` against the emulator without --cloudformation-execution-policies, then `cdk deploy` an app.
2. Wait for the toolkit stack to reach CREATE_COMPLETE.
3. Call ecr:GetLifecyclePolicy(repositoryName='cdk-<qualifier>-container-assets-<account>-<region>').
4. Observe HTTP 400 LifecyclePolicyNotFoundException instead of HTTP 200 with the declared policy text.
