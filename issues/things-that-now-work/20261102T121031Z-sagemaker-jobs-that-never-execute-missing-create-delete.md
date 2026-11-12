Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.automl-job-server-side-orchestration-progress sagemaker.endpoint-lifecycle-creating-to-inservice-to-gone sagemaker.create-ai-benchmark-job-lifecycle sagemaker.e2e-ai-workload-config-delete-succeeds-while-benchmark-job-references-it sagemaker.create-model-package-duplicate-name-version-conflict-rejected sagemaker.delete-algorithm-conflict-while-referenced-accepted sagemaker.delete-algorithm-in-use-rejected sagemaker.partner-app-presigned-url-expires-after-ttl sagemaker.batch-replace-cluster-nodes-stale-reference-rejected sagemaker.notebook-lifecycle-config-applied-server-side
Issue-Key: sagemaker.automl-job-server-side-orchestration-progress|run#22|AssertionError|assert final_status == "Completed", f"expected the AutoML job to complete, got {final_status}"
Issue-Key: sagemaker.endpoint-lifecycle-creating-to-inservice-to-gone|run#16|AssertionError|assert training_desc["TrainingJobStatus"] == "Completed", training_desc.get("FailureReason")
Issue-Key: sagemaker.create-ai-benchmark-job-lifecycle|_error#8|AssertionError|raise AssertionError(f"AWS refuses this call, but it succeeded and returned {result!r}")
Issue-Key: sagemaker.e2e-ai-workload-config-delete-succeeds-while-benchmark-job-references-it|run#42|AssertionError|assert running_delete_code == "ValidationException", (
Issue-Key: sagemaker.create-model-package-duplicate-name-version-conflict-rejected|run#10|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: sagemaker.delete-algorithm-conflict-while-referenced-accepted|run#27|AssertionError|assert job_described["AlgorithmSpecification"]["AlgorithmName"] == algo_referenced_arn, (
Issue-Key: sagemaker.delete-algorithm-in-use-rejected|_error_code_and_message#10|AssertionError|raise AssertionError(f"expected a rejection, but the call succeeded and returned {result!r}")
Issue-Key: sagemaker.partner-app-presigned-url-expires-after-ttl|run#8|AssertionError|assert message == expected, (
Issue-Key: sagemaker.batch-replace-cluster-nodes-stale-reference-rejected|_wait_for_node_presence#13|Timeout|time.sleep(2)
Issue-Key: sagemaker.notebook-lifecycle-config-applied-server-side|run#17|Timeout|time.sleep(2)
Issue-Commit: 54ea3114e7f16ccf9b82e09febe15fda2a85bac2

# SageMaker: jobs that never execute, missing create/delete-state gates, emulator-authored refusal texts, an unresolved AlgorithmName, a 501 DeleteAIBenchmarkJob, and two never-converging lifecycles (10 causes)

