Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr__eks__logs.eks-disabled-log-type-produces-no-log-group
Issue-Key: ecr__eks__logs.eks-disabled-log-type-produces-no-log-group|_poll_log_group_present#8|Timeout|time.sleep(interval_s)
Issue-Commit: 458648e5d077c2a050f91416b837fc3012b6ecb7

# ecr__eks__logs: the EKS control-plane logging contrast never completes — enabling log type 'api' on an existing cluster still produces no /aws/eks/<cluster>/cluster log group (1 cause)

> _Note: this issue was identified against the code base at commit `458648e5d077c2a050f91416b837fc3012b6ecb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateClusterConfig

## Description
One root cause. Cause count: 1.

Note on existing coverage: open issues 20261029T072657Z-ecr-eks-logs-eks-updateclusterconfig-enabling-control-plane.md, 20261024T150703Z-ecr-eks-logs-eks-updateclusterconfig-enabling-a-control.md and 20261023T113357Z-eks-control-plane-logging-has-no-effect-no.md describe the same provider behaviour, but none of them carries this check's replay key, so the suite did not mark this failure as owned and it is filed here to carry that key. A fixer should close it together with those if the underlying provider change is one change.

=== Cause 1 of 1: an EKS control-plane log type that is enabled on an existing cluster has no observable effect on CloudWatch Logs, so the disabled/enabled contrast never resolves ===

AWS: Smithy com.amazonaws.eks#LogSetup — "An object representing the enabled or disabled Kubernetes control plane logs for your cluster"; member `enabled`: "If a log type is enabled, that log type exports its control plane logs to CloudWatch Logs. If a log type isn't enabled, that log type doesn't export its control plane logs." com.amazonaws.eks#LogType values are api, audit, authenticator, controllerManager, scheduler. On real AWS a CreateCluster carrying logging={'clusterLogging':[{'types':[all five],'enabled':false}]} reaches status ACTIVE and never causes the derived log group /aws/eks/<cluster-name>/cluster to exist; a subsequent UpdateClusterConfig with logging={'clusterLogging':[{'types':['api'],'enabled':true}]} returns an Update whose id reaches DescribeUpdate status 'Successful' and the service then creates /aws/eks/<cluster-name>/cluster itself (the group name is derived from the request's cluster name; it is not a per-run literal) so that logs.DescribeLogGroups(logGroupNamePrefix='/aws/eks/<cluster>/cluster') returns it.

Emulator: the all-disabled CreateCluster and the enabling UpdateClusterConfig are both accepted (and the update reports Successful), but the derived log group is never created in the CloudWatch Logs state, so the check's positive control polls DescribeLogGroups to its deadline and the check consumes its whole budget instead of grading. Replay verdict is [mismatch] with detail `Check timed out after 60.0s`; the report carries no traceback for this check, so the exact poll it is parked on is not stated — the only observable state the check waits on that the emulator never reaches is the appearance of /aws/eks/<cluster>/cluster after api logging is enabled, which the issues named above independently record as never happening. Hypothesis: the clusterLogging field is stored on the cluster record and otherwise inert, so neither the enabled nor the disabled arm has any effect in logs.

Why: a client that enables control-plane api/audit logging is told the update succeeded and gets no logs at all. Control-plane logging is the only audit channel for an EKS cluster, and the single externally visible effect of enabling it is absent while the EKS API reports success — a cluster with logging correctly enabled and one with every log type disabled are indistinguishable, so any check or tool that verifies logging is on cannot distinguish the two and simply hangs on the group that never appears.

Fix: the clusterLogging effect in the eks UpdateClusterConfig handler (and the same effect in CreateCluster) — the handler must provision the derived log group /aws/eks/<cluster>/cluster in the CloudWatch Logs service's state when a log type transitions to enabled, and must leave it absent while every type is disabled.

Traces: ecr__eks__logs.eks-disabled-log-type-produces-no-log-group.

## Evidence
Check ecr__eks__logs.eks-disabled-log-type-produces-no-log-group, verdict [mismatch]:

  Error: Check timed out after 60.0s

The check (source quoted in the replay report) does: create VPC + two subnets, create an IAM role trusting eks.amazonaws.com with AmazonEKSClusterPolicy, CreateCluster with logging={'clusterLogging':[{'types':['api','audit','authenticator','controllerManager','scheduler'],'enabled':False}]}, wait for ACTIVE, assert /aws/eks/<cluster>/cluster is ABSENT (negative arm), then UpdateClusterConfig logging={'clusterLogging':[{'types':['api'],'enabled':True}]}, wait for DescribeUpdate status 'Successful', then assert the same group is PRESENT (positive control).

Expected (real AWS): the negative arm observes no /aws/eks/<cluster>/cluster log group, and after the enabling UpdateClusterConfig reaches status Successful, logs.describe_log_groups(logGroupNamePrefix='/aws/eks/<cluster>/cluster') returns that group.
Actual (emulator): CreateCluster, UpdateClusterConfig and DescribeUpdate all succeed (update reports Successful) and the derived log group never appears in DescribeLogGroups, so the positive control polls until the check's budget ends and the check is never graded. The same never-appearing group is recorded by open issues 20261029T072657Z-ecr-eks-logs-eks-updateclusterconfig-enabling-control-plane.md and 20261024T150703Z-ecr-eks-logs-eks-updateclusterconfig-enabling-a-control.md for the sibling check ecr__eks__logs.cli-eks-update-logging-config-workflow.

Note: the cluster name, VPC/subnet ids, role name and the wall-clock duration are per-run values; the requirement asserted here is only the derived group name /aws/eks/<cluster>/cluster and its absence-then-presence across the disable/enable transition.

## Steps to Reproduce
1. Boot the emulator.
2. ec2.create_vpc(CidrBlock='10.42.0.0/16') and two ec2.create_subnet calls in two AZs.
3. iam.create_role trusting eks.amazonaws.com for sts:AssumeRole, then iam.attach_role_policy(PolicyArn='arn:aws:iam::aws:policy/AmazonEKSClusterPolicy').
4. eks.create_cluster(name=<cluster>, roleArn=..., resourcesVpcConfig={'subnetIds':[...]}, logging={'clusterLogging':[{'types':['api','audit','authenticator','controllerManager','scheduler'],'enabled':False}]}); poll eks.describe_cluster until status ACTIVE.
5. Poll logs.describe_log_groups(logGroupNamePrefix='/aws/eks/<cluster>/cluster') — correctly empty while every type is disabled.
6. eks.update_cluster_config(name=<cluster>, logging={'clusterLogging':[{'types':['api'],'enabled':True}]}); poll eks.describe_update(name=<cluster>, updateId=<update.id>) until status == 'Successful'.
7. Poll logs.describe_log_groups(logGroupNamePrefix='/aws/eks/<cluster>/cluster') again — on AWS the group is now present; on the emulator it stays empty forever, so the caller (and the check) never observes the enabled state.
8. Re-replay the check id in trace_ids to confirm a fix.
