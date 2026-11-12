Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.get-scaling-configuration-recommendation-endpoint-not-found sagemaker.get-scaling-configuration-recommendation-for-valid-endpoint sagemaker.get-scaling-configuration-recommendation-not-found-for-missing-endpoint sagemaker.get-scaling-configuration-recommendation-unknown-endpoint-not-found sagemaker.scaling-configuration-recommendation-not-found-for-unknown-endpoint sagemaker.get-scaling-configuration-recommendation-not-found sagemaker.model-card-archived-update-rejected sagemaker.cluster-node-batch-lifecycle sagemaker.cluster-node-batch-add-delete-lifecycle
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker.get-scaling-configuration-recommendation-endpoint-not-found|run#9|AssertionError|assert side_effect_code_message == f"Inference recommendations job {bogus_job_name} for {ctx.account} not found.", f"unexpected message: {side_effect_code_messa
Issue-Key: sagemaker.get-scaling-configuration-recommendation-for-valid-endpoint|run#9|AssertionError|assert message == f"Inference recommendations job {bogus_job_name} for {ctx.account} not found.", f"unexpected DescribeInferenceRecommendationsJob message: {mes
Issue-Key: sagemaker.get-scaling-configuration-recommendation-not-found-for-missing-endpoint|run#9|AssertionError|assert describe_code_message == f"Inference recommendations job {missing_job_name} for {ctx.account} not found.", f"unexpected message: {describe_code_message!r
Issue-Key: sagemaker.get-scaling-configuration-recommendation-unknown-endpoint-not-found|run#9|AssertionError|assert describe_code_message == f"Inference recommendations job {fake_job_name} for {ctx.account} not found.", f"unexpected message: {describe_code_message!r}"
Issue-Key: sagemaker.scaling-configuration-recommendation-not-found-for-unknown-endpoint|run#11|AssertionError|assert describe_message == expected_describe, (
Issue-Key: sagemaker.get-scaling-configuration-recommendation-not-found|run#9|AssertionError|assert shape_code_message == (
Issue-Key: sagemaker.model-card-archived-update-rejected|_error#16|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: sagemaker.cluster-node-batch-lifecycle|run#9|AssertionError|assert reboot_resp["Code"] == "ResourceNotFound", (
Issue-Key: sagemaker.cluster-node-batch-add-delete-lifecycle|run#8|AssertionError|assert delete_code == "ResourceNotFound", (
Issue-Commit: e85e90cdff07042376477e1cc3e0643dc4bd2bb8

# SageMaker: inference-recommendation refusal texts, an Archived model card accepting updates, and two unwired HyperPod batch node operations (5 causes)

