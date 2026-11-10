Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ecr__iam__s3__ssm.cdk-bootstrap-container-assets-repository-refuses-to-overwrite-an-image-tag cloudformation__ecr__iam__s3__ssm.cdk-bootstrap-custom-permissions-boundary-bounds-only-the-exec-role cloudformation__ecr__iam__s3__ssm.cdk-bootstrap-custom-permissions-boundary-with-a-path-bounds-the-exec-role cloudformation__ecr__iam__s3__ssm.cdk-bootstrap-example-permissions-boundary-bounds-the-exec-role-and-denies-its-removal cloudformation__ecr__iam__s3__ssm.cdk-bootstrap-removing-custom-permissions-boundary-clears-it-from-the-exec-role-in-place cloudformation__ecr__iam__s3__ssm.cdk-bootstrap-image-asset-already-in-ecr-is-not-rebuilt-or-pushed-again cloudformation__ecr__iam__s3__ssm.cdk-bootstrap-termination-protection-survives-a-forced-re-bootstrap-and-refuses-delete cloudformation__ecr__iam__s3__ssm.cdk-bootstrap-untrust-removes-the-account-from-every-role-trust-in-place
Issue-Key: cloudformation__ecr__iam__s3__ssm|GetLifecyclePolicy|LifecyclePolicyNotFoundException
Issue-Key: cloudformation__ecr__iam__s3__ssm.cdk-bootstrap-custom-permissions-boundary-bounds-only-the-exec-role|run#17|AssertionError|assert got == (policy_name, "false"), (
Issue-Key: cloudformation__ecr__iam__s3__ssm.cdk-bootstrap-custom-permissions-boundary-with-a-path-bounds-the-exec-role|run#18|AssertionError|assert got == (boundary, "false"), (
Issue-Key: cloudformation__ecr__iam__s3__ssm.cdk-bootstrap-example-permissions-boundary-bounds-the-exec-role-and-denies-its-removal|_assert_bootstrap_roles#23|AssertionError|assert got.get("PermissionsBoundary") == want_boundary, (
Issue-Key: cloudformation__ecr__iam__s3__ssm.cdk-bootstrap-removing-custom-permissions-boundary-clears-it-from-the-exec-role-in-place|_assert_bootstrap_roles#15|AssertionError|assert got.get("PermissionsBoundary") == want_boundary, (
Issue-Key: cloudformation__ecr__iam__s3__ssm.cdk-bootstrap-image-asset-already-in-ecr-is-not-rebuilt-or-pushed-again|_assert_staging_bucket#36|AssertionError|assert defaults == _KMS_DEFAULT_RULES, (
Issue-Key: cloudformation__ecr__iam__s3__ssm.cdk-bootstrap-termination-protection-survives-a-forced-re-bootstrap-and-refuses-delete|_assert_staging_bucket#18|AssertionError|assert defaults == _KMS_DEFAULT_RULES, (
Issue-Key: cloudformation__ecr__iam__s3__ssm.cdk-bootstrap-untrust-removes-the-account-from-every-role-trust-in-place|_assert_bootstrap_roles#12|AssertionError|assert trust == want, f"{role} trust statements are {trust!r}, expected {want!r}"
Issue-Commit: cd6d73564e5f4cf53934bca1327f08de31f2ee5f

# cdk bootstrap toolkit fidelity: ECR LifecyclePolicy dropped, defaulted stack parameters unreported, IAM PermissionsBoundary dropped, bucket encryption downgraded, account principal not canonicalized (5 causes)

