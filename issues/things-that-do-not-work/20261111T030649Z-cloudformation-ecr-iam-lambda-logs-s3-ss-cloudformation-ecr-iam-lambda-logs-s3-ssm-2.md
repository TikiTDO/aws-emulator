Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ecr__iam__lambda__logs__s3__ssm.cdk-bootstrap-omitted-execution-policies-default-to-admin-for-a-lambda-deploy cloudformation__ecr__iam__lambda__logs__s3__ssm.cdk-bootstrap-unknown-toolkit-stack-name-still-deploys-a-lambda-asset cloudformation__ecr__iam__lambda__logs__s3__ssm.cdk-bootstrap-legacy-upgrade-in-use-retains-old-bucket-redeploys-from-new
Issue-Key: cloudformation__ecr__iam__lambda__logs__s3__ssm|GetLifecyclePolicy|LifecyclePolicyNotFoundException
Issue-Key: cloudformation__ecr__iam__lambda__logs__s3__ssm.cdk-bootstrap-legacy-upgrade-in-use-retains-old-bucket-redeploys-from-new|_assert_function#29|AssertionError|assert (response["StatusCode"], response.get("FunctionError")) == (200, None), (
Issue-Commit: 6e1ef7d8ebea1acbe69ee2691b3319db9dddeedf

# cloudformation__ecr__iam__lambda__logs__s3__ssm: 2 net-new causes — a stack-declared ECR LifecyclePolicy is never stored, and an asset-bundle Lambda cannot read a file packaged beside its handler

> _Note: this issue was identified against the code base at commit `6e1ef7d8ebea1acbe69ee2691b3319db9dddeedf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetLifecyclePolicy

## Description
Two net-new root causes. None of these three checks is marked ALREADY FILED or OWNED. The open issue 20261110T232635Z-cloudformation-ecr-iam-lambda-logs-s3-ss-a-lambda-function-deployed-from-an-asset-bundle.md covers two sibling checks of cause 2 but does not carry the replay key of the check listed below, and 20261109T141609Z-ecr-ecs-iam-the-ecr-access-e2e-deploy.md covers cause 1's shape for a different service family only.

Cause 1 of 2: an AWS::ECR::Repository whose template declares a LifecyclePolicy is created without that policy, so GetLifecyclePolicy is refused.
AWS: a stack that creates an AWS::ECR::Repository carrying the LifecyclePolicy property sets the repository's lifecycle policy at create time; ecr GetLifecyclePolicy(repositoryName=<repo>) then returns registryId, repositoryName and lifecyclePolicyText. Smithy: ecr GetLifecyclePolicy declares LifecyclePolicyNotFoundException (client, HTTP 400) for a repository that has no policy, with the fixed message text "Lifecycle policy does not exist for the repository with name '<repositoryName>' in the registry with id '<registryId>'" — only the repository name and registry id come from the request/account.
Emulator: the bootstrap stack deploys, but GetLifecyclePolicy on the CDK bootstrap container-assets repository raises LifecyclePolicyNotFoundException for a repository whose template declared a policy. Both checks die in their bootstrap phase before reaching their subject.
Why: every CDK app on the standard bootstrap stack ends up with a container-assets repository that has no retention rules, and code that reads the policy back is refused — while the stack reports success.
Fix: the CloudFormation AWS::ECR::Repository resource handler must apply the declared LifecyclePolicy into ECR state on create and update.
Traces: cloudformation__ecr__iam__lambda__logs__s3__ssm.cdk-bootstrap-omitted-execution-policies-default-to-admin-for-a-lambda-deploy, cloudformation__ecr__iam__lambda__logs__s3__ssm.cdk-bootstrap-unknown-toolkit-stack-name-still-deploys-a-lambda-asset.

Cause 2 of 2: a Lambda function deployed from an asset bundle cannot open a file that was packaged next to its handler.
AWS: a function's deployment package is extracted under /var/task, so a handler may read any file shipped in the bundle — `(pathlib.Path(__file__).parent / "response.json").read_text()` succeeds and Invoke returns StatusCode 200 with the handler's payload and NO X-Amz-Function-Error header.
Emulator: Invoke returns HTTP 200 with X-Amz-Function-Error 'Unhandled' and the payload {"errorMessage":"a Lambda function in this emulator cannot open a file of the host","errorType":"OSError", ...} — an emulator-authored refusal, not an AWS error type. The whole asset-bundle deployment path (the CDK legacy->new-style bootstrap upgrade this check exercises) therefore cannot be graded.
Why: a client whose handler reads any packaged data file — templates, model files, config, response fixtures, vendored modules loaded by path — gets a function error AWS would never produce, and the error type/message are not anything a client can match on.
Fix: the Lambda invoke runtime's filesystem view for the function's deployment package — the extracted bundle must be readable at /var/task (the handler's own directory) instead of being refused as host access.
Traces: cloudformation__ecr__iam__lambda__logs__s3__ssm.cdk-bootstrap-legacy-upgrade-in-use-retains-old-bucket-redeploys-from-new.

## Evidence
Cause 1 — Expected: GetLifecyclePolicy returns the lifecyclePolicyText the AWS::ECR::Repository declared. Actual (both checks): `botocore.errorfactory.LifecyclePolicyNotFoundException: An error occurred (LifecyclePolicyNotFoundException) when calling the GetLifecyclePolicy operation: Lifecycle policy does not exist for the repository with name 'cdk-<qualifier>-container-assets-000000000000-us-west-2' in the registry with id '000000000000'` (observed with qualifiers q294fbd6e0 and q85adf9a99 — per-run values).
Cause 2 — Expected: Invoke returns 200 with the handler's JSON payload and no FunctionError. Actual: `AssertionError: Invoke returned status 200 error 'Unhandled': b'{"errorMessage":"a Lambda function in this emulator cannot open a file of the host","errorType":"OSError","requestId":"...","stackTrace":["  File \\"/var/task/index.py\\", line 6, in handler\\n    return json.loads((pathlib.Path(__file__).parent / \\"response.json\\").read_text())\\n", ...]}'`.

## Steps to Reproduce
Cause 1: deploy the CDK bootstrap stack (with omitted execution policies, and again under an unknown toolkit stack name) and then call ecr GetLifecyclePolicy(repositoryName='cdk-<qualifier>-container-assets-<account>-<region>').
Cause 2: publish a CDK Lambda asset bundle containing index.py plus response.json, deploy it via CloudFormation, then lambda Invoke the function; the handler reads response.json from its own directory.
