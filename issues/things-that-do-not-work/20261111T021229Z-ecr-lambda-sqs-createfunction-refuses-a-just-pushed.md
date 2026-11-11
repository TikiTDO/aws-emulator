Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr__lambda__sqs.container-image-custom-runtime-sqs-batch-item-failures
Issue-Key: ecr__lambda__sqs|CreateFunction|InvalidParameterValueException
Issue-Commit: 1acf76f902f2311eb0ad0e51c71d77c4cf2a9a30

# ecr__lambda__sqs: CreateFunction refuses a just-pushed ECR image with InvalidParameterValueException 'Source image ... is not valid', so no container-image function can be created

> _Note: this issue was identified against the code base at commit `1acf76f902f2311eb0ad0e51c71d77c4cf2a9a30`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFunction

## Description
Cause 1 of 1: CreateFunction with PackageType='Image' refuses an ImageUri naming an image that exists in this emulator's ECR registry, so the SQS event-source / ReportBatchItemFailures scenario behind that function never starts.

AWS: Smithy (lambda CreateFunction): Code (FunctionCode) is required, PackageType selects Image, InvalidParameterValueException is a modeled 400 client error, success code 201. AWS resolves Code.ImageUri against ECR; for an image that exists and a role it can assume it answers 201 and the function settles Active. The message the emulator returns, 'Source image <image-uri> is not valid. Provide a valid source image.', is AWS's wording for an ImageUri that does not resolve to an ECR image — fixed text 'Source image ' / ' is not valid. Provide a valid source image.' with the request's image URI as the only variable part. Recorded behavior: the check pushes config, layer and manifest to ECR successfully before calling CreateFunction.

Emulator: HTTP 400 InvalidParameterValueException with that message, for the very image URI the emulator minted and accepted a push to.

Why: a client gets told its image is invalid when it exists, and no container-image function can be created. The whole batch-item-failure contract downstream (event source mapping delivering an SQS batch, the handler returning batchItemFailures, the named message returning to the queue) cannot be exercised, and a user debugging this would go looking at their image build instead of the emulator.

Fix: the lambda CreateFunction handler's image-source resolution/validation — look the ImageUri up in ECR registry state (repository + tag or digest) and accept it when present; only emit InvalidParameterValueException for URIs that do not resolve.

Traces: ecr__lambda__sqs.container-image-custom-runtime-sqs-batch-item-failures

## Evidence
Expected (AWS): CreateFunction(PackageType='Image', Code={'ImageUri': '<registryId>.dkr.ecr.<region>.amazonaws.com/<repo>:v1'}) -> HTTP 201, PackageType='Image'.
Actual (emulator): botocore.errorfactory.InvalidParameterValueException: An error occurred (InvalidParameterValueException) when calling the CreateFunction operation: Source image 127.0.0.1:36157/sim2real-xsvc-cir-5209627791ff:v1 is not valid. Provide a valid source image. — raised although the image was pushed to that repository in the same check.

## Steps to Reproduce
1. CreateRepository in ecr; push a v2 manifest with its config and layer blobs tagged v1.
2. Call lambda CreateFunction with PackageType='Image' and Code={'ImageUri': '<repositoryUri>:v1'}.
3. AWS: 201 Created. Emulator: 400 InvalidParameterValueException 'Source image <uri> is not valid. Provide a valid source image.'
4. Replay ecr__lambda__sqs.container-image-custom-runtime-sqs-batch-item-failures.
