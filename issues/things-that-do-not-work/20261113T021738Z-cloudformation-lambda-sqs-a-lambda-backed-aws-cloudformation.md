Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__sqs.cfn-custom-resource-create-delete-lifecycle-signals cloudformation__lambda__sqs.cfn-custom-resource-failed-response-rollback-negative
Issue-Key: cloudformation__lambda__sqs.cfn-custom-resource-create-delete-lifecycle-signals|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Key: cloudformation__lambda__sqs.cfn-custom-resource-failed-response-rollback-negative|_poll_until#9|Timeout|last = probe()
Issue-Commit: f3c33bab928599ae9956c8a8be7ee7afed317dab

# cloudformation__lambda__sqs: a Lambda-backed AWS::CloudFormation::CustomResource never settles — both the SUCCESS and the FAILED legs hang until the check's 60 s budget expires

> _Note: this issue was identified against the code base at commit `f3c33bab928599ae9956c8a8be7ee7afed317dab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: CloudFormation does not drive the custom-resource provider protocol to a terminal stack status. Both failing checks deploy the SAME template (deployments/custom-resource-response-status.yaml: an SQS PartialQueue, an IAM ResponderRole, a python3.12 Responder function, and a `Recorded: AWS::CloudFormation::CustomResource` whose `ServiceToken` is `!GetAtt Responder.Arn`), differing only in the `ResponseStatus` parameter (SUCCESS vs FAILED). Neither leg reaches any terminal stack status: the check is killed at its 60 s budget while still polling DescribeStacks, even though its own tolerance is 900 s.

AWS: CloudFormation resource types are not part of the Smithy API model, so this is AWS's documented custom-resource behavior plus the recording. On Create, CloudFormation invokes the ServiceToken Lambda with a request carrying RequestType=Create, LogicalResourceId, StackId, RequestId, ResourceProperties and a PRESIGNED ResponseURL (query string carries SigV4 X-Amz-Signature/X-Amz-Credential, or the SigV2 Signature/AWSAccessKeyId form), then blocks that resource until the provider PUTs its response to that URL. It adopts the `PhysicalResourceId` from the response body (so `Ref` on the custom resource returns the responder-minted id, here `sim2real-xsvc-cr-<hex>`), and publishes the response body's `Data` members so `Fn::GetAtt Recorded.Echo` resolves to the echoed value. A SUCCESS response takes the stack to CREATE_COMPLETE; DeleteStack then re-invokes the SAME provider with RequestType=Delete and the SAME PhysicalResourceId CloudFormation learned at Create time, and after DELETE_COMPLETE `DescribeStacks` on that stack name is refused with error code `ValidationError`, HTTP 400, message `Stack with id <stack-name> does not exist` (fixed text `Stack with id ` + the requested stack name + ` does not exist`). A FAILED Create response instead rolls the stack back to ROLLBACK_COMPLETE, carrying the provider's `Reason` on the resource's stack event, and the resource created before the custom resource (PartialQueue) is deleted by the rollback.

Emulator: neither leg produces any terminal status. The only observable is `Check timed out after 60.0s` — the DescribeStacks poll (interval 5 s, predicate CREATE_COMPLETE/CREATE_FAILED/ROLLBACK_COMPLETE/ROLLBACK_FAILED) never sees a terminal status, so no assertion on GetAtt Data, Ref, the Create/Delete lifecycle records on the external recorder queue, the ResponseURL's signature, or the post-delete ValidationError is ever reached. Hypothesis: the resource is created as an inert record — the ServiceToken Lambda is never invoked, or it is invoked but the resource is never released by a ResponseURL callback — so the stack stays CREATE_IN_PROGRESS indefinitely.

Why: every Lambda-backed custom resource is a dead end for a client. Deploys that use one (CDK custom resources, bucket deployments, bootstrap helpers, any Custom:: provider) never converge, so no stack containing one can be created, read back through Fn::GetAtt, or torn down; the negative leg additionally means a provider's FAILED verdict never rolls a stack back, so a half-built stack is never cleaned up.

Fix: the CloudFormation stack-provisioning engine's custom-resource provisioner (the handler for AWS::CloudFormation::CustomResource / Custom::*): invoke the ServiceToken provider with the documented request envelope and a presigned ResponseURL, hold the resource until the callback lands, then adopt PhysicalResourceId, expose Data to Fn::GetAtt, and settle the stack (CREATE_COMPLETE on SUCCESS; rollback to ROLLBACK_COMPLETE carrying Reason on FAILED), re-invoking the provider with RequestType=Delete on stack delete.

Related but NOT covering this: open issue 20261107T081525Z-cloudformation-lambda-sqs-a-custom-resource-that-answers.md describes a FAILED response ending in ROLLBACK_FAILED (a terminal status), and 20261112T140000Z-cloudformation-hooks-are-never-enforced-and-a-lambda-backed-custom-resource-never-completes.md covers a different service grouping. Here no terminal status is reached at all, on either leg.

Traces: cloudformation__lambda__sqs.cfn-custom-resource-create-delete-lifecycle-signals, cloudformation__lambda__sqs.cfn-custom-resource-failed-response-rollback-negative.

## Evidence
Both checks, verdict [mismatch]:
  Error: Check timed out after 60.0s

Expected (SUCCESS leg): stack reaches CREATE_COMPLETE; Outputs.EchoedByCustomResource == the EchoValue parameter (from Fn::GetAtt Recorded.Echo); Outputs.CustomResourcePhysicalId (Ref) starts with the responder-minted prefix `sim2real-xsvc-cr-`; a Create lifecycle record arrives on the external recorder queue with ResponseUrlSigned=true and the same PhysicalResourceId; after DeleteStack a Delete record arrives with that SAME PhysicalResourceId; DescribeStacks on the deleted stack then raises ClientError with Error.Code `ValidationError` and Error.Message `Stack with id <stack-name> does not exist`.
Expected (FAILED leg): the provider's FAILED Create response rolls the stack back to ROLLBACK_COMPLETE with the provider's Reason on the resource event, and the in-stack PartialQueue is gone.
Actual: no terminal stack status within the check's budget; nothing after the create_stack poll is observable.

## Steps to Reproduce
1. Create an SQS queue outside the stack to record lifecycle requests.
2. CreateStack with a template containing an SQS queue, an IAM role, a python3.12 Lambda that PUTs {Status, Reason, PhysicalResourceId, StackId, RequestId, LogicalResourceId, Data:{Echo}} to event['ResponseURL'], and `Recorded: AWS::CloudFormation::CustomResource` with ServiceToken = !GetAtt Responder.Arn, Echo = <value>, ResponseStatus = SUCCESS (then repeat with FAILED), Capabilities=['CAPABILITY_IAM'].
3. Poll DescribeStacks for a terminal status: it never leaves CREATE_IN_PROGRESS.
4. On the SUCCESS leg, read Outputs.EchoedByCustomResource (Fn::GetAtt Recorded.Echo) and Outputs.CustomResourcePhysicalId (Ref), then DeleteStack and look for the Delete lifecycle record on the recorder queue and for the ValidationError on DescribeStacks.
5. On the FAILED leg, expect ROLLBACK_COMPLETE and the PartialQueue to be deleted.
