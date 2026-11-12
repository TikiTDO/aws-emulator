Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__sagemaker.sagemaker-autopilot-job-writes-candidates-to-s3 s3__sagemaker.sagemaker-batch-transform-and-realtime-endpoint-agree-on-every-row s3__sagemaker.sagemaker-endpoint-config-model-data-url-resolution s3__sagemaker.sagemaker-endpoint-deploy-from-a-deleted-model-artifact-fails-with-an-s3-failure-reason s3__sagemaker.sagemaker-endpoint-invocation-output-captured-to-s3
Issue-Key: s3__sagemaker.sagemaker-autopilot-job-writes-candidates-to-s3|run#19|AssertionError|assert model_data_urls, f"best candidate {best.get('CandidateName')!r} has no InferenceContainers with a ModelDataUrl: {best['InferenceContainers']}"
Issue-Key: s3__sagemaker.sagemaker-batch-transform-and-realtime-endpoint-agree-on-every-row|run#40|AssertionError|assert xform_desc["TransformJobStatus"] == "Completed", (
Issue-Key: s3__sagemaker.sagemaker-endpoint-config-model-data-url-resolution|run#31|AssertionError|assert 0.0 <= prob_high <= 1.0, f"predicted probability out of range: {prob_high}"
Issue-Key: s3__sagemaker.sagemaker-endpoint-deploy-from-a-deleted-model-artifact-fails-with-an-s3-failure-reason|run#43|AssertionError|assert broken["EndpointStatus"] == "Failed", (
Issue-Key: s3__sagemaker.sagemaker-endpoint-invocation-output-captured-to-s3|_poll_until#30|Timeout|time.sleep(interval_s)
Issue-Commit: c3c8a5d5636f74138d44709d078c46bc0a5a7c2e

# s3__sagemaker: 5 net-new causes — Autopilot BestCandidate carries no InferenceContainers, a transform job runs no inference container, InvokeEndpoint returns a score outside [0,1], CreateEndpoint deploys a model whose S3 artifact is gone, and endpoint data capture never lands

