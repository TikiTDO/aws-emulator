Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr__lambda.container-image-repository-policy-deny-rejected
Issue-Key: ecr__lambda|CreateFunction|InvalidParameterValueException
Issue-Commit: 1acf76f902f2311eb0ad0e51c71d77c4cf2a9a30

# ecr__lambda: CreateFunction refuses an image whose ECR repository policy denies Lambda with the wrong error — 400 InvalidParameterValueException 'Source image ... is not valid' instead of 403 AccessDeniedException

> _Note: this issue was identified against the code base at commit `1acf76f902f2311eb0ad0e51c71d77c4cf2a9a30`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFunction

## Description
Cause 1 of 1: when a container image's ECR repository policy Denies lambda.amazonaws.com the pull actions, CreateFunction is now refused — but with the wrong status, error code and message: it reports the image itself as invalid instead of reporting the permission failure.

AWS: Smithy (lambda CreateFunction) models InvalidParameterValueException as a 400 client error and the service's AccessDeniedException as the 403-class refusal; Lambda pulls the image at create time with its own service principal (docs: images-create.html#gettingstarted-images-permissions). With the repository policy denying ecr:BatchGetImage / ecr:GetDownloadUrlForLayer to Principal Service lambda.amazonaws.com, CreateFunction is refused HTTP 403 AccessDeniedException with the message 'Lambda does not have permission to access the ECR image. Check the ECR permissions.' — that whole sentence is fixed text with no variable parts. The refused function must not exist afterwards: GetFunction answers ResourceNotFoundException with 'Function not found: arn:<partition>:lambda:<region>:<account>:function:<name>' (fixed text 'Function not found: ' plus the ARN built from partition/region/account and the requested name). AWS reserves 'Source image <image-uri> is not valid. Provide a valid source image.' for an ImageUri that does not resolve to an ECR image at all.

Emulator: CreateFunction answers HTTP 400 InvalidParameterValueException, message 'Source image <image-uri> is not valid. Provide a valid source image.' — i.e. the emulator does not distinguish 'image absent' from 'Lambda not permitted to pull it', and in fact rejects an image that was successfully pushed to that repository (the sibling positive-control repository with no deny policy is refused the same way, see the companion issues filed for apigateway__ecr__lambda and ecr__lambda__sqs where no deny policy exists at all).

Why: a client that mis-scopes an ECR repository policy is told the image is invalid, so it rebuilds and re-pushes the image instead of fixing the policy; and a client with a correct policy is refused for a reason that does not exist. The cross-service authorization boundary is reported through the wrong error class, so a caller that branches on AccessDeniedException (retry vs fix-permissions) takes the wrong branch, and an SDK's 400-vs-403 handling differs.

Fix: the lambda CreateFunction handler's image-source resolution and its authorization step — resolve the ImageUri against ECR state first and, when the image exists but the repository policy denies the Lambda service principal the pull actions, refuse with 403 AccessDeniedException and AWS's fixed message while creating nothing (GetFunction must then answer ResourceNotFoundException with the 'Function not found: <arn>' message). Note: the open issue 20261103T031416Z-ecr-lambda-createfunction-accepts-a-container-image-whose.md records the EARLIER symptom of this same check (CreateFunction succeeded outright); it does not cover this refusal, whose code and message are both wrong.

Traces: ecr__lambda.container-image-repository-policy-deny-rejected

## Evidence
Expected (AWS): CreateFunction(PackageType='Image', Code={'ImageUri': '<repo-with-deny-policy>@sha256:<digest>'}) -> HTTP 403, Error.Code 'AccessDeniedException', Error.Message 'Lambda does not have permission to access the ECR image. Check the ECR permissions.'; then GetFunction(<name>) -> ResourceNotFoundException, 'Function not found: arn:<partition>:lambda:<region>:<account>:function:<name>'.
Actual (emulator): AssertionError: expected AccessDeniedException, got 'InvalidParameterValueException': 'Source image 127.0.0.1:37831/sim2real-behav-img-deny-fa0ca607d83d@sha256:df8d09ecef99701e4f53624a45fba670a7422d0a9222557aca4023018c9fd3fe is not valid. Provide a valid source image.' — HTTP 400, wrong code, and a message about image validity rather than permissions.

## Steps to Reproduce
1. CreateRepository in ecr, push a minimal image, and note its digest-addressed URI.
2. SetRepositoryPolicy with a statement Effect=Deny, Principal={'Service':'lambda.amazonaws.com'}, Action=['ecr:BatchGetImage','ecr:GetDownloadUrlForLayer'].
3. Call lambda CreateFunction with PackageType='Image' and Code={'ImageUri': '<repo>@<digest>'}.
4. AWS: 403 AccessDeniedException 'Lambda does not have permission to access the ECR image. Check the ECR permissions.' Emulator: 400 InvalidParameterValueException 'Source image <uri> is not valid. Provide a valid source image.'
5. Replay ecr__lambda.container-image-repository-policy-deny-rejected.
