Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.create-table-keys-only-gsi-with-non-key-attributes-rejected dynamodb.create-table-on-demand-throughput-below-minimum-rejected dynamodb.delete-table-with-active-gsi-reports-deleting dynamodb.gsi-creating-blocks-other-index-but-accepts-self-delete dynamodb.update-table-duplicate-and-bulk-gsi-delete-rejected dynamodb.update-table-gsi-delete-targeting-lsi-name-rejected dynamodb.update-table-rejected-while-a-prior-throughput-change-is-still-applying dynamodb.update-table-sse-nonexistent-cmk-rejected dynamodb.restore-invalid-time dynamodb.stream-arn-tag-list-untag-independent-of-table dynamodb.tag-resource-nonexistent-stream-arn-rejected dynamodb.transact-get-and-write-items-table-arn-naming-another-region-rejected
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.create-table-keys-only-gsi-with-non-key-attributes-rejected|run#7|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: dynamodb.delete-table-with-active-gsi-reports-deleting|run#8|AssertionError|assert "GlobalSecondaryIndexes" not in resp, (
Issue-Key: dynamodb.gsi-creating-blocks-other-index-but-accepts-self-delete|run#14|AssertionError|assert err.get("Code") == "LimitExceededException", (
Issue-Key: dynamodb.update-table-duplicate-and-bulk-gsi-delete-rejected|run#9|AssertionError|assert (code, msg) == expected_dup, (
Issue-Key: dynamodb.update-table-gsi-delete-targeting-lsi-name-rejected|run#7|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}: {message!r}"
Issue-Key: dynamodb.update-table-rejected-while-a-prior-throughput-change-is-still-applying|run#10|AssertionError|assert (code, message) == want, (
Issue-Key: dynamodb.restore-invalid-time|run#18|AssertionError|assert re.fullmatch(
Issue-Key: dynamodb|TagResource|ValidationException
Issue-Key: dynamodb.transact-get-and-write-items-table-arn-naming-another-region-rejected|expect_error#13|AssertionError|assert got_message == message, f"unexpected message {got_message!r}"
Issue-Related: 20261109T172142Z-dynamodb-7-net-new-causes-validationexception-message-text.md
Issue-Related: 20261111T170613Z-dynamodb-non-constraint-validationexception-messages-are-wrapped-in.md
Issue-Commit: cdd06381ed07ce191d4321618e22d6aefb363f32

# dynamodb: control-plane refusals use emulator-authored text and the wrong error class, DeleteTable over-reports, and tagging rejects a valid stream ARN (10 causes)

> _Note: this issue was identified against the code base at commit `cdd06381ed07ce191d4321618e22d6aefb363f32`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateTable

## Description
Ten net-new root causes in the control plane: CreateTable/UpdateTable/DeleteTable validation and state guards, restore validation, resource tagging, and cross-region ARN addressing. Smithy: UpdateTable's modeled client errors are LimitExceededException, ResourceInUseException and ResourceNotFoundException (all 400); TagResource takes a ResourceArn (@length 1..1283) and models ResourceNotFoundException / ResourceInUseException / LimitExceededException. None of the message text is modeled, so the wording — and the choice between those error shapes — is the contract. Variable parts of every message below (table name, index name, attribute names, region, key ARN) come from the request or the emulator's state; the sentences around them are AWS's fixed text.

Cause 1 of 10 — a KEYS_ONLY GSI that also specifies NonKeyAttributes. AWS: 'One or more parameter values were invalid: ProjectionType is KEYS_ONLY, but NonKeyAttributes is specified'. Emulator: 'Invalid Projection for index <index>: NonKeyAttributes can only be specified with a ProjectionType of INCLUDE'. Fix: CreateTable projection validation's message. Traces: create-table-keys-only-gsi-with-non-key-attributes-rejected.

Cause 2 of 10 — OnDemandThroughput below its minimum. AWS: 'One or more parameter values were invalid: Requested MaxReadRequestUnits for OnDemandThroughput for table is outside of valid range'. Emulator: 'One or more parameter values were invalid: MaxReadRequestUnits for OnDemandThroughput must be at least 1 to set a maximum, or -1 to remove it, got 0'. Fix: CreateTable OnDemandThroughput validation's message. Traces: create-table-on-demand-throughput-below-minimum-rejected.

