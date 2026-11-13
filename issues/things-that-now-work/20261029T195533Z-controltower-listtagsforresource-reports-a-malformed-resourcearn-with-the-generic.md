Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: controltower.list-tags-for-resource-malformed-arn-validation
Issue-Shape-Id: com.amazonaws.controltower#AWSControlTowerApis
Issue-Key: controltower.list-tags-for-resource-malformed-arn-validation|run#5|AssertionError|assert malformed_message == "Invalid input resource arn: bad syntax", (
Issue-Commit: 128b971c27980fd8fd6bfdfd4c5ec95349a617e4

# ListTagsForResource reports a malformed resourceArn with the generic front-door message 'Invalid request body' instead of AWS's 'Invalid input resource arn: bad syntax'

> _Note: this issue was identified against the code base at commit `128b971c27980fd8fd6bfdfd4c5ec95349a617e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
One cause.

Cause 1 of 1 — the tag-plane ARN rejection uses the service's generic request-body message rather than the resourceArn-specific one.
AWS: Smithy: ListTagsForResource.resourceArn @httpLabel @required, target Arn @length 20..2048 @pattern ^arn:aws[0-9a-zA-Z_\-:\/]+$; modelled errors ValidationException (400), ResourceNotFoundException (404), InternalServerException (500). Recorded behavior: the resourceArn path parameter is checked at the front door, ahead of the modelled handler, and a value that does not match the ARN pattern is refused with BadRequestException (400) and the message 'Invalid input resource arn: bad syntax' — entirely fixed text, parameterised by nothing from the request (the offending ARN is not echoed). This is the per-parameter wording for a resourceArn syntax failure, distinct from the generic 'Invalid request body' AWS uses for other malformed-body rejections in this service (which the emulator already emits correctly for DisableControl's enabledControlIdentifier, EnableBaseline's targetIdentifier, ListEnabledControls' targetIdentifier and GetControlOperation's operationIdentifier — those assertions pass).
Emulator: BadRequestException (400) — the status and error code are right — with the message 'Invalid request body'.
Why: the error code alone does not tell a client WHICH input it got wrong. Callers and operators match on this message to distinguish a bad resource ARN from a bad body elsewhere in the request; the generic text misdirects that triage, and the message is part of the requirement for this error class.
Fix: the ListTagsForResource handler's resourceArn validation (the message it hands to the BadRequestException front-door envelope), shared with the other tag-plane operations so all of them word an ARN-syntax refusal identically.
Note on related issues: the fixed issue 20261022T032312Z-controltower-input-validation-rejections-answer-validationexception-instead-of introduced the BadRequestException front-door envelope, and this call now correctly reaches it — this is NOT a regression of that fix, only its message being too generic for this parameter. The OPEN issue 20261029T183207Z-controltower-control-tower-tag-plane-arn-validation-is-skipped describes the tag plane answering 404 ResourceNotFoundException with store prose and expects 'Invalid input resource arn'; the emulator in this checkout now answers 400 BadRequestException, so that description no longer matches what is observed here, and it does not carry this behavioral check key.
Traces: controltower.list-tags-for-resource-malformed-arn-validation.

## Evidence
Replay of suite simparity-behavioral-aws-v1 (controltower), check controltower.list-tags-for-resource-malformed-arn-validation [mismatch]:

AssertionError: expected 'Invalid input resource arn: bad syntax' for a malformed resourceArn, got 'Invalid request body'.

The preceding assertion in the same check passed: error code BadRequestException (HTTP 400) for GET /tags/{resourceArn} with resourceArn='totally-not-an-arn-but-long-enough' (>= 20 chars so botocore's local Arn length check does not short-circuit the call). Only the message diverges. The check's later assertions — NotFoundException / 'Resource was not found' for a well-formed but nonexistent resourceArn — were never reached and are not evidenced here.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay suite simparity-behavioral-aws-v1, check controltower.list-tags-for-resource-malformed-arn-validation.
2. Call GET /tags/totally-not-an-arn-but-long-enough (ListTagsForResource with resourceArn='totally-not-an-arn-but-long-enough' — 34 chars, so it clears botocore's client-side min-20 length check and reaches the service).
3. Observe HTTP 400 with x-amzn-errortype BadRequestException and message 'Invalid request body'; AWS returns the same status and code with message 'Invalid input resource arn: bad syntax'.
