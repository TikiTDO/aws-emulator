Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 7eba44b4c78724d09fd14d928dd44bf5d39abe6e

# UpdateWorkflow accepts an unreachable S3 definition location and a non-assumable execution role

_Filed: 2026-10-24T05:24:34Z_
_Operation: UpdateWorkflow | Class: validation_

> _Note: this issue was identified against the code base at commit `7eba44b4c78724d09fd14d928dd44bf5d39abe6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: UpdateWorkflow validates the dependencies of the configuration the new version would freeze before appending it. The operation declares ValidationException (400), whose model documentation names exactly these causes — 'when workflow definitions contain unsupported operators, when required IAM permissions are missing, when S3 locations are inaccessible' — and AccessDeniedException (403) for an execution role the service cannot assume. So an update whose required DefinitionS3Location points at a bucket/object that does not exist should be refused with ValidationException, and an update whose required RoleArn names a role that does not exist (or whose trust policy does not let airflow-serverless assume it) should be refused with AccessDeniedException, leaving the workflow's version history untouched.

Actual: `UpdateWorkflow::handle` runs `validate_input`, which only drives the framework constraint walk over the declared @length/@pattern of WorkflowArn, RoleArn, Description, TriggerMode, Code and NetworkConfiguration. DefinitionS3Location's Bucket/ObjectKey target plain String and declare no constraints at all, so nothing can reject them; nothing in the crate ever dereferences the S3 pointer or checks the role. The handler then appends a new WorkflowVersionRecord whose snapshot merely stores the unverified DefinitionS3Location/RoleArn, flips every earlier version's `is_latest` to false, bumps `current_version`/`revision_id`/`modified_at`/`code_snapshotted_at`, and answers 200 with a fresh 32-hex WorkflowVersion. A client therefore gets a new 'latest' version — and loses the previously-scheduled one — for a configuration that could never run on AWS, and the workflow is left pointing at it.

Note: this is the UpdateWorkflow counterpart of the already-filed CreateWorkflow validation gap; it is a separate operation with its own, additionally destructive, consequence (the previous latest version is demoted).

## Evidence

Smithy model (smithy_query query=operation operation=UpdateWorkflow): input requires DefinitionS3Location and RoleArn; errors include ValidationException (httpError 400) and AccessDeniedException (httpError 403); the ValidationException shape doc names 'when S3 locations are inaccessible' and 'when required IAM permissions are missing' as its causes. DefinitionS3Location.Bucket/ObjectKey target String with no length/pattern (see ops/types/shared_types.rs lines 65-71 and the model), so the request boundary cannot reject them. Handler: services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/src/ops/update_workflow.rs — `validate_input` (lines ~370-400) checks only declared constraints, and the snapshot built at lines ~268-278 clones `input.definition_s3_location` / `input.role_arn` straight into state, with the version appended and promoted at lines ~285-297. Test coverage: tests/all/workflow_update_versions.rs asserts only that an update appends a version carrying exactly the supplied configuration (`an_update_appends_a_version_that_carries_exactly_the_configuration_supplied`) and that an unknown workflow is refused; tests/all/validation_errors.rs covers only @length/@pattern violations. No test asserts an S3-inaccessible or untrusted-role refusal for UpdateWorkflow.

## Steps to Reproduce

1. Boot the emulator and CreateWorkflow with a valid Name, RoleArn and DefinitionS3Location; note the WorkflowArn and the returned WorkflowVersion (call it V1).
2. Call UpdateWorkflow on that WorkflowArn with DefinitionS3Location={Bucket:'no-such-bucket', ObjectKey:'dags/none.yaml'} and RoleArn='arn:aws:iam::123456789012:role/never-created'.
3. Observe HTTP 200 with a new WorkflowVersion (V2) instead of 400 ValidationException (S3 location inaccessible) / 403 AccessDeniedException (role not assumable by airflow-serverless).
4. Call GetWorkflow on the WorkflowArn with no WorkflowVersion and observe it now reports V2 and the unreachable definition/role, i.e. V1 is no longer the latest version even though the update should have been refused.
