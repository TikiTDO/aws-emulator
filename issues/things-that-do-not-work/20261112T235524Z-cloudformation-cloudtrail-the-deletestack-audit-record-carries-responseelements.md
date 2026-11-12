Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cloudtrail.obs-cloudformation-1-stack-lifecycle-is-audited-as-the-caller-and-provisioning-as-the-service
Issue-Key: cloudformation__cloudtrail.obs-cloudformation-1-stack-lifecycle-is-audited-as-the-caller-and-provisioning-as-the-service|run#67|AssertionError|assert found["DeleteStack"].get("responseElements") is None, (
Issue-Commit: 0814fbf24ca8581d1be982874f66970e2423c484

# cloudformation__cloudtrail: the DeleteStack audit record carries responseElements {} instead of null, so a no-output call is indistinguishable from one that returned data

> _Note: this issue was identified against the code base at commit `0814fbf24ca8581d1be982874f66970e2423c484`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteStack

## Description
One net-new root cause in the CloudTrail record the emulator serves for a CloudFormation stack-lifecycle call.

Cause 1 of 1: a CloudTrail event record for an operation with an empty output shape must carry responseElements as null, not as an empty object. Failing check: cloudformation__cloudtrail.obs-cloudformation-1-stack-lifecycle-is-audited-as-the-caller-and-provisioning-as-the-service.

AWS:      CloudTrail populates an event record's responseElements only from the response data the audited call actually returned, and records null for a call that returns none. CloudFormation's DeleteStack returns no fields at all — Smithy: DeleteStack output_shape is "Unit" with no members (errors: TokenAlreadyExistsException) — so in a real LookupEvents/CloudTrail record for DeleteStack the responseElements member is null (absent from the record JSON). The same rule holds for a refused mutating call, which minted nothing: the check's parallel assertion that a duplicate CreateStack's record carries responseElements null does pass here, so the emulator already implements the null case for that path; only the no-output operation diverges. CreateStack and UpdateStack, whose output shapes DO carry StackId, correctly report responseElements.stackId.
          Reference: https://docs.aws.amazon.com/awscloudtrail/latest/userguide/cloudtrail-event-reference-record-contents.html
Emulator: the record LookupEvents serves for DeleteStack carries responseElements as an empty object. Replay output: "AssertionError: DeleteStack returns no fields, so its record's responseElements must be null; actual {}". The remainder of the record is right (eventSource cloudformation.amazonaws.com, eventName DeleteStack, userIdentity.arn the caller, no errorCode, readOnly false, eventTime in the call's window), so this is purely the serialization of the empty-output case.
Why:      a consumer tells "this call returned nothing" from "this call returned data" by whether responseElements is null. An empty object is a present, non-null value: audit pipelines, SIEM normalizers and policy code that branch on the member's presence (or that diff an emulator record against a recorded AWS record) take the DeleteStack record as carrying response data and either mis-shape it or fail the comparison, and any schema that types responseElements as nullable-object-with-content sees a record AWS never emits.
Fix:      in the CloudTrail record builder/serializer that turns an audited management call into an event record — the step that projects the operation's response into responseElements. When the audited operation's output shape has no members (or the serialized projection is empty), emit the member as null / omit it rather than serializing an empty map. The refused-call path already does this, so the empty-output path should take the same branch.
Traces:   cloudformation__cloudtrail.obs-cloudformation-1-stack-lifecycle-is-audited-as-the-caller-and-provisioning-as-the-service (the DeleteStack leg; every earlier assertion in the same check — caller attribution, the duplicate CreateStack's AlreadyExistsException errorCode and errorMessage, CreateStack/UpdateStack responseElements.stackId, and the CloudFormation-attributed CreateQueue provisioning record — passed before this one).

## Evidence
Check cloudformation__cloudtrail.obs-cloudformation-1-stack-lifecycle-is-audited-as-the-caller-and-provisioning-as-the-service, verdict mismatch.

Expected (real AWS): the LookupEvents record for the DeleteStack call has responseElements == null, because Smithy gives DeleteStack an empty output shape (output_shape "Unit", no members).

Actual (emulator): responseElements == {} (empty object).

Replay failure, verbatim:
  File "check_cloudformation__cloudtrail_obs_cloudformation_1_stack_lifecycle_is_audited_as_the_caller_and_provisioning_as_the_service.py", line 474, in run
    assert found["DeleteStack"].get("responseElements") is None, (
  AssertionError: DeleteStack returns no fields, so its record's responseElements must be null; actual {}

Contrast inside the same check (both pass, so they pin the expected shape): the refused duplicate CreateStack's record has responseElements null, and CreateStack/UpdateStack records have responseElements.stackId equal to the StackId CreateStack returned. Only the empty-output operation is serialized as {}.

## Steps to Reproduce
1. Create an IAM service role for CloudFormation and a stack whose template declares one AWS::SQS::Queue, passing RoleARN on CreateStack; wait for CREATE_COMPLETE.
2. Re-issue CreateStack with the same stack name (expect AlreadyExistsException), then UpdateStack the queue's VisibilityTimeout, then DeleteStack the stack by StackId.
3. Poll cloudtrail:LookupEvents (LookupAttributes ResourceName/EventName, StartTime before step 1) until the CreateStack, duplicate CreateStack, UpdateStack and DeleteStack records are all present.
4. Read the DeleteStack record's CloudTrailEvent JSON and inspect responseElements.
   Expected: responseElements is null/absent. Actual: responseElements is {}.
5. For the contrast, inspect the duplicate CreateStack record's responseElements: it is already null, which is the branch the DeleteStack record should take.
