Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__iam__opensearch__s3.xsvc-opensearch-1-firehose-fgac-strict-mapping-indexes-only-conforming-documents
Issue-Key: firehose__iam__opensearch__s3.xsvc-opensearch-1-firehose-fgac-strict-mapping-indexes-only-conforming-documents|_wait_admin#18|AssertionError|raise AssertionError(f"the IAM admin never got 200 on /_cluster/health within {timeout_s}s "
Issue-Commit: 3c33b9e3725ab606ee2a23453a6abf264aabc19e

# firehose__iam__opensearch__s3: the OpenSearch domain endpoint serves no data plane — a SigV4 (`es`) GET /_cluster/health on the domain's published host is answered 404 UnknownService, so Firehose-to-OpenSearch strict-mapping delivery can never be observed

> _Note: this issue was identified against the code base at commit `3c33b9e3725ab606ee2a23453a6abf264aabc19e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDomain

## Description
Cause 1 of 1: the host an OpenSearch domain publishes as its endpoint is not routed to any OpenSearch data plane. The emulator's request router does not recognise the `es` signing name / the per-domain endpoint host, so every REST request on that endpoint is answered with a service-level UnknownService 404 instead of the OpenSearch REST API.

AWS: Smithy (services/service-com-amazonaws-opensearch-amazonopensearchservice) — DescribeDomain GET /2021-01-01/opensearch/domain/{DomainName} returns DomainStatus (required), whose Endpoint/Endpoints members carry the domain's data-plane host. That host serves the OpenSearch/Elasticsearch REST API, authenticated with SigV4 under signing name `es`. `GET https://<endpoint>/_cluster/health` signed by a principal mapped as the domain's fine-grained-access-control master/admin answers HTTP 200 with a cluster-health JSON document; a principal that is not mapped is refused 403 by the domain's security plugin with its own OpenSearch-shaped error body. Index/mapping, _bulk and _search paths on that same host behave likewise. The model says nothing about the data plane itself (it is not an AWS-API surface) — recorded behavior only for the data plane: the check's IAM admin role reaches HTTP 200 on /_cluster/health within the poll budget.

Emulator: the request never reaches an OpenSearch data plane. The router answers HTTP 404 with body `{"__type": "UnknownService", "message": "no emulated service is registered for 'es'"}`. Fixed text of that message is `no emulated service is registered for '<signing-name>'`, with the signing name taken from the request's SigV4 credential scope (here `es`); the `__type` is the router's UnknownService, not anything the OpenSearch REST API or the opensearch control plane can return. The check polls for its full 420 s budget and never sees a 200, so it fails with `AssertionError: the IAM admin never got 200 on /_cluster/health within 420.0s`.

Why: a client that reads the Endpoint from DescribeDomain and talks to it — the only way the OpenSearch data plane is addressed — gets a 404 for a service that does not exist rather than cluster health. Concretely it cannot install a strict index mapping, cannot read back which documents were indexed, and cannot tell "Firehose delivered nothing" apart from "the domain is not answering". That makes the Firehose→OpenSearch behaviour this check pins (only mapping-conforming documents are indexed, non-conforming ones are routed to the S3 backup) unobservable. The 404/UnknownService shape is also wrong for an authorization outcome: an unmapped principal must be refused 403 by the domain, not 404 by the router.

Fix: the emulator's request router / endpoint resolution must bind the `es` signing name and each domain's published endpoint host to an OpenSearch data-plane handler (cluster health, index + mapping, _bulk, _search) with the fine-grained-access-control authorization in front of it, so DescribeDomain's Endpoint is an address that answers.

Related open issue: 20261111T103332Z-firehose-iam-opensearch-s3-the-opensearch-domain-endpoint.md reports this same unrouted-endpoint fault, but carries only the replay keys for `...routes-conflicts-to-s3` and `...unmapped-role-denied-all-records-to-s3`; it does not carry this check's key, so this check is not covered by it. 20261102T091623Z-opensearch-a-created-domain-s-endpoint-never-answers.md reports it from the opensearch section and likewise does not carry this key.

Traces: firehose__iam__opensearch__s3.xsvc-opensearch-1-firehose-fgac-strict-mapping-indexes-only-conforming-documents (the assertion is raised at _wait_admin, check file check_firehose__iam__opensearch__s3_xsvc_opensearch_1_firehose_fgac_strict_mapping_indexes_only_conforming_documents.py line 257, reached from run at line 432).

## Evidence
Behavioral suite replay (simparity-behavioral-aws-v1), `### firehose__iam__opensearch__s3` section: 0/3 passed, 3 failed (the other two are OWNED by 20261111T103332Z-firehose-iam-opensearch-s3-the-opensearch-domain-endpoint.md and were skipped).

[mismatch] firehose__iam__opensearch__s3.xsvc-opensearch-1-firehose-fgac-strict-mapping-indexes-only-conforming-documents
  File "check_firehose__iam__opensearch__s3_xsvc_opensearch_1_firehose_fgac_strict_mapping_indexes_only_conforming_documents.py", line 432, in run -> _wait_admin(ctx, host, admin)
  File "...", line 257, in _wait_admin
  AssertionError: the IAM admin never got 200 on /_cluster/health within 420.0s (last HTTP 404: {'__type': 'UnknownService', 'message': "no emulated service is registered for 'es'"})

Expected (AWS): HTTP 200 and a cluster-health JSON body for the FGAC-mapped admin role on GET /_cluster/health of the host DescribeDomain published in DomainStatus.Endpoint; HTTP 403 from the domain's security plugin for a principal that is not mapped.

Actual (emulator): HTTP 404, body {"__type": "UnknownService", "message": "no emulated service is registered for 'es'"} — on every one of the ~420 s of poll attempts. The check signs the request itself (botocore SigV4Auth with service name `es`, ctx.region) and sends it over a TLS socket it opens to ctx.endpoint_route(host), so no SDK or runner tooling is involved: the router simply has no `es` service registered.

## Steps to Reproduce
1. git_clone the repo; build and boot the emulator from the checkout.
2. Create an OpenSearch domain with fine-grained access control naming an IAM role as master, and create a second IAM role mapped as admin.
3. Call DescribeDomain and read DomainStatus.Endpoint.
4. AssumeRole the admin role, SigV4-sign (service name `es`) a `GET https://<Endpoint>/_cluster/health` and send it. AWS answers 200 with cluster health; the emulator answers 404 with {"__type":"UnknownService","message":"no emulated service is registered for 'es'"}.
5. Replay firehose__iam__opensearch__s3.xsvc-opensearch-1-firehose-fgac-strict-mapping-indexes-only-conforming-documents from the behavioral suite; it exhausts its 420 s wait on that 404 before it can install the strict index mapping or assert which documents Firehose indexed.
