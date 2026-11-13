Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__logs.cfn-custom-resource-failure-reason-exact-and-delete-removes-resources
Issue-Key: cloudformation__lambda__logs.cfn-custom-resource-failure-reason-exact-and-delete-removes-resources|_poll_until#8|Timeout|time.sleep(interval_s)
Issue-Commit: 7eb5bf3e83f51302e9d7bc7ffd19e2ebc91fbccc

# cloudformation__lambda__logs: a Lambda-backed custom resource's FAILED Create never rolls the stack back, and no Delete invocation is ever made, so the check hangs

> _Note: this issue was identified against the code base at commit `7eb5bf3e83f51302e9d7bc7ffd19e2ebc91fbccc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: a stack whose Custom:: resource provider answers Status=FAILED on Create never reaches a terminal StackStatus, so neither the verbatim FAILED reason nor the rollback's RequestType=Delete invocation of the same provider is ever produced.

AWS: on a custom resource's FAILED Create response, CloudFormation marks the logical resource ResourceStatus=CREATE_FAILED with ResourceStatusReason of the fixed form `Received response status [FAILED] from custom resource. Message returned: <the handler's own Reason, verbatim> (RequestId: <the invocation's AWS-minted uuid>)` — the sentence is fixed text, the Reason comes from the provider's response and the RequestId from the invocation — then rolls the stack back to StackStatus=ROLLBACK_COMPLETE, invoking the SAME provider again with RequestType=Delete (visible in the function's /aws/lambda/<fn> log group) rather than merely dropping the resource from its record; an explicit DeleteStack likewise drives the Delete branch and leaves neither the function nor its log group behind. Smithy (cloudformation): StackStatus enum includes ROLLBACK_IN_PROGRESS/ROLLBACK_COMPLETE; ResourceStatus enum includes CREATE_FAILED. The reason wrapping is recorded behavior of CloudFormation's custom-resource response handling.

Emulator: the stack created with ForceFail=true never reaches a terminal status — the wait for ROLLBACK_COMPLETE never returns and the run is killed with 'Check timed out after 60.0s'. Consequently there is no CREATE_FAILED event to carry the wrapped reason, and no RequestType=Delete line is ever written to the provider's log group.

Why: a client cannot abort a deploy through a custom resource: the deploy hangs instead of rolling back, the failure reason automation parses (and surfaces to users) is never emitted, and provider-managed external state is never cleaned up because the Delete branch is never invoked — leaking resources the provider created before it failed.

Fix: the CloudFormation stack provisioner's custom-resource lifecycle — consuming the provider's presigned-URL response, mapping FAILED onto CREATE_FAILED with CloudFormation's fixed reason template, and driving rollback/DeleteStack so the provider is re-invoked with RequestType=Delete.

Traces: behavioral check cloudformation__lambda__logs.cfn-custom-resource-failure-reason-exact-and-delete-removes-resources (verdict mismatch; 'Check timed out after 60.0s'). Neighbouring open issues that do not carry this check's replay key: 20261107T081525Z-cloudformation-lambda-sqs-a-custom-resource-that-answers.md, 20261112T140000Z-cloudformation-hooks-are-never-enforced-and-a-lambda-backed-custom-resource-never-completes.md, 20261110T214500Z-cloudformation-a-custom-resources-delete-is-sent-and-its-provider-torn-down.md.

## Evidence
Expected: DescribeStacks reaches StackStatus=ROLLBACK_COMPLETE; DescribeStackEvents has LogicalResourceId=Signal with ResourceStatus=CREATE_FAILED whose ResourceStatusReason fully matches `Received response status [FAILED] from custom resource. Message returned: <FailReason> (RequestId: <uuid>)`; the provider's log group /aws/lambda/<FunctionName> contains an event with "RequestType": "Delete"; after DeleteStack neither the function nor its log group remains.
Actual: no terminal StackStatus is ever reported — the wait aborts with 'Check timed out after 60.0s'; no CREATE_FAILED event, no wrapped reason, and no Delete invocation are observed.

## Steps to Reproduce
1. CreateStack with a template whose Custom:: resource's ServiceToken is a Lambda function that, when ForceFail=true, PUTs {"Status":"FAILED","Reason":"<FailReason>"} to the event ResponseURL and logs the whole event.
2. Poll DescribeStacks for a terminal StackStatus.
3. Observe it never becomes ROLLBACK_COMPLETE; DescribeStackEvents shows no CREATE_FAILED for the custom resource, and FilterLogEvents on /aws/lambda/<FunctionName> shows no "RequestType": "Delete" record.
4. Replay behavioral check cloudformation__lambda__logs.cfn-custom-resource-failure-reason-exact-and-delete-removes-resources.
