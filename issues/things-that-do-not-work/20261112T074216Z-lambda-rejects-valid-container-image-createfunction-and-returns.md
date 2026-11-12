Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-container-image-handler-and-runtime-rejected
Issue-Step: setup#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-container-image-handler-and-runtime-rejected lambda-function-url-config-lifecycle
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|CreateFunction|2xx->4xx|status
Issue-Key: lambda|GetFunctionConfiguration|2xx->4xx|$.State
Issue-Key: lambda|GetFunctionConfiguration|2xx->4xx|status
Issue-Key: lambda|GetFunctionConfiguration|2xx->4xx|$.Architectures
Issue-Key: lambda|GetFunctionConfiguration|2xx->4xx|$.CodeSha256
Issue-Key: lambda|GetFunctionConfiguration|2xx->4xx|$.CodeSize
Issue-Key: lambda|GetFunctionConfiguration|2xx->4xx|$.Description
Issue-Key: lambda|GetFunctionConfiguration|2xx->4xx|$.EphemeralStorage
Issue-Key: lambda|GetFunctionConfiguration|2xx->4xx|$.FunctionArn
Issue-Key: lambda|GetFunctionConfiguration|2xx->4xx|$.FunctionName
Issue-Key: lambda|GetFunctionConfiguration|2xx->4xx|$.LastModified
Issue-Key: lambda|GetFunctionConfiguration|2xx->4xx|$.LastUpdateStatus
Issue-Key: lambda|GetFunctionConfiguration|2xx->4xx|$.LoggingConfig
Issue-Key: lambda|GetFunctionConfiguration|2xx->4xx|$.MemorySize
Issue-Key: lambda|GetFunctionConfiguration|2xx->4xx|$.PackageType
Issue-Key: lambda|GetFunctionConfiguration|2xx->4xx|$.RevisionId
Issue-Key: lambda|GetFunctionConfiguration|2xx->4xx|$.Role
Issue-Key: lambda|GetFunctionConfiguration|2xx->4xx|$.SnapStart
Issue-Key: lambda|GetFunctionConfiguration|2xx->4xx|$.Timeout
Issue-Key: lambda|GetFunctionConfiguration|2xx->4xx|$.TracingConfig
Issue-Key: lambda|GetFunctionConfiguration|2xx->4xx|$.Version
Issue-Key: lambda|DeleteFunction|2xx->4xx|status
Issue-Key: lambda|CreateFunctionUrlConfig|2xx->2xx|$.FunctionUrl
Issue-Related: 20261029T233239Z-lambda-function-url-configs-return-a-default-invokemode.md
Issue-Commit: 7b7f109607710b8d51bafd33bd8e146725ea3b62

# Lambda rejects valid container-image CreateFunction and returns non-AWS function URL payload

> _Note: this issue was identified against the code base at commit `7b7f109607710b8d51bafd33bd8e146725ea3b62`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFunction

## Description
2 causes, both in the lambda function surface: a well-formed container-image CreateFunction is refused outright (and every later step against that function therefore 404s), and the CreateFunctionUrlConfig response body is not shaped the way AWS shapes it.

Cause 1 of 2 - CreateFunction with PackageType='Image' is refused with HTTP 400 where AWS creates the function.
AWS: Smithy (lambda CreateFunction): Code (FunctionCode) is required, PackageType is an enum whose Image member selects a container deployment, the operation's success code is 201, and InvalidParameterValueException is a modeled 400 client error. For a request that carries PackageType='Image' with Code.ImageUri and (correctly) no Handler and no Runtime, AWS answers 201 with the FunctionConfiguration: PackageType='Image', CodeSize 0, Version='$LATEST', State settling to 'Active', LastUpdateStatus='Successful', TracingConfig {Mode:'PassThrough'}, SnapStart {OptimizationStatus:'Off', ApplyOn:'None'}, EphemeralStorage {Size:512}, Architectures ['x86_64'], Description '', and LoggingConfig with LogFormat 'Text' and LogGroup '/aws/lambda/<function-name>' (the function name from the request is the only variable part of that log-group path). The function is then readable with GetFunctionConfiguration (200) and deletable with DeleteFunction (204). InvalidParameterValueException is reserved for an ImageUri that does not resolve to an image; its AWS wording is 'Source image <image-uri> is not valid. Provide a valid source image.' - fixed text 'Source image ' / ' is not valid. Provide a valid source image.' with the request's image URI as the only variable part.
Emulator: answers HTTP 400 to the create, so no function exists; the recording asserts 201 here, so only the status was compared and no message was captured for this step. The same refusal observed through other corpora carries the 'Source image <uri> is not valid. Provide a valid source image.' message for an image URI the emulator itself minted, which points at image-source resolution rather than a shape problem in the request. Because nothing is created, the three dependent steps in the same fixture also diverge, all as consequences of this one cause and not as separate faults: GetFunctionConfiguration polling for State='Active' never leaves 404 (expected 200, exhausted 60 attempts), the later GetFunctionConfiguration read answers 404 with an empty body instead of 200 with the full configuration (FunctionName/FunctionArn/Role/Version/PackageType/State/LastUpdateStatus/Timeout/MemorySize/CodeSize/CodeSha256/RevisionId/LastModified/Description/TracingConfig/SnapStart/EphemeralStorage/Architectures/LoggingConfig), and teardown's DeleteFunction answers 404 instead of 204.
Why: container-image packaging is unusable. A client that deploys from an image gets told its image is invalid when it is not, never reaches an Active function, and every read, invoke and delete against it fails as 'not found' - so the fault surfaces as a cascade of missing-resource errors far from the create that actually failed, and a user debugging it goes looking at their image build instead of the emulator.
Fix: the CreateFunction handler's image-source resolution/validation for PackageType='Image' (accept an ImageUri that resolves in the emulator's own registry state and create the function, reserving InvalidParameterValueException for URIs that do not resolve), plus the function-state write that the dependent reads and the delete depend on.
Traces: lambda-container-image-handler-and-runtime-rejected#setup#10 (CreateFunction, expected 201, got 400); cascading: #setup#11 and #task#16 (GetFunctionConfiguration, expected 200, got 404) and #teardown#17 (DeleteFunction, expected 204, got 404). The open behavioral issues 20261111T021229Z-ecr-lambda-sqs-createfunction-refuses-a-just-pushed.md and 20261111T020136Z-apigateway-ecr-lambda-createfunction-refuses-a-container-image.md describe the same refusal from the behavioral corpus but carry no wire replay key, so they do not cover these fixture steps; 20261103T165236Z-lambda-createfunction-skips-package-and-code-signing-validation.md owns only the Handler-on-Image refusal message in this same fixture (task#12/#14), not this valid-image create.

