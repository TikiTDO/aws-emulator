Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-snx-traj-cross-region-function-arn
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-snx-traj-cross-region-function-arn lambda-lvau-traj-wrong-function-identifier lambda-lls-layer-reference-forms-name-arn-and-version-arn lambda-bpt-traj-list-functions-parameter-mistakes lambda-bpt-traj-list-tags-identifier-mistakes
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|GetFunctionConfiguration|4xx->2xx|$.Message
Issue-Key: lambda|GetFunctionConfiguration|4xx->2xx|$.Type
Issue-Key: lambda|GetFunctionConfiguration|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: lambda|GetFunctionConfiguration|4xx->2xx|status
Issue-Key: lambda|GetPolicy|4xx->2xx|$.Message
Issue-Key: lambda|GetPolicy|4xx->2xx|$.Type
Issue-Key: lambda|GetPolicy|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: lambda|GetPolicy|4xx->2xx|status
Issue-Key: lambda|UpdateFunctionConfiguration|4xx->2xx|$.Message
Issue-Key: lambda|GetLayerVersion|4xx->2xx|$.Type
Issue-Key: lambda|GetLayerVersion|4xx->2xx|$.message
Issue-Key: lambda|GetLayerVersion|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: lambda|GetLayerVersion|4xx->2xx|status
Issue-Key: lambda|ListFunctions|4xx->4xx|$.message
Issue-Key: lambda|ListTags|4xx->4xx|$.message
Issue-Related: 20261103T165236Z-lambda-updatefunctionconfiguration-skips-vpc-and-environment-validation.md
Issue-Commit: dd64cbf0b038e7955d699468def5671676941ec2

# Lambda accepts foreign-region and qualified resource identifiers

