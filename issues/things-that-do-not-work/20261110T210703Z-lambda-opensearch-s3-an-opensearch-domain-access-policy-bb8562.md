Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__opensearch__s3.deploy-opensearch-domain-policy-restricts-lambda-denied
Issue-Key: lambda__opensearch__s3.deploy-opensearch-domain-policy-restricts-lambda-denied|_poll_until#41|Timeout|last = probe()
Issue-Commit: 2d0b651e448fe6a44fa56d00833f67c02b248ecb

# lambda__opensearch__s3: an OpenSearch domain access policy that excludes the Lambda role never produces the 403 / document-absent contrast — the check hangs to the 60 s budget

> _Note: this issue was identified against the code base at commit `2d0b651e448fe6a44fa56d00833f67c02b248ecb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
OpenSearch domain data-plane write under a domain access policy (UpdateDomainConfig AccessPolicies)

## Description
Cause 1 of 1: a domain access policy is never enforced observably on the OpenSearch data plane — the check reaches no verdict and is cut off at the harness's 60 s budget. Failing check: lambda__opensearch__s3.deploy-opensearch-domain-policy-restricts-lambda-denied.

AWS: recorded behavior only for the data-plane leg (the domain's HTTP data plane is outside the opensearch Smithy model; the model covers CreateDomain/UpdateDomainConfig and the AccessPolicies document). While the domain access policy names both the deployer and the Lambda role, the Lambda role's signed index write succeeds; once the policy is narrowed to the deployer alone, the identical write is refused HTTP 403 and the document is absent (a subsequent GET answers 404); restoring the policy makes the same write succeed again.

Emulator: the check produces no result — "Error: Check timed out after 60.0s". Neither the permitted write, the 403 refusal, the 404 absence nor the restored success is observed.

Why: a client using the domain access policy as its authorization boundary gets no signal at all — it cannot confirm that a role it removed is actually denied (403, document absent) or that a role it granted can write, so the policy is untestable and may be entirely inert.

Fix: the OpenSearch domain data plane's AccessPolicies evaluation for SigV4-signed requests (403 refusal and no write on denial) and the domain endpoint answering requests rather than hanging.

Traces: the failing check id above.

## Evidence
Expected (AWS): with both principals in AccessPolicies, the Lambda role's write succeeds; with only the deployer listed, the same write is refused 403 and a GET of the document answers 404; restoring the policy makes the write succeed again.
Actual (emulator): Error: Check timed out after 60.0s — no leg reaches a verdict.

## Steps to Reproduce
1. Create an OpenSearch domain whose AccessPolicies name both the deployer and a Lambda execution role, plus the Lambda and an S3 bucket.
2. Invoke the Lambda to write a document (succeeds on AWS).
3. UpdateDomainConfig so AccessPolicies name only the deployer; invoke again and read the document back.
4. Restore the policy and invoke once more; observe the check never reaches any of these verdicts before the 60 s cap.
