Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.get-resource-policy-malformed-arn-rejected
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.get-resource-policy-malformed-arn-rejected|run#36|AssertionError|assert bogus_message == expected_bogus, (
Issue-Commit: a983232b5da502bccde79d757bd972103fe44705

# dynamodb GetResourcePolicy: a foreign-service ARN is refused with an emulator-authored "Invalid TableArn: Invalid ResourceArn provided as input ..." message instead of AWS's "One or more parameter values were invalid: Invalid resource Arn: ..."

> _Note: this issue was identified against the code base at commit `a983232b5da502bccde79d757bd972103fe44705`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourcePolicy

## Description
Cause 1 of 1: GetResourcePolicy's ValidationException message for a well-formed ARN that does not address a DynamoDB resource does not use AWS's wording.

AWS: Smithy (com-amazonaws-dynamodb-dynamodb-20120810): GetResourcePolicy input member ResourceArn targets ResourceArnString, required, @length min 1 / max 1283; the operation's modeled errors are InternalServerError, InvalidEndpointException, PolicyNotFoundException and ResourceNotFoundException — ValidationException is DynamoDB's service-common refusal and is not listed on the operation, so its message text is recorded behavior only. For a ResourceArn that parses as an ARN but names another service's resource (e.g. an IAM role ARN), DynamoDB answers HTTP 400, error code ValidationException, with Message "One or more parameter values were invalid: Invalid resource Arn: <the ResourceArn exactly as supplied>". Fixed text: "One or more parameter values were invalid: Invalid resource Arn: ". Variable part: the ResourceArn value taken from the request. The adjacent case — a value that is not an ARN at all — is "One or more parameter values were invalid: ARNs must start with 'arn:': <value>", and the emulator already matches that one, which isolates the divergence to the foreign-ARN branch.

Emulator: the status and error code are right (400 ValidationException) but the Message is "Invalid TableArn: Invalid ResourceArn provided as input <arn>" — entirely different fixed text, with a duplicated/doubled prefix and a member name ("TableArn") that is not the member the request carries ("ResourceArn").

Why: a client relying on AWS's behavior cannot classify the failure. DynamoDB's parameter refusals are uniformly prefixed "One or more parameter values were invalid: " and SDK users, retry wrappers and operator tooling branch on that prefix and on the "Invalid resource Arn" phrase; neither appears. The message also misnames the offending request member, so a caller fixing its input is pointed at a parameter it never sent.

Fix: GetResourcePolicy's input validation / ARN parsing in the dynamodb handler — specifically the ValidationException message composition on the branch where the ARN is well-formed but is not a DynamoDB table (or table-stream) ARN.

Traces: dynamodb.get-resource-policy-malformed-arn-rejected.

## Evidence
Check dynamodb.get-resource-policy-malformed-arn-rejected.

Expected (AWS): code ValidationException, Message "One or more parameter values were invalid: Invalid resource Arn: <ResourceArn from the request>".

Actual (emulator): code ValidationException (correct), Message "Invalid TableArn: Invalid ResourceArn provided as input <ResourceArn from the request>".

AssertionError from the replay: expected message 'One or more parameter values were invalid: Invalid resource Arn: arn:aws:iam::<account>:role/not-a-dynamodb-resource', got 'Invalid TableArn: Invalid ResourceArn provided as input arn:aws:iam::<account>:role/not-a-dynamodb-resource'.

Message anatomy: "One or more parameter values were invalid: Invalid resource Arn: " is the fixed text; the ARN that follows is the ResourceArn value echoed from the request (the account id in the recorded run is a per-run value and is not part of the requirement). The preceding assertions in the same check pass, so the sibling branches (PolicyNotFoundException for a policy-less table, ResourceNotFoundException for a missing table, and the "ARNs must start with 'arn:'" non-ARN message) are already correct.

## Steps to Reproduce
1. CreateTable with a simple HASH key, wait for ACTIVE.
2. Call GetResourcePolicy with ResourceArn set to an ARN of another service, e.g. arn:aws:iam::<account>:role/not-a-dynamodb-resource.
3. Observe 400 ValidationException with Message "Invalid TableArn: Invalid ResourceArn provided as input <arn>". AWS returns Message "One or more parameter values were invalid: Invalid resource Arn: <arn>".
4. For contrast, call GetResourcePolicy with ResourceArn="not-a-valid-arn": that branch already answers AWS's "One or more parameter values were invalid: ARNs must start with 'arn:': not-a-valid-arn".
