Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__ecr__lambda.container-image-custom-runtime-apigateway-proxy-response
Issue-Key: apigateway__ecr__lambda|CreateFunction|InvalidParameterValueException
Issue-Commit: c4fcf6952896a9081c9de16adffed799f1736bf9

# apigateway__ecr__lambda: CreateFunction refuses a container image that the emulator's own ECR published and holds, with InvalidParameterValueException "is not valid. Provide a valid source image."

> _Note: this issue was identified against the code base at commit `c4fcf6952896a9081c9de16adffed799f1736bf9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFunction

## Description
One net-new root cause, grounded in a failing behavioral check that no open issue carries the replay key for.

Cause 1 of 1: Lambda CreateFunction with PackageType=Image rejects a Code.ImageUri built from the repositoryUri the emulator's own ECR CreateRepository returned, after the image has been successfully pushed to that repository.
Failing check: apigateway__ecr__lambda.container-image-custom-runtime-apigateway-proxy-response

AWS: CreateFunction (Smithy: AWSGirApiService#CreateFunction, POST /2015-03-31/functions, success http code 201; Code -> FunctionCode @required; PackageType -> enum PackageType; errors include InvalidParameterValueException httpError 400 and ResourceNotFoundException httpError 404) accepts PackageType="Image" with Code={"ImageUri": "<repositoryUri>:<tag>"} whenever that repository exists in the caller's account and region and that tag resolves to a pushed manifest, and answers 201 with PackageType=Image, State=Pending settling to Active — including for a from-scratch image carrying a custom Runtime Interface Client and no Runtime/Handler. The canonical source of the URI is the repositoryUri the ECR control plane itself reports for the repository (CreateRepository/DescribeRepositories), so a URI ECR published for a repository that holds the pushed tag is by construction valid. AWS reserves InvalidParameterValueException with the message "Source image <image-uri> is not valid. Provide a valid source image." for the opposite case — the image does not exist, the tag does not resolve, or the manifest is not a Lambda-compatible image. In that message the fixed text is "Source image " ... " is not valid. Provide a valid source image." and the only variable part is the image URI echoed from Code.ImageUri; the host, port, repository name and tag in any one run are environment values, not part of the contract.
Emulator: the check pushes a config blob, a layer and a manifest (application/vnd.docker.distribution.manifest.v2+json) to the repository the emulator's ECR created, reads repositoryUri back from CreateRepository, and then CreateFunction(PackageType="Image", Code={"ImageUri": "<that repositoryUri>:v1"}, Role=<execution role>, Timeout=10, MemorySize=256) is refused 400 InvalidParameterValueException with "Source image <repositoryUri>:v1 is not valid. Provide a valid source image." — i.e. the Lambda emulator refuses the exact URI its own ECR emulator published and that its own registry just accepted a push for, so no container-image function can be created at all and every downstream assertion (function becomes Active, REST API AWS_PROXY integration, deployed-stage proxy response) is unreachable.
Hypothesis (neither the model nor the recording states it): the validator accepts only the canonical <account>.dkr.ecr.<region>.amazonaws.com/<repo>:<tag> spelling and rejects the host:port registry spelling the emulator's ECR publishes locally, instead of resolving the URI against the registry/repository/tag state it actually has.
Why: container-image packaging is completely unusable. A client that follows the normal flow (CreateRepository, read repositoryUri, push, CreateFunction with PackageType=Image) is refused at CreateFunction with an error AWS only returns for a missing or unusable image, so it is told its freshly pushed image does not exist. Nothing built on image-packaged functions — custom runtimes, the API Gateway AWS_PROXY response contract over such a function — can be exercised.
Fix: the Lambda CreateFunction handler's Code/PackageType validation for PackageType=Image — it must resolve Code.ImageUri against the emulated ECR registry's repositories and tags (accepting the repositoryUri form that the emulated ECR control plane itself reports) and only raise InvalidParameterValueException "Source image <uri> is not valid. Provide a valid source image." when no pushed manifest resolves for it.
Related open issue that does NOT cover this cause: 20261109T141609Z-apigateway-ecr-lambda-a-get-through-a-deployed.md carries the same check id but a different replay key (a TimeoutError in the deployed-stage GET poll, after CreateFunction had succeeded) and reports the data-plane hang, not this CreateFunction refusal; 20261103T031416Z-ecr-lambda-createfunction-accepts-a-container-image-whose.md is the opposite, over-permissive case (an ECR repository policy that denies Lambda being accepted).
Traces: apigateway__ecr__lambda.container-image-custom-runtime-apigateway-proxy-response.

## Evidence
Cause 1 — apigateway__ecr__lambda.container-image-custom-runtime-apigateway-proxy-response
  Expected: CreateFunction(FunctionName=<fn>, PackageType="Image", Role=<execution-role-arn>, Code={"ImageUri": "<repositoryUri from ECR CreateRepository>:v1"}, Timeout=10, MemorySize=256) -> HTTP 201 with PackageType="Image" and State Pending -> Active, after the v1 manifest has been pushed to that repository.
  Actual: botocore.errorfactory.InvalidParameterValueException: An error occurred (InvalidParameterValueException) when calling the CreateFunction operation: Source image <repositoryUri>:v1 is not valid. Provide a valid source image.
  (In the replayed run the echoed URI was the emulator's local registry spelling `<host>:<port>/sim2real-xsvc-cir-<suffix>:v1` — host, port, repository name and tag are per-run environment values; the divergence is that a URI ECR published and whose tag was pushed is refused at all.)
  The refusal is the emulator's own response on an operation the recording completes; no runner tool, TLS or name-resolution failure is involved.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Re-replay apigateway__ecr__lambda.container-image-custom-runtime-apigateway-proxy-response.
3. Or by hand: ecr CreateRepository(repositoryName=<repo>) and keep repository.repositoryUri from the response.
4. Push a Docker v2 image to that repository as tag v1: PUT the config blob (application/vnd.docker.container.image.v1+json), PUT the layer blob (application/vnd.docker.image.rootfs.diff.tar.gzip), then PUT the manifest (application/vnd.docker.distribution.manifest.v2+json) for tag v1.
5. iam CreateRole with a lambda.amazonaws.com trust policy for the execution role.
6. lambda CreateFunction(FunctionName=<fn>, PackageType="Image", Role=<role-arn>, Code={"ImageUri": f"{repositoryUri}:v1"}, Timeout=10, MemorySize=256).
7. Observe 400 InvalidParameterValueException "Source image <repositoryUri>:v1 is not valid. Provide a valid source image." instead of 201 with PackageType=Image.
