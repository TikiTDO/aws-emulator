Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda.in-handler-transact-write-cancellation-reasons-carry-per-item-codes
Issue-Key: dynamodb__lambda.in-handler-transact-write-cancellation-reasons-carry-per-item-codes|run#15|AssertionError|assert got["reasons"] == _REASONS, (
Issue-Commit: 53bfbc08a5ec2983a62510ba6bf3546a998019d9

# dynamodb__lambda: a cancelled TransactWriteItems answers TransactionCanceledException with no CancellationReasons list, so per-item codes and the ALL_OLD item are lost

> _Note: this issue was identified against the code base at commit `53bfbc08a5ec2983a62510ba6bf3546a998019d9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TransactWriteItems

## Description
One net-new root cause.

Cause 1 of 1: TransactWriteItems serializes TransactionCanceledException without its CancellationReasons member.

AWS: Smithy: TransactWriteItems error TransactionCanceledException (client error, HTTP 400) has members Message and CancellationReasons -> CancellationReasonList (@length min 1, max 100) of CancellationReason{Code, Message, Item}. The model documents CancellationReasonList as "An ordered list of errors for each item in the request ... ordered according to the ordering of the TransactWriteItems request parameter. If no error occurred for the associated item an error with a Null code and Null message will be present." So for a two-item transaction whose first action succeeds and whose second Update fails its ConditionExpression with ReturnValuesOnConditionCheckFailure=ALL_OLD, AWS answers HTTP 400, Error.Code 'TransactionCanceledException', Error.Message 'Transaction cancelled, please refer cancellation reasons for specific reasons [None, ConditionalCheckFailed]', and CancellationReasons [{Code: 'None'}, {Code: 'ConditionalCheckFailed', Message: 'The conditional request failed', Item: <the ALL_OLD image of the item>}]. The fixed text is 'Transaction cancelled, please refer cancellation reasons for specific reasons [...]' and 'The conditional request failed'; the bracketed code list, and the Item map, are values derived from the request and the table's current state.

Emulator: the refusal itself is produced (the handler's boto3 reports the exception and Error.Code/Message), but exc.response['CancellationReasons'] is absent — the handler reads None where the ordered two-element list is required.

Why: a client relying on AWS's behavior cannot tell WHICH action of the transaction was cancelled or why. Every retry/compensation path keyed on reasons[i]['Code'] (the documented way to distinguish ConditionalCheckFailed from TransactionConflict, ProvisionedThroughputExceeded or ValidationError) sees nothing, and ReturnValuesOnConditionCheckFailure=ALL_OLD becomes useless because the conflicting item image is never returned.

Fix: the TransactWriteItems handler's error path and its serializer — build a CancellationReason per request action in request order (Code 'None' for actions that did not fail, the failing action's code/message, and the ALL_OLD Item when ReturnValuesOnConditionCheckFailure asked for it) and emit it as the CancellationReasons member of the TransactionCanceledException body.

## Evidence
dynamodb__lambda.in-handler-transact-write-cancellation-reasons-carry-per-item-codes [mismatch]:
AssertionError: CancellationReasons in the handler were None, expected [{'Code': 'None'}, {'Code': 'ConditionalCheckFailed', 'Message': 'The conditional request failed', 'Item': {'pk': {'S': 'acct'}, 'v': {'N': '1'}}}]

The transaction was [Put {pk: 'fresh'}] + [Update key {pk: 'acct'} with ConditionExpression 'v = :expected' (:expected = 2) against a stored v = 1, ReturnValuesOnConditionCheckFailure = ALL_OLD]. The expected list's shape and codes are the requirement; the pk value 'acct' and v=1 are this request's/this table's values filling the Item template.

## Steps to Reproduce
1. CreateTable with key pk (S) and put an item {pk: 'acct', v: 1}.
2. Call TransactWriteItems with two actions: (a) Put {pk: 'fresh'}, (b) Update key {pk: 'acct'} with UpdateExpression 'SET v = :next', ConditionExpression 'v = :expected', :expected = 2, :next = 3, ReturnValuesOnConditionCheckFailure = 'ALL_OLD'.
3. The call is refused HTTP 400 TransactionCanceledException as expected, but the response body carries no CancellationReasons member, so botocore's exc.response.get('CancellationReasons') is None instead of the two-element ordered list [{'Code': 'None'}, {'Code': 'ConditionalCheckFailed', 'Message': 'The conditional request failed', 'Item': {...}}].
