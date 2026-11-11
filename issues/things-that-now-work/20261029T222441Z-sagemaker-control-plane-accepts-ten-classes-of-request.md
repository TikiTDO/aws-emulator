Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.add-tags-aws-prefixed-key-denied sagemaker.create-training-job-invalid-instance-count-rejected sagemaker.create-training-job-out-of-range-instance-count-rejected sagemaker.create-training-job-out-of-range-volume-size-rejected sagemaker.create-training-job-invalid-role-arn-denied sagemaker.create-cluster-out-of-range-node-count-rejected sagemaker.batch-add-cluster-nodes-wrong-state-rejected sagemaker.delete-algorithm-in-use-conflict sagemaker.delete-algorithm-in-use-rejected sagemaker.model-package-group-policy-byte-roundtrip sagemaker.model-package-group-policy-unicode-byte-round-trip sagemaker.model-package-group-policy-unicode-round-trip sagemaker.model-package-group-policy-invalid-json-rejected sagemaker.model-package-group-policy-malformed-document-rejected sagemaker.put-model-package-group-policy-invalid-json-conflict sagemaker.put-model-package-group-policy-malformed-json-rejected sagemaker.put-model-package-group-policy-invalid-policy-rejected sagemaker.e2e-trial-component-association-nonexistent-trial-rejected
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker.add-tags-aws-prefixed-key-denied|_client_error#5|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: sagemaker.create-training-job-invalid-instance-count-rejected|_error#9|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: sagemaker.create-training-job-out-of-range-instance-count-rejected|_error#9|AssertionError|raise AssertionError(
Issue-Key: sagemaker.create-training-job-out-of-range-volume-size-rejected|_error#6|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: sagemaker.create-training-job-invalid-role-arn-denied|_error#7|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: sagemaker.create-cluster-out-of-range-node-count-rejected|_error#4|AssertionError|raise AssertionError("expected the call to raise a ClientError, it succeeded")
Issue-Key: sagemaker.batch-add-cluster-nodes-wrong-state-rejected|_error_response#17|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: sagemaker.delete-algorithm-in-use-conflict|_error_code_and_message#13|AssertionError|raise AssertionError(
Issue-Key: sagemaker.delete-algorithm-in-use-rejected|run#11|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}: {msg}"
Issue-Key: sagemaker.model-package-group-policy-byte-roundtrip|run#19|AssertionError|raise AssertionError(
Issue-Key: sagemaker.model-package-group-policy-unicode-byte-round-trip|run#16|AssertionError|raise AssertionError(
Issue-Key: sagemaker.model-package-group-policy-unicode-round-trip|_put_error#14|AssertionError|raise AssertionError("expected PutModelPackageGroupPolicy to raise for a non-ASCII policy, but it succeeded")
Issue-Key: sagemaker.model-package-group-policy-invalid-json-rejected|run#10|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: sagemaker.model-package-group-policy-malformed-document-rejected|run#11|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: sagemaker.put-model-package-group-policy-invalid-json-conflict|run#11|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: sagemaker.put-model-package-group-policy-malformed-json-rejected|run#8|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}"
Issue-Key: sagemaker.put-model-package-group-policy-invalid-policy-rejected|_error#8|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: sagemaker.e2e-trial-component-association-nonexistent-trial-rejected|_error#21|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Commit: 066b7cda09b446bd4bd9be1c3ecfcbe16940769e

# SageMaker control plane accepts ten classes of request real AWS refuses (10 causes)

