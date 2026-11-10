Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr__glue__s3.emr-cluster-missing-s3-logs-permission-negative
Issue-Key: emr__glue__s3.emr-cluster-missing-s3-logs-permission-negative|_poll_cluster_state#15|Timeout|time.sleep(interval_s)
Issue-Commit: b35089c281a7e84768d928c8c72034d27eee1ae8

# emr__glue__s3: the S3 log-permission boundary scenario reaches no verdict — a step-running cluster never terminates and never uploads logs to its LogUri, so the positive control never sees logs (1 cause)

> _Note: this issue was identified against the code base at commit `b35089c281a7e84768d928c8c72034d27eee1ae8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunJobFlow

## Description
Cause 1 of 1: an EMR cluster launched with LogUri and a trivial step, with KeepJobFlowAliveWhenNoSteps=false, never reaches a terminal ClusterState and never persists its logs under its LogUri prefix, so the whole permission-boundary scenario is cut off at the 60 s harness cap before it can grade anything.
AWS: RunJobFlow with LogUri=s3://<bucket>/<prefix>/ and Instances.KeepJobFlowAliveWhenNoSteps=false runs its step, self-terminates (ClusterState reaches TERMINATED/TERMINATED_WITH_ERRORS — Smithy ClusterState = STARTING|BOOTSTRAPPING|RUNNING|WAITING|TERMINATING|TERMINATED|TERMINATED_WITH_ERRORS) and uploads the cluster's log bundle under s3://<bucket>/<prefix>/<clusterId>/, which ListObjectsV2 on that prefix then returns. A sibling cluster whose EC2 instance role carries an explicit Deny on s3:PutObject for a different log prefix uploads nothing there — the absence is caused by the IAM Deny, and the scenario sizes its negative wait from the positive control's measured latency. Recorded behavior only for the log-bundle layout; the model says nothing about log upload.
Emulator: the check never reaches either assertion. It is killed at the 60 s cap while polling DescribeCluster for a terminal state for the two clusters and then ListObjectsV2 for the allowed cluster's logs, so the positive control ("an unrestricted instance role DOES get logs to S3") is never satisfied and the negative half — the denied role must NOT get logs — is never graded. Hypothesis: the cluster neither executes its step nor self-terminates nor uploads logs (the open issues 20261108T104909Z-emr-glue-s3-2-net-new-causes-cluster.md and 20261109T150612Z-emr-glue-s3-a-terminated-cluster-with-an.md describe exactly these missing side effects for other checks, and neither carries this check's replay key).
Why: a client cannot observe EMR's log delivery at all — neither that logs land under LogUri for a permitted instance role, nor that an s3:PutObject Deny suppresses them — so no log-based troubleshooting or permission-boundary verification works against the emulator, and any wait for an auto-terminating cluster hangs.
Fix: the RunJobFlow cluster state machine (step execution and the KeepJobFlowAliveWhenNoSteps=false self-termination path) and the cluster's log-upload side effect to LogUri, which must be evaluated against the EC2 instance role's S3 permissions.
Traces: emr__glue__s3.emr-cluster-missing-s3-logs-permission-negative ("Check timed out after 60.0s").

## Evidence
Suite report simparity-behavioral-aws-v1, service heading '### emr__glue__s3' (7/47 passed):
  [mismatch] emr__glue__s3.emr-cluster-missing-s3-logs-permission-negative — "Error: Check timed out after 60.0s"
Not marked ALREADY FILED and not OWNED by any open issue in this report.

Expected (AWS): the two clusters (one instance role unrestricted, one with an explicit Deny on s3:PutObject for arn:aws:s3:::<bucket>/logs-denied/*) both reach TERMINATED/TERMINATED_WITH_ERRORS after their trivial step; ListObjectsV2 on logs-allowed/<clusterId>/ returns objects (positive control), and ListObjectsV2 on logs-denied/<clusterId>/ returns none for at least 3x the positive control's measured latency.
Actual (emulator): no polled condition is satisfied — the runner kills the check at the 60 s cap with the clusters not terminal and no log objects under either prefix, so the permission boundary is never observed. The reads are served from memory, so the budget is not the constraint (7 checks of this same service and budget complete).

## Steps to Reproduce
1. Create a bucket. Create an EMR service role, and two EC2 instance profiles: one unrestricted, one whose role carries an inline Deny of s3:PutObject on arn:aws:s3:::<bucket>/logs-denied/*.
2. RunJobFlow twice with the same trivial step (command-runner.jar, ["bash","-c","echo emr-permission-check"]), Instances.KeepJobFlowAliveWhenNoSteps=false — one with LogUri=s3://<bucket>/logs-denied/ and the denied profile, one with LogUri=s3://<bucket>/logs-allowed/ and the unrestricted profile.
3. Poll DescribeCluster for both until Status.State is TERMINATED or TERMINATED_WITH_ERRORS: neither cluster gets there.
4. ListObjectsV2(Bucket=<bucket>, Prefix="logs-allowed/<clusterId>/"): no log objects ever appear (positive control fails), so the denied-side absence proves nothing.
