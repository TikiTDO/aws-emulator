Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudformation__lambda.cfn-stack-rollback-on-custom-resource-failure
Issue-Key: apigateway__cloudformation__lambda.cfn-stack-rollback-on-custom-resource-failure|_poll_until#7|Timeout|last = probe()
Issue-Commit: 7eb5bf3e83f51302e9d7bc7ffd19e2ebc91fbccc

# apigateway__cloudformation__lambda: a stack whose Lambda-backed custom resource answers FAILED never reaches a terminal status, so the rollback check hangs

> _Note: this issue was identified against the code base at commit `7eb5bf3e83f51302e9d7bc7ffd19e2ebc91fbccc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: a CreateStack whose Custom:: resource is backed by a Lambda provider never settles — the stack stays non-terminal, so no rollback and no ROLLBACK_COMPLETE is ever observable.

AWS: CloudFormation invokes the custom resource's ServiceToken Lambda with an event carrying RequestType=Create and a presigned ResponseURL. When the provider PUTs {"Status":"FAILED"} to that URL, CloudFormation moves that logical resource to ResourceStatus CREATE_FAILED with a ResourceStatusReason of the fixed form `Received response status [FAILED] from custom resource. Message returned: <the provider's own Reason verbatim> (RequestId: <the invocation's AWS-minted request id>)` — only the Reason text and the request id vary with the request/state — then rolls the stack back: already-created siblings are deleted and DescribeStacks reports StackStatus ROLLBACK_COMPLETE. The same template without the forced failure ends CREATE_COMPLETE. Smithy (cloudformation): StackStatus enum includes CREATE_IN_PROGRESS, ROLLBACK_IN_PROGRESS, ROLLBACK_COMPLETE, CREATE_COMPLETE; ResourceStatus enum includes CREATE_FAILED and CREATE_COMPLETE. The response-status wrapping is CloudFormation's documented custom-resource response handling (recorded behavior for the exact literal).

Emulator: CreateStack is accepted, but the stack never leaves a non-terminal status: polling DescribeStacks for any of ROLLBACK_COMPLETE / ROLLBACK_FAILED / CREATE_COMPLETE never succeeds and the check is killed by the harness ('Check timed out after 60.0s'). No CREATE_FAILED event for the custom resource and no rollback of the sibling resource is ever produced, because the Lambda-backed provider's response is never consumed to advance the resource.

Why: a client that deploys any template containing a Lambda-backed Custom:: resource waits forever — `aws cloudformation wait stack-create-complete`, CDK/SAM deploys and any CI gate that polls for a terminal StackStatus never return, and a deliberately failing provider (the normal way to abort a deploy) never produces the ROLLBACK_COMPLETE plus CREATE_FAILED reason that automation reads to decide a deploy failed.

Fix: the CloudFormation stack provisioner's custom-resource handling — the component that invokes the ServiceToken provider, serves/accepts the presigned ResponseURL callback, maps Status=SUCCESS/FAILED onto the resource's ResourceStatus and ResourceStatusReason, and drives the stack state machine on to ROLLBACK_IN_PROGRESS/ROLLBACK_COMPLETE (deleting already-created siblings) or CREATE_COMPLETE.

Traces: behavioral check apigateway__cloudformation__lambda.cfn-stack-rollback-on-custom-resource-failure (verdict mismatch; 'Check timed out after 60.0s'). Related open issues describe neighbouring symptoms but do not carry this check's replay key: 20261109T152222Z-apigateway-cloudformation-lambda-cfn-generated-lambda-permission-statementid.md, 20261111T081112Z-apigateway-cloudformation-lambda-a-cloudformation-custom-resource-delete-never-completes-so.md, 20261105T022123Z-apigateway-cloudformation-lambda-4-net-new-root-causes.md.

## Evidence
Expected: DescribeStacks for the stack created with ForceFail=true reaches StackStatus=ROLLBACK_COMPLETE; DescribeStackEvents contains LogicalResourceId=MyResource with ResourceStatus=CREATE_FAILED (and a CREATE_COMPLETE event for the sibling AlwaysTopic before it); the twin stack without ForceFail reaches CREATE_COMPLETE.
Actual: the stack never reports any terminal StackStatus; the poll for ROLLBACK_COMPLETE/ROLLBACK_FAILED/CREATE_COMPLETE never matches and the run aborts with 'Check timed out after 60.0s' — no CREATE_FAILED event and no rollback are ever observed.

## Steps to Reproduce
1. CreateStack with a template containing a Custom:: resource whose ServiceToken is a Lambda function that PUTs {"Status":"FAILED","Reason":"<text>"} to the event's ResponseURL, plus one sibling resource (e.g. an SNS topic) that creates successfully; pass Parameters ForceFail=true and Capabilities CAPABILITY_IAM/CAPABILITY_NAMED_IAM.
2. Poll DescribeStacks(StackName) for a terminal StackStatus.
3. Observe the stack never reaches ROLLBACK_COMPLETE (or any terminal status) and DescribeStackEvents never shows the custom resource CREATE_FAILED with the `Received response status [FAILED] from custom resource. Message returned: ... (RequestId: ...)` reason.
4. Replay behavioral check apigateway__cloudformation__lambda.cfn-stack-rollback-on-custom-resource-failure.
