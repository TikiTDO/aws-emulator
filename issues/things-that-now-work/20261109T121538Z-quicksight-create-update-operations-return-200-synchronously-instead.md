Issue-Source: trace-sweep-divergence
Issue-Trace: quicksight-data-source-lifecycle-create-update-delete
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: quicksight-data-source-lifecycle-create-update-delete quicksight-group-lifecycle-and-duplicate-rejected quicksight-iam-policy-assignment-lifecycle
Issue-Shape-Id: com.amazonaws.quicksight#QuickSight_20180401
Issue-Key: quicksight|CreateDataSource|2xx->2xx|$.CreationStatus
Issue-Key: quicksight|CreateDataSource|2xx->2xx|status
Issue-Key: quicksight|UpdateDataSource|2xx->2xx|$.UpdateStatus
Issue-Key: quicksight|UpdateDataSource|2xx->2xx|status
Issue-Key: quicksight|CreateGroup|2xx->2xx|status
Issue-Key: quicksight|CreateIAMPolicyAssignment|2xx->2xx|$.Arn
Issue-Key: quicksight|CreateIAMPolicyAssignment|2xx->2xx|status
Issue-Commit: ae5156e659812a6269d5e057615ff3791f30822e

# QuickSight create/update operations return 200 synchronously instead of AWS's 201/202 in-progress response