Cause 2 of 2 - CreateFunctionUrlConfig's response body diverges in two members.
AWS: Smithy (lambda CreateFunctionUrlConfig): success code 201; output member FunctionUrl is required with @length min 40 / max 100; output member InvokeMode is an optional enum (its documentation names BUFFERED 'the default option' for invocation behavior). AWS returns a function URL in its own hosted form, 'https://<url-id>.lambda-url.<region>.on.aws/' - fixed text '.lambda-url.', '.on.aws/' and the https scheme, with the generated url-id and the region as its variable parts; it is never a loopback host:port with a path prefix. Recorded behavior only for InvokeMode: when the request does not set InvokeMode, AWS's create response omits the member entirely rather than materialising the documented default.
Emulator: returns FunctionUrl as a loopback 'https://<host>:<port>/_aws/lambda-url/<id>/' URL, which fails the recording's shape assertion on that member, and populates InvokeMode='BUFFERED' on a request that did not ask for an invoke mode (expected: member absent).
Why: a client that parses or pattern-matches the returned URL (extracting the url-id, asserting the on.aws host, or deriving the region from it) gets a value that matches nothing AWS would emit - this is about the URL's structure, not about which host or port the emulator happens to listen on. And a client that treats the presence of InvokeMode as 'an invoke mode was configured' sees a mode it never set, so round-tripping create -> update sends back a value the user did not choose.
Fix: the CreateFunctionUrlConfig handler's response serializer - mint the function URL in AWS's lambda-url/on.aws form (keeping the emulator's reachable endpoint out of the modelled value) and leave InvokeMode unset in the response when the request omitted it.
Traces: lambda-function-url-config-lifecycle#task#8 (CreateFunctionUrlConfig, 201 vs 201, $.FunctionUrl shape mismatch and $.InvokeMode type mismatch).

## Evidence
Cause 1: lambda-container-image-handler-and-runtime-rejected#setup#10 - CreateFunction (PackageType='Image'): expected status 201, got 400. Cascade in the same fixture: #setup#11 GetFunctionConfiguration - expected 200 with $.State polled to 'Active', got 404 and the poll never saw 'Active' after 60 attempts; #task#16 GetFunctionConfiguration - expected 200 with the full configuration ($.FunctionName='sim2real-probe-lambda-imghr-ok-<suffix>', $.FunctionArn='arn:aws:lambda:<region>:<account>:function:<name>', $.Version='$LATEST', $.PackageType='Image', $.State='Active', $.LastUpdateStatus='Successful', $.Timeout=3, $.MemorySize=128, $.CodeSize=0, $.Description='', $.TracingConfig={'Mode':'PassThrough'}, $.SnapStart={'OptimizationStatus':'Off','ApplyOn':'None'}, $.EphemeralStorage={'Size':512}, $.Architectures=['x86_64'], $.LoggingConfig={'LogFormat':'Text','LogGroup':'/aws/lambda/<name>'}, plus shaped $.RevisionId/$.LastModified/$.CodeSha256/$.Role), got 404 with every one of those members absent; #teardown#17 DeleteFunction - expected 204, got 404. (The names, ARNs and suffixes above are this recording's values filling the template, not requirements.)
Cause 2: lambda-function-url-config-lifecycle#task#8 - CreateFunctionUrlConfig, 201 vs 201: $.FunctionUrl expected to match AWS's function-url shape, got 'https://127.0.0.1:38423/_aws/lambda-url/37454539d21a4572a24edf13bffbfcfd/' (a loopback host:port with an /_aws/lambda-url/ path prefix instead of '<url-id>.lambda-url.<region>.on.aws'); $.InvokeMode expected absent (None), got 'BUFFERED'.

## Steps to Reproduce
1. Replay lambda-container-image-handler-and-runtime-rejected and inspect setup#10: CreateFunction with PackageType='Image', Code.ImageUri naming an image the emulator holds, no Handler and no Runtime. AWS: 201 with PackageType='Image'. Emulator: 400.
2. In the same fixture, inspect setup#11 and task#16 (GetFunctionConfiguration, expected 200/State=Active, got 404) and teardown#17 (DeleteFunction, expected 204, got 404) - these clear once the create in step 1 succeeds.
3. Replay lambda-function-url-config-lifecycle and inspect task#8: CreateFunctionUrlConfig with AuthType set and no InvokeMode. Compare $.FunctionUrl against AWS's 'https://<url-id>.lambda-url.<region>.on.aws/' form and check that $.InvokeMode is absent from the response.
