Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sns__sqs.custom-resource-getatt-missing-data-key-fails cloudformation__sns__sqs.custom-resource-service-timeout-out-of-range-fails-the-resource cloudformation__sns__sqs.xsvc-cloudformation-3-sns-custom-resource-failed-response-rolls-back-with-its-reason cloudformation__sns__sqs.custom-resource-response-over-4096-bytes-fails cloudformation__sns__sqs.xsvc-cloudformation-3-sns-custom-resource-new-physical-id-on-update-triggers-cleanup-delete-of-old
Issue-Key: cloudformation__sns__sqs.custom-resource-getatt-missing-data-key-fails|run#12|AssertionError|assert sink["ResourceProperties"] == {"ServiceToken": topic_arn, "Value": value}, (
Issue-Key: cloudformation__sns__sqs.custom-resource-service-timeout-out-of-range-fails-the-resource|run#12|AssertionError|assert final["StackStatus"] == "ROLLBACK_COMPLETE", (
Issue-Key: cloudformation__sns__sqs.xsvc-cloudformation-3-sns-custom-resource-failed-response-rolls-back-with-its-reason|run#26|AssertionError|assert dreq["PhysicalResourceId"] == failed_pid, (
Issue-Key: cloudformation__sns__sqs.custom-resource-response-over-4096-bytes-fails|pull#21|Timeout|for msg in resp.get("Messages", []):
Issue-Key: cloudformation__sns__sqs.xsvc-cloudformation-3-sns-custom-resource-new-physical-id-on-update-triggers-cleanup-delete-of-old|pull#17|Timeout|for msg in resp.get("Messages", []):
Issue-Commit: 68ec0c9cae77cfbc133f0186a4c923306bef5402

# cloudformation__sns__sqs: 5 net-new custom-resource provider causes — GetAtt Data is not resolved into the consumer's request, ServiceTimeout is not range-checked, a FAILED response's PhysicalResourceId is not adopted, and later/replacement provider requests never arrive

> _Note: this issue was identified against the code base at commit `68ec0c9cae77cfbc133f0186a4c923306bef5402`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Five net-new root causes of service `cloudformation__sns__sqs`, all in the CloudFormation custom-resource (SNS `ServiceToken`) provisioner. None of the five failing checks was marked ALREADY FILED or OWNED by the replay report. Context: the resolved issue 20261025T180059Z-cloudformation-custom-resource-providers-and-hook-configuration.md made the provider invocation itself work (requests now do reach the SNS topic and the control legs of these checks complete), and the open issue 20261101T212352Z-cloudformation-sns-sqs-a-custom-resource-whose-servicetoken.md describes that same (now-fixed) invocation gap and lists two of these check ids — but it no longer owns them: they now fail further downstream, on the rules below. The open issue 20261105T040000Z-cloudformation-6-of-the-7-custom-resource-and-hook-keys-remain-open.md names the related 'LATER provider requests never arrive' symptom and the open issue 20261105T022225Z-cloudformation-lambda-sqs-2-net-new-causes-the.md names the PhysicalResourceId-adoption symptom for a different service token; both are named here so a fixer can close them together. No cause below is a refusal, so none of them reopens a resolved issue.

