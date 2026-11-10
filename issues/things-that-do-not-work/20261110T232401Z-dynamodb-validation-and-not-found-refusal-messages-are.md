Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-attribute-set-null-member-rejected
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-attribute-set-null-member-rejected dynamodb-batch-write-item-request-key-defects dynamodb-expression-attribute-value-empty-rejected ddbval-batch-write-item-oversized-nested-put-rejects-whole-call ddbval-nesting-depth-over-32-levels-rejected dynamodb-batch-caps-and-unprocessed dynamodb-batch-request-items-key-not-a-table-name-rejected ddbval-transact-write-items-nesting-is-top-level-validation dynamodb-malformed-n-in-key-top-level-not-cancellation transact-write-items-update-grows-item-past-size-limit ddbval-item-over-400kb-rejected-not-stored ddbval-update-item-growth-over-400kb-rejected dynamodb-ddbstrip-traj-update-expression-grammar-mistakes dynamodb-multiple-simultaneous-constraint-violations-rejected dynamodb-transaction-failure-class-discrimination dynamodb-rbp-traj-malformed-resource-arn-classes dynamodb-import-table-pipeline dynamodb-point-in-time-export-pipeline dynamodb-csvtraj-gsi-added-after-create-via-update-table
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|PutItem|4xx->4xx|$.message
Issue-Key: dynamodb|BatchWriteItem|4xx->4xx|$.message
Issue-Key: dynamodb|TransactWriteItems|4xx->4xx|$.message
Issue-Key: dynamodb|TransactGetItems|4xx->4xx|$.message
Issue-Key: dynamodb|TransactWriteItems|4xx->4xx|$.CancellationReasons[*].Message
Issue-Key: dynamodb|GetResourcePolicy|4xx->4xx|$.message
Issue-Key: dynamodb|DescribeImport|4xx->4xx|$.message
Issue-Key: dynamodb|ExportTableToPointInTime|4xx->4xx|$.message
Issue-Key: dynamodb|UpdateTable|4xx->4xx|$.message
Issue-Related: 20261110T232401Z-dynamodb-expression-diagnostics-4-net-new-causes-operand.md
Issue-Commit: b1b0bc7f12f2b32e092da76cddcf605dbbe6d9ca

# dynamodb validation and not-found refusal messages are emulator-authored across ten operations

> _Note: this issue was identified against the code base at commit `b1b0bc7f12f2b32e092da76cddcf605dbbe6d9ca`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutItem

## Description
This bundle carries 10 net-new causes. In every one the emulator picks the right HTTP status and the right error class but writes its own sentence, or echoes the wrong value into AWS's sentence template. The shared fault is that these refusal messages are authored at the refusal site rather than rendered from AWS's message template for the error class. Each cause must be closed individually.