> _Note: this issue was identified against the code base at commit `e85e90cdff07042376477e1cc3e0643dc4bd2bb8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInferenceRecommendationsJob

## Description
Five net-new root causes on the SageMaker control plane, all surviving the current replay after the owned/auto-filed clusters are excluded. Causes 1 and 2 are the residue of a PARTIAL fix: the open issue 20261029T222441Z-sagemaker-lineage-update-projection-and-scaling-recommendation-paths.md (cause 10) named the GetScalingConfigurationRecommendation refusal surface, and GetScalingConfigurationRecommendation's own not-found text now matches AWS, but the two sibling texts named below are still wrong and that issue's replay keys do not cover the assertions that now fail. Causes 3 is net-new relative to 20261029T222441Z-sagemaker-resourcenotfound-validationexception-refusal-texts-are-emulator-authored.md, which only names the model card NOT-FOUND message and says nothing about the Archived-state gate. Cause 5 shares an operation with an auto-filed scaffold-stub issue for BatchDeleteClusterNodes and is recorded here only so its replay key is carried; closing that stub closes this cause.

In every message template below, the quoted wording is the FIXED text and the variable parts are the job name / cluster name from the request and the caller's account id from the signing context — never the literal ids from a single replay run.

Cause 1 of 5 — DescribeInferenceRecommendationsJob's ResourceNotFound Message omits the account id AWS names.
AWS: Smithy: DescribeInferenceRecommendationsJob input JobName (@required, @length 1..64, @pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,63}$); errors = [ResourceNotFound] (client, HTTP 400); Message target FailureReason (@length max 1024). Recorded behavior: AWS words it "Inference recommendations job <job-name> for <account-id> not found." — fixed text "Inference recommendations job " ... " for " ... " not found.", with the job name taken from the request and the 12-digit account id of the caller.
Emulator: HTTP 400, code ResourceNotFound (both correct), Message "Inference recommendations job <job-name> was not found." — different wording and the account id is dropped entirely.
Why: a client that keys cleanup/polling logic on AWS's wording never matches, and the account qualifier that tells a caller it probed the wrong account is gone, so a cross-account mistake is indistinguishable from a deleted job.
Fix: the describe-inference-recommendations-job handler's not-found refusal rendering in the sagemaker crate's ops (not the status or the code, which are already right).
Traces: sagemaker.get-scaling-configuration-recommendation-endpoint-not-found, sagemaker.get-scaling-configuration-recommendation-for-valid-endpoint, sagemaker.get-scaling-configuration-recommendation-not-found-for-missing-endpoint, sagemaker.get-scaling-configuration-recommendation-unknown-endpoint-not-found, sagemaker.scaling-configuration-recommendation-not-found-for-unknown-endpoint.

Cause 2 of 5 — GetScalingConfigurationRecommendation's missing-target ValidationException uses an emulator-authored one-liner.
AWS: Smithy: GetScalingConfigurationRecommendation input InferenceRecommendationsJobName (@required) plus the optional, mutually-exclusive-but-one-required pair RecommendationId / EndpointName; modeled errors = [ResourceNotFound] only, so the shape refusal is the service-common ValidationException. Recorded behavior: when NEITHER RecommendationId nor EndpointName is supplied, AWS refuses with ValidationException and the two-sentence, variable-free Message "Neither RecommendationId or EndpointName were provided in the input. Please provide a RecommendationId or EndpointName in the request."
Emulator: ValidationException (code correct) with Message "Specify either RecommendationId or EndpointName." — an invented one-sentence text with no variable parts, so the whole string diverges.
Why: this is the only signal distinguishing a request-shape problem from a not-found problem on this operation; a client matching AWS's text treats the refusal as unrecognised and cannot tell it simply failed to send a target field.
Fix: the get-scaling-configuration-recommendation handler's input validation / refusal rendering.
Traces: sagemaker.get-scaling-configuration-recommendation-not-found.

Cause 3 of 5 — UpdateModelCard on an Archived model card is ACCEPTED where AWS refuses it (overly permissive).
AWS: Smithy: UpdateModelCard input ModelCardName (@required), optional Content and ModelCardStatus (enum); errors = [ConflictException, ResourceLimitExceeded, ResourceNotFound]. Recorded behavior: once a card's ModelCardStatus has reached Archived, a further UpdateModelCard is refused with HTTP 400 ValidationException and the fixed, variable-free Message "Model card is in archived state", and the card's status stays Archived. Archived is a terminal status for the card.
Emulator: answers HTTP 200 with {"ModelCardArn": "arn:aws:sagemaker:<region>:<account>:model-card/<name>"} and mutates the Archived card back to Draft — no refusal at all.
Why: Archived is a governance state; a client (or a model-governance audit) relying on it being terminal sees an archived card silently re-opened and re-edited, and gets no error on a write AWS rejects. The over-permissive accept is worse than a wrong message because state is actually corrupted.
Fix: the update-model-card handler's state gate — reject a status transition out of Archived (ValidationException) and leave the stored card unchanged; this is a missing validation plus state guard, not a serializer issue.
Traces: sagemaker.model-card-archived-update-rejected.

Cause 4 of 5 — BatchRebootClusterNodes is an unimplemented scaffold stub, so it is never wired to a cluster lookup.
AWS: Smithy: BatchRebootClusterNodes input ClusterName (@required, ClusterNameOrArn) + NodeIds / NodeLogicalIds; errors = [ResourceNotFound] (client, HTTP 400). Recorded behavior: pointed at a cluster name that does not exist, AWS refuses at the TOP level with ResourceNotFound and Message "Cluster with name <cluster-name> not found" — fixed text with the cluster name from the request and no trailing period (node-level lookup failures go in the response's Failed list instead, which only applies once the cluster exists).
Emulator: HTTP 400 with code NotImplemented ("operation is scaffolded but not implemented"), an error code AWS never returns from this operation and one that is not in its modeled error set.
Why: a HyperPod client cannot distinguish a missing cluster from an unavailable API, and the node-reboot half of the cluster-node lifecycle is unreachable.
Fix: implement the batch-reboot-cluster-nodes handler, wiring ClusterName to the cluster state lookup and rendering the top-level ResourceNotFound above.
Traces: sagemaker.cluster-node-batch-lifecycle.

Cause 5 of 5 — BatchDeleteClusterNodes answers NotImplemented where AWS refuses with the cluster-not-found text.
AWS: Smithy: BatchDeleteClusterNodes input ClusterName (@required) + NodeIds / NodeLogicalIds; errors = [ResourceNotFound] (client, HTTP 400). Recorded behavior: for a cluster name that does not exist, ResourceNotFound with Message "Cluster with name <cluster-name> not found" — same template as cause 4.
Emulator: HTTP 400 NotImplemented. Note the sibling BatchAddClusterNodes on the SAME check already returns the correct ResourceNotFound and the correct message, so only the delete path is unwired. A scaffold-stub issue naming BatchDeleteClusterNodes (services/service-com-amazonaws-sagemaker-sagemaker/src/ops/batch_delete_cluster_nodes.rs) was auto-filed for the sibling check sagemaker.batch-delete-cluster-nodes-not-found; closing that stub also closes this cause, and the replay key below is recorded here only so a fixer re-replays it.
Why: the delete half of the cluster-node lifecycle is unreachable, and the error code is outside the operation's modeled error set.
Fix: implement the batch-delete-cluster-nodes handler and its cluster lookup / refusal rendering.
Traces: sagemaker.cluster-node-batch-add-delete-lifecycle.

## Evidence
Replayed against the emulator (suite simparity-behavioral-aws-v1, 180/449 passed). Representative assertion per cause:

1. sagemaker.scaling-configuration-recommendation-not-found-for-unknown-endpoint: AssertionError: expected message 'Inference recommendations job <job-name> for <account-id> not found.', got 'Inference recommendations job <job-name> was not found.' — same divergence in sagemaker.get-scaling-configuration-recommendation-endpoint-not-found, -for-valid-endpoint, -not-found-for-missing-endpoint and -unknown-endpoint-not-found. HTTP status 400 and code ResourceNotFound matched in every case; only the Message diverged (the account qualifier is absent and "not found." is reworded to "was not found.").

2. sagemaker.get-scaling-configuration-recommendation-not-found: AssertionError: unexpected message: 'Specify either RecommendationId or EndpointName.' — expected 'Neither RecommendationId or EndpointName were provided in the input. Please provide a RecommendationId or EndpointName in the request.' The code (ValidationException) matched and was correctly distinct from the not-found code; only the text diverged.

3. sagemaker.model-card-archived-update-rejected: AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'ModelCardArn': 'arn:aws:sagemaker:<region>:<account>:model-card/<card-name>', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}. The check had just driven the card Draft -> PendingReview -> Approved -> Archived successfully, then issued UpdateModelCard(ModelCardStatus='Draft'); AWS answers 400 ValidationException 'Model card is in archived state' and leaves the card Archived.

4. sagemaker.cluster-node-batch-lifecycle: AssertionError: BatchRebootClusterNodes: expected ResourceNotFound, got NotImplemented. The preceding BatchAddClusterNodes assertion on the same bogus cluster name passed with code ResourceNotFound and Message 'Cluster with name <cluster-name> not found', so the expected shape is demonstrably reachable in this emulator for a sibling operation.

5. sagemaker.cluster-node-batch-add-delete-lifecycle: AssertionError: BatchDeleteClusterNodes: expected ResourceNotFound, got 'NotImplemented' (again after BatchAddClusterNodes answered correctly on the same request).

## Steps to Reproduce
Re-replay the checks listed in trace_ids against the emulator.

Cause 1: call DescribeInferenceRecommendationsJob with a JobName that was never created and compare the ResourceNotFound Message to 'Inference recommendations job <job-name> for <caller-account-id> not found.'
Cause 2: call GetScalingConfigurationRecommendation with only InferenceRecommendationsJobName set — omitting BOTH RecommendationId and EndpointName — and compare the ValidationException Message to the two-sentence AWS text.
Cause 3: CreateModelCard (ModelCardStatus=Draft), UpdateModelCard to PendingReview, then Approved, then Archived (each must succeed), then issue UpdateModelCard(ModelCardStatus='Draft') on the same card: expect HTTP 400 ValidationException 'Model card is in archived state' and DescribeModelCard still reporting ModelCardStatus=Archived.
Cause 4: call BatchRebootClusterNodes with a ClusterName that does not exist and NodeIds=['i-0123456789abcdef0']: expect HTTP 400 ResourceNotFound with Message 'Cluster with name <cluster-name> not found'.
Cause 5: same request shape against BatchDeleteClusterNodes with a nonexistent ClusterName; also confirm DescribeCluster on that name still refuses ResourceNotFound (no cluster conjured as a side effect) and ListClusters(MaxResults=1) still succeeds as the positive control.