=== Cause 1 of 5: Fn::GetAtt on a custom resource's response Data is not resolved into the dependent resource's ResourceProperties ===
AWS: the custom-resource protocol is not in the Smithy model (recorded behavior only; AWS reference: Custom resource response objects, crpg-ref). A provider that answers SUCCESS with a `Data` map makes `Fn::GetAtt <LogicalId>.<Key>` resolve to `Data[<Key>]`. A second resource whose own property is `{"Fn::GetAtt": ["Provider", "Present"]}` must therefore receive a Create provider request whose `ResourceProperties` contains BOTH `ServiceToken` and that resolved member — i.e. `{"ServiceToken": "<topic ARN>", "Value": "<the value the provider returned under Data.Present>"}`. (Here `ServiceToken`, `Value` and the member names are fixed template text; the topic ARN and the value are taken from the request and from the provider's own response.) `Fn::GetAtt` on a key the `Data` map LACKS must instead fail the consuming resource with the attribute error, not silently drop the property.
Emulator: the dependent resource's request arrives with the GetAtt member missing entirely — `ResourceProperties` is `{'ServiceToken': 'arn:aws:sns:<region>:<account>:<topic>'}` with no `Value` key at all, for the key the provider DID return. The GetAtt is resolved to nothing and dropped rather than to the provider's Data value, so the missing-key leg can never be distinguished from the present-key leg either.
Why: a client whose template consumes a provider's output (every CDK/SAM construct implemented as a custom resource that exports a value, and every hand-written provider+consumer pair) gets a consumer that is provisioned with its property silently absent instead of with the provider's value — and gets no error, so the breakage surfaces only inside the consumer.
Fix: the CloudFormation custom-resource provisioner's intrinsic-resolution step must record the provider response's `Data` map against the logical id and resolve `Fn::GetAtt` from it when rendering the dependent resource's `ResourceProperties`, and must fail the consuming resource when the named key is absent from `Data`.
Traces: cloudformation__sns__sqs.custom-resource-getatt-missing-data-key-fails.

=== Cause 2 of 5: a custom resource's ServiceTimeout outside 1-14400 is accepted and the stack completes ===
AWS: `AWS::CloudFormation::CustomResource`/`Custom::*` accepts `ServiceTimeout` in seconds, constrained to 1-14400 (AWS template reference for AWS::CloudFormation::CustomResource; not in the Smithy model, so recorded behavior only for the exact text). A value outside that range fails the custom resource before any request is published to the provider, and the stack settles in `ROLLBACK_COMPLETE`; the boundary value `14400` is accepted, the request is published, and the stack completes when the provider answers.
Emulator: `ServiceTimeout: "0"` is accepted and the stack settles in `CREATE_COMPLETE` (`AssertionError: ServiceTimeout '0' must fail the create; got 'CREATE_COMPLETE'`); `14401` is likewise not refused. The range is not validated at all.
Why: a client that mis-sets ServiceTimeout gets a stack that reports success and a resource whose stabilisation ceiling is whatever the emulator defaults to, instead of the deterministic template-validation failure AWS gives — so a template that AWS would reject deploys here and behaves differently in production.
Fix: template/property validation for `Custom::*` and `AWS::CloudFormation::CustomResource` in the CloudFormation provisioner: range-check `ServiceTimeout` (1-14400 inclusive) and fail the resource with its reason before publishing any provider request.
Traces: cloudformation__sns__sqs.custom-resource-service-timeout-out-of-range-fails-the-resource.

=== Cause 3 of 5: the PhysicalResourceId returned with a FAILED provider response is not adopted, so the rollback Delete names a CloudFormation-generated id ===
AWS: when a provider answers `Status=FAILED`, the `PhysicalResourceId` in that response becomes the resource's physical id, and the rollback that follows publishes a `Delete` request carrying exactly that id (recorded behavior only; AWS reference: crpg-ref). The id is a value the provider chose — CloudFormation must echo it back, never substitute one of its own.
Emulator: the rollback Delete carries a CloudFormation-generated id of the shape `<stack-name>-<LogicalId>-<suffix>` instead of the provider's id: `AssertionError: rollback Delete PhysicalResourceId: expected the id returned with FAILED ('<provider id>'), got '<stack>-Thing-<suffix>'`. (The concrete names in that message are per-run values; the requirement is that the two ids are the SAME string, the one the provider sent.)
Why: a provider uses the Delete request's PhysicalResourceId to find and release what it created. Given an id it never issued, the provider cannot clean up, so provider-side resources leak on every failed custom resource — and idempotent providers will treat the Delete as being for an unknown resource.
Fix: the custom-resource provisioner's response-handling path must adopt the response body's `PhysicalResourceId` on FAILED as well as on SUCCESS, and the rollback Delete must be rendered from the stored physical id rather than from the generated-name helper.
Traces: cloudformation__sns__sqs.xsvc-cloudformation-3-sns-custom-resource-failed-response-rolls-back-with-its-reason.

=== Cause 4 of 5: the 4096-byte custom-resource response-size rule is unreachable — the follow-on resource's provider request never arrives ===
AWS: a custom-resource response document is limited to 4096 bytes; a response of exactly 4096 bytes completes its resource, and one of 4097 bytes fails the next resource with the too-long reason, after which the rollback deletes it before the resource it depended on (recorded behavior only; AWS reference: crpg-ref). Two `Custom::Sized` resources chained with `DependsOn` must therefore each receive their own Create request in dependency order.
Emulator: after the first resource's request is answered with the 4096-byte response, the second (DependsOn) resource's Create request is never published, so the check makes no further progress and is killed at the harness ceiling: `Check timed out after 60.0s`. Neither the accept-at-4096 nor the fail-at-4097 behaviour is reachable. Hypothesis: nothing drives the next resource's provisioning once the first provider response lands — the same symptom the open issue 20261105T040000Z-cloudformation-6-of-the-7-custom-resource-and-hook-keys-remain-open.md records as 'a custom resource's LATER provider requests never arrive'.
Why: any template with more than one custom resource, or one custom resource plus a dependent resource, never settles: the client sees a stack stuck in CREATE_IN_PROGRESS forever rather than either a completed stack or the documented too-long failure.
Fix: the custom-resource provisioner must resume the dependency walk after a provider response is accepted — publishing the next resource's request from the stabilisation barrier — and must enforce the 4096-byte response-body ceiling, failing the resource with its reason when the response exceeds it.
Traces: cloudformation__sns__sqs.custom-resource-response-over-4096-bytes-fails.

=== Cause 5 of 5: an Update answered with a NEW PhysicalResourceId produces no cleanup Delete for the old id ===
AWS: the provider's Update response decides replacement (recorded behavior only; AWS reference: crpg-ref). A response carrying a PhysicalResourceId DIFFERENT from the one the resource already had is a replacement: the stack passes through `UPDATE_COMPLETE_CLEANUP_IN_PROGRESS` and CloudFormation publishes a `Delete` request for the OLD physical id carrying the OLD `ResourceProperties`. A response repeating the same id is an in-place update and publishes no Delete.
Emulator: after the stack is updated and the provider answers Update with a new id, no cleanup Delete is ever published, so the check blocks reading the provider queue and is killed at the harness ceiling: `Check timed out after 60.0s`. The in-place-vs-replacement distinction is not implemented.
Why: a replacement leaves the old provider-side object alive forever — the client's stack reports UPDATE_COMPLETE while the resource the old id named is never released, and a provider can never garbage-collect it.
Fix: the custom-resource provisioner's update path must compare the response's PhysicalResourceId with the stored one and, when it differs, enter the cleanup phase and publish a Delete for the old id with the old properties.
Traces: cloudformation__sns__sqs.xsvc-cloudformation-3-sns-custom-resource-new-physical-id-on-update-triggers-cleanup-delete-of-old.

## Evidence
Cause 1 — cloudformation__sns__sqs.custom-resource-getatt-missing-data-key-fails [mismatch]:
  expected: the Consumer (`Custom::Sink`) Create request's ResourceProperties == {"ServiceToken": "<topic ARN>", "Value": "<the value returned under the Provider's Data.Present>"}
  actual:   AssertionError: GetAtt Provider.Present must deliver the provider's Data: {'ServiceToken': 'arn:aws:sns:us-west-2:000000000000:sim2real-xsvc-cr-t-<id>'}  (the 'Value' member is absent)

Cause 2 — cloudformation__sns__sqs.custom-resource-service-timeout-out-of-range-fails-the-resource [mismatch]:
  expected: a stack whose only resource is Custom::Bounded with Properties.ServiceTimeout = "0" settles in StackStatus ROLLBACK_COMPLETE, and no provider request is published; the control stack with ServiceTimeout "14400" is published and completes
  actual:   AssertionError: ServiceTimeout '0' must fail the create; got 'CREATE_COMPLETE'

Cause 3 — cloudformation__sns__sqs.xsvc-cloudformation-3-sns-custom-resource-failed-response-rolls-back-with-its-reason [mismatch]:
  expected: rollback Delete request's PhysicalResourceId == the PhysicalResourceId sent with the FAILED response
  actual:   AssertionError: rollback Delete PhysicalResourceId: expected the id returned with FAILED ('sim2real-cr-failed-pid-<run id>'), got 'sim2real-xsvc-cfn3-fail-<run id>-Thing-<suffix>' — i.e. a generated <stack>-<logical>-<suffix> id, not the provider's

Cause 4 — cloudformation__sns__sqs.custom-resource-response-over-4096-bytes-fails [mismatch]:
  expected: after the 'Fits' resource is answered with a response body of exactly 4096 bytes, the DependsOn resource 'Overflows' receives its own Create provider request; answering it with 4097 bytes fails that resource with the too-long reason and rolls the stack back
  actual:   no request for 'Overflows' is ever published; the check spends its whole window in ReceiveMessage and the harness reports `Check timed out after 60.0s`

Cause 5 — cloudformation__sns__sqs.xsvc-cloudformation-3-sns-custom-resource-new-physical-id-on-update-triggers-cleanup-delete-of-old [mismatch]:
  expected: an Update answered with a new PhysicalResourceId is followed by a cleanup Delete request for the OLD id carrying the OLD ResourceProperties (and the same-id variant publishes no Delete)
  actual:   no cleanup Delete is ever published; `Check timed out after 60.0s`

Smithy grounding used: com.amazonaws.cloudformation#Output has members OutputKey/OutputValue/Description/ExportName (cited for the sibling Outputs defect filed against cloudformation__lambda__sqs). The custom-resource request/response protocol itself is not modelled in Smithy, hence 'recorded behavior only' above.

## Steps to Reproduce
Boot the emulator and replay the five checks in trace_ids from suite simparity-behavioral-aws-v1.

By hand (all five share this provider harness):
1. sns.CreateTopic; sqs.CreateQueue; sqs.SetQueueAttributes with a Policy allowing Principal {"Service":"sns.amazonaws.com"} sqs:SendMessage on the queue ARN with Condition ArnEquals aws:SourceArn = the topic ARN; sns.Subscribe(Protocol="sqs", Endpoint=<queue ARN>). Provider requests arrive on the queue; answer each by PUTting a response document to its ResponseURL.

Cause 1: cloudformation.CreateStack with TemplateBody {"Resources":{"Provider":{"Type":"Custom::Source","Properties":{"ServiceToken":"<topic ARN>"}},"Consumer":{"Type":"Custom::Sink","Properties":{"ServiceToken":"<topic ARN>","Value":{"Fn::GetAtt":["Provider","Present"]}}}}}. Answer the Provider request with {"Status":"SUCCESS","PhysicalResourceId":"p-1","Data":{"Present":"v-1"},...}. Read the next request: its ResourceProperties must be {"ServiceToken":"<topic ARN>","Value":"v-1"}; the emulator sends only {"ServiceToken":...}. Repeat with Fn::GetAtt ["Provider","Absent"] — that must fail the Consumer.

Cause 2: CreateStack with {"Resources":{"Thing":{"Type":"Custom::Bounded","Properties":{"ServiceToken":"<topic ARN>","ServiceTimeout":"0"}}}} and DescribeStacks — AWS settles ROLLBACK_COMPLETE with no provider request; the emulator reports CREATE_COMPLETE. Repeat with "14401" (must also fail) and "14400" (must be published and complete).

Cause 3: CreateStack with one Custom:: resource; answer its Create with {"Status":"FAILED","Reason":"<reason>","PhysicalResourceId":"my-failed-pid",...}. Read the rollback Delete request: its PhysicalResourceId must be "my-failed-pid"; the emulator sends a generated <stack>-Thing-<suffix> id.

Cause 4: CreateStack with two Custom::Sized resources where the second has DependsOn the first; answer the first with a response document padded to exactly 4096 bytes. AWS then publishes the second resource's Create request; the emulator publishes nothing further and the stack never settles. Answering the second with 4097 bytes must fail it with the too-long reason.

Cause 5: CreateStack with {"Resources":{"Thing":{"Type":"Custom::Versioned","Properties":{"ServiceToken":"<topic ARN>","Name":"n1"}}}}; answer Create with PhysicalResourceId "pid-1". UpdateStack changing Name to "n2"; answer the Update with a DIFFERENT PhysicalResourceId "pid-2". AWS then publishes a Delete for "pid-1" with the old properties; the emulator publishes none.