> _Note: this issue was identified against the code base at commit `c3c8a5d5636f74138d44709d078c46bc0a5a7c2e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAutoMLJobV2 / DescribeTransformJob / CreateEndpoint / InvokeEndpoint

## Description
Five independent root causes in the SageMaker job/inference surface exercised through S3. They are bundled because they all belong to the s3__sagemaker service heading; each must be closed separately. Numbered below: Cause 1 of 5 .. Cause 5 of 5.

--------------------------------------------------------------------
Cause 1 of 5: DescribeAutoMLJobV2 reports a Completed AutoML job whose BestCandidate has an EMPTY InferenceContainers list.
AWS: Smithy (com.amazonaws.sagemaker): AutoMLCandidate.InferenceContainers -> AutoMLContainerDefinitions, @length min 0 max 5, documented as "Information about the recommended inference container definitions"; each AutoMLContainerDefinition has REQUIRED members Image (@pattern ^[\S]+$, max 255) and ModelDataUrl (@pattern ^(https|s3)://([^/]+)/?(.*)$, max 1024) plus optional Environment. For a tabular ENSEMBLING job that reaches AutoMLJobStatus Completed, AWS populates BestCandidate.InferenceContainers with at least one definition whose ModelDataUrl points at a model artifact the candidate-generation pipeline actually wrote under OutputDataConfig.S3OutputPath, so the candidate can be deployed with CreateModel without re-deriving anything.
Emulator: the job settles Completed and AutoMLJobArtifacts.CandidateDefinitionNotebookLocation / DataExplorationNotebookLocation resolve to existing S3 objects, but BestCandidate.InferenceContainers is serialized as `[]`. Replay detail: "AssertionError: best candidate '<candidate-name>' has no InferenceContainers with a ModelDataUrl: []" (the candidate name is a per-run generated value; the empty list is the requirement violation).
Why: a client that follows the documented Autopilot deploy path — describe the job, read BestCandidate.InferenceContainers[0].{Image,ModelDataUrl}, pass them to CreateModel — gets an IndexError/KeyError instead of a deployable container, so no Autopilot model can ever be promoted to an endpoint. The job also claims Completed while the candidate-generation pipeline produced no inference artifact, which makes a non-executing job indistinguishable from a successful one.
Fix: the DescribeAutoMLJobV2 response projection for BestCandidate (and the AutoML job state that backs it) must carry the candidate's inference container definitions — Image and a ModelDataUrl under the job's OutputDataConfig.S3OutputPath — rather than an empty list.
Traces: s3__sagemaker.sagemaker-autopilot-job-writes-candidates-to-s3

--------------------------------------------------------------------
Cause 2 of 5: a transform job never completes — DescribeTransformJob reports a FailureReason authored by the emulator saying it runs no inference container.
AWS: recorded behavior only for the execution itself (the model does not describe container execution). AWS runs the model's inference container over TransformInput, writes one output object per input record under TransformOutput.S3OutputPath, and settles TransformJobStatus at Completed. TransformJobStatus is a modeled enum whose Completed value is the success terminal state; FailureReason is only populated on Failed.
Emulator: the job settles non-Completed with FailureReason verbatim "This emulator runs no model inference container, so no records were transformed and no output object was written to the job's S3OutputPath." That text is entirely emulator-authored; AWS never emits a FailureReason that describes the emulator. Replay detail: "AssertionError: the batch half did not complete: \"This emulator runs no model inference container, so no records were transformed and no output object was written to the job's S3OutputPath.\""
Why: any batch-inference consumer blocks forever or sees a bogus failure; and because the batch half never produces rows, a client cannot compare batch and real-time predictions at all. Note: open issue 20261109T020211Z-s3-sagemaker-2-net-new-causes-a-transform.md describes a transform job running no inference container, but it does not carry this check's replay key, so this cause is filed here and that issue is named for cross-reference, not treated as covering it.
Fix: the CreateTransformJob/DescribeTransformJob execution path (the transform job runner and the state it settles) must perform the inference and write the per-record output objects, and must never publish an emulator-authored FailureReason.
Traces: s3__sagemaker.sagemaker-batch-transform-and-realtime-endpoint-agree-on-every-row

--------------------------------------------------------------------
Cause 3 of 5: InvokeEndpoint on an InService XGBoost endpoint returns a score OUTSIDE the probability range the model was trained to emit.
AWS: recorded behavior only. A SageMaker built-in/framework XGBoost model trained with objective=binary:logistic serves, for text/csv input, a single value in the closed interval [0, 1] per row — it is a probability. AWS obtains it by loading the weights from the model's S3 ModelDataUrl and running the container; the value therefore also varies monotonically with the input (a high-feature row scores above a low-feature row).
Emulator: the endpoint reaches InService under the EndpointConfig and InvokeEndpoint answers 200, but the body carries a value greater than 1. Replay detail: "AssertionError: predicted probability out of range: 1.1539694316302571". The specific number is a per-run value; the requirement is that the served score lie in [0, 1] for a binary:logistic model, which proves the weights at ModelDataUrl were actually loaded instead of a synthesized number.
Why: a client that thresholds the response (score > 0.5), feeds it into a calibration step, or validates it as a probability gets arithmetically impossible values; and because the number is not derived from the artifact at ModelDataUrl, EndpointConfig/Model ModelDataUrl resolution is unverifiable through the data plane.
Fix: the sagemaker-runtime InvokeEndpoint inference path (and the model-artifact resolution that feeds it) must produce its response from the artifact named by the deployed Model's ModelDataUrl and keep a binary:logistic score within [0, 1].
Traces: s3__sagemaker.sagemaker-endpoint-config-model-data-url-resolution

--------------------------------------------------------------------
Cause 4 of 5: CreateEndpoint brings an endpoint to InService even though the Model's S3 model artifact has been deleted.
AWS: Smithy: EndpointStatus is an enum including Creating, InService and Failed; DescribeEndpoint.FailureReason is populated when the status is Failed. AWS fetches the artifact at the Model's PrimaryContainer.ModelDataUrl while bringing the endpoint up; when that S3 object does not exist the endpoint settles EndpointStatus=Failed and FailureReason names the S3 failure (the inaccessible/absent model data location). It never reports InService.
Emulator: DescribeEndpoint reports EndpointStatus 'InService' for an endpoint whose model artifact object was deleted from the bucket before deployment. Replay detail: "AssertionError: an endpoint whose model artifact was deleted must fail to deploy rather than come up serving something: 'InService'".
Why: the emulator is more permissive than AWS on a safety-relevant path — a broken deployment reports healthy, so a client never sees the S3 failure it must handle, and anything the endpoint then serves is not backed by any model artifact. Rollback/alarm logic keyed on EndpointStatus=Failed never fires.
Fix: the CreateEndpoint deployment path must resolve the referenced Model's ModelDataUrl against S3 and settle EndpointStatus=Failed with a FailureReason naming the missing S3 model-data location instead of reporting InService.
Traces: s3__sagemaker.sagemaker-endpoint-deploy-from-a-deleted-model-artifact-fails-with-an-s3-failure-reason

--------------------------------------------------------------------
Cause 5 of 5: an endpoint with DataCaptureConfig never publishes capture objects, so the invocation-capture flow does not converge.
AWS: Smithy: DataCaptureConfig requires InitialSamplingPercentage (@range 0-100), DestinationS3Uri (@pattern ^(https|s3)://([^/])/?(.*)$, max 512) and CaptureOptions (@length 1-32), and is documented as "Configuration to control how SageMaker AI captures inference data". With capture enabled, AWS writes capture files under the endpoint's name beneath DestinationS3Uri, and a capture record pairs the invocation's endpointInput with the matching endpointOutput.
Emulator: the check never reaches its capture assertions — it exhausts its whole budget. Replay detail: "Error: Check timed out after 60.0s" (the 60 s figure is the harness budget, not a requirement). Hypothesis: no capture object ever appears beneath DestinationS3Uri, so the poll for capture files never terminates; the preceding legs of the same flow (training job, model, endpoint) are the ones covered by the already-open S3+SageMaker job-execution issues, and this cause is specifically the capture leg.
Why: Model Monitor and any audit consumer that reads capture files to join request and response legs gets nothing, and the client cannot distinguish "capture disabled" from "capture configured but silently dropped". Open issue 20261029T203709Z-s3-sagemaker-training-jobs-never-execute-no-model.md owns two neighbouring data-capture checks but not this one's replay key.
Fix: the endpoint data-plane capture path (DataCaptureConfig handling on CreateEndpointConfig/CreateEndpoint and the InvokeEndpoint capture writer) must write capture objects under the endpoint name beneath DestinationS3Uri, each pairing endpointInput with endpointOutput.
Traces: s3__sagemaker.sagemaker-endpoint-invocation-output-captured-to-s3

## Evidence
Behavioral suite: 1704/2549 passed (66.8%); 680 mismatched, 165 errored. 820 of the 825 listed failing checks are marked ALREADY FILED or OWNED; these 5 are the net-new ones, all listed under the `### s3__sagemaker` heading.

