Issue-Source: trace-sweep-divergence
Issue-Trace: emr-run-job-flow-step-writes-s3-output
Issue-Step: task#21
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: emr-run-job-flow-step-writes-s3-output elasticmapreduce-release-label-and-instance-type-catalog
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: elasticmapreduce|ListSteps|2xx->2xx|$.Steps[*].AdditionalInfo
Issue-Key: elasticmapreduce|ListSteps|2xx->2xx|$.Steps[*].Status.Timeline.CreationDateTime
Issue-Key: elasticmapreduce|ListSteps|2xx->2xx|$.Steps[*].Status.Timeline.EndDateTime
Issue-Key: elasticmapreduce|ListSteps|2xx->2xx|$.Steps[*].Status.Timeline.StartDateTime
Issue-Key: elasticmapreduce|DescribeStep|2xx->2xx|$.Step.AdditionalInfo
Issue-Key: elasticmapreduce|DescribeStep|2xx->2xx|$.Step.Status.Timeline.CreationDateTime
Issue-Key: elasticmapreduce|DescribeStep|2xx->2xx|$.Step.Status.Timeline.EndDateTime
Issue-Key: elasticmapreduce|DescribeStep|2xx->2xx|$.Step.Status.Timeline.StartDateTime
Issue-Key: elasticmapreduce|DescribeCluster|2xx->2xx|$.Cluster.Ec2InstanceAttributes.RequestedEc2SubnetIds[*]
Issue-Key: elasticmapreduce|DescribeCluster|2xx->2xx|$.Cluster.Status.Timeline.CreationDateTime
Issue-Key: elasticmapreduce|DescribeCluster|2xx->2xx|$.Cluster.Status.Timeline.EndDateTime
Issue-Key: elasticmapreduce|DescribeCluster|2xx->2xx|$.Cluster.Status.Timeline.ReadyDateTime
Issue-Key: elasticmapreduce|DescribeReleaseLabel|2xx->2xx|$.AvailableOSReleases[*]
Issue-Related: 20261102T083234Z-elasticmapreduce-emr-cluster-and-instance-group-reads-omit-modeled.md
Issue-Commit: 59f14ace25ed5753b46a36483d50334ce01f6026

# EMR 200 read paths serve incomplete step and cluster projections and a stale OS-release catalog

> _Note: this issue was identified against the code base at commit `59f14ace25ed5753b46a36483d50334ce01f6026`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListSteps

## Description
Successful EMR reads answer 200 with partially-populated bodies: the step and cluster projections drop members AWS always emits for a step/cluster that has run, their lifecycle timestamps are serialized in a form the recorded timestamp shape rejects, and the Amazon Linux release catalog served for a release label is a short stale hard-coded list rather than the catalog AWS serves.

THIS ISSUE BUNDLES 4 CAUSES. All four must be closed.

Cause 1 of 4: ListSteps omits Steps[].AdditionalInfo and serializes the step timeline in a non-AWS form.
  AWS: 200 with each StepSummary carrying the step's execution metadata. AdditionalInfo is recorded behavior only (it is not a member of com.amazonaws.emr#StepSummary in the on-disk model) and AWS returns it for a step that reached the YARN resource manager, as a map with ResourceManager = "YARN" and ResourceManagerApplicationIds = the list of YARN application ids the step produced. Smithy: StepSummary.Status -> StepStatus.Timeline -> StepTimeline{CreationDateTime, StartDateTime, EndDateTime}, each targeting com.amazonaws.emr#Date (timestamp); EMR is awsJson1_1, so a Date is epoch seconds. Hypothesis: the recorded shape is AWS's epoch-seconds value at millisecond precision; the corpus does not assert the instant itself (expected is '<shaped-not-asserted>'), so the per-run clock value is not what diverges here.
  Emulator: 200, but $.Steps[0].AdditionalInfo is absent entirely, and the three Timeline members are emitted as bare floats with seven fractional digits, which the timestamp-shape assertion rejects.
  Why: a client reading the YARN application id of a submitted step (the normal way to correlate an EMR step with the YARN/Spark history it produced) finds no AdditionalInfo at all and cannot correlate; a client parsing Timeline with an AWS SDK sees sub-millisecond precision AWS never emits.
  Fix: ListSteps' serializer (step projection: populate AdditionalInfo from the step's resource-manager state) and the shared EMR timestamp serializer for Date members.
  Traces: emr-run-job-flow-step-writes-s3-output#task#21.