> _Note: this issue was identified against the code base at commit `ae5156e659812a6269d5e057615ff3791f30822e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDataSource

## Description
Four net-new root causes with one shared fault: QuickSight's mutating operations carry their HTTP status in the response body's Status member (@httpResponseCode), and AWS answers resource creation/update with 201 or 202 and an in-progress lifecycle status. The emulator hardcodes 200 and reports the terminal lifecycle status immediately. This bundle has 4 causes; all four must be closed.

Cause 1 of 4 — CreateDataSource (fixture quicksight-data-source-lifecycle-create-update-delete, step task#4).
AWS: CreateDataSource (Smithy: POST /accounts/{AwsAccountId}/data-sources; CreateDataSourceResponse.Status is targeted at StatusCode and marked @httpResponseCode, so the operation's HTTP status is service-chosen, not pinned to the trait's default 200; CreationStatus targets the ResourceStatus enum whose values include CREATION_IN_PROGRESS and CREATION_SUCCESSFUL). Data-source creation is asynchronous: AWS accepts the request with HTTP 202 and CreationStatus = "CREATION_IN_PROGRESS", and the caller polls DescribeDataSource until DataSource.Status reaches CREATION_SUCCESSFUL / CREATION_FAILED.
Emulator: HTTP 200 with CreationStatus = "CREATION_SUCCESSFUL" in the create response. Replay: "status: expected 202, got 200; $.CreationStatus: value_mismatch (expected 'CREATION_IN_PROGRESS', got 'CREATION_SUCCESSFUL')".
Why: a client that branches on 202/CREATION_IN_PROGRESS (or that asserts the create response is not yet terminal before starting its poll loop) takes a path AWS never gives it; code that treats CREATION_SUCCESSFUL on the create call as "connection validated" draws a conclusion AWS only supplies after the asynchronous test completes.
Fix: CreateDataSource's handler and its response serializer (services/service-com-amazonaws-quicksight-quicksight-20180401/src/ops/create_data_source.rs plus the data-source state model) — the create must set the response Status member to 202 and the stored lifecycle state to CREATION_IN_PROGRESS, with the terminal state reached by the describe path.

Cause 2 of 4 — UpdateDataSource (same fixture, step task#6).
AWS: UpdateDataSource (Smithy: PUT /accounts/{AwsAccountId}/data-sources/{DataSourceId}; UpdateDataSourceResponse.Status is @httpResponseCode; UpdateStatus targets ResourceStatus, values include UPDATE_IN_PROGRESS / UPDATE_SUCCESSFUL). Updates are asynchronous the same way: HTTP 202 with UpdateStatus = "UPDATE_IN_PROGRESS".
Emulator: HTTP 200 with UpdateStatus = "UPDATE_SUCCESSFUL". Replay: "status: expected 202, got 200; $.UpdateStatus: value_mismatch (expected 'UPDATE_IN_PROGRESS', got 'UPDATE_SUCCESSFUL')".
Why: same as cause 1 for the update path — the in-progress state is never observable, so poll-until-settled logic and any assertion on the 202 fail.
Fix: UpdateDataSource's handler/serializer and the data-source lifecycle state.

Cause 3 of 4 — CreateGroup (fixture quicksight-group-lifecycle-and-duplicate-rejected, step setup#1).
AWS: CreateGroup (Smithy: POST /accounts/{AwsAccountId}/namespaces/{Namespace}/groups; CreateGroupResponse.Status is @httpResponseCode). Group creation is synchronous but answers HTTP 201 Created, with the Status body member carrying 201.
Emulator: HTTP 200. Replay: "status: expected 201, got 200".
Why: clients and SDK middleware that distinguish created (201) from ok (200) — retry/idempotency wrappers, HTTP-level assertions, recorded-contract tests — see the wrong status, and the body's Status member disagrees with AWS.
Fix: CreateGroup's handler/serializer (ops/create_group.rs) must emit 201 both as the HTTP status and in the response's Status member.

Cause 4 of 4 — CreateIAMPolicyAssignment (fixture quicksight-iam-policy-assignment-lifecycle, step task#2).
AWS: CreateIAMPolicyAssignment (Smithy: POST /accounts/{AwsAccountId}/namespaces/{Namespace}/iam-policy-assignments; CreateIAMPolicyAssignmentResponse.Status is @httpResponseCode). AWS answers HTTP 201 and the recorded body additionally carries the assignment's ARN, an Arn member the recording shows but this model revision does not declare (recorded behavior only): fixed text arn:aws:quicksight::<AwsAccountId>:assignment/<AssignmentName> — note the empty region field in the assignment ARN — with the account and assignment name taken from the request.
Emulator: HTTP 200 and no Arn member at all. Replay: "status: expected 201, got 200; $.Arn: missing (expected 'arn:aws:quicksight::000000000000:assignment/<name>', got None)".
Why: the create looks like a plain 200 and the caller cannot read back the assignment's ARN, so anything that stores or passes that ARN onward has nothing to use.
Fix: CreateIAMPolicyAssignment's handler/serializer (ops/create_iam_policy_assignment.rs) — emit 201 and populate the assignment ARN. The same Arn omission on the describe/update/delete assignment operations is filed separately as the QuickSight response-member-omission bundle; this cause covers the create response.

Traces: quicksight-data-source-lifecycle-create-update-delete#task#4 and #task#6, quicksight-group-lifecycle-and-duplicate-rejected#setup#1, quicksight-iam-policy-assignment-lifecycle#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1:

1) CreateDataSource — quicksight-data-source-lifecycle-create-update-delete#task#4
   Expected: HTTP 202, CreationStatus = "CREATION_IN_PROGRESS".
   Actual: HTTP 200, CreationStatus = "CREATION_SUCCESSFUL".

2) UpdateDataSource — quicksight-data-source-lifecycle-create-update-delete#task#6
   Expected: HTTP 202, UpdateStatus = "UPDATE_IN_PROGRESS".
   Actual: HTTP 200, UpdateStatus = "UPDATE_SUCCESSFUL".

3) CreateGroup — quicksight-group-lifecycle-and-duplicate-rejected#setup#1
   Expected: HTTP 201. Actual: HTTP 200.

4) CreateIAMPolicyAssignment — quicksight-iam-policy-assignment-lifecycle#task#2
   Expected: HTTP 201 and Arn = "arn:aws:quicksight::<account>:assignment/<AssignmentName>".
   Actual: HTTP 200, Arn missing.

Smithy citations: CreateDataSourceResponse.Status / UpdateDataSourceResponse.Status / CreateGroupResponse.Status / CreateIAMPolicyAssignmentResponse.Status all target StatusCode with httpResponseCode=true (the http trait's code 200 is only the default); CreationStatus and UpdateStatus target the ResourceStatus enum = [CREATION_IN_PROGRESS, CREATION_SUCCESSFUL, CREATION_FAILED, UPDATE_IN_PROGRESS, UPDATE_SUCCESSFUL, UPDATE_FAILED, DELETED]. The create-assignment Arn member is not in this model revision, so it is recorded behavior only.

## Steps to Reproduce
1. Replay fixture quicksight-data-source-lifecycle-create-update-delete: CreateDataSource(AwsAccountId, DataSourceId, Name, Type, DataSourceParameters) and read the HTTP status and CreationStatus — the emulator returns 200/CREATION_SUCCESSFUL where AWS returns 202/CREATION_IN_PROGRESS (step task#4).
2. In the same fixture, UpdateDataSource with a new Name and read the HTTP status and UpdateStatus — 200/UPDATE_SUCCESSFUL instead of 202/UPDATE_IN_PROGRESS (step task#6).
3. Replay fixture quicksight-group-lifecycle-and-duplicate-rejected: CreateGroup(AwsAccountId, Namespace=default, GroupName, Description) and read the HTTP status — 200 instead of 201 (step setup#1).
4. Replay fixture quicksight-iam-policy-assignment-lifecycle: CreateIAMPolicyAssignment(AwsAccountId, Namespace=default, AssignmentName, AssignmentStatus) and read the HTTP status and Arn — 200 with no Arn instead of 201 with arn:aws:quicksight::<account>:assignment/<AssignmentName> (step task#2).
