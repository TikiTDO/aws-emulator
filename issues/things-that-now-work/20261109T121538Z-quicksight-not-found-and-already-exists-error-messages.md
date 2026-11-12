Issue-Source: trace-sweep-divergence
Issue-Trace: quicksight-group-lifecycle-and-duplicate-rejected
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: quicksight-group-lifecycle-and-duplicate-rejected quicksight-folder-lifecycle-and-duplicate-rejected quicksight-data-source-lifecycle-create-update-delete
Issue-Shape-Id: com.amazonaws.quicksight#QuickSight_20180401
Issue-Key: quicksight|CreateGroup|4xx->4xx|$.Message
Issue-Key: quicksight|CreateFolder|4xx->4xx|$.Message
Issue-Key: quicksight|CreateFolder|4xx->4xx|$.RequestId
Issue-Key: quicksight|DescribeDataSource|4xx->4xx|$.Message
Issue-Key: quicksight|DescribeDataSource|4xx->4xx|$.RequestId
Issue-Key: quicksight|DescribeDataSource|4xx->4xx|$.ResourceType
Issue-Commit: ae5156e659812a6269d5e057615ff3791f30822e

# QuickSight not-found and already-exists error messages are emulator-authored instead of AWS's templates

> _Note: this issue was identified against the code base at commit `ae5156e659812a6269d5e057615ff3791f30822e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGroup

## Description
Three net-new root causes with one shared fault: the emulator raises the right error shape and HTTP status, but writes its own message text (and in one case an extra ResourceType discriminator AWS does not send), so the message a client reads is text AWS never emits. The message is part of the contract for these error classes. This bundle has 3 causes; all three must be closed.

Cause 1 of 3 — CreateGroup duplicate-name message (fixture quicksight-group-lifecycle-and-duplicate-rejected, step task#5).
AWS: CreateGroup (Smithy: POST /accounts/{AwsAccountId}/namespaces/{Namespace}/groups; errors include ResourceExistsException, errorKind client, httpError 409, documented "The resource specified already exists."). Re-creating an existing group answers HTTP 409 with the identity-service already-exists template: fixed text "Group ", " already exists. AWS account id: ", ", groupName: ", ". " — note the message ends with ". " (trailing space) — and the variable parts are the request's GroupName (twice) and AwsAccountId. The model fixes the shape and status but not the text, so the template is recorded behavior only.
Emulator: HTTP 409 (correct) with message "The group <GroupName> already exists in namespace <Namespace>." Replay: "$.Message: value_mismatch (expected 'Group <name> already exists. AWS account id: <account>, groupName: <name>. ', got 'The group <name> already exists in namespace default.')".
Why: a client that surfaces, logs or pattern-matches the duplicate-group message — or parses the account id / groupName pairs out of it — gets wording that never appears on AWS and finds none of the key/value fragments it expects.
Fix: CreateGroup's duplicate-detection error path (services/service-com-amazonaws-quicksight-quicksight-20180401/src/ops/create_group.rs) — render AWS's template from the request's GroupName and AwsAccountId instead of an emulator-authored sentence.

Cause 2 of 3 — CreateFolder duplicate-id message (fixture quicksight-folder-lifecycle-and-duplicate-rejected, step task#5).
AWS: CreateFolder (Smithy: POST /accounts/{AwsAccountId}/folders/{FolderId}; errors include ResourceExistsException, client, httpError 409). Re-creating an existing folder answers HTTP 409 with the ARN-based already-exists template: fixed text "Folder " and " already exists" (no trailing period) around one variable part, the folder ARN arn:aws:quicksight:<region>:<AwsAccountId>:folder/<FolderId>, assembled from the serving region and the request's path labels. Recorded behavior only for the text; the model fixes shape and status.
Emulator: HTTP 409 with message "The folder with ID <FolderId> already exists." Replay: "$.Message: value_mismatch (expected 'Folder arn:aws:quicksight:us-west-2:000000000000:folder/<id> already exists', got 'The folder with ID <id> already exists.')".
Why: the emulator's message has no ARN, so anything that extracts the conflicting resource's ARN (to report which folder of a batch collided, or to resolve its account/region) finds nothing; message-sensitive assertions diverge.
Fix: CreateFolder's conflict error path (ops/create_folder.rs) — build the folder ARN (the same derivation already used for the created folder's Arn) and render AWS's template.

Cause 3 of 3 — DescribeDataSource not-found message and spurious ResourceType (fixture quicksight-data-source-lifecycle-create-update-delete, step task#9).
AWS: DescribeDataSource (Smithy: GET /accounts/{AwsAccountId}/data-sources/{DataSourceId}; errors include ResourceNotFoundException, client, httpError 404, whose shape declares Message, ResourceType and RequestId). For a deleted data source AWS answers 404 with fixed text "DataSource " and " is not found" around the data-source ARN arn:aws:quicksight:<region>:<AwsAccountId>:datasource/<DataSourceId>, and the recorded response carries no ResourceType member — ResourceType is optional and QuickSight does not populate it on the data-source not-found path (it does on the group path).
Emulator: HTTP 404 and Code ResourceNotFoundException (both correct) with message "Data source with ID <DataSourceId> not found." — template "Data source with ID {DataSourceId} not found.", no ARN, trailing period — plus ResourceType = "DATA_SOURCE". Replay: "$.Message: value_mismatch (expected 'DataSource arn:aws:quicksight:us-west-2:000000000000:datasource/<id> is not found', got 'Data source with ID <id> not found.'); $.ResourceType: type_mismatch (expected None, got 'DATA_SOURCE')".
Why: no ARN in the message, so ARN-parsing clients fail; and a client that switches on the presence of ResourceType sees a discriminator AWS did not send on this path.
Fix: the not-found error construction shared by the data-source handlers (ops/describe_data_source.rs and ops/update_data_source.rs use the same wording) — render the ARN-based template and stop populating ResourceType for the data-source not-found error. Open behavioral issue 20261031T224711Z-quicksight-data-source-resourcenotfoundexception-message-is-invented-wording.md describes the same wording defect from the behavioral corpus; it carries no wire replay key and does not cover the spurious ResourceType member, so this cause is filed with its own fixture/step binding.

Traces: quicksight-group-lifecycle-and-duplicate-rejected#task#5, quicksight-folder-lifecycle-and-duplicate-rejected#task#5, quicksight-data-source-lifecycle-create-update-delete#task#9.

## Evidence
Replay of trace set simparity-wire-aws-v1 (HTTP statuses matched; the message bodies diverge):

1) CreateGroup on an existing group — quicksight-group-lifecycle-and-duplicate-rejected#task#5, HTTP 409 both sides
   Expected: Message = "Group <GroupName> already exists. AWS account id: <AwsAccountId>, groupName: <GroupName>. " (fixed text "Group " / " already exists. AWS account id: " / ", groupName: " / ". "; variables = GroupName, AwsAccountId from the request).
   Actual: Message = "The group <GroupName> already exists in namespace <Namespace>."

2) CreateFolder on an existing folder — quicksight-folder-lifecycle-and-duplicate-rejected#task#5, HTTP 409 both sides
   Expected: Message = "Folder arn:aws:quicksight:<region>:<AwsAccountId>:folder/<FolderId> already exists".
   Actual: Message = "The folder with ID <FolderId> already exists."

3) DescribeDataSource on a deleted data source — quicksight-data-source-lifecycle-create-update-delete#task#9, HTTP 404 both sides
   Expected: Message = "DataSource arn:aws:quicksight:<region>:<AwsAccountId>:datasource/<DataSourceId> is not found"; no ResourceType member.
   Actual: Message = "Data source with ID <DataSourceId> not found."; ResourceType = "DATA_SOURCE".

Smithy citations: CreateGroup / CreateFolder errors -> ResourceExistsException (client, httpError 409, "The resource specified already exists."); DescribeDataSource errors -> ResourceNotFoundException (client, httpError 404, "One or more resources can't be found."); both error shapes declare Message, ResourceType (ExceptionResourceType enum) and RequestId, and neither fixes the message text — the templates above are recorded behavior only, and the ids/accounts in the recordings only fill them.

## Steps to Reproduce
1. Replay fixture quicksight-group-lifecycle-and-duplicate-rejected: CreateGroup(AwsAccountId, Namespace=default, GroupName) twice and read the second call's Error.Message — the emulator answers "The group <name> already exists in namespace default." where AWS answers "Group <name> already exists. AWS account id: <account>, groupName: <name>. " (step task#5).
2. Replay fixture quicksight-folder-lifecycle-and-duplicate-rejected: CreateFolder(AwsAccountId, FolderId) twice and read the second call's Error.Message — "The folder with ID <id> already exists." instead of "Folder <folder arn> already exists" (step task#5).
3. Replay fixture quicksight-data-source-lifecycle-create-update-delete: create, update, DeleteDataSource, then DescribeDataSource on the deleted id and read Error.Message and Error.ResourceType — "Data source with ID <id> not found." plus ResourceType DATA_SOURCE, instead of "DataSource <datasource arn> is not found" with no ResourceType (step task#9).