Cause 2 of 4: DescribeStep omits Step.AdditionalInfo and has the same timeline serialization.
  AWS: 200 with the Step structure carrying AdditionalInfo (recorded behavior only; not a member of com.amazonaws.emr#Step in the on-disk model) as ResourceManager = "YARN" plus ResourceManagerApplicationIds, and Status.Timeline's CreationDateTime/StartDateTime/EndDateTime (Smithy: StepTimeline, Date) in AWS's epoch-seconds form.
  Emulator: 200 with $.Step.AdditionalInfo absent and the three Timeline members again emitted as seven-fraction-digit floats that fail the timestamp-shape assertion.
  Why: the single-step read is the path a client polls after AddJobFlowSteps/RunJobFlow, so the same correlation data and timestamp shape are missing on the read clients use most; it is a separate handler from ListSteps and must be re-replayed separately.
  Fix: DescribeStep's serializer (step projection) and the shared Date serializer.
  Traces: emr-run-job-flow-step-writes-s3-output#task#22.

Cause 3 of 4: DescribeCluster drops the requested subnet and the cluster's defaulted members, and serializes the cluster timeline the same way.
  AWS: 200 with the Cluster structure fully populated. Smithy: Cluster.Ec2InstanceAttributes -> Ec2InstanceAttributes.RequestedEc2SubnetIds (XmlStringMaxLen256List) echoes the subnet identifier(s) the cluster was launched into, so a cluster created with an Ec2SubnetId reports that same subnet id there; Cluster.KerberosAttributes (KerberosAttributes) is serialized as an empty structure when Kerberos is not configured; Cluster.Status.Timeline -> ClusterTimeline{CreationDateTime, ReadyDateTime, EndDateTime} are Date timestamps. StepExecutionMode ('STANDARD') and SystemLogUploadDisabled (false) are recorded behavior only - neither is a member of the on-disk Cluster shape - and AWS returns both on every cluster read.
  Emulator: 200, but $.Cluster.Ec2InstanceAttributes.RequestedEc2SubnetIds is empty/absent where the cluster was launched into a subnet created earlier in the same fixture, $.Cluster.KerberosAttributes, $.Cluster.StepExecutionMode and $.Cluster.SystemLogUploadDisabled are all absent, and Status.Timeline.CreationDateTime/ReadyDateTime/EndDateTime are the same seven-fraction-digit floats that fail the shape assertion.
  Why: a client that reconciles where its cluster actually landed (RequestedEc2SubnetIds) gets an empty answer, and code that dereferences KerberosAttributes, StepExecutionMode or SystemLogUploadDisabled faults on a null AWS never returns.
  Fix: DescribeCluster's serializer (default-member population) plus RunJobFlow's cluster state, which must persist the requested subnet id into Ec2InstanceAttributes.
  Traces: emr-run-job-flow-step-writes-s3-output#task#23.

Cause 4 of 4: DescribeReleaseLabel serves a stale, truncated AvailableOSReleases catalog.
  AWS: 200 with AvailableOSReleases listing every Amazon Linux release available for the requested ReleaseLabel, newest first - on the order of fifty entries for a current release label. Smithy: DescribeReleaseLabelOutput.AvailableOSReleases -> OSReleaseList of OSRelease{Label}; the Label format is the Amazon Linux release form (recorded behavior for the catalog contents, since the model only constrains the shape).
  Emulator: 200 with exactly three hard-coded entries, all years older than the catalog the recording shows (the first three labels served are 2.0.20220218.1, 2.0.20230808.0, 2.0.20231116.0) and every later index absent. The list is the wrong catalog rather than one wrong field, and it does not vary with the requested release label.
  Why: a client that picks an OSReleaseLabel for a RunJobFlow request from this list can only ever choose a years-old Amazon Linux release, and a client asserting that a given OS release is available for its EMR release label is told it is not.
  Fix: DescribeReleaseLabel's handler/state - the per-release-label OS-release catalog backing it.
  Traces: elasticmapreduce-release-label-and-instance-type-catalog#task#2.

Related but NOT covering: 20261102T083234Z-elasticmapreduce-emr-cluster-and-instance-group-reads-omit-modeled.md owns the DescribeCluster/ListInstanceGroups omissions on fixture elasticmapreduce-multi-master-reference-create-and-protected-terminate only - it does not carry the replay keys for the run-job-flow-step fixture's cluster read, and it says nothing about the step projections, the Date serialization or the OS-release catalog. 20261029T212242Z-emr-listreleaselabels-serves-a-stale-and-truncated-release.md covers ListReleaseLabels (service token 'emr'), a different operation and catalog from DescribeReleaseLabel's AvailableOSReleases.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, score 86.3%), verdict=mismatch for each cluster:

