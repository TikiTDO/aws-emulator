Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__opensearch__s3.deploy-opensearch-domain-policy-restricts-lambda-denied
Issue-Key: lambda__opensearch__s3.deploy-opensearch-domain-policy-restricts-lambda-denied|_invoke_lambda#41|Timeout|raise AssertionError(
Issue-Commit: cf4b8496ba0c958ea150f5d2d9329060f4b043af

# lambda__opensearch__s3: the CFN-deployed domain-access-policy denial trajectory never reaches a verdict — the check hangs to the 60 s harness cap

> _Note: this issue was identified against the code base at commit `cf4b8496ba0c958ea150f5d2d9329060f4b043af`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateDomainConfig

## Description
Cause 1 of 1: the deploy-then-restrict-then-restore access-policy scenario makes no observable progress and is cut off at the suite's 60 s cap, so the 403-and-absent-document contract is never graded.
AWS: recorded behavior only for the end-to-end timing; the contract is the documented resource-based access policy of an OpenSearch domain: while the domain AccessPolicies Allow statement names both the deployer and the Lambda execution role, the function's SigV4-signed PUT /<index>/_doc/<id> succeeds (200/201); once UpdateDomainConfig narrows the policy to the deployer only, the same signed PUT is refused HTTP 403 and the document stays absent (a signed GET of it answers 404); restoring the policy makes the identical write succeed again. CloudFormation reports CREATE_COMPLETE for the stack that carries the bucket, role, function and domain.
Emulator: no assertion is reached before the cap ('Check timed out after 60.0s'). The scenario's unbounded waits are CloudFormation reaching a terminal stack status (CREATE_COMPLETE for a stack containing AWS::OpenSearchService::Domain, polled every 10 s) and the domain publishing an Endpoint. Hypothesis: the stack never reaches a terminal status within the budget, or the domain it declares never publishes a usable endpoint, so the scenario cannot get as far as its first signed write. Open issue 20261110T153718Z-lambda-opensearch-s3-an-opensearch-domain-access-policy.md records an access-policy divergence for a different check and does not carry this check's replay key; 20261028T032345Z-ec2-elbv2-opensearch-cloudformation-provisioned-aws-opensearchservice-domain-is-refused-by.md records a CFN-provisioned AWS::OpenSearchService::Domain being refused, which may be the same upstream cause.
Why: the single most important OpenSearch authorization behavior — a domain access policy actually denying a principal it does not name, and the denied write leaving no document — cannot be exercised at all; a client testing least-privilege against an emulated domain gets no answer either way.
Fix: the CloudFormation provisioning path for AWS::OpenSearchService::Domain (stack status progression to a terminal state) and the domain access-policy authorizer on the data plane, in the cloudformation and opensearch service crates.
Traces: lambda__opensearch__s3.deploy-opensearch-domain-policy-restricts-lambda-denied.

## Evidence
Replay output: 'Error: Check timed out after 60.0s' with no assertion reached. Expected sequence: stack CREATE_COMPLETE; signed PUT from the Lambda role -> 200/201; UpdateDomainConfig AccessPolicies naming only the deployer -> same PUT 403 and a signed GET of the id 404; policy restored -> PUT 200/201 again. Actual: the check never gets past its stack/endpoint waits.

## Steps to Reproduce
1. CreateStack with an S3 bucket, an IAM indexer role, a Lambda function and an AWS::OpenSearchService::Domain whose AccessPolicies name both the caller and the indexer role; poll DescribeStacks for a terminal status.
2. DescribeDomain for the Endpoint; invoke the function so it signs a PUT /<index>/_doc/<id> — expect 200/201.
3. UpdateDomainConfig with AccessPolicies listing only the deployer; re-invoke — expect HTTP 403 and a signed GET of the id to answer 404.
4. Restore the two-principal policy and re-invoke — expect 200/201. In the emulator the check never reaches step 2 within the 60 s cap.
