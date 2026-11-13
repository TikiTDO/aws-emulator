Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock.tag-resource-quota-exceeded-rejected
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock|TagResource|ResourceNotFoundException
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# REOPENS bedrock TagResource: an automated-reasoning-policy ARN minted seconds earlier is refused 404 ResourceNotFoundException, so the tag-quota contract is unreachable (1 cause)

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
REOPENS 20261024T025841Z-bedrock-tagresource-stub.md: its fix left the call refused.

=== Cause 1 of 1: TagResource does not resolve automated-reasoning-policy ARNs, so tagging a policy it just created 404s ===
AWS: Smithy (com.amazonaws.bedrock) TagResource = POST /tagResource, success 200, required members resourceARN (TaggableResourcesArn, @length 20..1011) and tags (TagList, @length 0..200); the resourceARN pattern explicitly admits the automated-reasoning-policy form (:automated-reasoning-policy/[a-zA-Z0-9]+(:[a-zA-Z0-9]+)?$) alongside guardrail, provisioned-model, evaluation-job and the rest; modelled errors are TooManyTagsException (httpError 400), ResourceNotFoundException (404), ValidationException (400), AccessDeniedException (403). For a resource that exists, the answer is 200; ResourceNotFoundException is reserved for an ARN naming no resource. Recorded behavior for the subject of this check: a batch of tags that would push the resource past its per-resource tag quota is refused with TooManyTagsException, NONE of the rejected tags are applied (ListTagsForResource still reports exactly the prior set), and a within-quota TagResource on the same resource afterwards still succeeds.
Emulator: TagResource on the policyArn that CreateAutomatedReasoningPolicy returned moments earlier is refused 404 ResourceNotFoundException "no resource arn:aws:bedrock:us-west-2:<account>:automated-reasoning-policy/<id> exists in this account" — the operation's resource lookup does not cover automated-reasoning-policy ARNs even though the modelled pattern admits them, so a resource that demonstrably exists is reported absent.
Why: automated-reasoning policies cannot be tagged at all, which breaks cost allocation, ABAC-style access control and any teardown that finds resources by tag; the 404 also misleads the caller into believing the create did not take effect. Everything this check asserts behind the call — the TooManyTagsException quota refusal, the all-or-nothing tag application, and the later within-quota success — is unverified.
Fix: TagResource's handler (and the matching UntagResource/ListTagsForResource lookups) must resolve every resource family its resourceARN pattern admits, automated-reasoning-policy included, and only then evaluate the tag-count quota and answer TooManyTagsException.
Traces: bedrock.tag-resource-quota-exceeded-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, service bedrock 91/199 passed.

[error] bedrock.tag-resource-quota-exceeded-rejected
  setup: CreateAutomatedReasoningPolicy returns policyArn='arn:aws:bedrock:us-west-2:<account>:automated-reasoning-policy/<id>'
  request: TagResource(resourceARN=<that policyArn>, tags=[...])
  expected: 200 for the within-quota batch, then TooManyTagsException for the batch that exceeds the per-resource tag quota, with no partial application
  actual:   botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the TagResource operation: no resource arn:aws:bedrock:us-west-2:<account>:automated-reasoning-policy/<id> exists in this account
  (the ARN is this run's value; the requirement is that any ARN the modelled resourceARN pattern admits and that names a live resource resolves)

Regression linkage: 20261024T025841Z-bedrock-tagresource-stub.md is recorded as fixed (TagResource no longer answers NotImplemented) but the call is still refused, now 404 ResourceNotFoundException for an existing resource.

Model facts used (services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json): TagResource POST /tagResource code 200; resourceARN required @length 20..1011 with a pattern whose alternatives include (:automated-reasoning-policy/[a-zA-Z0-9]+(:[a-zA-Z0-9]+)?$); tags required TagList @length 0..200; errors TooManyTagsException 400 / ResourceNotFoundException 404 / ValidationException 400 / AccessDeniedException 403. CreateAutomatedReasoningPolicy output policyArn @pattern ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:automated-reasoning-policy/[a-z0-9]{12}(:([1-9][0-9]{0,11}))?$.

## Steps to Reproduce
Boot the emulator from this checkout and replay bedrock.tag-resource-quota-exceeded-rejected (suite simparity-behavioral-aws-v1).
1. CreateAutomatedReasoningPolicy(name=<unique>) and keep policyArn from the response.
2. TagResource(resourceARN=<policyArn>, tags=[{key,value}, ...]) within quota. Expect HTTP 200; observe 404 ResourceNotFoundException 'no resource <policyArn> exists in this account'.
3. (Unreached) TagResource with a batch that pushes the resource past its per-resource tag quota -> expect TooManyTagsException and ListTagsForResource unchanged.
4. (Unreached) A further within-quota TagResource on the same resource -> expect 200.
