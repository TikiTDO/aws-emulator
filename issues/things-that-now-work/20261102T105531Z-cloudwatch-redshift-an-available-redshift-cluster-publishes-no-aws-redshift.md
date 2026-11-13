Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__redshift.single-node-cluster-publishes-health-status-and-no-leader-series cloudwatch__redshift.multi-node-cluster-publishes-leader-health-status-the-leader-alarm-evaluates
Issue-Key: cloudwatch__redshift.single-node-cluster-publishes-health-status-and-no-leader-series|_poll#10|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__redshift.multi-node-cluster-publishes-leader-health-status-the-leader-alarm-evaluates|_poll#15|Timeout|time.sleep(interval_s)
Issue-Commit: 276fdc9b01f0fecfb4ec23b6285a10289d7fb0d2

# An available Redshift cluster publishes no AWS/Redshift HealthStatus series, so GetMetricStatistics never returns a datapoint for ClusterIdentifier/NodeID (1 cause)

> _Note: this issue was identified against the code base at commit `276fdc9b01f0fecfb4ec23b6285a10289d7fb0d2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
One net-new root cause.

=== Cause 1 of 1: the Redshift cluster lifecycle publishes no AWS/Redshift HealthStatus metric, so the cluster-wide and per-node series are permanently empty ===
Failing checks: cloudwatch__redshift.single-node-cluster-publishes-health-status-and-no-leader-series, cloudwatch__redshift.multi-node-cluster-publishes-leader-health-status-the-leader-alarm-evaluates

AWS: a Redshift cluster that reaches ClusterStatus 'available' publishes the AWS/Redshift HealthStatus metric every minute. HealthStatus is a 0/1 flag that a healthy cluster holds at 1.0, and it is published at several dimension sets: cluster-wide (Dimensions=[ClusterIdentifier]) and per node (Dimensions=[ClusterIdentifier, NodeID]), where NodeID takes the node's role — 'Shared' for the single node of a single-node cluster, and 'Leader'/'Compute-N' for a multi-node cluster. Consequently a single-node cluster has a populated ClusterIdentifier series and a populated NodeID=Shared series covering the same minutes, and NO NodeID=Leader series at all; a multi-node cluster does have the NodeID=Leader series. cloudwatch GetMetricStatistics(Namespace='AWS/Redshift', MetricName='HealthStatus', Period=60, Statistics=[Average,Maximum,Minimum]) returns these as Datapoints, and the absence of a series is reported as an empty Datapoints list rather than an error. Recorded behavior only for the publication cadence: the checks treat roughly one datapoint per minute, appearing within a couple of minutes of 'available', as the environment-fidelity expectation.

Emulator: neither check produces a verdict — both report 'Check timed out after 60.0s'. In both, the first barrier after the cluster is created is a poll of GetMetricStatistics for a HealthStatus datapoint whose Maximum reaches 1.0, and neither converges, so no metric assertion is ever evaluated and the later differential (Shared covering the cluster-wide minutes, Leader empty on single-node / present on multi-node) is unobservable. The replay output carries no frame beyond the timeout. Hypothesis: either the Redshift cluster lifecycle never emits any AWS/Redshift HealthStatus series for an available cluster and its nodes, or the cluster never reaches 'available' at all and so never begins publishing; the suite's redshift-only checks that poll DescribeClusters to 'available' do pass, which points at the metric publication rather than the cluster lifecycle.

Why: a client cannot observe the health of an emulated cluster at all. Dashboards, alarm wiring, autoscaling or runbook code that reads AWS/Redshift HealthStatus gets a permanently empty series, which is indistinguishable from a cluster that does not exist; and the operationally meaningful distinction AWS exposes — a NodeID dimension that legitimately has no series (Leader on a single-node cluster) versus one that does — cannot be exercised, so a monitoring configuration validated against the emulator says nothing about production.

Fix: the Redshift cluster lifecycle's metric publication (the component that should emit AWS/Redshift metrics into CloudWatch's metric store as a cluster becomes and stays 'available'), plus CloudWatch's multi-dimension series selection for GetMetricStatistics so that [ClusterIdentifier] and [ClusterIdentifier, NodeID] are distinct, independently queryable series and an unpopulated dimension set answers an empty Datapoints list.

Traces: cloudwatch__redshift.single-node-cluster-publishes-health-status-and-no-leader-series, cloudwatch__redshift.multi-node-cluster-publishes-leader-health-status-the-leader-alarm-evaluates.

Note: open issue 20261024T115115Z-cloudwatch-redshift-cloudwatch-alarms-on-aws-redshift-healthstatus-never-evaluate.md reports that ALARMS over this metric never evaluate; this issue is about the metric series itself being unreadable through GetMetricStatistics. That issue does not carry either of these checks' replay keys and the suite did not mark these failures owned, so they are filed here and it is named for context (fixing the publication gap plausibly unblocks both).

Note on attribution: these checks are listed by the suite under the service heading 'cloudwatch__redshift'. Both constituent services, cloudwatch and redshift, are bootstrapped, so the finding is filed rather than deferred.

## Evidence
Expected (single-node): after CreateCluster(ClusterType='single-node', Encrypted=true, PubliclyAccessible=false) reaches ClusterStatus='available' with ClusterNodes[].NodeRole == ['SHARED'], GetMetricStatistics(Namespace='AWS/Redshift', MetricName='HealthStatus', Dimensions=[{ClusterIdentifier,<cluster>}], Period=60, Statistics=[Average,Maximum,Minimum]) returns Datapoints that reach Maximum==1.0 and keep growing by one per minute, all values in {0.0, 1.0}; the NodeID='Shared' series covers every timestamp the cluster-wide series has; the NodeID='Leader' series is []. Expected (multi-node): the NodeID='Leader' series is populated.

Actual: both checks end with 'Check timed out after 60.0s' with no assertion frame — the first GetMetricStatistics poll ('the cluster-wide HealthStatus series reaching 1.0') never converges, so no Datapoints verdict is produced at all and no Shared/Leader differential is reached.

## Steps to Reproduce
1) redshift DescribeOrderableClusterOptions and pick an offered node type (ra3.large / ra3.xlplus / dc2.large). 2) redshift CreateCluster with ClusterType='single-node' (then repeat with a 2-node cluster), Encrypted=true, PubliclyAccessible=false, AutomatedSnapshotRetentionPeriod=1; poll DescribeClusters until ClusterStatus='available'. 3) cloudwatch GetMetricStatistics(Namespace='AWS/Redshift', MetricName='HealthStatus', Dimensions=[{Name:'ClusterIdentifier',Value:<cluster>}], StartTime=now-60m, EndTime=now, Period=60, Statistics=['Average','Maximum','Minimum']) and observe Datapoints stays empty. 4) Repeat step 3 with Dimensions=[ClusterIdentifier, {NodeID:'Shared'}] and with [ClusterIdentifier, {NodeID:'Leader'}]. Replays: cloudwatch__redshift.single-node-cluster-publishes-health-status-and-no-leader-series, cloudwatch__redshift.multi-node-cluster-publishes-leader-health-status-the-leader-alarm-evaluates.
