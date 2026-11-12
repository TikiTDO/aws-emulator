Issue-Source: trace-sweep-divergence
Issue-Trace: mwaa-serverless-resource-not-found-errors
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mwaa-serverless-resource-not-found-errors
Issue-Shape-Id: com.amazonaws.mwaaserverless#AmazonMWAAServerless
Issue-Key: mwaa-serverless|UpdateWorkflow|4xx->4xx|$.Message
Issue-Key: mwaa-serverless|UpdateWorkflow|4xx->4xx|$.ResourceId
Issue-Key: mwaa-serverless|UpdateWorkflow|4xx->4xx|$.ResourceType
Issue-Key: mwaa-serverless|UpdateWorkflow|4xx->4xx|$.__type
Issue-Commit: 7dd4699bc4a2e0a9eda2ebc58db9ed5a87bbd163

# UpdateWorkflow judges the request's DefinitionS3Location before the workflow it addresses, so a missing workflow answers ValidationException

> _Note: this issue was identified against the code base at commit `7dd4699bc4a2e0a9eda2ebc58db9ed5a87bbd163`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateWorkflow

## Description
ONE net-new root cause in the `mwaa-serverless` emulator, grounded in a wire-fixture divergence replayed from trace set simparity-wire-aws-v1. It is an error-precedence defect: UpdateWorkflow decides the request's configuration dependencies BEFORE it resolves the workflow the request addresses, so a request sent to a workflow that does not exist is answered with the wrong modeled error shape.

Model used: services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/smithy-model/com-amazonaws-mwaaserverless-amazonmwaaserverless.json (sdkId "MWAA Serverless", protocol awsJson1_0).

=== Cause 1 of 1: a reference to a nonexistent workflow is reported as a DefinitionS3Location validation failure instead of ResourceNotFoundException ===

AWS: Smithy: UpdateWorkflow declares both `ResourceNotFoundException` and `ValidationException`. `ResourceNotFoundException` is documented "The specified resource was not found. You can only access or modify a resource that already exists." and its members are `Message` (ErrorMessage, required, len 1..2048), `ResourceId` (String, required, "The unique identifier of the resource") and `ResourceType` (String, required, "The type of the resource"). `ValidationException` is documented for "request parameters [that] are invalid, missing, or inconsistent ... when S3 locations are inaccessible". AWS resolves the resource the request ADDRESSES (`WorkflowArn`, @required @httpLabel) first: when that workflow does not exist, the call fails with `ResourceNotFoundException` and the service never reaches the point of judging whether the supplied `DefinitionS3Location` is readable. The recording shows exactly that: a single request carrying BOTH a nonexistent workflow ARN and an unreadable definition object comes back as the error about the missing resource. Its body (recorded behavior for this error class) is `__type` = `com.amazonaws.mwaaserverless.exceptions#ResourceNotFoundException`, `Message` = the fixed text `Resource not found`, `ResourceType` = the fixed resource-type token `Workflow`, and `ResourceId` = the addressed workflow's own identifier — the ARN's resource-id segment taken from the request (in the recording `sim2real-probe-mwaa-serverless-wf-missing-30316a11-abcdefghij`; the identifier is a request value, the `Message` text and the `ResourceType` token are the fixed parts). The recorded HTTP status for this step is 400 and the emulator already answers 400, so the STATUS is not the divergence; note only that the model declares `httpError` 404 on this shape while the corpus records 400, so a fix must keep answering the recorded 400 for this error and not let the shape's modeled status change the step's status.