Cause 3 of 10 — DeleteTable's TableDescription carries GlobalSecondaryIndexes. AWS omits the field from the DELETING description; the emulator echoes the full ACTIVE index list. Fix: the DeleteTable response serializer. Traces: delete-table-with-active-gsi-reports-deleting.

Cause 4 of 10 — the refusal class while a GSI is CREATING. AWS: LimitExceededException. Emulator: ResourceInUseException 'Attempt to change a resource which is still in use: Table is being updated: <table>'. Fix: the index-concurrency guard's error class in UpdateTable. Traces: gsi-creating-blocks-other-index-but-accepts-self-delete.

Cause 5 of 10 — two GlobalSecondaryIndexUpdates naming the same index. AWS: 'One or more parameter values were invalid: Only one global secondary index update per index is allowed simultaneously. Index: <index>'. Emulator: 'Invalid GlobalSecondaryIndexUpdates: index <index> is named more than once'. The same emulator-authored style appears for a duplicate index name ('Invalid GlobalSecondaryIndexUpdates: index <name> already exists on table <table>' where AWS serves 'Attempting to create an index which already exists'). Fix: UpdateTable GSI-update validation messages. Traces: update-table-duplicate-and-bulk-gsi-delete-rejected.

Cause 6 of 10 — a GSI Delete that names an LSI. AWS refuses with ValidationException; the emulator answers ResourceNotFoundException 'Requested resource not found: Index <lsi> for table <table>'. Why: the client sees 'not found' for an index that exists. Fix: UpdateTable's index-kind resolution. Traces: update-table-gsi-delete-targeting-lsi-name-rejected.

Cause 7 of 10 — a second UpdateTable while a throughput change is applying. AWS: ResourceInUseException 'Attempt to change a resource which is still in use: Table IOPS are currently being updated. Table: <table>'. Emulator: '... Table is being updated: <table>'. Fix: the throughput-update state guard's message. Traces: update-table-rejected-while-a-prior-throughput-change-is-still-applying.

Cause 8 of 10 — the wrapped KMS NotFoundException for a nonexistent CMK is re-rendered. AWS: "KMS validation error: com.amazonaws.services.kms.model.NotFoundException: Key '<arn>' does not exist (Service: AWSKMS; Status Code: 400; Error Code: NotFoundException; Request ID: <id>; Proxy: null)" — service name AWSKMS, semicolon separators and an Error Code field. Emulator: "(Service: Kms, Status Code: 400, Request ID: <id>; Proxy: null)" — wrong service name, comma separators, no Error Code. Fix: the KMS-error wrapper in UpdateTable's SSE path. Traces: update-table-sse-nonexistent-cmk-rejected.

Cause 9 of 10 — restore with an out-of-window RestoreDateTime does not produce AWS's refusal for that case. Fix: RestoreTableToPointInTime time-window validation. Traces: restore-invalid-time.

Cause 10 of 10 — TagResource/ListTagsOfResource/UntagResource reject a table's stream ARN. AWS accepts a stream ARN wherever a ResourceArn is taken, and tags on a stream ARN are independent of the table's; a stream ARN for a nonexistent stream is refused as not found, not as malformed. Emulator: ValidationException 'Invalid TableArn: Invalid ResourceArn provided as input arn:aws:dynamodb:<region>:<acct>:table/<table>/stream/<label>'. The same ARN-parsing defect shows up for TransactGetItems/TransactWriteItems, where a table ARN naming another region is refused with the emulator-authored "1 validation error detected: Invalid AWS region in '<arn>'" instead of AWS's text. Fix: the shared ARN parser/validator used by the tagging and transaction handlers. Traces: stream-arn-tag-list-untag-independent-of-table, tag-resource-nonexistent-stream-arn-rejected, transact-get-and-write-items-table-arn-naming-another-region-rejected.

