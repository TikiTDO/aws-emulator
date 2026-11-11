Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__opensearch__s3.lambda-sdk-opensearch-missing-permission-negative
Issue-Key: lambda__opensearch__s3.lambda-sdk-opensearch-missing-permission-negative|_invoke#25|Timeout|resp = lam.invoke(FunctionName=function_name, Payload=json.dumps(payload).encode("utf-8"))
Issue-Commit: 181b4db8a49f9f99cf56c1b2f4aacf19bf82375e

# lambda__opensearch__s3: the identity-policy permission-boundary trajectory never reaches a verdict — the es:ESHttpGet-only / es:ESHttpPut contrast hangs to the 60 s harness cap (1 cause)

> _Note: this issue was identified against the code base at commit `181b4db8a49f9f99cf56c1b2f4aacf19bf82375e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke (Lambda) against the OpenSearch domain REST data plane (es:ESHttpGet / es:ESHttpPut)

## Description
One cause.

Cause 1 of 1: the scenario that distinguishes an identity policy granting only es:ESHttpGet from one granting the write action never produces any response. The check is cut off at the 60 s harness cap with no verdict at all — not a wrong status, not a wrong body: nothing comes back.

AWS: recorded behavior only for the trajectory (the contrast is between two IAM identity policies against one unchanged domain access policy). The recording shows three outcomes in sequence: (a) a Lambda role whose identity policy grants only es:ESHttpGet on arn:aws:es:<region>:<account>:domain/<domain>/* is refused with HTTP 403 on a document write, and the document never lands; (b) a role whose identity policy grants the write action succeeds on the identical write; (c) adding the write action to the restricted role's OWN identity policy — with the domain access policy untouched — makes the identical write succeed. Each of those calls returns promptly.
Emulator: the check is terminated by the harness with "Check timed out after 60.0s". No 403, no 200, and no document state is ever observable, so none of the three outcomes is produced within a time a client would wait.
Why: a client gets no answer at all on the authorization path. A request that AWS refuses in milliseconds with 403 instead blocks until the caller's own timeout, which a caller reads as an availability failure rather than as a permission denial — and the retry it then performs is on a request that may or may not have taken effect. IAM-vs-resource-policy authorization on the OpenSearch data plane cannot be relied on in either direction.
Fix: the Lambda Invoke execution path and the OpenSearch domain data plane's authorization component it calls — the identity-policy evaluation on es:ESHttp* must terminate and answer (403 when the action is not granted, 200 when it is) rather than leaving the request outstanding.
Traces: lambda__opensearch__s3.lambda-sdk-opensearch-missing-permission-negative.

Note: open issues 20261110T210703Z-lambda-opensearch-s3-an-opensearch-domain-access-policy-bb8562.md and 20261111T000041Z-lambda-opensearch-s3-the-cfn-deployed-domain-access.md describe the same hang symptom for DOMAIN-ACCESS-POLICY denial scenarios; this one is the IDENTITY-policy (permission-boundary) trajectory and the replay marks it as neither already-filed nor owned by either.

## Evidence
Expected (recorded AWS): (a) HTTP 403 on the write from the es:ESHttpGet-only role and the document absent afterwards; (b) HTTP 200/201 on the identical write from the write-capable role; (c) HTTP 200/201 on the identical write after the write action is added to the restricted role's own identity policy, the domain access policy unchanged. All three answer promptly.

Actual (emulator), verbatim from the replay:
  #### [mismatch] lambda__opensearch__s3.lambda-sdk-opensearch-missing-permission-negative
  Error: Check timed out after 60.0s

No HTTP status or body was produced for any of the three calls. The 60 s figure is the harness cap, not a requirement; the requirement is that the authorization decision be returned at all.

## Steps to Reproduce
1. CreateDomain (EngineVersion OpenSearch_2.11, single t3.small.search node, gp3 10 GiB), with a public non-FGAC access policy that names only a separate control role as principal; wait for Processing=false.
2. Create two Lambda roles: one whose identity policy grants only es:ESHttpGet on arn:aws:es:<region>:<account>:domain/<domain>/*, one that also grants the write action (es:ESHttpPut/es:ESHttpPost).
3. Deploy a Lambda function per role whose handler uses the in-function SDK to PUT a document to the domain endpoint, and Invoke the restricted one: expect HTTP 403 from the data plane and the document absent; observe no response at all.
4. Invoke the write-capable one: expect HTTP 200/201 and the document present.
5. Add the write action to the restricted role's own identity policy (leaving the domain access policy untouched) and re-Invoke: expect the identical write to succeed.
6. Observe the whole trajectory is cut off at the 60 s harness cap with no verdict.