Emulator: answers HTTP 400 with the WRONG error shape — `__type` = `ValidationException` and `Message` = "1 validation error detected: Value at 'DefinitionS3Location' failed to satisfy constraint: Member must address an Amazon S3 object this service can read, and s3://<bucket>/<key> is inaccessible" (the bucket/key are request values; the clause is the emulator's fixed text). `ResourceId` and `ResourceType` are absent entirely, because they are not members of ValidationException. The ordering is explicit and deliberate in the handler: services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/src/ops/update_workflow.rs reads the definition object at lines 231-236 and refuses an unmet configuration dependency at lines 248-265 — its own comment at lines 243-247 says the configuration is judged "before the workflow the request ADDRESSES is resolved" — while the workflow lookup that mints the ResourceNotFoundException (`crate::state::lookup_miss` / `crate::state::unresolved(UnresolvedEntity::Workflow, ...)`) only runs afterwards, at lines 284-296. Every other ResourceNotFound step of this fixture passes, so the emulator's ResourceNotFoundException body is already correct where it is reached; only UpdateWorkflow never reaches it when the configuration is also unmet.

Why: a client cannot tell "this workflow does not exist" from "your definition is bad". The two errors drive opposite recovery paths: a create-if-missing or reconcile loop keys on ResourceNotFoundException to fall back to CreateWorkflow, and on ValidationException to stop and report a bad request — against the emulator it does the second where AWS does the first, so the workflow is never created and the loop dead-ends. SDK callers catching the typed `ResourceNotFoundException` see it escape as `ValidationException`, and code reading the required `ResourceId`/`ResourceType` members to log or route by resource gets a missing-member error. The defect is reachable with an entirely valid definition too, because any update whose definition the emulator cannot read masks the missing workflow.

Fix: the precedence in UpdateWorkflow's handler (services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/src/ops/update_workflow.rs `handle`) — resolve the addressed workflow in the region first and return the lookup miss before the definition read and the `configuration_dependencies::unmet_dependency` refusal are allowed to decide the response; the modeled-constraint `validate_input` refusal stays first, since AWS still rejects a malformed `WorkflowArn` as a validation failure. The ResourceNotFoundException body itself needs no change.

Traces: mwaa-serverless-resource-not-found-errors#task#3 (UpdateWorkflow, expected 400 / actual 400).

## Evidence
Trace set simparity-wire-aws-v1, fixture `mwaa-serverless-resource-not-found-errors`, step `task#3`, operation UpdateWorkflow, expected status 400 / actual status 400 (status matches; only the body diverges). emulator_up=true (/__admin/health 200). The cluster carried no stub_path and no owned_by.

  $.__type: value_mismatch (expected 'com.amazonaws.mwaaserverless.exceptions#ResourceNotFoundException', got 'ValidationException')
  $.Message: value_mismatch (expected 'Resource not found', got "1 validation error detected: Value at 'DefinitionS3Location' failed to satisfy constraint: Member must address an Amazon S3 object this service can read, and s3://sim2real-infra-000000000000/definitions/missing.yaml is inaccessible")
  $.ResourceId: missing (expected 'sim2real-probe-mwaa-serverless-wf-missing-30316a11-abcdefghij', got None)
  $.ResourceType: missing (expected 'Workflow', got None)

Which parts of the recorded message are requirements: `Resource not found` is fixed text; `Workflow` is the fixed resource-type token for this entity; the recorded ResourceId value and the recorded bucket/key in the emulator's message are per-run values from the request (the bucket name embeds an account id and the workflow identifier embeds a generated suffix) and are not themselves requirements.

Code evidence (services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/src/ops/update_workflow.rs): lines 231-236 read the definition object; lines 248-265 return ValidationException/AccessDeniedException for an unmet configuration dependency, with the comment at lines 243-247 stating the configuration is judged before the addressed workflow is resolved; lines 284-296 are the workflow lookup that would have produced ResourceNotFoundException.

Related open issues that do NOT cover this: 20261109T175019Z-mwaa-serverless-responses-diverge-epoch-timestamps-missing-service.md owns the three other clusters in this replay (CreateWorkflow/ListTaskInstances/GetWorkflow 200-body defects) and carries no UpdateWorkflow error-precedence key; 20261024T002400Z-mwaa-serverless-createworkflow-validation.md is about CreateWorkflow ACCEPTING an unreachable S3 location (the opposite direction, and a different operation); 20261029T185031Z-mwaa-serverless-six-behavioural-divergences-across-createworkflow-validation.md is a behavioral issue about CreateWorkflow validation wording and the run lifecycle. None of them carry this step's replay key.

## Steps to Reproduce
Replay the bound wire corpus against a freshly built emulator: `replay_wire_corpus()` on trace set simparity-wire-aws-v1, fixture `mwaa-serverless-resource-not-found-errors`, step `task#3`.

Equivalently, by hand against the emulator's mwaa-serverless endpoint (awsJson1_0, X-Amz-Target dispatch):
1. Pick a workflow ARN that is syntactically valid (it must satisfy the WorkflowArn pattern) but names a workflow that was never created in this region, e.g. arn:aws:airflow-serverless:us-west-2:000000000000:workflow/<name>-abcdefghij.
2. Call UpdateWorkflow(WorkflowArn=<that ARN>, DefinitionS3Location={Bucket=<an existing bucket>, ObjectKey=<a key that does not exist>}, RoleArn=<any valid role ARN>).
3. Expected: the error about the MISSING WORKFLOW — `__type` ResourceNotFoundException, `Message` "Resource not found", `ResourceType` "Workflow", `ResourceId` the ARN's resource-id segment, at the status the corpus records for this error (400). Actual: `__type` ValidationException with the "Value at 'DefinitionS3Location' failed to satisfy constraint ... is inaccessible" message and no ResourceId/ResourceType.
4. Repeat with a READABLE DefinitionS3Location and the same nonexistent workflow ARN: the emulator then does answer ResourceNotFoundException correctly, which isolates the defect to the order in which the two refusals are decided.
