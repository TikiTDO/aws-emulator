Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__iam__opensearch__s3.xsvc-opensearch-1-firehose-fgac-strict-mapping-routes-conflicts-to-s3 firehose__iam__opensearch__s3.xsvc-opensearch-1-firehose-unmapped-role-denied-all-records-to-s3
Issue-Key: firehose__iam__opensearch__s3.xsvc-opensearch-1-firehose-fgac-strict-mapping-routes-conflicts-to-s3|_wait_master#18|AssertionError|raise AssertionError(f"the IAM master never got 200 on /_cluster/health within {timeout_s}s "
Issue-Key: firehose__iam__opensearch__s3.xsvc-opensearch-1-firehose-unmapped-role-denied-all-records-to-s3|_wait_admin#20|AssertionError|raise AssertionError(f"the IAM admin role never got 200 on /_cluster/health within {timeout_s}s "
Issue-Commit: 94350cf775acbc496950503935930c48964de18f

# firehose__iam__opensearch__s3: the OpenSearch domain endpoint is not served — a SigV4 request to the created domain's host is answered 404 UnknownService "no emulated service is registered for 'es'"

> _Note: this issue was identified against the code base at commit `94350cf775acbc496950503935930c48964de18f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDomain

## Description
Cause 1 of 1: the data-plane host an OpenSearch domain reports has no OpenSearch data plane behind it; the emulator's service router does not recognise the `es` signing name and answers an UnknownService 404 to every request on that endpoint.
AWS: CreateDomain / DescribeDomain report the domain's `Endpoint` (or `Endpoints`), and that host serves the OpenSearch/Elasticsearch REST API, authenticated with SigV4 for signing name `es`. `GET https://<endpoint>/_cluster/health` signed by a principal mapped as the domain's master/admin in fine-grained access control answers HTTP 200 with a cluster-health JSON body; an unmapped principal is answered 403 by the domain's security plugin. Index, bulk and search paths on that same host behave likewise. Recorded behavior for these checks: the IAM master role reaches 200 on /_cluster/health within the poll budget.
Emulator: the request to the domain endpoint never reaches an OpenSearch data plane at all. The router answers HTTP 404 with body `{"__type": "UnknownService", "message": "no emulated service is registered for 'es'"}` — i.e. the signing name/host is unrouted, so the domain's REST surface (cluster health, index mapping, bulk, search) does not exist. Both checks spend their full 420 s budget polling and never get a 200.
Why: every workflow that reads or writes a domain through its endpoint is unreachable, and that is the only way the OpenSearch data plane is addressed. Here it blocks the whole Firehose-to-OpenSearch fidelity story: a client cannot install a strict index mapping, cannot confirm which records Firehose indexed, and cannot distinguish "Firehose delivered nothing" from "the domain is not answering" — so the S3 backup-routing behaviour these checks exist to pin (conflicting documents routed to the S3 backup bucket; an unmapped delivery role sending all records to S3) can never be observed. The 404 is also the wrong shape: an unroutable host is reported as a service-level UnknownService error rather than anything the OpenSearch REST API can return.
Fix: the emulator's request router / endpoint resolution for OpenSearch domain endpoints — the `es` signing name and the per-domain endpoint host must be routed to an OpenSearch data-plane handler (cluster health, index/mapping, bulk, search, and the fine-grained-access-control authorization in front of them).
Related open issue: 20261102T091623Z-opensearch-a-created-domain-s-endpoint-never-answers.md reports the same unanswered-endpoint fault from the `iam__opensearch__sts` section; it does not carry these two firehose check ids, so neither of these checks is covered by it.
Traces: firehose__iam__opensearch__s3.xsvc-opensearch-1-firehose-fgac-strict-mapping-routes-conflicts-to-s3, firehose__iam__opensearch__s3.xsvc-opensearch-1-firehose-unmapped-role-denied-all-records-to-s3.

## Evidence
Behavioral suite replay (simparity-behavioral-aws-v1), `### firehose__iam__opensearch__s3` section (0/3 passed).

[mismatch] firehose__iam__opensearch__s3.xsvc-opensearch-1-firehose-fgac-strict-mapping-routes-conflicts-to-s3
  AssertionError: the IAM master never got 200 on /_cluster/health within 420.0s (last HTTP 404: {'__type': 'UnknownService', 'message': "no emulated service is registered for 'es'"})

[mismatch] firehose__iam__opensearch__s3.xsvc-opensearch-1-firehose-unmapped-role-denied-all-records-to-s3
  AssertionError: the IAM admin role never got 200 on /_cluster/health within 420.0s (last HTTP 404: {'__type': 'UnknownService', 'message': "no emulated service is registered for 'es'"})

Both checks SigV4-sign the request themselves (botocore SigV4Auth over the endpoint host DescribeDomain reported) and poll /_cluster/health; the response is the router's UnknownService 404 on every attempt, so the signing name `es` is never routed.

## Steps to Reproduce
1. git_clone the repo; build and boot the emulator from the checkout.
2. Create an OpenSearch domain with fine-grained access control and an IAM master role; read its Endpoint from DescribeDomain.
3. SigV4-sign (service name `es`) a GET https://<endpoint>/_cluster/health as that master role. AWS answers 200 with cluster health; the emulator answers 404 {"__type":"UnknownService","message":"no emulated service is registered for 'es'"}.
4. Replay firehose__iam__opensearch__s3.xsvc-opensearch-1-firehose-fgac-strict-mapping-routes-conflicts-to-s3 and firehose__iam__opensearch__s3.xsvc-opensearch-1-firehose-unmapped-role-denied-all-records-to-s3 from the behavioral suite; both time out at 420 s on that 404.