1. ListSteps emr-run-job-flow-step-writes-s3-output#task#21, 200 vs 200 - $.Steps[0].AdditionalInfo missing (expected {'ResourceManager': 'YARN', 'ResourceManagerApplicationIds': ['<shaped-not-asserted>']}); $.Steps[0].Status.Timeline.CreationDateTime / StartDateTime / EndDateTime each value_mismatch against the recorded timestamp shape '<shaped-not-asserted>', got 1794489309.7755058 / 1794489315.7755058 / 1794489317.7755058.
2. DescribeStep ...#task#22, 200 vs 200 - $.Step.AdditionalInfo missing (same expected map); $.Step.Status.Timeline.CreationDateTime / StartDateTime / EndDateTime the same three shape mismatches.
3. DescribeCluster ...#task#23, 200 vs 200 - $.Cluster.Ec2InstanceAttributes.RequestedEc2SubnetIds[0] missing (expected the subnet id the cluster was launched into); $.Cluster.KerberosAttributes missing (expected {}); $.Cluster.StepExecutionMode missing (expected 'STANDARD'); $.Cluster.SystemLogUploadDisabled missing (expected False); $.Cluster.Status.Timeline.CreationDateTime / ReadyDateTime / EndDateTime shape mismatches, got 1794489309.7755058 / 1794489315.7755058 / 1794489318.2755058.
4. DescribeReleaseLabel elasticmapreduce-release-label-and-instance-type-catalog#task#2, 200 vs 200 - AvailableOSReleases[0..2] expected the three newest Amazon Linux labels for the requested release label, got 2.0.20220218.1 / 2.0.20230808.0 / 2.0.20231116.0; AvailableOSReleases[3] through [50] expected present, all got None (48 further indices absent).

## Steps to Reproduce
1. git clone the repository and boot the emulator built from that checkout.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Representative: fixture emr-run-job-flow-step-writes-s3-output, step task#21 - RunJobFlow a cluster with a step that writes to S3, let the step complete, then ListSteps and compare Steps[0].AdditionalInfo and Steps[0].Status.Timeline.
4. Same fixture, step task#22 (DescribeStep on that step id) and step task#23 (DescribeCluster on that cluster: check Ec2InstanceAttributes.RequestedEc2SubnetIds, KerberosAttributes, StepExecutionMode, SystemLogUploadDisabled and Status.Timeline).
5. Fixture elasticmapreduce-release-label-and-instance-type-catalog, step task#2 - DescribeReleaseLabel for the fixture's release label and compare the whole AvailableOSReleases list, not just its first entries.