> _Note: this issue was identified against the code base at commit `54ea3114e7f16ccf9b82e09febe15fda2a85bac2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAutoMLJobV2 (also DescribeTrainingJob, CreateAIBenchmarkJob, DeleteAIBenchmarkJob, CreateModelPackage, DeleteAlgorithm, CreatePartnerApp, BatchReplaceClusterNodes, CreateNotebookInstance)

## Description
Ten net-new root causes in the sagemaker service. None of these ten failing checks was marked ALREADY FILED or OWNED by the behavioral replay, so none carries an existing replay key. A fixer must close all ten before submitting.

Cause 1 of 10: an accepted CreateAutoMLJobV2 is never orchestrated — the job ends Failed instead of Completed.
Failing check id: sagemaker.automl-job-server-side-orchestration-progress.
  AWS:      An accepted CreateAutoMLJobV2 progresses autonomously server-side through candidate generation with no further client action; repeated DescribeAutoMLJobV2 shows AutoMLJobStatus advancing to "Completed" (with AutoMLJobSecondaryStatus moving through its phases). Smithy: CreateAutoMLJobV2 input requires AutoMLJobName, AutoMLJobInputDataConfig, OutputDataConfig, AutoMLProblemTypeConfig and RoleArn, and returns AutoMLJobArn; its modeled errors are only ResourceInUse and ResourceLimitExceeded, i.e. an accepted job is expected to run. Recorded behavior only for the terminal state: the recording reaches AutoMLJobStatus "Completed".
  Emulator: the create is accepted, then DescribeAutoMLJobV2 settles on AutoMLJobStatus "Failed". No candidate generation happens.
  Why:      A client polling for Completed never gets it and cannot read BestCandidate or deploy the resulting model; worse, it is told the job FAILED for a request AWS would have completed, so the emulator inverts the verdict of every AutoML positive scenario.
  Fix:      The CreateAutoMLJobV2 handler plus the AutoML job state machine that advances a job to its terminal state: an accepted job must progress to Completed and populate the describe response's candidate/best-candidate members rather than settling Failed.
  Traces:   sagemaker.automl-job-server-side-orchestration-progress. Open issues 20261024T160943Z-sagemaker-runs-no-job-work-processing-pipeline-step.md and 20261030T171442Z-sagemaker-server-side-job-execution-is-absent-training.md describe the same family but the report did not attribute this check to either.

Cause 2 of 10: an accepted CreateTrainingJob is never executed, blocking the endpoint lifecycle.
Failing check id: sagemaker.endpoint-lifecycle-creating-to-inservice-to-gone.
  AWS:      A CreateTrainingJob accepted with a resolvable training image and reachable S3 input/output runs the algorithm container and reaches TrainingJobStatus "Completed", with ModelArtifacts.S3ModelArtifacts naming an artifact that exists. Smithy: TrainingJobStatus enum {InProgress, Completed, Failed, Stopping, Stopped}; FailureReason is set only for a job that genuinely failed. Recorded behavior only for the terminal Completed.
  Emulator: the job settles TrainingJobStatus "Failed" with FailureReason exactly "This emulator has no container runtime, so the training job's algorithm image was never started and no model artifacts were written to the output S3 location" — emulator-authored prose describing the server's own implementation, which is not an AWS failure-reason class; AWS's failure reasons name the algorithm, container or permission condition that failed.
  Why:      Every endpoint scenario that must first train a model (Creating -> InService -> serves an inference -> DeleteEndpoint -> refused describe) is unreachable, and a client branching on FailureReason sees text matching no AWS failure class.
  Fix:      The training-job state machine behind CreateTrainingJob/DescribeTrainingJob: progress an accepted job to Completed, publish a real artifact URI in ModelArtifacts.S3ModelArtifacts, and never place emulator-implementation prose in FailureReason.
  Traces:   sagemaker.endpoint-lifecycle-creating-to-inservice-to-gone. Related open issue: 20261029T203709Z-sagemaker-an-accepted-createtrainingjob-is-never-executed-the.md (the report did not attribute this check to it).

Cause 3 of 10: CreateAIBenchmarkJob accepts a BenchmarkTarget naming an endpoint that does not exist.
Failing check id: sagemaker.create-ai-benchmark-job-lifecycle.
  AWS:      CreateAIBenchmarkJob resolves its BenchmarkTarget before creating anything; when the target names a SageMaker endpoint that does not exist the call is refused with a 400 ValidationException whose message is "SageMaker endpoint '<name>' does not exist" — fixed text "SageMaker endpoint '" ... "' does not exist", with <name> the endpoint name taken from the request's BenchmarkTarget. No job is created, so a following DescribeAIBenchmarkJob is refused with ResourceNotFound "AIBenchmarkJob '<job>' not found." (<job> from the request). Smithy: CreateAIBenchmarkJob requires AIBenchmarkJobName, BenchmarkTarget, OutputConfig, AIWorkloadConfigIdentifier and RoleArn and models ResourceNotFound (400) among its errors, i.e. an unresolvable target is a modeled refusal.
  Emulator: the call succeeds with HTTP 200 and returns an AIBenchmarkJobArn (arn:aws:sagemaker:<region>:<account>:ai-benchmark-job/<name>) for a target endpoint that was never created.
  Why:      A client that mistypes or races its endpoint name is told the benchmark job was created; it then polls a job that can never produce results, and the negative guardrail test passes vacuously. The emulator is more permissive than AWS.
  Fix:      The CreateAIBenchmarkJob handler's input validation: resolve BenchmarkTarget's endpoint against endpoint state and refuse with ValidationException before any job state is written.
  Traces:   sagemaker.create-ai-benchmark-job-lifecycle.

Cause 4 of 10: DeleteAIBenchmarkJob answers 501 NotImplemented instead of the modeled refusal/success.
Failing check id: sagemaker.e2e-ai-workload-config-delete-succeeds-while-benchmark-job-references-it.
  AWS:      DeleteAIBenchmarkJob on a job still InProgress is refused with a 400 ValidationException (the job must reach a terminal state first); on a deletable job it returns the job's AIBenchmarkJobArn. Smithy: DeleteAIBenchmarkJob input AIBenchmarkJobName (required), output AIBenchmarkJobArn, modeled error ResourceNotFound (400) — the operation is modeled as a real, answerable call, never a 501.
  Emulator: the call returns error code "NotImplemented" (HTTP 501), so neither the state gate nor the success path exists.
  Why:      A 501 is not in the operation's modeled error set; a client cannot distinguish "wrong state" from "operation absent", botocore surfaces an unmodelled code, and the whole AI-workload-config/benchmark-job teardown flow is unusable.
  Fix:      services/service-com-amazonaws-sagemaker-sagemaker/src/ops/delete_ai_benchmark_job.rs — replace the scaffold stub with the state gate (ValidationException while the job is InProgress) and the success path returning AIBenchmarkJobArn.
  Traces:   sagemaker.e2e-ai-workload-config-delete-succeeds-while-benchmark-job-references-it. Open issue 20261024T160943Z-sagemaker-control-plane-gaps-createuserprofile-and-deleteaibenchmarkjob-are.md names this operation, but the replay did not attribute this check to it, so the InProgress-state gate asserted here is not carried by any existing replay key.

Cause 5 of 10: the duplicate-CreateModelPackage refusal message is emulator-authored and omits the existing package's ARN.
Failing check id: sagemaker.create-model-package-duplicate-name-version-conflict-rejected.
  AWS:      CreateModelPackage with a ModelPackageName that already exists is refused with the message "ModelPackage already exists: <model-package-arn>" — fixed text "ModelPackage already exists: " followed by the ARN of the ALREADY-EXISTING package, taken from the emulator's own state (arn:aws:sagemaker:<region>:<account>:model-package/<name>); the original package is left unchanged and a different name still gets its own ARN. Smithy: CreateModelPackage.ModelPackageName target EntityName @length 1..63 @pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$; the duplicate is a modeled client refusal (ConflictException/ValidationException, httpError 400).
  Emulator: the message is 'Cannot create already existing model package "<name>".' — different fixed text, and it substitutes the bare name where AWS puts the resource ARN.
  Why:      Clients and SDK add-ons that parse the conflict message to recover the colliding resource's ARN (to describe or reuse the existing version) get a name they cannot use as an ARN; log-scraping and idempotency logic keyed on AWS's wording never matches.
  Fix:      The CreateModelPackage handler's duplicate-name refusal path (its error message construction), which must emit AWS's fixed text and interpolate the existing package's ARN from state.
  Traces:   sagemaker.create-model-package-duplicate-name-version-conflict-rejected.

Cause 6 of 10: DescribeTrainingJob echoes AlgorithmSpecification.AlgorithmName as the bare name instead of resolving it to the algorithm's ARN.
Failing check id: sagemaker.delete-algorithm-conflict-while-referenced-accepted.
  AWS:      When a training job is created with AlgorithmSpecification.AlgorithmName given as a name, SageMaker stores and reports the resolved algorithm ARN: DescribeTrainingJob returns AlgorithmSpecification.AlgorithmName == arn:aws:sagemaker:<region>:<account>:algorithm/<name>, and it keeps reporting that ARN even after the algorithm itself is deleted (DescribeAlgorithm then answering ValidationException "Algorithm <arn> does not exist."). Smithy: AlgorithmSpecification.AlgorithmName targets ArnOrName (@length 1..170, @pattern accepting an optional arn:...:sagemaker:...:<type>/ prefix), i.e. the member legitimately carries an ARN on the response side.
  Emulator: DescribeTrainingJob returns the bare name the request supplied (e.g. 'sim2real-behav-algo-ref-<id>'), never the ARN form.
  Why:      A client that uses the describe response to construct a cross-account/cross-region reference, to compare against an ARN it recorded at create time, or to audit which algorithm a job used after that algorithm was deleted, gets a value that is not an ARN and cannot be matched or re-resolved.
  Fix:      The CreateTrainingJob handler's algorithm-reference normalization (resolve a name to the algorithm ARN when the job is stored) and the DescribeTrainingJob serializer that emits AlgorithmSpecification.
  Traces:   sagemaker.delete-algorithm-conflict-while-referenced-accepted.

Cause 7 of 10: DeleteAlgorithm is accepted with 200 while the algorithm is still in Pending/InProgress validation.
Failing check id: sagemaker.delete-algorithm-in-use-rejected.
  AWS:      DeleteAlgorithm on an algorithm whose AlgorithmStatus is Pending or InProgress is refused with error code ValidationException and the exact message "Cannot delete Algorithm in <status> status." — fixed text "Cannot delete Algorithm in " ... " status.", with <status> the algorithm's AlgorithmStatus at the moment of the delete, read from state (Pending or InProgress). The refused delete leaves the algorithm describable; once validation reaches Completed the same delete succeeds, after which DescribeAlgorithm answers ValidationException "Algorithm <algorithm-arn> does not exist." (ARN from state). Smithy: DeleteAlgorithm input AlgorithmName (required), output Unit, modeled error ConflictException (400); the recorded refusal for the in-validation state is a 400 ValidationException.
  Emulator: the call returns HTTP 200 with an empty body while the algorithm is mid-validation — no state gate at all.
  Why:      A client can tear down an algorithm whose validation is still running; in AWS that request is refused, so cleanup/retry loops written against AWS (which retry the refusal until it clears) behave differently, and the emulator silently destroys a resource AWS protects.
  Fix:      The DeleteAlgorithm handler's state gate: reject with ValidationException naming the current AlgorithmStatus whenever that status is Pending or InProgress, and leave the resource intact.
  Traces:   sagemaker.delete-algorithm-in-use-rejected.

Cause 8 of 10: the CreatePartnerApp marketplace-subscription refusal message is emulator-authored prose.
Failing check id: sagemaker.partner-app-presigned-url-expires-after-ttl.
  AWS:      CreatePartnerApp for a partner app Type the account is not subscribed to in AWS Marketplace is refused with error code ValidationException and the exact message "Caller is not subscribed to the marketplace offering." — the whole string is fixed text with no interpolated values; no app is created, so no presigned URL can exist. Smithy: CreatePartnerApp requires Name, Type (enum PartnerAppType), ExecutionRoleArn, Tier and AuthType; the entitlement refusal is the service-common ValidationException (400).
  Emulator: same code but a different message: "This account does not have an active AWS Marketplace subscription for the SageMaker Partner AI App type comet. Subscribe to the partner application in AWS Marketplace before creating the app." — invented fixed text that additionally interpolates the requested Type.
  Why:      The message is a requirement of this error class; a client or test that matches AWS's wording to distinguish an entitlement failure from other validation failures never matches, so subscription-gated code paths are misclassified.
  Fix:      The CreatePartnerApp handler's marketplace-entitlement refusal path (its error message construction) must emit AWS's fixed text verbatim.
  Traces:   sagemaker.partner-app-presigned-url-expires-after-ttl.

Cause 9 of 10: the HyperPod Continuous-provisioning node lifecycle never converges, so the BatchReplaceClusterNodes stale-reference scenario cannot complete.
Failing check id: sagemaker.batch-replace-cluster-nodes-stale-reference-rejected.
  AWS:      A Slurm/Continuous HyperPod cluster created with a Controller and a Compute instance group reaches ClusterStatus InService; BatchAddClusterNodes returns a NodeLogicalId that becomes visible and Running via ListClusterNodes, BatchDeleteClusterNodes removes it from ListClusterNodes, and BatchReplaceClusterNodes naming only that deleted NodeLogicalId is refused as a whole with ValidationException and the exact message "All provided nodes do not exist. Please check the input and try again." (entirely fixed text), leaving no node under the stale id while a fresh BatchAddClusterNodes still succeeds. Smithy: BatchReplaceClusterNodes takes ClusterName plus NodeIds/NodeLogicalIds (@length 1..25) and models ResourceNotFound (400); its response carries Successful/Failed/SuccessfulNodeLogicalIds/FailedNodeLogicalIds.
  Emulator: the check never reaches the BatchReplaceClusterNodes assertion — the run hangs and is killed by the harness ("Check timed out after 60.0s"). The cluster/node state machine never reaches the state the next call waits on (cluster InService, then the added node present and Running, then absent after delete), so the whole scenario is unreachable.
  Why:      Any client that polls DescribeCluster/ListClusterNodes for convergence blocks forever instead of proceeding, and the stale-reference refusal semantics of BatchReplaceClusterNodes can never be observed.
  Fix:      The HyperPod cluster/node state machine behind CreateCluster, BatchAddClusterNodes, BatchDeleteClusterNodes and ListClusterNodes (its status progression), plus the BatchReplaceClusterNodes handler's all-nodes-missing refusal path.
  Traces:   sagemaker.batch-replace-cluster-nodes-stale-reference-rejected. Open issue 20261030T171442Z-sagemaker-hyperpod-and-notebook-paths-diverge-batchdeleteclusternodes-mis.md covers a similar family but the report did not attribute this check to it.

Cause 10 of 10: a notebook instance's attached lifecycle config OnCreate hook is never run server-side, and the notebook never converges.
Failing check id: sagemaker.notebook-lifecycle-config-applied-server-side.
  AWS:      CreateNotebookInstance with LifecycleConfigName set runs the config's OnCreate script server-side before the instance reaches NotebookInstanceStatus InService, and the script's stdout appears in the platform's CloudWatch Logs stream for that notebook instance — i.e. the config is executed, not merely stored. Recorded behavior only: the recording observes the OnCreate marker in the logs stream after the instance reaches InService.
  Emulator: the check hangs and is killed by the harness ("Check timed out after 60.0s"): the notebook instance never reaches the state the poll waits on and no OnCreate output is ever emitted to logs.
  Why:      A client that waits for InService blocks forever, and any workflow that depends on bootstrap performed by an OnCreate hook (package install, config drop) silently has no effect while the API reports the config attached.
  Fix:      The CreateNotebookInstance handler and the notebook-instance state machine (status progression to InService) plus the lifecycle-config execution step that must emit the OnCreate script output to the notebook's CloudWatch Logs stream.
  Traces:   sagemaker.notebook-lifecycle-config-applied-server-side. Related open issue: 20261030T171442Z-sagemaker-hyperpod-and-notebook-paths-diverge-batchdeleteclusternodes-mis.md (not attributed to this check by the report).

## Evidence
Cause 1 (sagemaker.automl-job-server-side-orchestration-progress)
  Expected: DescribeAutoMLJobV2.AutoMLJobStatus == "Completed" after autonomous server-side progress.
  Actual:   AutoMLJobStatus == "Failed".
  Replay detail: AssertionError: expected the AutoML job to complete, got Failed

Cause 2 (sagemaker.endpoint-lifecycle-creating-to-inservice-to-gone)
  Expected: DescribeTrainingJob.TrainingJobStatus == "Completed", FailureReason absent.
  Actual:   "Failed" with FailureReason == "This emulator has no container runtime, so the training job's algorithm image was never started and no model artifacts were written to the output S3 location".
  Replay detail: AssertionError: This emulator has no container runtime, so the training job's algorithm image was never started and no model artifacts were written to the output S3 location

Cause 3 (sagemaker.create-ai-benchmark-job-lifecycle)
  Expected: HTTP 400, Error.Code "ValidationException", Message "SageMaker endpoint '<name>' does not exist" (<name> from the request's BenchmarkTarget); no job created.
  Actual:   HTTP 200 with {'AIBenchmarkJobArn': 'arn:aws:sagemaker:<region>:<account>:ai-benchmark-job/<name>'}.
  Replay detail: AssertionError: AWS refuses this call, but it succeeded and returned {'AIBenchmarkJobArn': 'arn:aws:sagemaker:us-west-2:000000000000:ai-benchmark-job/sim2real-behav-abj-d7863d1eedac', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}} (the region/account/name shown are this run's values, not requirements)

Cause 4 (sagemaker.e2e-ai-workload-config-delete-succeeds-while-benchmark-job-references-it)
  Expected: DeleteAIBenchmarkJob on an InProgress job -> HTTP 400, Error.Code "ValidationException".
  Actual:   Error.Code "NotImplemented" (HTTP 501).
  Replay detail: AssertionError: expected DeleteAIBenchmarkJob on an InProgress job to be rejected with ValidationException, got 'NotImplemented'

Cause 5 (sagemaker.create-model-package-duplicate-name-version-conflict-rejected)
  Expected message: "ModelPackage already exists: <arn of the existing package>".
  Actual message:   'Cannot create already existing model package "<name>".'
  Replay detail: AssertionError: expected message 'ModelPackage already exists: arn:aws:sagemaker:us-west-2:000000000000:model-package/sim2real-mp-7e659ea9968e', got 'Cannot create already existing model package "sim2real-mp-7e659ea9968e".' (the ARN/name shown are this run's values; the fixed text and the ARN-vs-name substitution are the requirement)

Cause 6 (sagemaker.delete-algorithm-conflict-while-referenced-accepted)
  Expected: DescribeTrainingJob.AlgorithmSpecification.AlgorithmName == the algorithm's ARN (arn:aws:sagemaker:<region>:<account>:algorithm/<name>), still reported after the algorithm is deleted.
  Actual:   the bare algorithm name.
  Replay detail: AssertionError: expected the training job's own AlgorithmSpecification to still name the deleted algorithm's ARN, got 'sim2real-behav-algo-ref-5b90c12945cb'

Cause 7 (sagemaker.delete-algorithm-in-use-rejected)
  Expected: DeleteAlgorithm while AlgorithmStatus in {Pending, InProgress} -> HTTP 400, Error.Code "ValidationException", Message "Cannot delete Algorithm in <status> status."; the algorithm stays describable.
  Actual:   HTTP 200, empty body ({}), algorithm deleted.
  Replay detail: AssertionError: expected a rejection, but the call succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ... 'content-length': '2' ...}}

Cause 8 (sagemaker.partner-app-presigned-url-expires-after-ttl)
  Expected: Error.Code "ValidationException", Message exactly "Caller is not subscribed to the marketplace offering." (all fixed text).
  Actual:   "This account does not have an active AWS Marketplace subscription for the SageMaker Partner AI App type comet. Subscribe to the partner application in AWS Marketplace before creating the app."
  Replay detail: AssertionError: expected the marketplace-subscription ValidationException 'Caller is not subscribed to the marketplace offering.', got: 'This account does not have an active AWS Marketplace subscription for the SageMaker Partner AI App type comet. ...'

Cause 9 (sagemaker.batch-replace-cluster-nodes-stale-reference-rejected)
  Expected: cluster reaches InService; added node becomes present and Running in ListClusterNodes; after BatchDeleteClusterNodes it disappears; BatchReplaceClusterNodes on the stale NodeLogicalId -> ValidationException "All provided nodes do not exist. Please check the input and try again."; a fresh BatchAddClusterNodes still succeeds.
  Actual:   the scenario never progresses past its convergence polling.
  Replay detail: Error: Check timed out after 60.0s

Cause 10 (sagemaker.notebook-lifecycle-config-applied-server-side)
  Expected: notebook instance with LifecycleConfigName reaches InService and the OnCreate script's echoed marker appears in the notebook's CloudWatch Logs stream.
  Actual:   the scenario never progresses past its convergence polling; no OnCreate output is ever emitted.
  Replay detail: Error: Check timed out after 60.0s

## Steps to Reproduce
Cause 1: CreateAutoMLJobV2 with a valid tabular input in S3, OutputDataConfig, AutoMLProblemTypeConfig and an execution role; poll DescribeAutoMLJobV2 to a terminal status -> "Failed" instead of "Completed".

Cause 2: create a bucket + training input + execution role, CreateTrainingJob with the region's XGBoost image, poll DescribeTrainingJob -> "Failed" with the no-container-runtime FailureReason, so the follow-on CreateModel/CreateEndpointConfig/CreateEndpoint lifecycle never starts.

Cause 3: CreateAIWorkloadConfig, then CreateAIBenchmarkJob whose BenchmarkTarget names an endpoint that was never created -> 200 with an AIBenchmarkJobArn instead of a 400 ValidationException.

Cause 4: create an AI workload config and an AI benchmark job, then call DeleteAIBenchmarkJob while DescribeAIBenchmarkJob still reports InProgress -> 501 NotImplemented instead of 400 ValidationException.

Cause 5: CreateModelPackage with a ModelPackageName N and a valid InferenceSpecification; call CreateModelPackage again with the same N -> compare the refusal Message against "ModelPackage already exists: <arn-of-N>".

Cause 6: CreateAlgorithm A; CreateTrainingJob with AlgorithmSpecification.AlgorithmName="A" (bare name); DescribeTrainingJob -> AlgorithmSpecification.AlgorithmName is "A" instead of A's ARN. DeleteAlgorithm A and describe the job again: still the bare name.

Cause 7: CreateAlgorithm with a resolvable built-in-algorithm image; while DescribeAlgorithm reports AlgorithmStatus Pending or InProgress, call DeleteAlgorithm -> 200 instead of 400 ValidationException "Cannot delete Algorithm in <status> status."; DescribeAlgorithm afterwards shows the algorithm is gone.

Cause 8: ListPartnerApps (control, succeeds), then CreatePartnerApp with Type="comet", a valid ExecutionRoleArn, Tier and AuthType, for an account with no Marketplace subscription -> compare the ValidationException Message against "Caller is not subscribed to the marketplace offering.".

Cause 9: CreateCluster (Orchestrator=Slurm, NodeProvisioningMode="Continuous", a Controller group and a Compute group with SlurmConfig); poll DescribeCluster for InService, then BatchAddClusterNodes and poll ListClusterNodes for the node to appear and be Running -> the polling never converges and the run is killed at the harness timeout, so BatchDeleteClusterNodes/BatchReplaceClusterNodes are never reached.

Cause 10: CreateNotebookInstanceLifecycleConfig with a base64 OnCreate script that echoes a marker; CreateNotebookInstance referencing it; poll DescribeNotebookInstance for InService and read the notebook's CloudWatch Logs stream for the marker -> the polling never converges and no marker is ever logged.
