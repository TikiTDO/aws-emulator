Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-table-admin-scalar-type-mismatch-rejected
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-table-admin-scalar-type-mismatch-rejected dynamodb-table-definition-list-and-map-member-scalar-rejected dynamodb-tag-untag-resource-list-member-scalar-rejected dynamodb-tagging-and-metadata-scalar-type-mismatch-rejected
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|CreateTable|4xx->4xx|$.Message
Issue-Key: dynamodb|UpdateTable|4xx->4xx|$.Message
Issue-Key: dynamodb|UpdateTable|4xx->4xx|$.__type
Issue-Key: dynamodb|ListTables|4xx->4xx|$.Message
Issue-Key: dynamodb|ListTables|4xx->4xx|$.__type
Issue-Key: dynamodb|TagResource|4xx->4xx|$.Message
Issue-Key: dynamodb|TagResource|4xx->4xx|$.__type
Issue-Key: dynamodb|UntagResource|4xx->4xx|$.Message
Issue-Key: dynamodb|UntagResource|4xx->4xx|$.__type
Issue-Key: dynamodb|ListTagsOfResource|4xx->4xx|$.Message
Issue-Key: dynamodb|ListTagsOfResource|4xx->4xx|$.__type
Issue-Related: 20261104T061030Z-dynamodb-serves-the-wrong-error-class-for-malformed.md
Issue-Commit: 0e3e027770f68a801846f0512bd9649d89ae04a7

# dynamodb: control-plane and tagging operations answer ValidationException where AWS answers SerializationException

> _Note: this issue was identified against the code base at commit `0e3e027770f68a801846f0512bd9649d89ae04a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTable

## Description
The same deserializer-vs-validator confusion as on the data plane, on the control plane and the tagging API: a JSON scalar of the wrong primitive type must fail deserialization with com.amazon.coral.service#SerializationException, but the emulator reaches validation and answers ValidationException. 6 causes, one per operation.

AWS (common to all six): recorded behavior plus the awsJson1_0 contract — status 400, body {"__type": "com.amazon.coral.service#SerializationException", "Message": "<fixed deserializer text>"}. The fixed texts here are 'NUMBER_VALUE cannot be converted to String' (JSON number where the model declares a string — e.g. CreateTable/UpdateTable TableName, which Smithy targets TableArn (string, @length 1..1024), or ListTables.ExclusiveStartTableName (string), or TagResource/ListTagsOfResource ResourceArn, which Smithy targets ResourceArnString (string, @length 1..1283)) and 'Unexpected field type' (a JSON scalar where the model declares a list/structure — e.g. TagResource.Tags, Smithy target TagList, or UntagResource.TagKeys, Smithy target TagKeyList). Neither text has variable parts.
Emulator (common to all six): status 400 with $.__type = 'ValidationException' and no $.Message; its own prose lands in $.message.
Why (common to all six): SDKs branch on __type, and a malformed-marshalling failure is not retryable or user-correctable the way a validation refusal is; error-reason extraction reads the capital-M 'Message' slot and finds nothing, so provisioning tools report a 400 with no reason.
Fix (common to all six): the DynamoDB awsJson1_0 request deserializer must raise com.amazon.coral.service#SerializationException with the fixed text in a 'Message' member before any operation handler or validator runs. Routing primitive-type mismatches into per-operation validation is the root cause.

Cause 1 of 6: CreateTable — number where a modeled string is expected. Traces: dynamodb-table-admin-scalar-type-mismatch-rejected#task#1. Expected Message 'NUMBER_VALUE cannot be converted to String'.
Cause 2 of 6: UpdateTable — same. Traces: dynamodb-table-admin-scalar-type-mismatch-rejected#task#5. Expected Message 'NUMBER_VALUE cannot be converted to String'.
Cause 3 of 6: ListTables — same. Traces: dynamodb-table-admin-scalar-type-mismatch-rejected#task#9. Expected Message 'NUMBER_VALUE cannot be converted to String'.
Cause 4 of 6: TagResource — scalar supplied for the Tags list member. Traces: dynamodb-tag-untag-resource-list-member-scalar-rejected#task#3. Expected Message 'Unexpected field type'.
Cause 5 of 6: UntagResource — scalar supplied for the TagKeys list member. Traces: dynamodb-tag-untag-resource-list-member-scalar-rejected#task#7. Expected Message 'Unexpected field type'.
Cause 6 of 6: ListTagsOfResource — number where a modeled string is expected. Traces: dynamodb-tagging-and-metadata-scalar-type-mismatch-rejected#task#5. Expected Message 'NUMBER_VALUE cannot be converted to String'.

## Evidence
All six: expected status 400 with $.__type = 'com.amazon.coral.service#SerializationException' and $.Message = the fixed deserializer text; got status 400 with $.__type = 'ValidationException' and $.Message missing.
Causes 1-3 (CreateTable / UpdateTable / ListTables) and cause 6 (ListTagsOfResource): expected $.Message 'NUMBER_VALUE cannot be converted to String'.
Causes 4-5 (TagResource / UntagResource): expected $.Message 'Unexpected field type'.

## Steps to Reproduce
1. Replay dynamodb-table-admin-scalar-type-mismatch-rejected#task#1, #task#5 and #task#9 (CreateTable, UpdateTable, ListTables).
2. Replay dynamodb-tag-untag-resource-list-member-scalar-rejected#task#3 and #task#7 (TagResource, UntagResource).
3. Replay dynamodb-tagging-and-metadata-scalar-type-mismatch-rejected#task#5 (ListTagsOfResource).
In each response assert $.__type == 'com.amazon.coral.service#SerializationException' and that the fixed deserializer sentence is in $.Message.
