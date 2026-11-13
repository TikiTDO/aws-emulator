Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda-web.delete-web-function-cascades-tags-and-resource-policy lambda-web.tag-resource-rejects-reserved-aws-prefix-key-and-applies-no-tags
Issue-Shape-Id: com.amazonaws.lambdaweb#LambdaWeb
Issue-Key: lambda-web.delete-web-function-cascades-tags-and-resource-policy|run#12|AssertionError|assert message == expected_message, (
Issue-Key: lambda-web.tag-resource-rejects-reserved-aws-prefix-key-and-applies-no-tags|run#4|AssertionError|assert message == (
Issue-Commit: 677b6e35c5226b67b9bfea5e554d250e3c084a9c

# lambda-web: resource-not-found and reserved-tag-key refusal messages are not AWS's wording (2 causes)

> _Note: this issue was identified against the code base at commit `677b6e35c5226b67b9bfea5e554d250e3c084a9c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourcePolicy

## Description
Two distinct, net-new root causes in the lambda-web emulator, both in the text of a modelled client error. Each is numbered below with its own failing check ids; BOTH must be closed before this issue is submitted.

--------------------------------------------------------------------
Cause 1 of 2: the ResourceNotFoundException message for a web function that is not stored uses the emulator's own wording instead of AWS's.
Failing check ids: lambda-web.delete-web-function-cascades-tags-and-resource-policy

AWS: Smithy (services/service-com-amazonaws-lambdaweb-lambdaweb/smithy-model/com-amazonaws-lambdaweb-lambdaweb.json), service LambdaWeb, restJson1. GetResourcePolicy is GET /2025-03-07/resource-policy/{resourceArn} (readonly, success 200) and ListTags is GET /2025-03-07/tags/{resource} (readonly, success 200); both model ResourceNotFoundException (client error, HTTP 404) whose `message` member is @required and whose shape doc is "The specified web function, revision, endpoint, or resource-based policy doesn't exist. Verify the resource identifier and try again." The model does not fix the message TEXT, so the text is recorded behavior only: for a resource-policy read and a tag read addressed at a web function ARN whose function no longer exists, AWS answers HTTP 404, error code ResourceNotFoundException, message `The function <resourceArn> does not exist.` In that message the FIXED text is "The function " and " does not exist."; the only VARIABLE part is the web-function ARN, taken verbatim from the request's `resourceArn`/`resource` URI label (the recording's own ARN value is only an instance of that variable and is not part of the requirement). AWS states the same wording for both readers, so after the function is deleted both GetResourcePolicy and ListTags report the FUNCTION as absent — not the policy, and not an empty answer.

Emulator: the status (404) and the error code (ResourceNotFoundException) are right, but the message is `The web function <resourceArn> doesn't exist.` — fixed text "The web function " / " doesn't exist.", same ARN variable. The replay shows GetResourcePolicy on a function deleted by DeleteWebFunction answering "The web function arn:aws:lambda:<region>:<account>:web-function/<name> doesn't exist." where the expected message is "The function arn:aws:lambda:<region>:<account>:web-function/<name> does not exist." The same literal is built in three handlers and so diverges identically on each of them (see Traces for file/line citations): GetResourcePolicy, ListTags and TagResource's not-found branch.

Why: a client that distinguishes error classes by message — the only way to tell the modelled "function gone" case from the "function exists but carries no policy" case, since both are the same code and status — mis-classifies the answer. Cleanup and cascade assertions ("after delete, the policy and the tags are gone, and the reader says the FUNCTION does not exist") pass against AWS and fail against the emulator, and log/alarm matchers keyed on AWS's sentence never fire.

Fix: belongs in the message construction of the not-found branch of the GetResourcePolicy, ListTags and TagResource handlers (and any other handler that spells this sentence), ideally hoisted into the function slice of the service state so one constant serves every reader. Emit AWS's sentence with the request's resource ARN interpolated; do not change the status or the code, which are already correct.

--------------------------------------------------------------------
Cause 2 of 2: TagResource's refusal of a reserved `aws:`-prefixed tag key uses the framework's constraint-violation sentence instead of AWS's tag-validation sentence.
Failing check ids: lambda-web.tag-resource-rejects-reserved-aws-prefix-key-and-applies-no-tags

