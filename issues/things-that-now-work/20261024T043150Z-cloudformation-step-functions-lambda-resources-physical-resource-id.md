Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__stepfunctions.update-state-machine-preserves-arn-and-bumps-revision cloudformation__stepfunctions.deploy-cfn-stack-reports-complete-while-state-machine-is-deleted stepfunctions.deploy-cfn-stack-deletion-removes-state-machine stepfunctions.deploy-sfn-1-type-change-replaces-state-machine lambda__sqs__stepfunctions.deploy-cdk-synth-express-workflow-lambda-sqs-stack stepfunctions.deploy-sfn-2-invalid-definition-rolls-back-stack-and-retained-machine-survives-delete lambda__sqs__stepfunctions.deploy-sfn-lambda-sqs-dlq-redrive-policy-stack
Issue-Key: sfn|DescribeStateMachine|InvalidArn
Issue-Key: sfn|StartSyncExecution|InvalidArn
Issue-Key: stepfunctions.deploy-sfn-2-invalid-definition-rolls-back-stack-and-retained-machine-survives-delete|run#16|AssertionError|assert bad_status == "ROLLBACK_COMPLETE", (
Issue-Key: lambda__sqs__stepfunctions.deploy-sfn-lambda-sqs-dlq-redrive-policy-stack|<lambda>#8|IndexError|lambda: lam.list_event_source_mappings(EventSourceArn=_queue_arn(ctx, source_url))[
Issue-Commit: 3ecafc116f2b44ac9fb7ddf17b83f092c778a529

# CloudFormation Step Functions / Lambda resources: physical resource id is the resource NAME not an ARN, invalid definitions never roll the stack back, and AWS::Lambda::EventSourceMapping is never created (3 causes)

> _Note: this issue was identified against the code base at commit `3ecafc116f2b44ac9fb7ddf17b83f092c778a529`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStackResources

## Description
This issue bundles THREE distinct root causes in the CloudFormation resource providers exercised by Step Functions deployments. All three must be closed before submitting. Re-replay every check id in trace_ids. Related open issue 20261023T114649Z-cloudformation-outputs-carry-no-outputvalue-failed-resources-never.md describes the same family of defects but carries the replay keys for only three sfn deploy checks; the checks below are not in its replay set.

Cause 1 of 3: the physical resource id (and `Ref`) of an `AWS::StepFunctions::StateMachine` is the state machine's NAME instead of its ARN.
AWS: `Ref` / `PhysicalResourceId` for AWS::StepFunctions::StateMachine is the state machine ARN (`arn:aws:states:<region>:<account>:stateMachine:<name>`); `Fn::GetAtt ... Name` is the name. Smithy (sfn): `DescribeStateMachine.stateMachineArn` and `StartSyncExecution.stateMachineArn` are `Arn`-typed and `@required`, and `InvalidArn` (HTTP 400) is the modeled refusal for a value that is not an ARN.
Emulator: DescribeStackResources / stack Outputs hand back a bare name such as the stack-qualified logical name, and the subsequent Step Functions call fails with `InvalidArn: Invalid Arn: 'Invalid ARN prefix: <name>'`.
Why: nothing deployed by CloudFormation (or by `cdk deploy`, which reads the same values) can be addressed through the Step Functions API — describing, executing, updating or deleting the deployed state machine is impossible.
Fix: the AWS::StepFunctions::StateMachine provider in the cloudformation crate must record the created state machine's ARN as the resource's physical id and as the value `Ref` resolves to.
Traces: cloudformation__sfn.update-state-machine-preserves-arn-and-bumps-revision, cloudformation__stepfunctions.deploy-cfn-stack-reports-complete-while-state-machine-is-deleted, sfn.deploy-cfn-stack-deletion-removes-state-machine, sfn.deploy-sfn-1-type-change-replaces-state-machine, lambda__sfn__sqs.deploy-cdk-synth-express-workflow-lambda-sqs-stack.

Cause 2 of 3: a resource whose properties the target service refuses does not fail the resource, so the stack never rolls back.
AWS: when AWS::StepFunctions::StateMachine carries a DefinitionString whose `StartAt` names a state the definition does not declare, the resource CREATE fails (Step Functions returns `InvalidDefinition`), CloudFormation rolls the stack back and `DescribeStacks` reports `ROLLBACK_COMPLETE`; resources with `DeletionPolicy: Retain` survive the subsequent stack delete.
Emulator: the stack reaches `CREATE_COMPLETE`.
Why: a template a client cannot deploy on AWS deploys cleanly here, and the rollback/retain semantics the client depends on are never exercised — failures surface only much later, in production.
Fix: the resource-create path in the cloudformation crate must propagate the target service's refusal into resource failure and stack rollback, not swallow it.
Traces: sfn.deploy-sfn-2-invalid-definition-rolls-back-stack-and-retained-machine-survives-delete.

Cause 3 of 3: an `AWS::Lambda::EventSourceMapping` declared in a stack is never created in Lambda.
AWS: after the stack completes, `lambda:ListEventSourceMappings(EventSourceArn=<queue arn>)` returns the mapping the template declared, with its UUID, batch size and the function it targets.
Emulator: `ListEventSourceMappings` returns an empty `EventSourceMappings` list, so the check's indexing into it raises `IndexError: list index out of range`.
Why: a CloudFormation-deployed SQS-to-Lambda pipeline (including its DLQ/redrive policy) has no mapping, so the deployed stack delivers nothing and the mapping cannot be read, updated or deleted through Lambda.
Fix: the AWS::Lambda::EventSourceMapping provider in the cloudformation crate must create the mapping in the Lambda state and record its UUID as the physical resource id.
Traces: lambda__sfn__sqs.deploy-sfn-lambda-sqs-dlq-redrive-policy-stack.

## Evidence
Cause 1: botocore.errorfactory.InvalidArn: An error occurred (InvalidArn) when calling the DescribeStateMachine operation: Invalid Arn: 'Invalid ARN prefix: sim2real-sfn-cfn-del-d4b839057c0b'; and when calling StartSyncExecution: Invalid Arn: 'Invalid ARN prefix: sim2real-xsvc-stk-2d1176ecd777-Machine6FC8AE80-92C63EE8DF23'. Expected: an ARN of the form arn:aws:states:<region>:<account>:stateMachine:<name>, on which the call succeeds.
Cause 2: "stack with an invalid StartAt ended 'CREATE_COMPLETE', expected 'ROLLBACK_COMPLETE'".
Cause 3: lam.list_event_source_mappings(EventSourceArn=<queue arn>)['EventSourceMappings'][0] -> IndexError: list index out of range; expected exactly the mapping the template declared.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Cause 1: CreateStack with a template containing an AWS::StepFunctions::StateMachine, wait for CREATE_COMPLETE, read the resource's PhysicalResourceId (or the stack Output that Refs it), and call sfn:DescribeStateMachine / StartSyncExecution with it.
3. Cause 2: CreateStack with an AWS::StepFunctions::StateMachine whose DefinitionString has a StartAt naming an undeclared state; poll DescribeStacks for the terminal status.
4. Cause 3: CreateStack with an AWS::SQS::Queue, an AWS::Lambda::Function and an AWS::Lambda::EventSourceMapping binding them; after CREATE_COMPLETE call lambda:ListEventSourceMappings with the queue ARN.