1) s3__sagemaker.sagemaker-autopilot-job-writes-candidates-to-s3 [mismatch]
   Expected: Completed AutoML V2 job -> BestCandidate.InferenceContainers holds >=1 AutoMLContainerDefinition with a ModelDataUrl (required member).
   Actual: AssertionError: best candidate '<candidate-name>' has no InferenceContainers with a ModelDataUrl: []
   (AutoMLJobArtifacts.CandidateDefinitionNotebookLocation and DataExplorationNotebookLocation did resolve, so only the candidate containers are missing.)

2) s3__sagemaker.sagemaker-batch-transform-and-realtime-endpoint-agree-on-every-row [mismatch]
   Expected: TransformJobStatus == "Completed" with one output object per input record under TransformOutput.S3OutputPath.
   Actual: AssertionError: the batch half did not complete: "This emulator runs no model inference container, so no records were transformed and no output object was written to the job's S3OutputPath."
   (FailureReason text is emulator-authored; AWS never words a FailureReason this way.)

3) s3__sagemaker.sagemaker-endpoint-config-model-data-url-resolution [mismatch]
   Expected: InvokeEndpoint on an InService binary:logistic XGBoost endpoint returns a score within [0, 1] (and the high-input score exceeds the low-input score).
   Actual: AssertionError: predicted probability out of range: 1.1539694316302571

