Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ecr__iam__lambda__logs__s3__ssm.cdk-bootstrap-default-synthesis-without-new-style-flag-deploys-as-exec-role cloudformation__ecr__iam__lambda__logs__s3__ssm.cdk-bootstrap-new-style-synthesis-deploys-the-asset-lambda-as-the-exec-role cloudformation__ecr__iam__lambda__logs__s3__ssm.cdk-bootstrap-legacy-upgrade-in-use-retains-old-bucket-redeploys-from-new cloudformation__ecr__iam__lambda__logs__s3__ssm.cdk-bootstrap-omitted-execution-policies-default-to-admin-for-a-lambda-deploy cloudformation__ecr__iam__lambda__logs__s3__ssm.cdk-bootstrap-unknown-toolkit-stack-name-still-deploys-a-lambda-asset
Issue-Key: cloudformation__ecr__iam__lambda__logs__s3__ssm.cdk-bootstrap-default-synthesis-without-new-style-flag-deploys-as-exec-role|_assert_function#24|AssertionError|assert (response["StatusCode"], response.get("FunctionError")) == (200, None), (
Issue-Key: cloudformation__ecr__iam__lambda__logs__s3__ssm.cdk-bootstrap-new-style-synthesis-deploys-the-asset-lambda-as-the-exec-role|_assert_function#24|AssertionError|assert (response["StatusCode"], response.get("FunctionError")) == (200, None), (
Issue-Key: cloudformation__ecr__iam__lambda__logs__s3__ssm.cdk-bootstrap-legacy-upgrade-in-use-retains-old-bucket-redeploys-from-new|_assert_staging_bucket#21|AssertionError|assert defaults == _KMS_DEFAULT_RULES, (
Issue-Key: cloudformation__ecr__iam__lambda__logs__s3__ssm.cdk-bootstrap-omitted-execution-policies-default-to-admin-for-a-lambda-deploy|_assert_modern_toolkit#13|AssertionError|assert defaults == _KMS_DEFAULT_RULES, (
Issue-Key: cloudformation__ecr__iam__lambda__logs__s3__ssm.cdk-bootstrap-unknown-toolkit-stack-name-still-deploys-a-lambda-asset|_assert_modern_toolkit#14|AssertionError|assert defaults == _KMS_DEFAULT_RULES, (
Issue-Commit: cd6d73564e5f4cf53934bca1327f08de31f2ee5f

# A Lambda function deployed from an asset bundle cannot open a file next to its handler, and the bootstrap staging bucket reports AES256 instead of aws:kms (2 causes)

