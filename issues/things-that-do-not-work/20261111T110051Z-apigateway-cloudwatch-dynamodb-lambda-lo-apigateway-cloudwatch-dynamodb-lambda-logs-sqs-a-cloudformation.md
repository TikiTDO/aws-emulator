Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-cfn
Issue-Key: apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-cfn|_wait_stack#4|AssertionError|raise AssertionError(
Issue-Commit: 56af4a38801370870cefc18c3ef22e7084182017

# apigateway__cloudwatch__dynamodb__lambda__logs__sqs: a CloudFormation-deployed AWS::Lambda::Function whose MemorySize comes from a template parameter is refused "expected number, got string", rolling the whole application stack back

> _Note: this issue was identified against the code base at commit `56af4a38801370870cefc18c3ef22e7084182017`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFunction

## Description
One net-new root cause, grounded in a failing behavioral check the replay report left unmarked (no ALREADY-FILED marker, no OWNED marker).

Cause 1 of 1 — Lambda's CreateFunction request deserializer refuses a JSON string for an integer-typed member, so a plain-CloudFormation stack that feeds a template parameter into AWS::Lambda::Function.MemorySize can never be created.

AWS: Smithy (services/service-com-amazonaws-lambda-awsgirapiservice/smithy-model/com-amazonaws-lambda-awsgirapiservice.json) models CreateFunctionRequest.MemorySize -> com.amazonaws.lambda#MemorySize, type integer, @range min 128 max 32768. CloudFormation template parameter values are carried as strings on the wire, so `{"Ref": "<a numeric parameter>"}` resolves to the character sequence "128"; AWS's AWS::Lambda::Function resource handler coerces that to the integer 128 and CreateFunction succeeds, which is why the documented pattern of parameterising MemorySize works. AWS never answers a raw deserializer sentence here: when a value really is unusable it answers a modeled client error (400) whose message names the member and the constraint, in the shape "1 validation error detected: Value '<value>' at '<memberName>' failed to satisfy constraint: <constraint sentence>" — fixed text plus the request's own value, member name and the member's modeled constraint.

Emulator: the stack reaches ROLLBACK_COMPLETE. The logical resource IntakeFunction fails with `Resource handler returned message: "expected number, got string (Service: Lambda, Status Code: 400, Request ID: <id>) (SDK Attempt Count: 1)" (HandlerErrorCode: InvalidRequest)`. "expected number, got string" is a bare JSON-deserializer sentence: it carries no AWS error code, does not name the member it rejected, does not name the value, and is not any wording AWS uses. Two facets of the same cause: the string-encoded integer is rejected at all, and the rejection is reported in deserializer prose instead of a modeled 400 with AWS's validation wording.

Why: every IaC path that parameterises a Lambda numeric property — MemorySize, Timeout, EphemeralStorage.Size, ReservedConcurrentExecutions — is unusable, because CloudFormation always hands those properties over as strings. A client gets ROLLBACK_COMPLETE on a template real AWS deploys, and the reason it is handed back names no property, so the failure is undiagnosable from the stack events. The same deserializer path also makes any direct SDK/CLI caller that sends `"MemorySize": "128"` fail with an uncatchable, uncoded message.

Fix: the CreateFunction (and UpdateFunctionConfiguration) handler's input deserialization/validation in the Lambda service crate — accept a JSON string that parses as an integer for integer-typed members as the AWS resource handlers do, and when a value is genuinely out of range or unparsable raise the modeled client error with AWS's "1 validation error detected: Value '<value>' at '<member>' failed to satisfy constraint: ..." wording instead of returning the deserializer's own sentence. The CloudFormation AWS::Lambda::Function resource handler's property coercion is the alternative seam; either way the string-encoded number must stop being a hard refusal.

Traces: apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-cfn. Related but NOT covering it: 20261109T143515Z-apigateway-cloudwatch-dynamodb-lambda-lo-cloudformation-deployed-cdk-sam-app-rolls-back-generated.md and 20261110T214424Z-apigateway-cloudwatch-dynamodb-lambda-lo-apigateway-cloudwatch-dynamodb-lambda-logs-sqs-two-more.md own the CDK and SAM flavours of this scenario (a statementId length refusal and an alarm-config divergence); neither names this deserializer refusal and the report marked this check unowned.

## Evidence
apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-cfn — verdict mismatch.

The check deploys the order-intake application through plain CloudFormation with a parameter set that includes IntakeMemorySize, whose value is the string "128" (the drift leg later raises it to "256"). The template feeds that parameter into the intake function's MemorySize.

Expected: CreateStack settles CREATE_COMPLETE, GetFunctionConfiguration reports MemorySize 128, and the second apply with "256" modifies the function in place.

Actual (AssertionError from the check's _wait_stack):
  stack <stack-name> ended in ROLLBACK_COMPLETE: IntakeFunction: Resource handler returned message: "expected number, got string (Service: Lambda, Status Code: 400, Request ID: <id>) (SDK Attempt Count: 1)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)

The fixed text of the emulator's message is "expected number, got string"; the stack name, request id and request token are per-run values and are not part of the requirement. Nothing after the deploy leg ran, so the wiring/worker/logs/metrics/alarm/denial/drift/teardown legs of this check are not claimed here — they should be re-asserted once the deploy succeeds.

Smithy citation: CreateFunction input member MemorySize targets com.amazonaws.lambda#MemorySize, an integer with @range {min: 128, max: 32768}; there is no modeled error whose message is a deserializer sentence.

## Steps to Reproduce
1. Author a CloudFormation template with a parameter (e.g. `IntakeMemorySize`) and a resource `AWS::Lambda::Function` whose `Properties.MemorySize` is `{"Ref": "IntakeMemorySize"}`, plus a role it can assume.
2. cloudformation:CreateStack with `Parameters=[{ParameterKey: IntakeMemorySize, ParameterValue: "128"}]` and Capabilities CAPABILITY_IAM/CAPABILITY_NAMED_IAM.
3. Poll DescribeStacks to a terminal status. Expected CREATE_COMPLETE; observed ROLLBACK_COMPLETE, and DescribeStackEvents shows the function's CREATE_FAILED reason `Resource handler returned message: "expected number, got string ..."`.
4. Equivalently, without CloudFormation: call lambda:CreateFunction with the JSON body member `"MemorySize": "128"` (a string). Expected: the function is created with MemorySize 128. Observed: HTTP 400 with the message "expected number, got string" and no AWS error code.
5. Then confirm `"MemorySize": 64` (a genuinely invalid integer) is refused with AWS's wording `1 validation error detected: Value '64' at 'memorySize' failed to satisfy constraint: Member must have value greater than or equal to 128`, not with a deserializer sentence.