Cause 1 of 10 — PutItem: a null member inside a string set (PutItem; fixture dynamodb-attribute-set-null-member-rejected step task#3).
AWS: Smithy: PutItem.Item targets PutItemInputAttributeMap of AttributeValue; SS is a list of String. The refusal is ValidationException, HTTP 400, message 'One or more parameter values were invalid: An string set may not have a null string as a member'. The whole sentence is fixed text for this class (AWS's own wording, 'An string set', is part of it); no variable parts.
Emulator: HTTP 400 with 'expected string, got null' — a deserializer-level message, not a service refusal.
Why: the caller is given no indication which parameter or which set member was wrong, and nothing identifies this as the parameter-value class AWS reports ('One or more parameter values were invalid: ...'), which clients substring-match to distinguish request-shape defects from schema defects.
Fix: AttributeValue validation in the item-write path (shared by PutItem/BatchWriteItem/UpdateItem) — set-member type checks must raise the service refusal, not surface the JSON decoder's complaint.

Cause 2 of 10 — BatchWriteItem: the nesting-limit message carries a spurious 'N validation error detected: ' prefix (BatchWriteItem; fixture ddbval-batch-write-item-oversized-nested-put-rejects-whole-call step task#3).
AWS: ValidationException, HTTP 400, message 'Nesting Levels have exceeded supported limits: Attributes in the item have nested levels beyond supported limit' — entirely fixed text, with no coral 'validation error detected' prefix for this class.
Emulator: HTTP 400 with '1 validation error detected: Nesting Levels have exceeded supported limits: Attributes in the item have nested levels beyond supported limit'.
Why: the emulator applies the coral prefix uniformly, so clients cannot tell the prefixed classes (per-member constraint failures, which do carry it) from the unprefixed service-rule classes, and exact-message assertions fail.
Fix: the BatchWriteItem validation path / the ValidationException renderer — the coral prefix must be applied only to the member-constraint class that AWS prefixes.

Cause 3 of 10 — TransactWriteItems: same spurious prefix on the nesting-limit message (TransactWriteItems; fixture ddbval-transact-write-items-nesting-is-top-level-validation step task#4).
AWS: ValidationException, HTTP 400, 'Nesting Levels have exceeded supported limits: Attributes in the item have nested levels beyond supported limit' (fixed text), raised as a top-level validation failure rather than a per-item cancellation reason.
Emulator: HTTP 400 with the same sentence prefixed by '1 validation error detected: '.
Why: as cause 2, for the transactional write path.
Fix: the TransactWriteItems top-level validation pass / the shared ValidationException renderer.

Cause 4 of 10 — UpdateItem: the item-size refusal uses the PutItem sentence (UpdateItem; fixture ddbval-item-over-400kb-rejected-not-stored step task#8).
AWS: ValidationException, HTTP 400. When a write grows an existing item past the maximum item size, UpdateItem's message is 'Item size to update has exceeded the maximum allowed size' — fixed text, distinct from the PutItem-shaped 'Item size has exceeded the maximum allowed size'. No variable parts.
Emulator: HTTP 400 with 'Item size has exceeded the maximum allowed size'.
Why: AWS's two sentences distinguish 'the item you sent is too big' from 'the update you asked for would make the stored item too big'. Collapsing them hides which one occurred, and a client that matches the update-specific sentence to decide whether to re-read and shrink the stored item never matches.
Fix: UpdateItem's size validation (post-apply size computation) — emit the update-specific sentence.

Cause 5 of 10 — TransactGetItems: the over-limit TransactItems refusal does not use AWS's coral constraint sentence (TransactGetItems; fixture dynamodb-batch-caps-and-unprocessed step task#11).
AWS: Smithy: TransactGetItems.TransactItems is a list of TransactGetItem with a modeled length constraint; exceeding it is ValidationException, HTTP 400, with the coral member-constraint message '<N> validation error detected: Value '<the rejected member, serialized>' at 'transactItems' failed to satisfy constraint: ...'. The fixed text is '<N> validation error detected: Value ' / ' at '<memberName>' failed to satisfy constraint: ' plus the constraint clause; the variable parts are the error count, the serialized value of the rejected member as the request supplied it (AWS renders each TransactGetItem/AttributeValue in its full field-by-field form) and the member name.
Emulator: HTTP 400 and the same error class, but $.message diverges. The replay report truncates this cluster's detail inside the expected string, so the emulator's exact text is not reproduced here — re-replay the step to capture it. The requirement is the coral template above.
Why: this is the message a client parses to learn which input member and which constraint it violated; an emulator-authored substitute names neither.
Fix: TransactGetItems input validation — render modeled length-constraint failures through the coral member-constraint template with the request's own serialized member value.

Cause 6 of 10 — TransactWriteItems: a cancellation reason reports the generic key-schema sentence instead of the typed mismatch (TransactWriteItems; fixture dynamodb-transaction-failure-class-discrimination step task#9).
AWS: HTTP 400, TransactionCanceledException with per-item CancellationReasons. For a key attribute supplied with the wrong AttributeValue type, the reason Message is 'One or more parameter values were invalid: Type mismatch for key <keyAttributeName> expected: <schemaType> actual: <suppliedType>'. Fixed text: 'One or more parameter values were invalid: Type mismatch for key ' / ' expected: ' / ' actual: '. Variable parts: the key attribute's name and its declared type from the table's KeySchema/AttributeDefinitions, and the AttributeValue type tag actually supplied in the request.
Emulator: 'One or more parameter values were invalid: The provided key element does not match the schema' — the sentence AWS uses when it will not say more.
Why: in a multi-item transaction the cancellation reason is the only per-item diagnostic; without the key name and the two types the caller cannot tell which item of the transaction was wrong or how.
Fix: the key-validation step that builds CancellationReasons for TransactWriteItems — carry the attribute name and both types into the reason message.

Cause 7 of 10 — GetResourcePolicy: the malformed-ARN refusal is emulator prose (GetResourcePolicy; fixture dynamodb-rbp-traj-malformed-resource-arn-classes step task#3).
AWS: ValidationException, HTTP 400, message 'One or more parameter values were invalid: Provided Arn is not a DynamoDB resource arn: <resourceArn>'. Fixed text: 'One or more parameter values were invalid: Provided Arn is not a DynamoDB resource arn: '; the variable part is the ResourceArn exactly as the request supplied it (including its wrong casing or wrong resource type).
Emulator: HTTP 400 with 'One or more parameter values were invalid: Invalid resource arn provided, only table or stream is accepted. Provided resource arn: <resourceArn>' — a two-sentence message AWS does not emit.
Why: the accepted-resource-type prose is invented; clients matching AWS's sentence see an unknown message for a correctly refused ARN.
Fix: ResourceArn validation shared by Get/Put/DeleteResourcePolicy.

Cause 8 of 10 — DescribeImport: the import-not-found message drops AWS's wrapper (DescribeImport; fixture dynamodb-import-table-pipeline step task#4).
AWS: Smithy: DescribeImport errors = ImportNotFoundException (client, HTTP 400). Message: 'Import not found: Import with arn: <importArn> doesn't exist'. Fixed text: 'Import not found: Import with arn: ' and ' doesn't exist'; the variable part is the ImportArn from the request.
Emulator: HTTP 400, right error class, message 'Import not found: <importArn>'.
Why: exact-message assertions fail, and the response loses the wording that distinguishes a well-formed but unknown import ARN from other not-found conditions.
Fix: the DescribeImport handler's not-found path.

Cause 9 of 10 — ExportTableToPointInTime: the table-not-found message echoes the table name instead of the ARN (ExportTableToPointInTime; fixture dynamodb-point-in-time-export-pipeline step task#4).
AWS: Smithy: ExportTableToPointInTime.TableArn is required and targets TableArn; errors include TableNotFoundException (client, HTTP 400). Message: 'Table not found: <tableArn>'. Fixed text 'Table not found: '; the variable part is the TableArn value the request supplied — an ARN, because that is what this operation's input member is.
Emulator: HTTP 400, right class, but 'Table not found: <bare table name>' — it echoes a name it derived from the ARN rather than the input value.
Why: the caller cannot correlate the refusal with the ARN it sent (and in cross-account/cross-region ARNs the name alone is ambiguous).
Fix: the ExportTableToPointInTime handler's table-resolution path — echo the request's TableArn verbatim.

Cause 10 of 10 — UpdateTable: adding a GSI without AttributeDefinitions reports the CreateTable-shaped message (UpdateTable; fixture dynamodb-csvtraj-gsi-added-after-create-via-update-table step task#4).
AWS: ValidationException, HTTP 400, message 'One or more parameter values were invalid: AttributeDefinitions is not specified for index: <indexName>'. Fixed text: 'One or more parameter values were invalid: AttributeDefinitions is not specified for index: '; the variable part is the name of the index being created by the GlobalSecondaryIndexUpdates entry.
Emulator: HTTP 400 with 'One or more parameter values were invalid: Some index key attributes are not defined in AttributeDefinitions. Keys: [<key names>], AttributeDefinitions: []' — the CreateTable-time sentence, which names key attributes rather than the index.
Why: on UpdateTable the actionable fact is which index lacks definitions (a request may update several); the substituted message names none, so the caller cannot tell which GlobalSecondaryIndexUpdates entry to fix.
Fix: UpdateTable's GSI-create validation — use the index-scoped sentence instead of reusing CreateTable's attribute-definition check.

Related open issues (the replay reports no owner for any cluster here, so none of them covers these): '20261109T055139Z-dynamodb-validation-refusal-messages-are-emulator-authored-for.md', '20261109T172142Z-dynamodb-7-net-new-causes-validationexception-message-text.md', '20261110T155738Z-dynamodb-5-net-new-causes-putitem-s-missing.md'.

## Evidence
Cause 1 (PutItem, dynamodb-attribute-set-null-member-rejected#task#3, 12 affected steps): expected 400 / 'One or more parameter values were invalid: An string set may not have a null string as a member'; actual 400 / 'expected string, got null'.
Cause 2 (BatchWriteItem, ddbval-batch-write-item-oversized-nested-put-rejects-whole-call#task#3, 4 affected): expected 'Nesting Levels have exceeded supported limits: Attributes in the item have nested levels beyond supported limit'; actual prefixed with '1 validation error detected: '.
Cause 3 (TransactWriteItems, ddbval-transact-write-items-nesting-is-top-level-validation#task#4, 5 affected): same expected sentence; actual prefixed with '1 validation error detected: '.
Cause 4 (UpdateItem, ddbval-item-over-400kb-rejected-not-stored#task#8, 10 affected): expected 'Item size to update has exceeded the maximum allowed size'; actual 'Item size has exceeded the maximum allowed size'.
Cause 5 (TransactGetItems, dynamodb-batch-caps-and-unprocessed#task#11, 2 affected): expected 400 with the coral sentence "1 validation error detected: Value '[TransactGetItem(get=Get(key={pk=AttributeValue(s=k0, ...)}, tableName=..., projectionExpression=null, ...)), ...]' at 'transactItems' failed to satisfy constraint: ..."; actual 400 with a differing $.message (the replay report truncates the cluster detail before the emulator's text).
Cause 6 (TransactWriteItems, dynamodb-transaction-failure-class-discrimination#task#9, 1 affected): expected $.CancellationReasons[0].Message 'One or more parameter values were invalid: Type mismatch for key pk expected: S actual: N'; actual 'One or more parameter values were invalid: The provided key element does not match the schema'.
Cause 7 (GetResourcePolicy, dynamodb-rbp-traj-malformed-resource-arn-classes#task#3, 3 affected): expected 'One or more parameter values were invalid: Provided Arn is not a DynamoDB resource arn: <arn>'; actual 'One or more parameter values were invalid: Invalid resource arn provided, only table or stream is accepted. Provided resource arn: <arn>'.
Cause 8 (DescribeImport, dynamodb-import-table-pipeline#task#4, 1 affected): expected "Import not found: Import with arn: <importArn> doesn't exist"; actual 'Import not found: <importArn>'.
Cause 9 (ExportTableToPointInTime, dynamodb-point-in-time-export-pipeline#task#4, 1 affected): expected 'Table not found: <table ARN>'; actual 'Table not found: <bare table name>'.
Cause 10 (UpdateTable, dynamodb-csvtraj-gsi-added-after-create-via-update-table#task#4, 2 affected): expected 'One or more parameter values were invalid: AttributeDefinitions is not specified for index: <indexName>'; actual 'One or more parameter values were invalid: Some index key attributes are not defined in AttributeDefinitions. Keys: [<keys>], AttributeDefinitions: []'.
All ten clusters keep the expected HTTP status and (except where noted) the expected error class; only the message text diverges.

## Steps to Reproduce
1. PutItem an item whose SS list contains a null member (fixture dynamodb-attribute-set-null-member-rejected step task#3) and compare $.message.
2. BatchWriteItem a PutRequest whose item nests beyond the supported depth (ddbval-batch-write-item-oversized-nested-put-rejects-whole-call step task#3); repeat through TransactWriteItems (ddbval-transact-write-items-nesting-is-top-level-validation step task#4). Check for the '1 validation error detected: ' prefix.
3. UpdateItem an existing item so the result exceeds the maximum item size (ddbval-item-over-400kb-rejected-not-stored step task#8).
4. TransactGetItems with more TransactItems than the modeled maximum (dynamodb-batch-caps-and-unprocessed step task#11).
5. TransactWriteItems with a key attribute of the wrong type (dynamodb-transaction-failure-class-discrimination step task#9) and read $.CancellationReasons[0].Message.
6. GetResourcePolicy with a syntactically valid but non-DynamoDB resource ARN (dynamodb-rbp-traj-malformed-resource-arn-classes step task#3).
7. DescribeImport with an unknown import ARN (dynamodb-import-table-pipeline step task#4).
8. ExportTableToPointInTime with a TableArn naming no table (dynamodb-point-in-time-export-pipeline step task#4).
9. UpdateTable adding a GSI without supplying AttributeDefinitions (dynamodb-csvtraj-gsi-added-after-create-via-update-table step task#4).
10. Re-replay the corpus: all ten clusters (41 affected steps) must pass.