> _Note: this issue was identified against the code base at commit `dd64cbf0b038e7955d699468def5671676941ec2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFunctionConfiguration

## Description
The emulator parses an ARN for its resource name and ignores the region field, so an ARN naming a different region resolves to the local resource instead of being refused; the related identifier refusals also use emulator wording. 6 causes, all in the shared function/layer identifier resolution and its refusal messages.

Cause 1 of 6 - GetFunctionConfiguration answers 200 for a function ARN in another region.
AWS: Smithy: GetFunctionConfiguration input FunctionName targets NamespacedFunctionName, whose pattern admits a full ARN including the region segment; the operation declares ResourceNotFoundException (404). A function ARN whose region differs from the endpoint's region is refused with HTTP 404, Type 'User' and the message "Functions from '<arn-region>' are not reachable in this region ('<endpoint-region>')" - fixed text with the two region names taken from the request ARN and from the endpoint.
Emulator: strips the region and resolves the name locally, answering HTTP 200 with the local function's configuration.
Why: a client that pastes a production ARN while pointed at another region reads and reasons about the wrong function and gets no indication that the regions disagree.
Fix: the shared function-identifier resolver - it must compare the ARN's region to the request's region.
Traces: lambda-snx-traj-cross-region-function-arn#task#7. A related open issue, 20261102T083644Z-lambda-reads-are-not-region-scoped-a-function.md, describes the same class for other reads but does not carry these clusters' replay keys.

Cause 2 of 6 - GetPolicy answers 200 for a foreign-region function ARN.
AWS: same 404 and the same message as cause 1 (GetPolicy declares ResourceNotFoundException 404).
Emulator: answers HTTP 200 with the local function's resource policy.
Why: a client auditing a policy believes it has read the remote function's policy.
Fix: as cause 1, reached from the GetPolicy handler.
Traces: lambda-snx-traj-cross-region-function-arn#task#8.

Cause 3 of 6 - UpdateFunctionConfiguration mutates the local function for a foreign-region ARN.
AWS: same 404 and the same message; the update is not performed.
Emulator: answers HTTP 200 and applies the update to the local function.
Why: this is a write: a configuration change aimed at another region's function silently lands on the local one.
Fix: as cause 1, reached from the UpdateFunctionConfiguration handler, before state is written.
Traces: lambda-lvau-traj-wrong-function-identifier#task#6.

Cause 4 of 6 - GetLayerVersion accepts a layer ARN from another region.
AWS: Smithy: GetLayerVersion input LayerName @pattern ^(arn:(aws[a-zA-Z-]*)?:lambda:<region>:<account>:layer:[a-zA-Z0-9-_]+)|[a-zA-Z0-9-_]+$; the operation declares InvalidParameterValueException (400). A layer ARN whose region is not the endpoint's region is refused with HTTP 400, Type 'User' and the message 'Invalid Layer name: <layer-arn-as-supplied>' - fixed prefix 'Invalid Layer name: ' plus the identifier from the request.
Emulator: answers HTTP 200 with the local layer version.
Why: a client reading a layer by its ARN gets a different layer's content metadata than the one it named.
Fix: the layer-identifier resolver's region check.
Traces: lambda-lls-layer-reference-forms-name-arn-and-version-arn#task#6.

Cause 5 of 6 - the ListFunctions MasterRegion/FunctionVersion refusal uses emulator wording.
AWS: Smithy: ListFunctions declares InvalidParameterValueException (400); the refusal for a MasterRegion given without FunctionVersion=ALL is HTTP 400 with the message 'The combination MasterArn=<master-region> and FunctionVersion=<function-version> is not supported. Please see our documentation for valid combinations of ListFunctions options.' - fixed text whose two variable parts are the supplied MasterRegion value and the FunctionVersion value (rendered NULL when the parameter was omitted).
Emulator: refuses with the correct 400 but the message 'MasterRegion can only be set when FunctionVersion is ALL', which names neither value.
Why: a client that surfaces the message to an operator loses the two values it must change, and a client matching on AWS's sentence does not match.
Fix: the ListFunctions handler's parameter-combination validation message.
Traces: lambda-bpt-traj-list-functions-parameter-mistakes#task#5.

Cause 6 of 6 - the ListTags qualified-ARN refusal uses emulator wording.
AWS: Smithy: ListTags declares InvalidParameterValueException (400). Tagging is only supported on the unqualified function, so a version- or alias-qualified ARN is refused with HTTP 400 and the message 'Tags on function aliases and versions are not supported. Please specify a function ARN.' (fixed text, no variable parts).
Emulator: refuses with the correct 400 but a wholly different sentence that quotes the ARN and the qualifier: "Tags are not supported for a function version or alias. The resource '<arn>' qualifies the function with '<qualifier>'; specify the unqualified function ARN instead."
Why: a client matching AWS's documented sentence to retry with the unqualified ARN does not match.
Fix: the ListTags handler's identifier validation message.
Traces: lambda-bpt-traj-list-tags-identifier-mistakes#task#6.

## Evidence
Cause 1: lambda-snx-traj-cross-region-function-arn#task#7 - expected 404 + $.Type='User' + $.Message="Functions from 'us-east-1' are not reachable in this region ('us-west-2')" + x-amzn-errortype; got 200 with none of them.
Cause 2: #task#8 (GetPolicy) - same expectation, got 200.
Cause 3: lambda-lvau-traj-wrong-function-identifier#task#6 (UpdateFunctionConfiguration) - same expectation, got 200.
Cause 4: lambda-lls-layer-reference-forms-name-arn-and-version-arn#task#6 - expected 400 + $.Type='User' + $.message='Invalid Layer name: arn:aws:lambda:eu-north-1:<account>:layer:<name>'; got 200.
Cause 5: lambda-bpt-traj-list-functions-parameter-mistakes#task#5 - 400 vs 400, $.message expected 'The combination MasterArn=<region> and FunctionVersion=<version> is not supported. Please see our documentation for valid combinations of ListFunctions options.', got 'MasterRegion can only be set when FunctionVersion is ALL'.
Cause 6: lambda-bpt-traj-list-tags-identifier-mistakes#task#6 - 400 vs 400, $.message expected 'Tags on function aliases and versions are not supported. Please specify a function ARN.', got the emulator's ARN-quoting sentence.

## Steps to Reproduce
1. Replay lambda-snx-traj-cross-region-function-arn task#7 (GetFunctionConfiguration) and task#8 (GetPolicy) with a us-east-1 function ARN against the us-west-2 endpoint.
2. Replay lambda-lvau-traj-wrong-function-identifier task#6 (UpdateFunctionConfiguration with a foreign-region ARN).
3. Replay lambda-lls-layer-reference-forms-name-arn-and-version-arn task#6 (GetLayerVersion with a eu-north-1 layer ARN).
4. Replay lambda-bpt-traj-list-functions-parameter-mistakes task#5 and lambda-bpt-traj-list-tags-identifier-mistakes task#6 and compare the refusal messages.