> _Note: this issue was identified against the code base at commit `cd6d73564e5f4cf53934bca1327f08de31f2ee5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Two net-new root causes, both reached through `cdk bootstrap` + `cdk deploy` of a file-asset Lambda.

Cause 1 of 2: a function whose deployment package bundles a data file beside the handler cannot read that file - the invocation fails with an emulator-authored OSError.
AWS: Smithy (lambda) Invoke -> StatusCode (200 for RequestResponse) and FunctionError ("If present, indicates that an error occurred during function execution. Details about the error are included in the response payload."). AWS unpacks the WHOLE deployment package under /var/task, so a handler reading `pathlib.Path(__file__).parent / "<bundled file>"` succeeds and Invoke answers StatusCode 200 with NO FunctionError and the handler's own payload.
Emulator: Invoke answers StatusCode 200 with FunctionError 'Unhandled' and a payload whose errorType is OSError and whose errorMessage is the emulator's own prose "a Lambda function in this emulator cannot open a file of the host", with a stackTrace through pathlib read_text. The message is emulator-authored: AWS never names "this emulator" and never refuses a read of the function's own code directory.
Why: every function that ships configuration, templates, model files or (as here) a CDK/SAM asset bundle next to its handler fails at runtime, and the failure is reported as an application error, so a client cannot distinguish its own bug from the emulator's missing filesystem.
Fix: the Lambda invoke runtime's filesystem for the function's unpacked code bundle (the whole package must be readable at /var/task), not the handler of Invoke itself.
Traces: cdk-bootstrap-default-synthesis-without-new-style-flag-deploys-as-exec-role, cdk-bootstrap-new-style-synthesis-deploys-the-asset-lambda-as-the-exec-role.

Cause 2 of 2: a bucket whose creating template declares BucketEncryption with SSEAlgorithm aws:kms reports SSE-S3 (AES256) on read, plus a BlockedEncryptionTypes member that was never configured.
AWS: Smithy (s3) GetBucketEncryption -> ServerSideEncryptionConfiguration.Rules[*]; ServerSideEncryptionByDefault.SSEAlgorithm is @required (enum AES256 | aws:kms | ...), and BlockedEncryptionTypes is documented as an opt-in bucket-level setting that "currently only supports blocking or unblocking server-side encryption with customer-provided keys (SSE-C)" - a bucket that never configured it reports no such member. Both the modern toolkit template and the legacy staging bucket are read back as a single rule {ApplyServerSideEncryptionByDefault: {SSEAlgorithm: aws:kms}, BucketKeyEnabled: false}.
Emulator: the read answers {SSEAlgorithm: AES256, BucketKeyEnabled: false, BlockedEncryptionTypes: {EncryptionType: [SSE-C]}} for both the modern and the legacy bucket - the template's aws:kms default is never applied and an unsolicited SSE-C block is reported.
Why: a client cannot tell SSE-KMS from SSE-S3 on a bucket it created from a template, so compliance checks and drift detection over it read the wrong algorithm; a Get -> Put round trip now asserts a block the bucket never had.
Fix: the CloudFormation AWS::S3::Bucket resource handler's BucketEncryption property mapping onto S3's bucket-encryption state, and S3's GetBucketEncryption serializer (omit BlockedEncryptionTypes unless configured).
Similar open issues under other service headings: 20261109T220008Z-cloudformation-dynamodb-ecr-iam-s3-ssm-the-cdk.md, 20261109T164957Z-cloudformation-kms-s3-a-bucket-created-by-a.md - neither carries these checks' replay keys.
Traces: cdk-bootstrap-legacy-upgrade-in-use-retains-old-bucket-redeploys-from-new, cdk-bootstrap-omitted-execution-policies-default-to-admin-for-a-lambda-deploy, cdk-bootstrap-unknown-toolkit-stack-name-still-deploys-a-lambda-asset.

## Evidence
Cause 1 - cdk-bootstrap-default-synthesis-without-new-style-flag-deploys-as-exec-role and cdk-bootstrap-new-style-synthesis-deploys-the-asset-lambda-as-the-exec-role:
AssertionError: Invoke returned status 200 error 'Unhandled': b'{"errorMessage":"a Lambda function in this emulator cannot open a file of the host","errorType":"OSError",...,"stackTrace":["  File \\"/var/task/index.py\\", line 6, in handler\\n    return json.loads((pathlib.Path(__file__).parent / \\"response.json\\").read_text())\\n", ...]}'
Expected (AWS): (StatusCode, FunctionError) == (200, None) and the handler's payload.

Cause 2 - cdk-bootstrap-legacy-upgrade-in-use-retains-old-bucket-redeploys-from-new, cdk-bootstrap-omitted-execution-policies-default-to-admin-for-a-lambda-deploy, cdk-bootstrap-unknown-toolkit-stack-name-still-deploys-a-lambda-asset:
AssertionError: bucket <name> encryption rules are [{'ApplyServerSideEncryptionByDefault': {'SSEAlgorithm': 'AES256'}, 'BucketKeyEnabled': False, 'BlockedEncryptionTypes': {'EncryptionType': ['SSE-C']}}], expected [{'ApplyServerSideEncryptionByDefault': {'SSEAlgorithm': 'aws:kms'}, 'BucketKeyEnabled': False}]

## Steps to Reproduce
Cause 1:
1. Create a Lambda function whose zip contains index.py plus response.json, with a handler that reads `pathlib.Path(__file__).parent / "response.json"`.
2. Invoke it (RequestResponse).
3. Observe StatusCode 200 with FunctionError 'Unhandled' and payload errorType OSError, errorMessage "a Lambda function in this emulator cannot open a file of the host".

Cause 2:
1. `cdk bootstrap` (modern or legacy template), or create a stack whose AWS::S3::Bucket declares BucketEncryption with SSEAlgorithm aws:kms.
2. Call s3:GetBucketEncryption on the staging bucket.
3. Observe SSEAlgorithm 'AES256' and an extra BlockedEncryptionTypes {'EncryptionType': ['SSE-C']}.