AWS: Smithy, TagResource = POST /2025-03-07/tags/{resource}, @idempotent, success 204, input `resource` (ResourceArn, @required, @httpLabel) and `tags` (Tags, @required, @length 0..200); modelled ValidationException is a client error, HTTP 400, `message` @required, doc "The request failed validation because a parameter was missing, malformed, or outside the allowed range." TagKey is @length 1..128 @pattern ^[a-zA-Z0-9 _\.:/=+\-@]+$ — it ADMITS a colon, so the reserved-namespace rule is not a modelled constraint and the message for it is recorded behavior only. AWS refuses the whole call with HTTP 400, code ValidationException and the single fixed sentence `One or more of your tags are invalid. Please ensure none of your tag keys start with "aws:", none are empty, and there are no duplicate tag keys.` That message has NO variable parts: it names neither the offending key nor its member path, and it covers the three rules it lists (reserved prefix, empty key, duplicate keys) with one text. The call is refused whole — the valid key in the same map is not applied either (the check's follow-up with only the valid key confirms tagging otherwise works).

Emulator: the whole-call refusal, the 400 and the ValidationException code are right, but the message is the framework's per-violation sentence wrapped in the shared count envelope: `1 validation error detected: Value 'aws:<key>' at 'tags.<key>' failed to satisfy constraint: Member must not begin with the reserved prefix aws:`. Its fixed text is "<n> validation error detected: Value '" / "' at '" / "' failed to satisfy constraint: Member must not begin with the reserved prefix aws:", and its variable parts are the submitted key (twice: as the value and inside the `tags.<key>` member path) and the violation count — none of which appear in AWS's sentence at all. The text is produced by the service's own tag slice, which deliberately reuses the constraint-violation spelling; the same wrong wording is served for the UntagResource key-list path, which shares the requirement constant.

Why: a client that asserts on, or branches over, AWS's documented tag-validation message (SDK integration tests, Terraform-style providers, tagging policy tooling) sees a completely different sentence and a leaked internal member path, so the reserved-prefix rejection is indistinguishable from an ordinary modelled-constraint rejection. The rejection is also over-specific: it names the caller's key, which AWS does not.

Fix: belongs in this service's tag validation (the tag slice of the service state that builds the reserved-key refusal text, and the TagResource/UntagResource handlers that return it). The reserved-prefix / empty-key / duplicate-key refusal must carry AWS's single fixed sentence, not the framework's per-violation clause and not the count envelope; the all-or-nothing behaviour and the 400/ValidationException classification already match and must be kept.

Traces: both causes are grounded in behavioral checks, listed under service `lambda-web`:
- Cause 1: lambda-web.delete-web-function-cascades-tags-and-resource-policy. Source: services/service-com-amazonaws-lambdaweb-lambdaweb/src/ops/get_resource_policy.rs:172, src/ops/list_tags.rs:172, src/ops/tag_resource.rs:217 (all `format!("The web function {} doesn't exist.", ...)`), with the name/ARN resolution at src/state.rs:451-458.
- Cause 2: lambda-web.tag-resource-rejects-reserved-aws-prefix-key-and-applies-no-tags. Source: services/service-com-amazonaws-lambdaweb-lambdaweb/src/state/tags.rs:31 (TAG_KEY_REQUIREMENT), :59-65 (`stated_requirement`, which wraps the clause in service_common::validation_message::envelope_over), :76-87 (`tag_map_violation`) and :94-101 (`tag_key_list_violation`), called from src/ops/tag_resource.rs:192-199.

Related but NOT covering these: open issue 20261104T212912Z-lambda-web-deletewebfunction-answers-404-resourcenotfoundexception-where-the.md concerns DeleteWebFunction's STATUS for an absent function (404 vs recorded 204) and quotes the same "The web function ... doesn't exist." literal in passing; it carries neither of these checks' replay keys and does not state the reader's required message, so Cause 1 is filed on its own. Open issue 20261106T141524Z-lambda-web-the-stale-revision-conflictexception-message-from.md owns the PutResourcePolicy stale-revision ConflictException message (a third check, skipped here as OWNED).

## Evidence
Cause 1 — GetResourcePolicy / ListTags after DeleteWebFunction:
  Expected (AWS): HTTP 404, Error.Code = ResourceNotFoundException, Error.Message = "The function <functionArn> does not exist." (fixed: "The function " ... " does not exist."; variable: the web-function ARN from the request URI label).
  Actual (emulator): HTTP 404, Error.Code = ResourceNotFoundException (correct), Error.Message = "The web function <functionArn> doesn't exist." (fixed: "The web function " ... " doesn't exist.").
  Replay assertion (check lambda-web.delete-web-function-cascades-tags-and-resource-policy, line 154): AssertionError: GetResourcePolicy on a deleted function said "The web function arn:aws:lambda:us-west-2:000000000000:web-function/sim2real-behav-lw-fn-90d3b55308e6 doesn't exist.", expected 'The function arn:aws:lambda:us-west-2:000000000000:web-function/sim2real-behav-lw-fn-90d3b55308e6 does not exist.' (the ARN is this run's instance of the variable part). The check's next assertion holds ListTags to the same sentence, so that reader diverges the same way.
  Model evidence: GetResourcePolicy http {GET /2025-03-07/resource-policy/{resourceArn}, 200}, ListTags http {GET /2025-03-07/tags/{resource}, 200}; both model ResourceNotFoundException httpError 404 with @required message.

Cause 2 — TagResource with one valid key and one reserved aws: key:
  Expected (AWS): HTTP 400, Error.Code = ValidationException, Error.Message = 'One or more of your tags are invalid. Please ensure none of your tag keys start with "aws:", none are empty, and there are no duplicate tag keys.' (entirely fixed text, no request-derived values), and no tag applied.
  Actual (emulator): HTTP 400, Error.Code = ValidationException (correct), no tag applied (correct), Error.Message = "1 validation error detected: Value 'aws:<key>' at 'tags.<key>' failed to satisfy constraint: Member must not begin with the reserved prefix aws:" — the count, the submitted key and the internal member path are interpolated.
  Replay assertion (check lambda-web.tag-resource-rejects-reserved-aws-prefix-key-and-applies-no-tags, line 129): AssertionError: unexpected validation message "1 validation error detected: Value 'aws:sim2real-reserved' at 'tags.aws:sim2real-reserved' failed to satisfy constraint: Member must not begin with the reserved prefix aws:" (that key is the run's instance of the variable part).
  Model evidence: TagResource http {POST /2025-03-07/tags/{resource}, 204}, idempotent; ValidationException httpError 400; TagKey @pattern ^[a-zA-Z0-9 _\.:/=+\-@]+$ admits ':' so the reserved-prefix rule is service-applied, not a modelled constraint.

## Steps to Reproduce
1. Boot the emulator and point a botocore `lambda-web` client at it (region us-west-2, any credentials).
2. Create an execution role (iam), a versioned code bucket (s3) with code.zip, then CreateWebFunction with revisionConfig{buildConfig{codeConfig.s3Object, runtimeConfig.runtime=nodejs24.x}, serviceConfig.executionRoleArn} and endpointConfig{endpointName=ep, endpointType=HomeRegion, authType=IamAuth}; keep the returned functionArn.

Cause 1:
3. PutResourcePolicy(resourceArn=functionArn, policy=<any valid policy>) and TagResource(resource=functionArn, tags={"k":"v"}); confirm GetResourcePolicy and ListTags return them.
4. DeleteWebFunction(functionName=<name>).
5. Call GetResourcePolicy(resourceArn=functionArn): the emulator answers 404 ResourceNotFoundException with message "The web function <functionArn> doesn't exist." instead of AWS's "The function <functionArn> does not exist."
6. Call ListTags(resource=functionArn): same divergent sentence.

Cause 2:
7. On a freshly created, Active web function call TagResource(resource=functionArn, tags={"ok":"v1", "aws:reserved":"x"}).
8. The emulator answers 400 ValidationException with message "1 validation error detected: Value 'aws:reserved' at 'tags.aws:reserved' failed to satisfy constraint: Member must not begin with the reserved prefix aws:" instead of AWS's 'One or more of your tags are invalid. Please ensure none of your tag keys start with "aws:", none are empty, and there are no duplicate tag keys.'
9. ListTags(resource=functionArn) correctly returns {} (the whole-call refusal is right); TagResource with only {"ok":"v1"} then succeeds, so only the message text diverges.
10. UntagResource(resource=functionArn, tagKeys=["aws:reserved"]) shows the same wrong wording from the shared requirement constant.
