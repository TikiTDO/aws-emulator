Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__opensearch__s3.lambda-sdk-opensearch-domain-access-policy-denies-negative
Issue-Key: lambda__opensearch__s3.lambda-sdk-opensearch-domain-access-policy-denies-negative|_invoke#17|Timeout|resp = lam.invoke(FunctionName=function_name, Payload=json.dumps(payload).encode("utf-8"))
Issue-Commit: 2d1f8bf5471c7181f318f860d2c7fc1188aa0d11

# lambda__opensearch__s3: an explicit Deny in the OpenSearch domain access policy is never enforced observably — the collide-table trajectory (IAM Allow vs resource-policy Deny) hangs to the harness cap (1 cause)

> _Note: this issue was identified against the code base at commit `2d1f8bf5471c7181f318f860d2c7fc1188aa0d11`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke (Lambda) against the OpenSearch domain REST data plane under AccessPolicies (CreateDomain / UpdateDomainConfig AccessPolicies, es:ESHttpPut / es:ESHttpGet)

## Description
One cause.

Cause 1 of 1: a domain access policy whose statement explicitly DENIES a principal's write action is never enforced observably — the whole trajectory produces no response at all and is cut off at the harness's 60 s cap. This is the resource-policy-Deny-beats-identity-Allow case of AWS's documented policy-collision table, and it is distinct from the two scenarios already filed: open issue 20261110T210703Z-lambda-opensearch-s3-an-opensearch-domain-access-policy-bb8562.md covers a policy that merely OMITS the role as a principal, and open issue 20261111T041122Z-lambda-opensearch-s3-the-identity-policy-permission-boundary.md covers the IDENTITY-policy (permission-boundary) contrast; neither exercises an explicit Deny against a broad IAM Allow, and the replay marks this check as neither already-filed nor owned.

AWS: recorded behavior only for the data-plane leg (the domain's HTTP data plane is outside the opensearch Smithy model; the model covers CreateDomain and UpdateDomainConfig and their AccessPolicies document). Per AWS's documented resource-vs-identity collision rule (opensearch-service developerguide ac.html#when-policies-collide), an explicit Deny in the domain's resource-based access policy always wins over an Allow in IAM. The recording shows, in order: (a) with the role's own IAM policy granting es:ESHttp* on arn:aws:es:<region>:<account>:domain/<domain>/* but the domain access policy carrying an explicit Deny on es:ESHttpPut for that principal, the SigV4-signed document write is refused with HTTP 403 and the document never lands (a subsequent GET by the same role — which the policy still allows — answers HTTP 404); (b) after the access policy is replaced with a plain Allow on es:ESHttp* for the same principal, with IAM untouched, the identical write answers HTTP 200/201 and the document reads back with its exact _source. Every one of those calls returns promptly.
Emulator: the check never reaches any of those verdicts — the replay reports only "Error: Check timed out after 60.0s". No 403, no 404, no 200/201 and no document state is ever observable, so neither the Deny leg nor the positive control after the policy is loosened produces an answer within a time a caller would wait.
Why: a client that uses the domain access policy as its authorization boundary gets no signal at all. A write AWS refuses in milliseconds with HTTP 403 instead blocks until the caller's own timeout, which a caller reads as an availability failure rather than as a permission denial, and it then retries a request that may or may not have taken effect. The strongest guarantee in the policy model — that an explicit Deny in the resource policy overrides any IAM Allow — cannot be verified at all, and an explicitly denied principal may in fact be writing.
Fix: the OpenSearch domain data plane's AccessPolicies evaluation for SigV4-signed requests (explicit Deny must short-circuit to a 403 refusal with no write, and an Allow must serve the request) together with whatever leaves the request outstanding on the Lambda Invoke → domain-endpoint path, so the authorization decision is returned instead of the request hanging.
Traces: lambda__opensearch__s3.lambda-sdk-opensearch-domain-access-policy-denies-negative.

## Evidence
Expected (recorded AWS): (a) HTTP 403 on the PUT of /<index>/_doc/<id>?refresh=true from a Lambda role whose IAM policy grants es:ESHttp* while the domain access policy explicitly Denies es:ESHttpPut for that principal, and the document absent afterwards (GET answers HTTP 404, taken by the same role which the policy still allows to GET); (b) after UpdateDomainConfig replaces AccessPolicies with a plain Allow on es:ESHttp* for the same principal, the identical write answers HTTP 200/201 and the GET answers HTTP 200 with the written _source. All calls answer promptly.

Actual (emulator), verbatim from the replay:
  #### [mismatch] lambda__opensearch__s3.lambda-sdk-opensearch-domain-access-policy-denies-negative
  Error: Check timed out after 60.0s

No HTTP status or body was produced for any leg. The 60 s figure is the harness cap and is not itself a requirement; the requirement is that the authorization decision (403 on explicit Deny, 200/201 once allowed) be returned at all.

## Steps to Reproduce
1. Create an IAM role trusted by lambda.amazonaws.com whose inline policy grants Allow es:ESHttp* on arn:aws:es:<region>:<account>:domain/<domain>/*.
2. CreateDomain (EngineVersion OpenSearch_2.11, single t3.small.search node, DedicatedMasterEnabled=false, ZoneAwarenessEnabled=false, gp3 10 GiB) with AccessPolicies containing two statements for that role as principal: Allow es:ESHttpGet on <domain-arn>/* and Deny es:ESHttpPut on <domain-arn>/*. Wait for Processing=false and read DomainStatus.Endpoint.
3. Deploy a Lambda function with that role whose handler SigV4-signs ('es') and issues an arbitrary HTTP request to the domain endpoint.
4. Invoke it with method=PUT, path=/<index>/_doc/blocked?refresh=true — expect HTTP 403 from the data plane; observe no response at all.
5. Invoke it with method=GET, path=/<index>/_doc/blocked — expect HTTP 404 (the write never landed).
6. UpdateDomainConfig with AccessPolicies = a single Allow es:ESHttp* for the same principal, leaving IAM unchanged; re-invoke the identical PUT — expect HTTP 200/201 — then the GET — expect HTTP 200 with _source {"v": 1}.
7. Observe the whole trajectory is cut off at the 60 s harness cap with no verdict.
