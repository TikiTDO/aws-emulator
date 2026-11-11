Issue-Source: trace-sweep-divergence
Issue-Trace: elasticmapreduce-multi-master-reference-create-and-protected-terminate
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticmapreduce-multi-master-reference-create-and-protected-terminate emr-run-job-flow-step-writes-s3-output
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: elasticmapreduce|DescribeCluster|2xx->2xx|$.Cluster.UnhealthyNodeReplacement
Issue-Key: elasticmapreduce|ListSteps|2xx->2xx|$.Steps[*].AdditionalInfo
Issue-Key: elasticmapreduce|ListSteps|2xx->2xx|$.Steps[*].Status.State
Issue-Key: elasticmapreduce|ListSteps|2xx->2xx|$.Steps[*].Status.Timeline.CreationDateTime
Issue-Key: elasticmapreduce|ListSteps|2xx->2xx|$.Steps[*].Status.Timeline.EndDateTime
Issue-Key: elasticmapreduce|ListSteps|2xx->2xx|$.Steps[*].Status.Timeline.StartDateTime
Issue-Key: elasticmapreduce|DescribeStep|2xx->2xx|$.Step.AdditionalInfo
Issue-Key: elasticmapreduce|DescribeStep|2xx->2xx|$.Step.Status.State
Issue-Key: elasticmapreduce|DescribeStep|2xx->2xx|$.Step.Status.Timeline.CreationDateTime
Issue-Key: elasticmapreduce|DescribeStep|2xx->2xx|$.Step.Status.Timeline.EndDateTime
Issue-Key: elasticmapreduce|DescribeStep|2xx->2xx|$.Step.Status.Timeline.StartDateTime
Issue-Key: elasticmapreduce|DescribeCluster|2xx->2xx|$.Cluster.Ec2InstanceAttributes.Ec2AvailabilityZone
Issue-Key: elasticmapreduce|DescribeCluster|2xx->2xx|$.Cluster.MasterPublicDnsName
Issue-Key: elasticmapreduce|DescribeCluster|2xx->2xx|$.Cluster.NormalizedInstanceHours
Issue-Key: elasticmapreduce|DescribeCluster|2xx->2xx|$.Cluster.OSReleaseLabel
Issue-Key: elasticmapreduce|DescribeCluster|2xx->2xx|$.Cluster.Status.StateChangeReason.Message
Issue-Key: elasticmapreduce|DescribeCluster|2xx->2xx|$.Cluster.Status.Timeline.CreationDateTime
Issue-Key: elasticmapreduce|DescribeCluster|2xx->2xx|$.Cluster.Status.Timeline.EndDateTime
Issue-Key: elasticmapreduce|DescribeCluster|2xx->2xx|$.Cluster.Status.Timeline.ReadyDateTime
Issue-Related: 20261102T083234Z-elasticmapreduce-emr-cluster-and-instance-group-reads-omit-modeled.md
Issue-Commit: 01897137b22b96058352710896276b71185e2aeb

# EMR run-job-flow lifecycle diverges: submitted steps never COMPLETE and cluster reads omit or invent modeled members

