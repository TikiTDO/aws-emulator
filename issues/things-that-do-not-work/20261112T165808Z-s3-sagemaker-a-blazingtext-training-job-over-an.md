Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__sagemaker.sagemaker-training-job-s3-input-manifest-format-positive
Issue-Key: s3__sagemaker.sagemaker-training-job-s3-input-manifest-format-positive|run#13|AssertionError|assert "train:accuracy" in metrics, (
Issue-Commit: 116dfabd29b018288b31e7394f72aeff79ed3e64

# s3__sagemaker: a BlazingText training job over an AugmentedManifestFile/Pipe-mode channel reports Completed with an EMPTY FinalMetricDataList, so no algorithm metric (train:accuracy) is ever reported

> _Note: this issue was identified against the code base at commit `116dfabd29b018288b31e7394f72aeff79ed3e64`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTrainingJob

## Description
One net-new root cause in the SageMaker training data plane, observed on an AugmentedManifestFile input channel in Pipe mode. The report does not mark this check as OWNED by any open issue; the nearest open issue, 20261108T162008Z-s3-sagemaker-8-net-new-causes-sagemaker-job.md (Cause 7), describes FinalMetricDataList being populated with a FIXED CANNED PAIR of metric names (train:rmse/validation:rmse) unrelated to the algorithm — it does not cover this observation, where the list comes back completely EMPTY for an augmented-manifest/Pipe job. Filing it separately and naming that issue.

=== Cause 1 of 1: a Completed training job on an AugmentedManifestFile (Pipe-mode) channel emits no metrics at all ===
AWS: Smithy (services/service-com-amazonaws-sagemaker-sagemaker/smithy-model/com-amazonaws-sagemaker-sagemaker.json) models S3DataSource.S3DataType with the value AugmentedManifestFile — "S3Uri identifies an object that is an augmented manifest file in JSON lines format. This file contains the data you want to use for model training. AugmentedManifestFile can only be used if the Channel's input mode is Pipe" — and S3DataSource.AttributeNames as "A list of one or more attribute names to use that are found in a specified augmented manifest file" (@length 0..16). DescribeTrainingJob.FinalMetricDataList is modelled as a list of MetricData (@length 0..40), each being "The name, value, and date and time of a metric that was emitted to Amazon CloudWatch" — i.e. the metrics the training container actually emitted, named from the algorithm's own metric definitions and its input channel names. Recorded behavior: a built-in BlazingText (mode=supervised) job whose single `train` channel is an AugmentedManifestFile in Pipe mode with AttributeNames ["source", "label"] parses the manifest's JSON lines, maps those two attributes into training records, runs, and reaches TrainingJobStatus=Completed with FinalMetricDataList containing at least the entry named `train:accuracy` (MetricName is the algorithm metric prefixed by the channel name; its Value and Timestamp come from the run). Nothing in the model or the recording says the list may be empty for a Completed job of this shape.
Emulator: the job is accepted, DescribeTrainingJob settles on TrainingJobStatus=Completed with no FailureReason, but FinalMetricDataList is absent/empty — the check reads `got []`. So the emulator reports a successful training run over an augmented manifest while emitting no metric whatsoever; the canned metric pair the emulator does synthesize for other job shapes is not produced here at all, which indicates the AugmentedManifestFile/Pipe input path produces no records to train on.
Why: a client that selects, tunes or gates a deployment on a named final metric (the whole point of a supervised BlazingText run) finds an empty list behind a Completed status, so it cannot distinguish "trained successfully" from "trained on zero records"; any pipeline that keys on FinalMetricDataList[].MetricName == 'train:accuracy' breaks, and label/attribute mapping bugs in an augmented manifest are invisible because the emulator reports success regardless.
Fix: the training-job execution/metrics path behind CreateTrainingJob — the input reader for S3DataType=AugmentedManifestFile with TrainingInputMode=Pipe (including AttributeNames mapping and RecordWrapperType=RecordIO handling) must build records from the manifest's JSON lines, and the job's emitted metrics must be projected into DescribeTrainingJob.FinalMetricDataList with the algorithm's channel-prefixed metric names. A job that produced no metrics must not be reported as Completed with an empty list.
Traces: s3__sagemaker.sagemaker-training-job-s3-input-manifest-format-positive.

## Evidence
Failing check s3__sagemaker.sagemaker-training-job-s3-input-manifest-format-positive [mismatch]:

  File "check_s3__sagemaker_sagemaker_training_job_s3_input_manifest_format_positive.py", line 110, in run
    assert "train:accuracy" in metrics, (
  AssertionError: expected train:accuracy from records built out of the manifest's mapped attributes, got []

The preceding assertion (`desc["TrainingJobStatus"] == "Completed"`) passed, so the emulator reported the job Completed with no FailureReason and then returned FinalMetricDataList == [] (metrics dict built as {m["MetricName"]: m["Value"] for m in desc.get("FinalMetricDataList", [])}).

Request shape that produced it (values such as bucket/job/role names are per-run fillers, not requirements):
  CreateTrainingJob
    AlgorithmSpecification={"TrainingImage": <region's built-in blazingtext:1 image>, "TrainingInputMode": "Pipe"}
    InputDataConfig=[{"ChannelName": "train",
                      "DataSource": {"S3DataSource": {"S3DataType": "AugmentedManifestFile",
                                                      "S3Uri": "s3://<bucket>/manifest/train.jsonl",
                                                      "S3DataDistributionType": "FullyReplicated",
                                                      "AttributeNames": ["source", "label"]}},
                      "RecordWrapperType": "RecordIO"}]
    HyperParameters={"mode": "supervised", "epochs": "5", "min_count": "1", "word_ngrams": "1", "vector_dim": "10"}
  with 60 JSON lines of the form {"source": "<three words>", "label": 1|2} at manifest/train.jsonl.

Expected (recorded AWS): TrainingJobStatus=Completed and FinalMetricDataList containing a MetricData entry whose MetricName is "train:accuracy" (channel name + algorithm metric), with a Value and Timestamp from the run.
Actual (emulator): TrainingJobStatus=Completed, FinalMetricDataList empty.

## Steps to Reproduce
1. Replay the behavioral check listed in trace_ids against the emulator (run_behavioral_suite).
2. Manual repro: create a bucket and put an augmented manifest at manifest/train.jsonl whose lines are JSON objects {"source": "<text>", "label": <int>} (60 lines is enough).
3. Create a SageMaker execution role trusting sagemaker.amazonaws.com with s3:GetObject/s3:ListBucket on the bucket and s3:PutObject on output/*.
4. Call CreateTrainingJob with AlgorithmSpecification={TrainingImage: <built-in blazingtext:1 image for the region>, TrainingInputMode: "Pipe"}, a single channel named "train" whose S3DataSource has S3DataType=AugmentedManifestFile, S3Uri=s3://<bucket>/manifest/train.jsonl, AttributeNames=["source","label"], RecordWrapperType="RecordIO", OutputDataConfig.S3OutputPath=s3://<bucket>/output/, and HyperParameters mode=supervised.
5. Poll DescribeTrainingJob until terminal. Expect TrainingJobStatus=Completed AND FinalMetricDataList to contain MetricName "train:accuracy"; the emulator returns Completed with FinalMetricDataList empty.
