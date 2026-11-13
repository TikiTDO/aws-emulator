Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot__lambda.cfn-deploy-iot-thing-custom-resource-lambda-positive
Issue-Key: iot__lambda.cfn-deploy-iot-thing-custom-resource-lambda-positive|_poll_until#3|Timeout|last = probe()
Issue-Commit: 7eb5bf3e83f51302e9d7bc7ffd19e2ebc91fbccc

# iot__lambda: a stack whose Lambda-backed custom resource creates an IoT thing never reaches CREATE_COMPLETE, so the deploy hangs

> _Note: this issue was identified against the code base at commit `7eb5bf3e83f51302e9d7bc7ffd19e2ebc91fbccc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: a CreateStack whose only work is done by a Lambda-backed Custom:: resource (CreateThing on Create, DeleteThing on Delete) never settles, so neither CREATE_COMPLETE with the resource's Data outputs nor the Delete branch is ever reached.

AWS: CloudFormation invokes the custom resource's ServiceToken Lambda with RequestType=Create and a presigned ResponseURL; when the provider PUTs {"Status":"SUCCESS","PhysicalResourceId":...,"Data":{...}}, the resource becomes ResourceStatus=CREATE_COMPLETE, GetAtt on the Data keys resolves (here stack Output ThingName), and the stack reports StackStatus=CREATE_COMPLETE; iot:DescribeThing then returns the created thing. On DeleteStack the same provider is invoked with RequestType=Delete, the thing is removed, and iot:DescribeThing is refused with error code ResourceNotFoundException and message `Thing <accountId>:<thingName> not found.` — fixed text with the account id and thing name taken from the request and state. Smithy (cloudformation): StackStatus/ResourceStatus enums include CREATE_IN_PROGRESS and CREATE_COMPLETE; (iot) DescribeThing declares ResourceNotFoundException.

Emulator: CreateStack is accepted but the stack never reaches a terminal status — the deploy helper's wait for CREATE_COMPLETE never returns and the run is killed with 'Check timed out after 60.0s'. Nothing downstream is observable: no stack Output ThingName, no created thing, and no Delete-branch removal.

Why: every template that uses a Lambda-backed custom resource to provision something the emulator's resource types do not cover (the standard escape hatch, and what CDK's custom-resource providers emit) hangs instead of deploying; clients that poll for CREATE_COMPLETE or read a custom resource's GetAtt data never proceed, and teardown never runs the provider's Delete branch so the external objects it created are leaked.

Fix: the CloudFormation stack provisioner's custom-resource handling — invoke the ServiceToken provider, accept/consume its presigned ResponseURL response, record PhysicalResourceId and Data for GetAtt, and advance the resource and stack to CREATE_COMPLETE (and to the Delete branch on stack delete).

Traces: behavioral check iot__lambda.cfn-deploy-iot-thing-custom-resource-lambda-positive (verdict mismatch; 'Check timed out after 60.0s'). Neighbouring open issues that do not carry this check's replay key: 20261029T211043Z-iot-lambda-iot-a-cloudformation-custom-resource-that-creates-a.md, 20261105T223150Z-iot-lambda-2-net-new-causes-the-managed.md.

## Evidence
Expected: the stack reaches StackStatus=CREATE_COMPLETE; its Outputs carry ThingName=<thingName>; iot:DescribeThing(thingName) returns thingName=<thingName>; after DeleteStack, iot:DescribeThing(thingName) is refused ResourceNotFoundException with message `Thing <accountId>:<thingName> not found.`
Actual: the stack never reaches any terminal status — the deploy wait aborts with 'Check timed out after 60.0s'; no Outputs, no thing, and no Delete-branch removal are observed.

## Steps to Reproduce
1. CreateStack with a template containing a Lambda function (inline code using urllib to answer the ResponseURL and boto3 to call iot:CreateThing/DeleteThing), an execution role, and a Custom:: resource whose ServiceToken is that function, plus an Output reading the custom resource's ThingName Data key.
2. Poll DescribeStacks for a terminal StackStatus.
3. Observe the stack never reaches CREATE_COMPLETE and iot:DescribeThing never finds the thing.
4. Replay behavioral check iot__lambda.cfn-deploy-iot-thing-custom-resource-lambda-positive.