## Evidence
Expected vs actual, verbatim from the replay:
- create-table-keys-only-gsi-with-non-key-attributes-rejected: expected 'One or more parameter values were invalid: ProjectionType is KEYS_ONLY, but NonKeyAttributes is specified', got 'Invalid Projection for index gsi1: NonKeyAttributes can only be specified with a ProjectionType of INCLUDE'.
- create-table-on-demand-throughput-below-minimum-rejected: expected 'One or more parameter values were invalid: Requested MaxReadRequestUnits for OnDemandThroughput for table is outside of valid range', got 'One or more parameter values were invalid: MaxReadRequestUnits for OnDemandThroughput must be at least 1 to set a maximum, or -1 to remove it, got 0'.
- delete-table-with-active-gsi-reports-deleting: 'DeleteTable response carried GlobalSecondaryIndexes [{...IndexStatus ACTIVE...}], expected the field to be omitted'.
- gsi-creating-blocks-other-index-but-accepts-self-delete: 'expected LimitExceededException, got 'ResourceInUseException' ('Attempt to change a resource which is still in use: Table is being updated: <table>')'.
- update-table-duplicate-and-bulk-gsi-delete-rejected: got ('ValidationException', 'Invalid GlobalSecondaryIndexUpdates: index idx-a is named more than once'), expected ('ValidationException', 'One or more parameter values were invalid: Only one global secondary index update per index is allowed simultaneously. Index: idx-a').
- update-table-gsi-delete-targeting-lsi-name-rejected: 'expected ValidationException, got ResourceNotFoundException: 'Requested resource not found: Index lsi-main for table <table>''.
- update-table-rejected-while-a-prior-throughput-change-is-still-applying: got 'Attempt to change a resource which is still in use: Table is being updated: <table>', expected 'Attempt to change a resource which is still in use: Table IOPS are currently being updated. Table: <table>'.
- update-table-sse-nonexistent-cmk-rejected: got "(Service: Kms, Status Code: 400, Request ID: <id>; Proxy: null)", expected "(Service: AWSKMS; Status Code: 400; Error Code: NotFoundException; Request ID: <id>; Proxy: null)".
- restore-invalid-time: 'An invalid restore time was specified. RestoreDateTime must be between EarliestRestorableDateTime and LatestRestorableDateTime'.
- stream-arn-tag-list-untag-independent-of-table and tag-resource-nonexistent-stream-arn-rejected: ClientError (ValidationException) on TagResource 'Invalid TableArn: Invalid ResourceArn provided as input arn:aws:dynamodb:us-west-2:000000000000:table/<table>/stream/<label>'.
- transact-get-and-write-items-table-arn-naming-another-region-rejected: unexpected message "1 validation error detected: Invalid AWS region in 'arn:aws:dynamodb:us-east-1:000000000000:table/<table>'".
(Index names idx-a/lsi-main/gsi1, table names, regions, key ARNs, stream labels and request ids above are this run's values.)

## Steps to Reproduce
1. CreateTable with a KEYS_ONLY GSI that also sets NonKeyAttributes; CreateTable with OnDemandThroughput MaxReadRequestUnits=0 — read both messages.
2. CreateTable with an ACTIVE GSI, then DeleteTable — check TableDescription omits GlobalSecondaryIndexes.
3. While a GSI is CREATING, issue another GSI update — expect LimitExceededException.
4. UpdateTable with two Delete entries for the same index name, and with a Create for an index name that already exists — read both messages.
5. UpdateTable with a GlobalSecondaryIndexUpdates Delete naming an LSI — expect ValidationException.
6. UpdateTable provisioned throughput, then immediately UpdateTable again while UPDATING — read the ResourceInUseException message.
7. UpdateTable SSESpecification with a nonexistent KMS key ARN — compare the wrapped KMS error's separators, service name and Error Code field.
8. RestoreTableToPointInTime with a RestoreDateTime outside the restorable window.
9. TagResource / ListTagsOfResource / UntagResource with the table's LatestStreamArn, and with a stream ARN for a stream that does not exist.
10. TransactGetItems and TransactWriteItems with a table ARN naming another region.