> _Note: this issue was identified against the code base at commit `066b7cda09b446bd4bd9be1c3ecfcbe16940769e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTrainingJob

## Description
Ten independent validation gates are missing or mis-coded: the emulator answers 200 (or the wrong error code) where real AWS refuses. These are over-permissive, so a client's negative test passes against the emulator and fails in production. This issue is net-new relative to the open issues 20261029T192819Z-sagemaker-create-paths-accept-references-and-documents-aws.md and 20261029T192819Z-sagemaker-delete-and-stop-paths-lack-not-found.md, neither of which carries the replay keys below.

Cause 1 of 10 — AddTags accepts a reserved "aws:"-prefixed tag key.
AWS: Smithy: AddTags models no operation errors, so this is the service-common ValidationException. Recorded behavior: AWS reserves the "aws:" tag-key prefix and refuses the call with 400 ValidationException; the key is never stored.
Emulator: answers HTTP 200 and echoes Tags=[{Key:'aws:<suffix>', Value:...}], storing a reserved key.
Why: tag-governance code that relies on AWS rejecting reserved keys silently writes them.
Fix: the add-tags handler's tag-key validation.
Traces: sagemaker.add-tags-aws-prefixed-key-denied.

Cause 2 of 10 — CreateTrainingJob accepts an invalid ResourceConfig.InstanceCount.
AWS: Smithy: ResourceConfig.InstanceCount targets TrainingInstanceCount (@range min 0); AWS additionally requires a launchable count and refuses an invalid one with 400 ValidationException naming the member.
Emulator: answers 200 with a TrainingJobArn.
Why: a mis-sized job is accepted and only discovered later (or never).
Fix: the create-training-job handler's ResourceConfig validation.
Traces: sagemaker.create-training-job-invalid-instance-count-rejected.

Cause 3 of 10 — CreateTrainingJob accepts an out-of-range InstanceCount.
AWS: as cause 2: a count beyond the service limit is refused 400 (ValidationException / ResourceLimitExceeded is the modeled limit error).
Emulator: answers 200 with a TrainingJobArn.
Why: quota-handling paths are never exercised.
Fix: the create-training-job handler's ResourceConfig range/limit validation.
Traces: sagemaker.create-training-job-out-of-range-instance-count-rejected.

Cause 4 of 10 — CreateTrainingJob accepts an out-of-range ResourceConfig.VolumeSizeInGB.
AWS: Smithy: VolumeSizeInGB is @range min 1 (ResourceConfig.VolumeSizeInGB targets OptionalVolumeSizeInGB, @range min 0); a value outside the admissible EBS range is refused 400 ValidationException naming the member.
Emulator: answers 200 with a TrainingJobArn.
Why: an unprovisionable volume size is accepted.
Fix: the create-training-job handler's ResourceConfig validation.
Traces: sagemaker.create-training-job-out-of-range-volume-size-rejected.

Cause 5 of 10 — CreateTrainingJob accepts a RoleArn the caller cannot use.
AWS: Smithy: CreateTrainingJob.RoleArn is @required, pattern ^arn:aws[a-z\\-]*:iam::\\d{12}:role/?[a-zA-Z_0-9+=,.@\\-_/]+$. Recorded behavior: AWS refuses a role it cannot resolve/assume with 400 ValidationException naming the role.
Emulator: accepts the call (the check recorded a success).
Why: an unusable execution role is never surfaced at submit time.
Fix: the create-training-job handler's RoleArn validation. The open issue 20261029T192819Z-sagemaker-create-operations-skip-the-execution-role-assumability.md names the assumability gate generally but does not carry this replay key.
Traces: sagemaker.create-training-job-invalid-role-arn-denied.

Cause 6 of 10 — CreateCluster accepts an out-of-range instance-group node count.
AWS: Smithy: ClusterInstanceCount is @range min 0 max 6758; a count outside that is refused 400 ValidationException.
Emulator: the call succeeds and a cluster is minted.
Why: a HyperPod cluster that can never be provisioned is accepted.
Fix: the create-cluster handler's instance-group validation.
Traces: sagemaker.create-cluster-out-of-range-node-count-rejected.

Cause 7 of 10 — BatchAddClusterNodes is accepted while the cluster is in a state that forbids it.
AWS: Smithy: BatchAddClusterNodes errors = [ResourceLimitExceeded, ResourceNotFound]; recorded behavior shows a cluster not in a node-add-admitting state refuses the call rather than queueing nodes.
Emulator: answers 200 with Successful=[{NodeLogicalId:..., Status:'Pending', ...}], Failed=[].
Why: the caller believes nodes were queued on a cluster that cannot take them.
Fix: the batch-add-cluster-nodes handler's cluster-state gate.
Traces: sagemaker.batch-add-cluster-nodes-wrong-state-rejected.

Cause 8 of 10 — DeleteAlgorithm succeeds while the algorithm is still in Pending/InProgress validation.
AWS: Smithy: DeleteAlgorithm errors = [ConflictException] (client, HTTP 400). Recorded behavior: the refusal Message is "Cannot delete Algorithm in <AlgorithmStatus> status." — fixed text with the current status from state.
Emulator: answers 200 with an empty body and tears the in-flight resource down.
Why: an algorithm is destroyed mid-validation and the caller's conflict-handling branch is dead code.
Fix: the delete-algorithm handler's status gate.
Traces: sagemaker.delete-algorithm-in-use-conflict, sagemaker.delete-algorithm-in-use-rejected.

Cause 9 of 10 — PutModelPackageGroupPolicy accepts a non-ASCII ResourcePolicy.
AWS: Smithy: ResourcePolicy targets PolicyString, @required, @length 1..20480. Recorded behavior: AWS requires an ASCII-only policy document and refuses a non-ASCII one with 400 ValidationException.
Emulator: accepts and stores the non-ASCII document.
Why: a policy AWS will never accept round-trips, so the client ships an unusable document.
Fix: the put-model-package-group-policy handler's policy-document validation.
Traces: sagemaker.model-package-group-policy-byte-roundtrip, sagemaker.model-package-group-policy-unicode-byte-round-trip, sagemaker.model-package-group-policy-unicode-round-trip.

Cause 10 of 10 — PutModelPackageGroupPolicy mis-codes a malformed policy, and accepts a structurally invalid one.
AWS: a syntactically malformed or structurally invalid policy document is a request-shaping failure: 400 ValidationException. ConflictException (the only operation-modeled error) is for the group-state conflict, not a bad document. Related: ListTrialComponents with a TrialName that names no trial is refused ValidationException, not answered with an empty page.
Emulator: returns error code 'ConflictException' for malformed JSON, and for one structurally invalid document answers 200 with a ModelPackageGroupArn; ListTrialComponents against a nonexistent TrialName answers 200 with TrialComponentSummaries=[].
Why: the caller cannot distinguish "your document is broken" (never retry) from "the group is busy" (retry), and a typo'd trial name reads as an empty-but-valid trial.
Fix: the put-model-package-group-policy handler's document parsing/classification, and the list-trial-components handler's trial-existence gate.
Traces: sagemaker.model-package-group-policy-invalid-json-rejected, sagemaker.model-package-group-policy-malformed-document-rejected, sagemaker.put-model-package-group-policy-invalid-json-conflict, sagemaker.put-model-package-group-policy-malformed-json-rejected, sagemaker.put-model-package-group-policy-invalid-policy-rejected, sagemaker.e2e-trial-component-association-nonexistent-trial-rejected.

## Evidence
Replayed against the emulator, representative assertion per cause:

1. sagemaker.add-tags-aws-prefixed-key-denied: real AWS rejects this call, but it SUCCEEDED and returned {'Tags': [{'Key': 'aws:sim2real-reserved', 'Value': 'blocked'}], ... 'HTTPStatusCode': 200}
2. sagemaker.create-training-job-invalid-instance-count-rejected: expected a rejection, but the call SUCCEEDED and returned {'TrainingJobArn': '<arn>', ... 'HTTPStatusCode': 200}
3. sagemaker.create-training-job-out-of-range-instance-count-rejected: expected the call to be rejected, but it SUCCEEDED and returned {'TrainingJobArn': '<arn>', ... 200}
4. sagemaker.create-training-job-out-of-range-volume-size-rejected: expected a rejection, but the call SUCCEEDED and returned {'TrainingJobArn': '<arn>', ... 200}
5. sagemaker.create-training-job-invalid-role-arn-denied: expected a rejection, but the call SUCCEEDED and returned None
6. sagemaker.create-cluster-out-of-range-node-count-rejected: expected the call to raise a ClientError, it succeeded
7. sagemaker.batch-add-cluster-nodes-wrong-state-rejected: expected the call to be rejected, but it succeeded: {'Successful': [{'NodeLogicalId': '...', 'InstanceGroupName': 'workers', 'Status': 'Pending', 'InstanceTypes': ['ml.t3.medium']}], 'Failed': [], ... 200}
8. sagemaker.delete-algorithm-in-use-conflict: expected the call to be rejected, but it succeeded and returned {... 'HTTPStatusCode': 200, 'content-length': '2'}; sagemaker.delete-algorithm-in-use-rejected: expected ValidationException, got None (expected message 'Cannot delete Algorithm in <status> status.')
9. sagemaker.model-package-group-policy-unicode-byte-round-trip: PutModelPackageGroupPolicy accepted a non-ASCII policy document; real AWS rejects this with ValidationException (policy strings must be ASCII-only)
10. sagemaker.model-package-group-policy-invalid-json-rejected: expected ValidationException, got 'ConflictException'; sagemaker.put-model-package-group-policy-invalid-policy-rejected: expected a ClientError, but the call SUCCEEDED and returned {'ModelPackageGroupArn': '<arn>', ... 200}; sagemaker.e2e-trial-component-association-nonexistent-trial-rejected: expected a ClientError, but the call succeeded and returned {'TrialComponentSummaries': [], ... 200}

## Steps to Reproduce
Re-replay the checks in trace_ids. Cause 1: AddTags on any SageMaker ARN with a tag key beginning "aws:". Causes 2-4: CreateTrainingJob with an invalid/out-of-range ResourceConfig.InstanceCount or VolumeSizeInGB. Cause 5: CreateTrainingJob with a RoleArn naming a role that does not exist. Cause 6: CreateCluster with an instance-group count outside ClusterInstanceCount's 0..6758 range. Cause 7: BatchAddClusterNodes against a cluster that is not in a node-add-admitting state. Cause 8: CreateAlgorithm, then DeleteAlgorithm while DescribeAlgorithm still reports Pending/InProgress. Cause 9: PutModelPackageGroupPolicy with a non-ASCII ResourcePolicy. Cause 10: PutModelPackageGroupPolicy with malformed JSON and with a structurally invalid policy; ListTrialComponents with a TrialName that names no trial.
