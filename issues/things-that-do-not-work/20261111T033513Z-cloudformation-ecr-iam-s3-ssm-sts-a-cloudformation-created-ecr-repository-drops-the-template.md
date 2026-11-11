Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ecr__iam__s3__ssm__sts.cdk-bootstrap-add-tags-propagate-to-toolkit-resources-and-survive-a-tagless-re-bootstrap cloudformation__ecr__iam__s3__ssm__sts.cdk-bootstrap-customized-vendor-template-does-not-overwrite-the-default-toolkit cloudformation__ecr__iam__s3__ssm__sts.cdk-bootstrap-dumped-template-with-an-added-output-deploys-as-the-toolkit cloudformation__ecr__iam__s3__ssm__sts.cdk-bootstrap-re-bootstrap-after-stack-deletion-imports-the-retained-staging-bucket cloudformation__ecr__iam__s3__ssm__sts.cdk-bootstrap-re-bootstrap-with-a-new-tag-value-updates-every-toolkit-resource-in-place
Issue-Key: cloudformation__ecr__iam__s3__ssm__sts|GetLifecyclePolicy|LifecyclePolicyNotFoundException
Issue-Commit: a5271544d134d49a9bef1d276b891cd8aa450a13

# A CloudFormation-created ECR repository drops the template's LifecyclePolicy, so GetLifecyclePolicy refuses with LifecyclePolicyNotFoundException and five toolkit flows abort

> _Note: this issue was identified against the code base at commit `a5271544d134d49a9bef1d276b891cd8aa450a13`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetLifecyclePolicy

## Description
One net-new root cause for this service heading, hit by all five of its failing checks.

Cause 1 of 1: an AWS::ECR::Repository's LifecyclePolicy property is never applied, so the created repository has no lifecycle policy at all and every read of it is refused.
AWS:      Smithy (ecr) GetLifecyclePolicy -> registryId, repositoryName, lifecyclePolicyText, lastEvaluatedAt, with modeled error LifecyclePolicyNotFoundException (client, HTTP 400) documented as "The lifecycle policy could not be found, and no policy is set to the repository." - raised only when no policy was ever put. CloudFormation's AWS::ECR::Repository applies LifecyclePolicy.LifecyclePolicyText with ecr:PutLifecyclePolicy at create and update, so a repository declared with a policy answers HTTP 200 with that policy text, both on first bootstrap and after a re-bootstrap/update.
Emulator: HTTP 400 LifecyclePolicyNotFoundException with the message "Lifecycle policy does not exist for the repository with name '<repositoryName>' in the registry with id '<registryId>'" - the fixed text is everything but the two quoted values, which are taken from the request and from state. The refusal is well-formed; the defect is that the repository was created WITHOUT the template's lifecycle policy. Because the policy read happens while asserting the toolkit, all five flows (tag propagation, customized vendor template, dumped template with an added output, re-bootstrap after stack deletion, re-bootstrap with a new tag value) abort before their own subject is graded.
Why:      a client that declares image expiry in its template believes untagged images are reaped; nothing is reaped, and any read-back or drift check of the policy is refused instead of answered - so neither the create nor the update path of the property can be verified.
Fix:      the CloudFormation AWS::ECR::Repository resource handler's LifecyclePolicy property mapping onto ECR's lifecycle-policy state (create and update), not ECR's GetLifecyclePolicy handler.
Traces:   cloudformation__ecr__iam__s3__ssm__sts.cdk-bootstrap-add-tags-propagate-to-toolkit-resources-and-survive-a-tagless-re-bootstrap, cloudformation__ecr__iam__s3__ssm__sts.cdk-bootstrap-customized-vendor-template-does-not-overwrite-the-default-toolkit, cloudformation__ecr__iam__s3__ssm__sts.cdk-bootstrap-dumped-template-with-an-added-output-deploys-as-the-toolkit, cloudformation__ecr__iam__s3__ssm__sts.cdk-bootstrap-re-bootstrap-after-stack-deletion-imports-the-retained-staging-bucket, cloudformation__ecr__iam__s3__ssm__sts.cdk-bootstrap-re-bootstrap-with-a-new-tag-value-updates-every-toolkit-resource-in-place.

Related open issues that do NOT carry these checks' replay keys: 20261109T141609Z-ecr-a-cloudformation-aws-ecr-repository-that-declares.md and 20261110T232635Z-cloudformation-ecr-iam-s3-ssm-cdk-bootstrap-toolkit-fidelity-ecr-lifecyclepolicy-dropped-defaulted.md (both under other service headings).

## Evidence
All five checks of cloudformation__ecr__iam__s3__ssm__sts [error], identical failure:

botocore.errorfactory.LifecyclePolicyNotFoundException: An error occurred (LifecyclePolicyNotFoundException) when calling the GetLifecyclePolicy operation: Lifecycle policy does not exist for the repository with name 'cdk-<qualifier>-container-assets-<account>-<region>' in the registry with id '<account>'

Expected (AWS): HTTP 200 with lifecyclePolicyText equal to the policy the toolkit template declared (one rule, rulePriority 1, selection tagStatus untagged / countType sinceImagePushed, action expire).
Actual (emulator): HTTP 400 LifecyclePolicyNotFoundException. The repository name and registry id in the message are values from the request and state; the qualifier, account and region observed in each run are per-environment values, not requirements.

## Steps to Reproduce
1. Run `cdk bootstrap` against the emulator (plain, with --tags, with a customized/dumped vendor template, or a second time after deleting the toolkit stack - all five reproduce).
2. Wait for the toolkit stack to reach a terminal success status.
3. Call ecr:GetLifecyclePolicy(repositoryName='cdk-<qualifier>-container-assets-<account>-<region>').
4. Observe HTTP 400 LifecyclePolicyNotFoundException instead of HTTP 200 with the declared policy text.
