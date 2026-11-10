Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.create-notebook-instance-nonexistent-role-arn-accepted-then-fails sagemaker.create-pipeline-missing-role-not-found
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker|CreateNotebookInstance|ValidationException
Issue-Key: sagemaker|CreatePipeline|ValidationException
Issue-Commit: 7e282e985d147868e376415a13aa29a1f3675b80

# SageMaker refuses creates AWS accepts: CreateNotebookInstance and CreatePipeline reject a well-formed RoleArn naming no existing role with 400 ValidationException instead of accepting and failing later (2 causes)

> _Note: this issue was identified against the code base at commit `7e282e985d147868e376415a13aa29a1f3675b80`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateNotebookInstance

## Description
Two root causes share one shape: a role-assumability guard is applied synchronously in create handlers where AWS does NOT validate assumability at create time, so a request AWS accepts with 200 is refused with 400 ValidationException and the resource/execution that AWS would have created never exists. This is the mirror image of open issue 20261029T192819Z-sagemaker-create-operations-skip-the-execution-role-assumability.md (creates that skip the check): the guard must be applied per operation, not blanket, because for these two operations AWS resolves the role asynchronously, at provisioning / at step execution.

Cause 1 of 2: CreateNotebookInstance.
AWS: recorded behavior only (Smithy sagemaker#CreateNotebookInstance declares exactly one error, ResourceLimitExceeded; RoleArn is only constrained by @length 20..2048 and @pattern ^arn:aws[a-z\\-]*:iam::\\d{12}:role/?[a-zA-Z_0-9+=,.@\\-_/]+$). A RoleArn that satisfies that pattern but names no existing role is ACCEPTED: the call returns 200 with NotebookInstanceArn, and the instance then settles asynchronously at NotebookInstanceStatus 'Failed' with a non-empty FailureReason (DescribeNotebookInstance models both members). The role is only resolved when provisioning runs.
Emulator: the create call itself is refused with 400 ValidationException and the message "Could not assume role <RoleArn>. Please ensure that the role exists and allows principal 'sagemaker.amazonaws.com' to assume the role." — fixed text is everything except <RoleArn>, which is echoed from the request. No notebook instance is created, so there is nothing to describe.
Why: a client that creates a notebook instance and then polls DescribeNotebookInstance for a terminal status gets an exception from the create instead, and can never observe the Failed status + FailureReason contract AWS exposes for an unusable role. Tooling that treats CreateNotebookInstance as accept-then-poll (CloudFormation, the SageMaker console flow, SDK waiters) breaks on a request real AWS admits.
Fix: the CreateNotebookInstance handler's validation stage (services/service-com-amazonaws-sagemaker-sagemaker/src/ops/create_notebook_instance.rs) must not refuse on role existence/assumability; move that resolution into the notebook-instance provisioning state machine, which must land NotebookInstanceStatus 'Failed' with a non-empty FailureReason naming the role.
Traces: sagemaker.create-notebook-instance-nonexistent-role-arn-accepted-then-fails.

Cause 2 of 2: CreatePipeline.
AWS: recorded behavior only (Smithy sagemaker#CreatePipeline declares ConflictException, ResourceLimitExceeded and ResourceNotFound — no assumability refusal; RoleArn carries the same length/pattern constraints only). CreatePipeline and StartPipelineExecution both accept a well-formed RoleArn that names no existing role; the role is resolved by the STEP. The execution then reaches PipelineExecutionStatus 'Failed', the single step reaches StepStatus 'Failed', and the step's FailureReason names the missing role ARN and states that it does not exist; the step's side effect (here, the callback SQS message) never happens.
Emulator: CreatePipeline is refused with 400 ValidationException and the same "Could not assume role <RoleArn>. Please ensure that the role exists and allows principal 'sagemaker.amazonaws.com' to assume the role." text, so no pipeline, no execution and no step-level FailureReason ever exist.
Why: the role-resolution failure is supposed to be observable per step, with the step that needed the role attributed in DescribePipelineExecutionSteps. A client that builds a pipeline whose role is created later (a very common deploy ordering) is blocked outright, and a client asserting on step-level failure attribution has nothing to read.
Fix: the CreatePipeline handler's validation stage (services/service-com-amazonaws-sagemaker-sagemaker/src/ops/create_pipeline.rs) must accept the role ARN on shape alone; role resolution belongs in the pipeline-step executor, which must fail the step and the execution and report the missing role in the step's FailureReason.
Traces: sagemaker.create-pipeline-missing-role-not-found.

## Evidence
Cause 1 — check sagemaker.create-notebook-instance-nonexistent-role-arn-accepted-then-fails:
  botocore.exceptions.ClientError: An error occurred (ValidationException) when calling the CreateNotebookInstance operation: Could not assume role arn:aws:iam::<account-id>:role/<generated-role-name>. Please ensure that the role exists and allows principal 'sagemaker.amazonaws.com' to assume the role.
Expected: HTTP 200 with NotebookInstanceArn, then DescribeNotebookInstance converging to NotebookInstanceStatus 'Failed' with a non-empty FailureReason. Actual: HTTP 400 ValidationException at create; the instance never exists. (The account id and role name above are per-run values; the requirement is the status/code/flow, and the message's fixed text is "Could not assume role <RoleArn>. Please ensure that the role exists and allows principal 'sagemaker.amazonaws.com' to assume the role." — which AWS does not emit for this operation at all.)

Cause 2 — check sagemaker.create-pipeline-missing-role-not-found:
  botocore.exceptions.ClientError: An error occurred (ValidationException) when calling the CreatePipeline operation: Could not assume role arn:aws:iam::<account-id>:role/<generated-role-name>. Please ensure that the role exists and allows principal 'sagemaker.amazonaws.com' to assume the role.
Expected: HTTP 200 with PipelineArn; StartPipelineExecution accepted; DescribePipelineExecution converging to PipelineExecutionStatus 'Failed'; the sole step StepStatus 'Failed' with a FailureReason containing the role ARN and 'does not exist'; no SQS callback message delivered. Actual: HTTP 400 ValidationException at CreatePipeline; no pipeline, execution or step exists.

## Steps to Reproduce
Cause 1:
1. Build a RoleArn of the form arn:aws:iam::<account-id>:role/<name> for a role that was never created (it matches the modeled @pattern).
2. Call CreateNotebookInstance(NotebookInstanceName=<name>, InstanceType='ml.t3.medium', RoleArn=<that arn>).
Observed: 400 ValidationException "Could not assume role ...". Expected: 200, then DescribeNotebookInstance eventually reports NotebookInstanceStatus 'Failed' with a non-empty FailureReason.
Equivalently: run behavioral check sagemaker.create-notebook-instance-nonexistent-role-arn-accepted-then-fails.

Cause 2:
1. Create an SQS queue and a pipeline definition containing a single Callback step targeting that queue.
2. Call CreatePipeline(PipelineName=<name>, ClientRequestToken=<uuid>, RoleArn=<arn of a role that was never created>, PipelineDefinition=<that definition>).
Observed: 400 ValidationException "Could not assume role ...". Expected: 200 with PipelineArn; StartPipelineExecution accepted; the execution and its sole step end 'Failed' with a step FailureReason naming the missing role, and no message is delivered to the queue.
Equivalently: run behavioral check sagemaker.create-pipeline-missing-role-not-found.

## Resolution
Both causes are fixed: no create refuses a well-formed RoleArn for a role nobody holds.

* Cause 1 (CreateNotebookInstance): the create resolves only a role this account's IAM HOLDS whose trust document withholds the SageMaker principal; a role IAM holds nothing under is accepted and the instance settles NotebookInstanceStatus 'Failed' with the FailureReason naming the role.
* Cause 2 (CreatePipeline): the create admits the RoleArn on its modelled shape alone. The role is resolved where a step is performed as it — StartPipelineExecution — and the reason travels on the run, so the execution settles PipelineExecutionStatus 'Failed' and its step reports StepStatus 'Failed' with a FailureReason naming the role and stating that it does not exist.

Still open, and NOT part of these two causes: a pipeline execution performs no step work (no SQS callback message is delivered for a healthy Callback step, no step reaches 'Executing', nothing reaches 'Succeeded'), so the behavioral check sagemaker.create-pipeline-missing-role-not-found still fails on its control pipeline. That step executor is issue 20261024T160943Z-sagemaker-runs-no-job-work-processing-pipeline-step.md (cause 2), which owns this same trace, together with the still-scaffolded SendPipelineExecutionStepSuccess / SendPipelineExecutionStepFailure.
