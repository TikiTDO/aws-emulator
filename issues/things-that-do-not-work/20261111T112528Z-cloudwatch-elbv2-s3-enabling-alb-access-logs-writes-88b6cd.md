Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__elbv2__s3.e2e-diagnose-alb-five-xx-errors-lifecycle
Issue-Key: cloudwatch__elbv2__s3.e2e-diagnose-alb-five-xx-errors-lifecycle|phase#9|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# cloudwatch__elbv2__s3: enabling ALB access logs writes no ELBAccessLogTestFile to the destination bucket, so a deploy that turns logging on sees an empty bucket

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyLoadBalancerAttributes

## Description
One net-new root cause (a failure point of a check whose replay key is not carried by any open issue).

Cause 1 of 1: when access logging is enabled on an application load balancer — here by a CloudFormation stack setting access_logs.s3.enabled=true with an access_logs.s3.bucket and an empty access_logs.s3.prefix — the load balancer never writes the log-delivery test object into the destination bucket. The bucket stays empty. Failing check id: cloudwatch__elbv2__s3.e2e-diagnose-alb-five-xx-errors-lifecycle

AWS: recorded behavior (the ELBv2 Smithy model describes only the attribute, not the side effect; AWS documents it under ALB access logs). The moment access_logs.s3.enabled is turned on, Elastic Load Balancing verifies it can write to the destination by putting a single zero-byte object at '<prefix>/AWSLogs/<account-id>/ELBAccessLogTestFile' (with no prefix configured the key is exactly 'AWSLogs/<account-id>/ELBAccessLogTestFile'). Listing the bucket under the 'AWSLogs/' prefix immediately after the attribute change therefore returns exactly that one key — the account id is the caller's account, the prefix is the configured access_logs.s3.prefix, and 'AWSLogs/.../ELBAccessLogTestFile' is fixed text.
Emulator: the attributes are stored correctly (DescribeLoadBalancerAttributes reads access_logs.s3.enabled='true', access_logs.s3.bucket=<bucket>, access_logs.s3.prefix=''), but ListObjectsV2 on the bucket under 'AWSLogs/' returns nothing: the check fails with 'enabling logs at deploy should leave exactly the test file, bucket holds []'.
Why: the test file is the only positive confirmation a client has that the destination bucket and its policy actually permit log delivery — operators and deployment pipelines poll for it, and alarms/runbooks treat its absence as a misconfigured bucket policy. Its absence makes a correctly configured ALB indistinguishable from one whose logs can never be delivered, and it blocks every later step of a logging workflow.
Fix: the elbv2 access-log writer that the ModifyLoadBalancerAttributes handler (and the CloudFormation AWS::ElasticLoadBalancingV2::LoadBalancer attribute path that calls it) must trigger when access_logs.s3.enabled transitions to true — it has to put the zero-byte '<prefix>/AWSLogs/<account>/ELBAccessLogTestFile' object through S3 before reporting the attribute change as applied.
Traces: cloudwatch__elbv2__s3.e2e-diagnose-alb-five-xx-errors-lifecycle — phase 1/5 (deploy) fails on the test-file assertion, so the remaining four phases never run. NOTE for the fixer: the open issue 20261104T055008Z-cloudwatch-elbv2-s3-5-net-new-root-causes.md lists this check under its Cause 5 (a since-changed DescribeLoadBalancerAttributes LoadBalancerNotFound refusal, which no longer reproduces) and names the missing test file inside its broad Cause 4 via the sibling check cloudwatch__elbv2__s3.enabling-access-logs-writes-the-test-file-under-the-configured-prefix; neither key covers this check's current failure point, so it is filed here and the two should be closed together.

## Evidence
Expected (AWS): after the stack create that sets access_logs.s3.enabled=true, access_logs.s3.bucket=<minted bucket>, access_logs.s3.prefix='', ListObjectsV2(Bucket=<bucket>, Prefix='AWSLogs/') returns exactly one key: 'AWSLogs/<account-id>/ELBAccessLogTestFile'.

Actual (emulator): DescribeLoadBalancerAttributes reports ('true', '<bucket>', '') as expected, but the bucket listing under 'AWSLogs/' is empty. Replay verdict: AssertionError: [phase 1/5 deploy] enabling logs at deploy should leave exactly the test file, bucket holds []. No error code and no non-2xx response is involved — the side effect is simply absent.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite.
2. Create an S3 bucket with the ELB log-delivery bucket policy, create an application load balancer, then ModifyLoadBalancerAttributes with access_logs.s3.enabled=true and access_logs.s3.bucket=<bucket> (no prefix) — or deploy a stack whose AWS::ElasticLoadBalancingV2::LoadBalancer carries those LoadBalancerAttributes.
3. Call ListObjectsV2(Bucket=<bucket>, Prefix='AWSLogs/').
4. Observe no objects, where AWS leaves exactly 'AWSLogs/<account-id>/ELBAccessLogTestFile'.
5. Replay cloudwatch__elbv2__s3.e2e-diagnose-alb-five-xx-errors-lifecycle to confirm.
