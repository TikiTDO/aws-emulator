Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__s3.lambda-without-dynamodb-permission-denied
Issue-Key: dynamodb__lambda__s3.lambda-without-dynamodb-permission-denied|run#6|AssertionError|assert denied_result["payload"]["ok"] is False, (
Issue-Commit: 5678b2d659e23eeff4b2f39316abc675f4291395

# dynamodb__lambda__s3: an in-function DynamoDB PutItem the execution role does not allow succeeds

> _Note: this issue was identified against the code base at commit `5678b2d659e23eeff4b2f39316abc675f4291395`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutItem

## Description
One root cause behind 1 failing check.

Cause 1 of 1: IAM is not enforced for an in-function DynamoDB write.
AWS: a handler calling PutItem on a table whose `dynamodb:PutItem` action its execution role does not grant is refused with HTTP 400 and error code `AccessDeniedException`, message `User: arn:aws:sts::<account>:assumed-role/<role-name>/<function-name> is not authorized to perform: dynamodb:PutItem on resource: <table-arn> because no identity-based policy allows the dynamodb:PutItem action` — fixed text with the account, role name, function name and table ARN taken from the request and the execution role. No item is written. Smithy: PutItem (services/service-com-amazonaws-dynamodb-dynamodb-20120810) models ConditionalCheckFailedException, ResourceNotFoundException, ProvisionedThroughputExceededException and others; the authorization refusal is the service-common AccessDeniedException.
Emulator: the call succeeds — `expected denial without dynamodb:PutItem, got {'ok': True}` — and the item is written.
Why: a least-privilege role has no effect, so an application that passes here is refused on AWS; and the emulator's table holds an item AWS would never have accepted, so subsequent reads diverge too.
Fix: the IAM authorization check on in-function AWS SDK calls, in the DynamoDB PutItem handler. The open issues 20261105T111951Z-dynamodb-lambda-s3-the-lambda-python-runtime-has.md and 20261105T112044Z-dynamodb-lambda-s3-the-lambda-python-runtime-provides.md cover the absent in-runtime SDK for other checks of this service; they do not carry this replay key, and this call does reach DynamoDB and is wrongly allowed.
Traces: dynamodb__lambda__s3.lambda-without-dynamodb-permission-denied.

## Evidence
Expected (AWS): HTTP 400, Error.Code `AccessDeniedException`, Error.Message `User: arn:aws:sts::<account>:assumed-role/<role>/<function> is not authorized to perform: dynamodb:PutItem on resource: <table-arn> because no identity-based policy allows the dynamodb:PutItem action`; the item is not written.

Actual (emulator): AssertionError: expected denial without dynamodb:PutItem, got {'ok': True}.

## Steps to Reproduce
1. Create a DynamoDB table and a Lambda execution role granting only logs actions.
2. Create a function whose handler calls boto3.client('dynamodb').put_item on that table and invoke it.
3. Expected: the handler sees AccessDeniedException and no item exists. Observed: ok=True and the item is present.
