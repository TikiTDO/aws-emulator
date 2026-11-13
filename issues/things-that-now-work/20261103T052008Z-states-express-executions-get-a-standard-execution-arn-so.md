Issue-Source: trace-sweep-divergence
Issue-Trace: sfn-express-failed-execution-lookups
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sfn-express-failed-execution-lookups
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: states|StartExecution|2xx->2xx|$.executionArn
Issue-Key: states|DescribeExecution|4xx->4xx|$.__type
Issue-Key: states|DescribeExecution|4xx->4xx|$.message
Issue-Key: states|GetExecutionHistory|4xx->4xx|$.message
Issue-Key: states|RedriveExecution|4xx->4xx|$.__type
Issue-Key: states|RedriveExecution|4xx->4xx|$.message
Issue-Commit: e331167544f8c61f25de403c8943054d68a37733

# EXPRESS executions get a standard execution ARN, so express-ARN refusals never fire

> _Note: this issue was identified against the code base at commit `e331167544f8c61f25de403c8943054d68a37733`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
An execution started asynchronously on an EXPRESS state machine is given a standard `execution` ARN with no execution id, instead of the `express` ARN AWS mints. Everything that keys off that resource type then diverges: the three read/redrive operations that AWS refuses with InvalidArn because the ARN's resource type is `express` instead answer with a different error class and a different message. 4 causes.

Cause 1 of 4 — StartExecution on an EXPRESS state machine returns an `execution` ARN without an execution UUID.
AWS: Smithy: StartExecution.output.executionArn -> Arn (required). For an EXPRESS state machine AWS mints arn:aws:states:<region>:<account>:express:<stateMachineName>:<executionName>:<executionUuid> — resource type `express`, and a trailing UUID segment that distinguishes the invocation from the execution name.
Emulator: returns arn:aws:states:<region>:<account>:execution:<stateMachineName>:<executionName> — resource type `execution`, no UUID segment.
Why: a client that parses the ARN to learn the state machine type, or that stores the ARN and later derives the invocation id from it, reads the wrong resource type and finds no invocation id; and because the ARN is well-formed as a standard execution ARN, every later call on it takes the standard path (causes 2-4) instead of being refused.
Fix: the StartExecution handler's ARN construction, keyed on the state machine's type.
Traces: sfn-express-failed-execution-lookups#task#9

Cause 2 of 4 — DescribeExecution on an express execution ARN answers ExecutionDoesNotExist instead of InvalidArn.
AWS: Smithy: DescribeExecution models both InvalidArn and ExecutionDoesNotExist (each HTTP 400). AWS rejects an ARN whose resource type is `express` with InvalidArn (wire __type com.amazonaws.swf.service.v2.model#InvalidArn) and the message "Invalid Arn: 'Resource type not valid in this context: express'" — fixed text "Invalid Arn: 'Resource type not valid in this context: " + the offending resource type from the ARN + "'".
Emulator: HTTP 400 with __type 'ExecutionDoesNotExist' and message "Execution Does Not Exist: '<the arn>'".
Why: a client distinguishing "this API does not accept express ARNs" (a permanent client error, re-route to the sync API) from "the execution has aged out" (retry or give up) takes the wrong branch; the error code it matches on is absent.
Fix: DescribeExecution's input validation — classify the ARN's resource type before the execution lookup.
Traces: sfn-express-failed-execution-lookups#task#10

Cause 3 of 4 — GetExecutionHistory answers InvalidArn but with its own message text.
AWS: message "Invalid Arn: 'Resource type not valid in this context: express'", as in cause 2 — the message names the rejected resource type and nothing else.
Emulator: HTTP 400, correct error class, but message "Invalid Arn: 'GetExecutionHistory is not supported by EXPRESS state machines: <the arn>'".
Why: the message is emulator-authored and embeds the whole ARN and the operation name, so a client matching AWS's wording (or redacting on it) does not match.
Fix: GetExecutionHistory's ARN validation must raise the shared InvalidArn resource-type refusal rather than an operation-specific sentence.
Traces: sfn-express-failed-execution-lookups#task#11

Cause 4 of 4 — RedriveExecution answers ValidationException instead of InvalidArn.
AWS: Smithy: RedriveExecution models InvalidArn (400) alongside ValidationException (400); an express-resource-type ARN is an InvalidArn with __type com.amazonaws.swf.service.v2.model#InvalidArn and the message "Invalid Arn: 'Resource type not valid in this context: express'".
Emulator: HTTP 400 with __type 'ValidationException' and message 'RedriveExecution does not support execution <the arn>, which belongs to an EXPRESS state machine'.
Why: the error code and message both differ, so a client's error-code dispatch on InvalidArn falls through to its generic ValidationException handling.
Fix: RedriveExecution's input validation — same shared ARN resource-type check as causes 2 and 3.
Traces: sfn-express-failed-execution-lookups#task#12

## Evidence
All four causes are mismatch clusters from the replay of simparity-wire-aws-v1 against the emulator built from this checkout.

Cause 1 (sfn-express-failed-execution-lookups#task#9, HTTP 200 expected and returned): $.executionArn: expected 'arn:aws:states:us-west-2:<account>:express:<sm-name>:<exec-name>:<uuid>', got 'arn:aws:states:us-west-2:<account>:execution:<sm-name>:<exec-name>' — resource type `execution` instead of `express`, and the trailing UUID segment dropped.

Cause 2 (#task#10, HTTP 400 expected and returned): $.__type: expected 'com.amazonaws.swf.service.v2.model#InvalidArn', got 'ExecutionDoesNotExist'; $.message: expected "Invalid Arn: 'Resource type not valid in this context: express'", got "Execution Does Not Exist: '<the execution arn from cause 1>'".

Cause 3 (#task#11, HTTP 400 expected and returned): $.message: expected "Invalid Arn: 'Resource type not valid in this context: express'", got "Invalid Arn: 'GetExecutionHistory is not supported by EXPRESS state machines: <the execution arn from cause 1>'".

Cause 4 (#task#12, HTTP 400 expected and returned): $.__type: expected 'com.amazonaws.swf.service.v2.model#InvalidArn', got 'ValidationException'; $.message: expected "Invalid Arn: 'Resource type not valid in this context: express'", got 'RedriveExecution does not support execution <the execution arn from cause 1>, which belongs to an EXPRESS state machine'.

## Steps to Reproduce
1. Replay fixture sfn-express-failed-execution-lookups against the emulator.
2. Step task#9 (the representative) calls StartExecution on the fixture's EXPRESS state machine; compare the returned $.executionArn resource-type segment against the recording's `express` segment and note the missing UUID segment.
3. Step task#10 calls DescribeExecution with an express execution ARN and expects HTTP 400 InvalidArn; the emulator answers ExecutionDoesNotExist.
4. Step task#11 calls GetExecutionHistory with the same ARN; compare $.message.
5. Step task#12 calls RedriveExecution with the same ARN; compare $.__type and $.message.
