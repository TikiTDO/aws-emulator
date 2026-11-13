Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr__eks__logs.cli-eks-update-logging-config-workflow
Issue-Key: ecr__eks__logs.cli-eks-update-logging-config-workflow|_poll_log_group_present#14|TimeoutError|raise TimeoutError(
Issue-Commit: 37c02434d582c8370b608107fbfd29504c12ec2b

# ecr__eks__logs: eks UpdateClusterConfig enabling control-plane log type 'api' still creates no /aws/eks/<cluster>/cluster log group (1 cause)

> _Note: this issue was identified against the code base at commit `37c02434d582c8370b608107fbfd29504c12ec2b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateClusterConfig

## Description
One root cause. Cause count: 1.

Note on existing coverage: open issue 20261024T150703Z-ecr-eks-logs-eks-updateclusterconfig-enabling-a-control.md describes this same behaviour and lists this same check id, but its stored replay key (`...|_poll_log_group_present#14|Timeout|time.sleep(interval_s)`) no longer matches how the check now fails (it now raises TimeoutError at _poll_log_group_present, check source line 270), so the suite did not mark this failure as owned. Open issue 20261023T113357Z-eks-control-plane-logging-has-no-effect-no.md carries only the negative arm (a DISABLED log type producing no group). Filing so the live replay key is carried; a fixer should close it together with those if the underlying provider is one change.

=== Cause 1 of 1: enabling an EKS control-plane log type has no observable effect on CloudWatch Logs ===

AWS: Smithy com.amazonaws.eks#LogSetup is documented as "An object representing the enabled or disabled Kubernetes control plane logs for your cluster." UpdateClusterConfig with logging={'clusterLogging':[{'types':['api'],'enabled':true}]} returns an Update whose id reaches status Successful (DescribeUpdate), and the service then delivers the enabled log type to the log group /aws/eks/<cluster-name>/cluster — EKS creates that group itself if it does not exist, and at least one log stream appears beneath it.

Emulator: the update is accepted and DescribeUpdate reports Successful, but logs DescribeLogGroups(logGroupNamePrefix='/aws/eks/<cluster>/cluster') stays empty forever and no log stream is ever created, so the check polls to its deadline. Nothing in the UpdateClusterConfig/DescribeUpdate response distinguishes this from the real thing.

Why: a client that enables control-plane api/audit logging gets a successful update and no logs at all. Control-plane logging is the only channel for auditing an EKS cluster, and its single externally visible effect is missing while the API reports success — so a misconfigured cluster and a correctly logging one are indistinguishable.

Fix: the clusterLogging effect in the eks UpdateClusterConfig (and CreateCluster) handler — it must provision /aws/eks/<cluster>/cluster in the CloudWatch Logs service's state and deliver the enabled log types into streams under it.

Traces: ecr__eks__logs.cli-eks-update-logging-config-workflow.

## Evidence
ecr__eks__logs.cli-eks-update-logging-config-workflow, verdict [error]:

  File "check_ecr__eks__logs_cli_eks_update_logging_config_workflow.py", line 54, in run -> line 270, in _poll_log_group_present
  TimeoutError: log group /aws/eks/<cluster-name>/cluster appearing: the run budget ended first; not graded

(the cluster name is a per-run generated value; the requirement is the derived group name /aws/eks/<cluster>/cluster.)

Expected (real AWS): after `aws eks update-cluster-config --name <cluster> --logging '{"clusterLogging":[{"types":["api"],"enabled":true}]}'` and the update id reaching Successful, logs.describe_log_groups(logGroupNamePrefix='/aws/eks/<cluster>/cluster') returns that group and logs.describe_log_streams on it returns >= 1 stream.
Actual (emulator): CreateCluster/UpdateClusterConfig/DescribeUpdate all succeed (the update reports Successful), and the log group never appears and no stream is ever created — the poll never observes it.

## Steps to Reproduce
1. Boot the emulator.
2. ec2.create_vpc + two ec2.create_subnet; iam.create_role trusting eks.amazonaws.com with AmazonEKSClusterPolicy attached.
3. eks.create_cluster(name=<cluster>, roleArn=..., resourcesVpcConfig={'subnetIds': [...]}) with NO logging; poll eks.describe_cluster until status ACTIVE.
4. Run `aws eks update-cluster-config --name <cluster> --logging '{"clusterLogging":[{"types":["api"],"enabled":true}]}' --output json`; take update.id and poll eks.describe_update until status == Successful.
5. Poll logs.describe_log_groups(logGroupNamePrefix='/aws/eks/<cluster>/cluster') — it stays empty; logs.describe_log_streams on that name finds nothing.
6. Re-replay the check id in trace_ids to confirm a fix.