> _Note: this issue was identified against the code base at commit `cd6d73564e5f4cf53934bca1327f08de31f2ee5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetLifecyclePolicy

## Description
Five net-new root causes, all reached by deploying the CDK bootstrap toolkit template and reading the resources back.

Cause 1 of 5: an AWS::ECR::Repository's LifecyclePolicy property is never applied, so the repository has no lifecycle policy.
AWS: Smithy (ecr) GetLifecyclePolicy -> lifecyclePolicyText, with error LifecyclePolicyNotFoundException (HTTP 400) "The lifecycle policy for the specified repository could not be found." - raised only when no policy was ever put. The bootstrap template gives its container-assets repository one rule (rulePriority 1, description "Untagged images should not exist, but expire any older than one year", selection tagStatus untagged / countType sinceImagePushed), so AWS answers 200 with that policy text.
Emulator: GetLifecyclePolicy on the repository the stack just created answers 400 LifecyclePolicyNotFoundException "Lifecycle policy does not exist for the repository with name '<repository>' in the registry with id '<registry-id>'" (fixed text; the repository name and registry id are values from the request and state). The refusal itself is well-formed - the defect is that the repository was created WITHOUT the template's lifecycle policy.
Why: a client that declares image expiry in its template believes untagged images expire; nothing expires, and any read-back or drift check of the policy is refused instead of answered.
Fix: the CloudFormation AWS::ECR::Repository resource handler's LifecyclePolicy property mapping onto ECR's lifecycle-policy state.
Traces: cdk-bootstrap-container-assets-repository-refuses-to-overwrite-an-image-tag.

Cause 2 of 5: DescribeStacks omits template parameters the caller did not supply, instead of reporting them with their resolved Default.
AWS: Smithy (cloudformation) DescribeStacks -> Stacks[*].Parameters, a list of Parameter{ParameterKey, ParameterValue, ResolvedValue, UsePreviousValue}; AWS reports EVERY parameter the stack's template declares with the value in effect, so a parameter left at its template Default is present with that default as ParameterValue. The bootstrap template declares ApplyPermissionsBoundaryToAllRoles with Default "false" and the CLI supplies only InputPermissionsBoundary, so AWS reports both keys, the second as "false".
Emulator: Stack.Parameters contains only the explicitly supplied parameters - ApplyPermissionsBoundaryToAllRoles is absent (reads as None).
Why: a client that reads a stack's effective configuration back from DescribeStacks (drift tooling, re-deploy with UsePreviousValue, change-set previews) sees a parameter as unset when it has an effective value, and cannot recover the default without re-fetching and parsing the template.
Fix: the CloudFormation DescribeStacks projection of a stack's parameters - resolve each declared parameter against the template Default when the caller supplied none.
Traces: cdk-bootstrap-custom-permissions-boundary-bounds-only-the-exec-role, cdk-bootstrap-custom-permissions-boundary-with-a-path-bounds-the-exec-role.

Cause 3 of 5: a role created with a permissions boundary carries none.
AWS: Smithy (iam) GetRole -> Role$PermissionsBoundary, an AttachedPermissionsBoundary{PermissionsBoundaryType: "Policy", PermissionsBoundaryArn}. CreateRole/PutRolePermissionsBoundary accept PermissionsBoundary, and CloudFormation's AWS::IAM::Role has a PermissionsBoundary property; GetPolicy reports PermissionsBoundaryUsageCount for the bounding policy. When the toolkit template bounds the cfn-exec role, AWS reports {PermissionsBoundaryType: "Policy", PermissionsBoundaryArn: arn:aws:iam::<account>:policy/<boundary-name>} on that role.
Emulator: GetRole on the cfn-exec role reports no PermissionsBoundary at all (None) after a bootstrap that named a boundary.
Why: the boundary is the only thing limiting what the deployment role may do; a client told the role is bounded is actually running unbounded, and a boundary-removal path cannot be observed either (the "before" state already looks unbounded).
Fix: the CloudFormation AWS::IAM::Role resource handler's PermissionsBoundary property mapping onto IAM's role state, and IAM's role projection that reports it.
A similar open issue exists under a different service heading - 20261109T163411Z-cloudformation-dynamodb-iam-sqs-cdk-bootstrap-custom-permissions-boundary-leaves-the-synthesized.md - it does not carry these checks' replay keys.
Traces: cdk-bootstrap-example-permissions-boundary-bounds-the-exec-role-and-denies-its-removal, cdk-bootstrap-removing-custom-permissions-boundary-clears-it-from-the-exec-role-in-place.

Cause 4 of 5: the staging bucket reports SSE-S3 (AES256) plus an unsolicited BlockedEncryptionTypes instead of the template's aws:kms rule.
AWS: Smithy (s3) GetBucketEncryption -> ServerSideEncryptionConfiguration.Rules[*]; ServerSideEncryptionByDefault.SSEAlgorithm is @required (enum AES256 | aws:kms | ...) and BlockedEncryptionTypes is documented as an opt-in setting that "currently only supports blocking or unblocking server-side encryption with customer-provided keys (SSE-C)". For the toolkit's staging bucket AWS answers exactly one rule: {ApplyServerSideEncryptionByDefault: {SSEAlgorithm: aws:kms}, BucketKeyEnabled: false}.
Emulator: {SSEAlgorithm: AES256, BucketKeyEnabled: false, BlockedEncryptionTypes: {EncryptionType: [SSE-C]}}.
Why: a client cannot distinguish SSE-KMS from SSE-S3 on a bucket it created from a template, and a Get -> Put round trip would assert an SSE-C block the bucket never had.
Fix: the CloudFormation AWS::S3::Bucket handler's BucketEncryption property mapping, and S3's GetBucketEncryption serializer (omit BlockedEncryptionTypes unless configured).
Traces: cdk-bootstrap-image-asset-already-in-ecr-is-not-rebuilt-or-pushed-again, cdk-bootstrap-termination-protection-survives-a-forced-re-bootstrap-and-refuses-delete.

Cause 5 of 5: a trust-policy principal given as a bare account id is returned as the bare account id instead of the canonical root ARN.
AWS: IAM canonicalizes an account principal when it stores a trust policy - a Principal {"AWS": "<account-id>"} is stored and returned as {"AWS": "arn:aws:iam::<account-id>:root"} (IAM JSON policy elements: Principal). So GetRole's AssumeRolePolicyDocument for the bootstrap roles carries Principal.AWS = ["arn:aws:iam::<account>:root"] on both the sts:AssumeRole and the sts:TagSession statements, whatever form the template used.
Emulator: the document is echoed verbatim - Principal.AWS = ["<account-id>"].
Why: tooling that compares, edits or asserts on a trust policy (CDK's untrust path, policy linters, drift detection) matches on the canonical root ARN and therefore sees no trusted account at all, so a trust-removal or trust-audit step cannot work.
Fix: IAM's trust-policy normalization on role create/update, so the stored and reported AssumeRolePolicyDocument carries the canonical principal.
A related open issue mentions IAM trust-policy projection defects - 20261104T152822Z-iam-role-and-policy-projections-5-net-new.md - it does not carry this check's replay key.
Traces: cdk-bootstrap-untrust-removes-the-account-from-every-role-trust-in-place.

## Evidence
Cause 1 - cdk-bootstrap-container-assets-repository-refuses-to-overwrite-an-image-tag:
botocore.errorfactory.LifecyclePolicyNotFoundException: An error occurred (LifecyclePolicyNotFoundException) when calling the GetLifecyclePolicy operation: Lifecycle policy does not exist for the repository with name 'cdk-<qualifier>-container-assets-<account>-<region>' in the registry with id '<account>'
Expected (AWS): 200 with lifecyclePolicyText == {"rules":[{"rulePriority":1,"description":"Untagged images should not exist, but expire any older than one year","selection":{"tagStatus":"untagged","countType":"sinceImagePushed",...}}]}.

Cause 2 - cdk-bootstrap-custom-permissions-boundary-bounds-only-the-exec-role / ...-with-a-path-...:
AssertionError: toolkit (InputPermissionsBoundary, ApplyPermissionsBoundaryToAllRoles) is ('<boundary-name>', None), expected ('<boundary-name>', 'false')
(read through DescribeStacks -> Stacks[0].Parameters)

Cause 3 - cdk-bootstrap-example-permissions-boundary-... / cdk-bootstrap-removing-custom-permissions-boundary-...:
AssertionError: cfn-exec-role permissions boundary is None, expected {'PermissionsBoundaryType': 'Policy', 'PermissionsBoundaryArn': 'arn:aws:iam::<account>:policy/<boundary-name>'}

Cause 4 - cdk-bootstrap-image-asset-already-in-ecr-... / cdk-bootstrap-termination-protection-...:
AssertionError: bucket <staging-bucket> encryption rules are [{'ApplyServerSideEncryptionByDefault': {'SSEAlgorithm': 'AES256'}, 'BucketKeyEnabled': False, 'BlockedEncryptionTypes': {'EncryptionType': ['SSE-C']}}], expected [{'ApplyServerSideEncryptionByDefault': {'SSEAlgorithm': 'aws:kms'}, 'BucketKeyEnabled': False}]

Cause 5 - cdk-bootstrap-untrust-removes-the-account-from-every-role-trust-in-place:
AssertionError: deploy-role trust statements are [{'Effect':'Allow','Condition':{'Null':{'sts:ExternalId':'true'}},'Action':['sts:AssumeRole'],'Principal':{'AWS':['<account-id>']}}, {'Effect':'Allow','Action':['sts:TagSession'],'Principal':{'AWS':['<account-id>']}}, ...], expected the same statements with 'Principal': {'AWS': ['arn:aws:iam::<account-id>:root']}

## Steps to Reproduce
Common setup: `cdk bootstrap` the toolkit stack against the emulator (with `--custom-permissions-boundary <policy>` for causes 2 and 3).

1. Cause 1: call ecr:GetLifecyclePolicy on the cdk-<qualifier>-container-assets-<account>-<region> repository -> 400 LifecyclePolicyNotFoundException instead of the template's policy text.
2. Cause 2: call cloudformation:DescribeStacks on the toolkit stack and read Stacks[0].Parameters -> ApplyPermissionsBoundaryToAllRoles (template Default "false") is absent.
3. Cause 3: call iam:GetRole on cdk-<qualifier>-cfn-exec-role-<account>-<region> -> no PermissionsBoundary member.
4. Cause 4: call s3:GetBucketEncryption on the staging bucket -> AES256 + BlockedEncryptionTypes.
5. Cause 5: call iam:GetRole on any cdk-<qualifier>-* role and inspect AssumeRolePolicyDocument -> Principal.AWS is the bare account id, not arn:aws:iam::<account>:root.