> _Note: this issue was identified against the code base at commit `01897137b22b96058352710896276b71185e2aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCluster

## Description
The emulator's EMR read path serves clusters and steps whose modeled members are absent, defaulted to the wrong value, or carry emulator-authored prose, and a step submitted with a cluster never reaches a successful terminal state. A client driving the standard RunJobFlow -> step -> auto-terminate lifecycle therefore cannot read the cluster's requested network/Kerberos/session attributes, cannot tell a working job from a broken one, and is told by the service's own state-change reason that nothing ran.

THIS ISSUE BUNDLES 4 CAUSES. All four must be closed.

Cause 1 of 4: DescribeCluster reports UnhealthyNodeReplacement true for a cluster that did not request it, and still omits the service-defaulted Cluster members.
  AWS: 200 with Cluster.UnhealthyNodeReplacement echoing the value the cluster was launched with, defaulting to false when RunJobFlow did not request it. Smithy: Cluster.UnhealthyNodeReplacement targets BooleanObject, doc "Indicates whether Amazon EMR should gracefully replace Amazon EC2 core instances that have degraded within the cluster." Alongside it, Smithy Cluster.KerberosAttributes (KerberosAttributes) is serialized as an empty structure, Cluster.SessionEnabled (BooleanObject, "Indicates whether Spark Connect sessions are enabled on the cluster") as false, and Ec2InstanceAttributes.RequestedEc2AvailabilityZones / RequestedEc2SubnetIds (XmlStringMaxLen256List) as empty lists when the cluster was not configured with instance fleets. StepExecutionMode ('STANDARD') and SystemLogUploadDisabled (false) are recorded behavior only - they are not members of the on-disk Cluster shape, but the recording returns both on every DescribeCluster.
  Emulator: 200, but UnhealthyNodeReplacement is true where the cluster was launched without it, and KerberosAttributes, SessionEnabled, StepExecutionMode, SystemLogUploadDisabled, Ec2InstanceAttributes.RequestedEc2AvailabilityZones and Ec2InstanceAttributes.RequestedEc2SubnetIds are all absent.
  Why: UnhealthyNodeReplacement is not merely missing but inverted, so a client auditing or reconciling cluster configuration reads a graceful-replacement setting the operator never asked for and may 'correct' it against a cluster that is already correct; the absent members hand a null to every client that iterates RequestedEc2SubnetIds or dereferences KerberosAttributes/SessionEnabled where AWS guarantees a value.
  Fix: DescribeCluster's serializer (population of service-defaulted Cluster members) plus the cluster state RunJobFlow writes, which must persist UnhealthyNodeReplacement from the request rather than hard-coding it.
  Traces: elasticmapreduce-multi-master-reference-create-and-protected-terminate#task#9.
  Note: open issue 20261102T083234Z-elasticmapreduce-emr-cluster-and-instance-group-reads-omit-modeled.md describes the omitted-member half of this read, but its replay keys do not include $.Cluster.UnhealthyNodeReplacement, so it does not own this cluster and the inverted flag is net-new.

Cause 2 of 4: ListSteps reports a submitted step FAILED instead of COMPLETED and omits its AdditionalInfo.
  AWS: 200 with Steps[].Status.State reaching COMPLETED for a step whose command succeeds, and with the step's AdditionalInfo map carrying the resource-manager identity for the submission (keys ResourceManager = 'YARN' and ResourceManagerApplicationIds, a list of the YARN application ids the step produced; the id values themselves are per-run and not a requirement). Recorded behavior only: Smithy StepSummary has members Id, Name, Config, ActionOnFailure, Status, LogUri and EncryptionKeyArn and no AdditionalInfo, and StepStatus/StepState model the state enum without describing execution - so the terminal state and AdditionalInfo are taken from the recorded response.
  Emulator: 200 but Steps[0].Status.State is FAILED - the replay's poll gave up after 2 attempts because FAILED is a terminal fail_on state - and Steps[0].AdditionalInfo is absent.
  Why: step outcome is independent of what the step actually submits, so a client cannot distinguish a genuinely broken job from a working one; retry and alerting logic fires on healthy jobs, and nothing downstream of a successful step can be emulated. The missing AdditionalInfo also removes the only link from an EMR step to the YARN application it ran.
  Fix: the EMR step execution/state machine behind AddJobFlowSteps, ListSteps and DescribeStep - it must derive a step's terminal StepState from the submitted HadoopJarStep and run the step's effects, and populate AdditionalInfo for an executed step.
  Traces: emr-run-job-flow-step-writes-s3-output#task#21.
  Note: open issue 20261108T093128Z-ec2-emr-emr-job-flow-steps-always-end-failed-a.md reports the same blanket-FAILED behavior from the BEHAVIORAL corpus; it carries no wire replay key for this fixture step, so it does not own this cluster.

Cause 3 of 4: DescribeStep reports the same step FAILED instead of COMPLETED and omits its AdditionalInfo.
  AWS: 200 with Step.Status.State = COMPLETED for the same successfully-submitted step and Step.AdditionalInfo carrying ResourceManager = 'YARN' plus ResourceManagerApplicationIds. Recorded behavior only, same as cause 2: Smithy Step{Id,Name,Config,ActionOnFailure,Status,ExecutionRoleArn,LogUri,EncryptionKeyArn} models no AdditionalInfo.
  Emulator: 200 but Step.Status.State is FAILED (poll gave up after 1 attempt on the terminal fail_on state) and Step.AdditionalInfo is absent.
  Why: the single-step read diverges exactly as the list read does, so neither access path lets a client observe a successful step; this must be re-replayed separately to confirm the fix covers DescribeStep's serializer and not only ListSteps'.
  Fix: same step execution/state component as cause 2, plus DescribeStep's serializer for AdditionalInfo.
  Traces: emr-run-job-flow-step-writes-s3-output#task#22.

Cause 4 of 4: DescribeCluster on an auto-terminated cluster omits resolved members, never accrues normalized instance hours, serves a stale application version, announces a master DNS name it does not serve, and replaces the state-change reason with emulator prose.
  AWS: 200 with Cluster.Ec2InstanceAttributes.Ec2AvailabilityZone present (Smithy doc: "The Availability Zone in which the cluster will run"); RequestedEc2SubnetIds echoing the subnet ids the launch requested and RequestedEc2AvailabilityZones as an empty list; KerberosAttributes as an empty structure; SessionEnabled false; OSReleaseLabel present because Smithy documents that "If no Amazon Linux release was specified, the default Amazon Linux release is shown in the response"; NormalizedInstanceHours accrued for the cluster's runtime (Smithy: "An approximation of the cost of the cluster, represented in m1.small/hours", incremented per instance-hour) rather than left at zero; Applications[].Version equal to the version the cluster's ReleaseLabel actually ships; and Status.StateChangeReason.Message = "Steps completed" for a cluster that auto-terminated after its steps finished - that text is fixed, with nothing interpolated from the request or state (Smithy: ClusterStateChangeReason{Code, Message}). StepExecutionMode ('STANDARD') and SystemLogUploadDisabled (false) are recorded behavior only.
  Emulator: 200 but Ec2InstanceAttributes.Ec2AvailabilityZone, RequestedEc2AvailabilityZones, RequestedEc2SubnetIds, KerberosAttributes, OSReleaseLabel, SessionEnabled, StepExecutionMode and SystemLogUploadDisabled are all absent; NormalizedInstanceHours is 0 where the cluster ran long enough to accrue hours; Applications[0].Version is an older patch version than the one the requested release label ships; Status.StateChangeReason.Message is the emulator-authored sentence "Shut down with no step left to run: this emulator ran no application for the cluster's steps, so none of them completed." instead of AWS's fixed "Steps completed"; and MasterPublicDnsName is reported as an internal-style host the emulator does not serve, so a client that connects to the announced master DNS name cannot reach anything (the replay flags this as host_not_own).
  Why: a client cannot determine where the cluster ran (no availability zone or requested subnets), cannot cost it (NormalizedInstanceHours stuck at 0), resolves the wrong application version for the release it asked for, and is told in a service field that the emulator ran nothing - prose that leaks the implementation into a value AWS defines as a fixed state-change reason. The master DNS name it publishes is unreachable, so SSH/UI access to the cluster is impossible.
  Fix: DescribeCluster's serializer and the cluster state written by RunJobFlow and the terminate path - resolved availability zone and requested subnets, the release-label-to-application-version catalog, the normalized-instance-hour accrual, the state-change-reason text for the auto-terminate transition, and the host the master DNS name is derived from (it must be an address the emulator answers on).
  Traces: emr-run-job-flow-step-writes-s3-output#task#23. The Status.Timeline.CreationDateTime / StartDateTime / ReadyDateTime / EndDateTime differences in this cluster are per-run clock values and are NOT part of the requirement.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false, score 81.9%); verdict=mismatch for each cluster below.

1. DescribeCluster - elasticmapreduce-multi-master-reference-create-and-protected-terminate#task#9, status 200 vs 200:
   $.Cluster.UnhealthyNodeReplacement: expected False, got True
   $.Cluster.Ec2InstanceAttributes.RequestedEc2AvailabilityZones: missing (expected [])
   $.Cluster.Ec2InstanceAttributes.RequestedEc2SubnetIds: missing (expected [])
   $.Cluster.KerberosAttributes: missing (expected {})
   $.Cluster.SessionEnabled: missing (expected False)
   $.Cluster.StepExecutionMode: missing (expected 'STANDARD')
   $.Cluster.SystemLogUploadDisabled: missing (expected False)
   (cluster also affects ...#task#14; divergence_key service=elasticmapreduce|DescribeCluster|status=200->200)

2. ListSteps - emr-run-job-flow-step-writes-s3-output#task#21, status 200 vs 200:
   $.Steps[0].Status.State: expected 'COMPLETED', got 'FAILED'
   $.Steps[0].Status.State: poll: 'FAILED' is in fail_on after 2 attempt(s)
   $.Steps[0].AdditionalInfo: missing (expected {'ResourceManager': 'YARN', 'ResourceManagerApplicationIds': ['<shaped-not-asserted>']})

3. DescribeStep - emr-run-job-flow-step-writes-s3-output#task#22, status 200 vs 200:
   $.Step.Status.State: expected 'COMPLETED', got 'FAILED'
   $.Step.Status.State: poll: 'FAILED' is in fail_on after 1 attempt(s)
   $.Step.AdditionalInfo: missing (expected {'ResourceManager': 'YARN', 'ResourceManagerApplicationIds': ['<shaped-not-asserted>']})

4. DescribeCluster - emr-run-job-flow-step-writes-s3-output#task#23, status 200 vs 200:
   $.Cluster.Applications[0].Version: expected the version the requested release label ships, got an older patch version (recorded run: expected '3.5.5', got '3.5.2')
   $.Cluster.Ec2InstanceAttributes.Ec2AvailabilityZone: missing (expected the AZ the cluster ran in)
   $.Cluster.Ec2InstanceAttributes.RequestedEc2AvailabilityZones: missing (expected [])
   $.Cluster.Ec2InstanceAttributes.RequestedEc2SubnetIds: missing (expected the subnet ids the launch requested)
   $.Cluster.KerberosAttributes: missing (expected {})
   $.Cluster.NormalizedInstanceHours: expected a non-zero accrual (recorded run: 8), got 0
   $.Cluster.OSReleaseLabel: missing (expected present, value shape-asserted only)
   $.Cluster.SessionEnabled: missing (expected False)
   $.Cluster.StepExecutionMode: missing (expected 'STANDARD')
   $.Cluster.SystemLogUploadDisabled: missing (expected False)
   $.Cluster.Status.StateChangeReason.Message: expected 'Steps completed' (fixed text, nothing interpolated), got "Shut down with no step left to run: this emulator ran no application for the cluster's steps, so none of them completed."
   $.Cluster.MasterPublicDnsName: host_not_own - the emulator announced 'ip-10-0-0-1.us-west-2.compute.internal', a host it does not serve
   Excluded as non-requirements: $.Cluster.Status.Timeline.{CreationDateTime,ReadyDateTime,EndDateTime} differ only as per-run clock values.

## Steps to Reproduce
1. git clone the repository and boot the emulator built from that checkout.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Cause 1 (representative): fixture elasticmapreduce-multi-master-reference-create-and-protected-terminate, step task#9 - RunJobFlow a multi-master instance-group cluster without requesting unhealthy-node replacement, then DescribeCluster and compare Cluster.UnhealthyNodeReplacement plus the KerberosAttributes / SessionEnabled / StepExecutionMode / SystemLogUploadDisabled / Ec2InstanceAttributes.RequestedEc2AvailabilityZones / RequestedEc2SubnetIds members.
4. Cause 2: fixture emr-run-job-flow-step-writes-s3-output, step task#21 - RunJobFlow a cluster with a step that writes its output to S3, then poll ListSteps(ClusterId) until the step is terminal and compare Steps[0].Status.State and Steps[0].AdditionalInfo.
5. Cause 3: same fixture, step task#22 - DescribeStep(ClusterId, StepId) for that step and compare Step.Status.State and Step.AdditionalInfo.
6. Cause 4: same fixture, step task#23 - after the cluster auto-terminates, DescribeCluster and compare Applications[0].Version, Ec2InstanceAttributes.Ec2AvailabilityZone / RequestedEc2AvailabilityZones / RequestedEc2SubnetIds, KerberosAttributes, NormalizedInstanceHours, OSReleaseLabel, SessionEnabled, StepExecutionMode, SystemLogUploadDisabled, Status.StateChangeReason.Message and MasterPublicDnsName.
7. Re-replay all four fixture steps (elasticmapreduce-multi-master-reference-create-and-protected-terminate#task#9 and emr-run-job-flow-step-writes-s3-output#task#21, #task#22, #task#23) to confirm the fix.