4) s3__sagemaker.sagemaker-endpoint-deploy-from-a-deleted-model-artifact-fails-with-an-s3-failure-reason [mismatch]
   Expected: EndpointStatus == "Failed" with a FailureReason naming the absent S3 model-data object.
   Actual: AssertionError: an endpoint whose model artifact was deleted must fail to deploy rather than come up serving something: 'InService'

5) s3__sagemaker.sagemaker-endpoint-invocation-output-captured-to-s3 [mismatch]
   Expected: capture files under the endpoint's name beneath DataCaptureConfig.DestinationS3Uri, one pairing endpointInput with an endpointOutput score in [0, 1].
   Actual: Error: Check timed out after 60.0s — the capture poll never observes an object.

Smithy grounding (com.amazonaws.sagemaker):
  AutoMLCandidate.InferenceContainers -> AutoMLContainerDefinitions, @length 0..5; AutoMLContainerDefinition.Image and .ModelDataUrl are both required (ModelDataUrl @pattern ^(https|s3)://([^/]+)/?(.*)$, max 1024).
  EndpointStatus enum: OutOfService | Creating | Updating | SystemUpdating | RollingBack | InService | Deleting | Failed | UpdateRollbackFailed.
  DataCaptureConfig: required InitialSamplingPercentage (@range 0..100), DestinationS3Uri (@pattern ^(https|s3)://([^/])/?(.*)$, max 512), CaptureOptions (@length 1..32).

## Steps to Reproduce
Prerequisite: git_clone(), then run_behavioral_suite() — the five checks below are replayed against the booted emulator.

Cause 1 — Autopilot candidate containers:
 1. s3:CreateBucket; PutObject a tabular CSV at automl-input/train.csv; create an execution role that trusts sagemaker.amazonaws.com.
 2. CreateAutoMLJobV2 with that S3Uri as the training channel, OutputDataConfig.S3OutputPath=s3://<bucket>/automl-output/, AutoMLProblemTypeConfig.TabularJobConfig {TargetAttributeName:target, Mode:ENSEMBLING, ProblemType:BinaryClassification, CompletionCriteria.MaxCandidates:3}, AutoMLJobObjective.MetricName=F1.
 3. Poll DescribeAutoMLJobV2 until AutoMLJobStatus is terminal; assert Completed.
 4. Read BestCandidate.InferenceContainers — observe `[]` instead of >=1 definition carrying Image + ModelDataUrl.

Cause 2 — transform job never completes:
 1. Train (or register) a model, then CreateTransformJob with TransformInput pointing at a seeded CSV prefix and TransformOutput.S3OutputPath in the same bucket.
 2. Poll DescribeTransformJob — observe a non-Completed terminal status with the emulator-authored FailureReason "This emulator runs no model inference container, …", and no output object under S3OutputPath.

Cause 3 — out-of-range inference score:
 1. Train an XGBoost model (objective=binary:logistic), CreateModel over its S3 model artifact, CreateEndpointConfig, CreateEndpoint; wait for InService.
 2. sagemaker-runtime InvokeEndpoint with a text/csv feature row; parse the returned score — observe a value > 1.0.

Cause 4 — deleted artifact still deploys:
 1. Train a model and note PrimaryContainer.ModelDataUrl; CreateModel over it.
 2. s3:DeleteObject the model.tar.gz at that ModelDataUrl.
 3. CreateEndpointConfig + CreateEndpoint over that Model and poll DescribeEndpoint — observe EndpointStatus 'InService' instead of 'Failed' with an S3 FailureReason.

Cause 5 — data capture never lands:
 1. CreateEndpointConfig with DataCaptureConfig {EnableCapture:true, InitialSamplingPercentage:100, DestinationS3Uri:s3://<bucket>/capture/, CaptureOptions:[Input,Output]}; CreateEndpoint and wait for InService.
 2. InvokeEndpoint once, then ListObjectsV2 under s3://<bucket>/capture/<endpoint-name>/ — observe no capture object ever appears; the check exhausts its budget ("Check timed out after 60.0s").
