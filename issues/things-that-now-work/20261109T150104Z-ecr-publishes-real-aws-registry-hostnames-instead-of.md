Issue-Source: trace-sweep-divergence
Issue-Trace: api-image-push-via-layer-upload
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: api-image-push-via-layer-upload api-image-scanning-and-findings api-immutable-tag-conflict-and-image-deletion api-lifecycle-policy-preview-and-apply api-registry-policy-and-scanning-configuration api-repository-lifecycle-with-policy-and-tags ecr-duplicate-push-and-tag-based-storage-class-update ecr-image-and-repo-not-found-errors ecr-image-lookup-errors ecr-image-push-and-inspect ecr-immutable-tag-conflict ecr-layer-upload-lifecycle-and-errors ecr-multi-image-isolation-in-shared-repository ecr-push-multi-layer-image ecr-put-image-missing-layers-error ecr-put-image-missing-layers-error-then-recover ecr-repository-create-verify-delete ecr-repository-lifecycle-and-errors ecr-ecracc-cross-account-registry-id-reads ecr-repository-pagination-and-scoping-conflict api-account-settings-and-authorization-token
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr|CreateRepository|2xx->2xx|$.repository.repositoryUri
Issue-Key: ecr|DeleteRepository|2xx->2xx|$.repository.repositoryUri
Issue-Key: ecr|DescribeRepositories|2xx->2xx|$.repositories[*].repositoryUri
Issue-Key: ecr|GetAuthorizationToken|2xx->2xx|$.authorizationData[*].proxyEndpoint
Issue-Commit: d115796fe5713602f1adc3e09d553e6de30aa6e5

# ECR publishes real-AWS registry hostnames instead of the emulator's own endpoint in repositoryUri and proxyEndpoint

> _Note: this issue was identified against the code base at commit `d115796fe5713602f1adc3e09d553e6de30aa6e5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRepository

## Description
Four net-new causes, one root fault: every ECR response member that announces a registry address is serialized as the real-AWS DNS name '<accountId>.dkr.ecr.<region>.amazonaws.com' rather than the host/port the emulator actually serves, so the replay harness rejects the announced address as not one of the emulator's own hosts (verdict host_not_own) on each of the four operations below. The same root cause is already described for the behavioral corpus in the open issue 20261109T141609Z-cloudwatch-ecr-the-ecr-registry-data-plane-address.md (and for ECR Public in 20261109T130943Z-ecr-public-putrepositorycatalogdata-mints-logourl-on-the-real.md); neither carries the replay keys of these wire fixture steps, so the wire causes are filed here.

Cause 1 of 4 — CreateRepository.repository.repositoryUri (fixture api-image-push-via-layer-upload, step setup#1; 18 fixtures affected).
AWS: repositoryUri is the address a client uses to push/pull that repository's images, i.e. the registry endpoint that actually answers the Docker Registry v2 API for the account and region. Smithy (ECR): CreateRepository -> output member repository (shape Repository), Repository$repositoryUri documented as the URI for the repository, of the form <aws_account_id>.dkr.ecr.<region>.amazonaws.com/<repositoryName>; for an emulator the requirement is the reachable-address part of that contract (recorded behavior: the recording's URI is the recording account's real registry host, which is why the comparator asserts ownership of the host rather than byte equality).
Emulator: returns '<accountId>.dkr.ecr.<region>.amazonaws.com/<repositoryName>' verbatim — a hostname the emulator does not listen on. Status is 200 and the shape is otherwise correct; only the host is wrong.
Why: any client that follows the address the API handed it (docker push/pull, a registry v2 client, a build pipeline deriving the image reference from repositoryUri) is sent to AWS's real DNS name instead of the emulator, so every image push/pull built on the create response fails or silently leaves the emulator.
Fix: the ECR repository serializer / endpoint-URI construction (emit the emulator's own registry listener host and port, with the repository path appended).

Cause 2 of 4 — DeleteRepository.repository.repositoryUri (fixture api-image-push-via-layer-upload, step teardown#14; 18 fixtures affected).
AWS: DeleteRepository echoes the deleted repository, including the same repositoryUri contract as above. Smithy (ECR): DeleteRepository -> output member repository (shape Repository).
Emulator: same real-AWS host emitted in the echoed repository, rejected as host_not_own.
Why: a client that records or logs the deleted repository's address, or that re-creates and reuses it, keeps a non-servable AWS hostname.
Fix: same serializer as cause 1 — the URI must be built once, from the emulator's own endpoint, for every Repository the service returns.

Cause 3 of 4 — DescribeRepositories.repositories[].repositoryUri (fixture api-repository-lifecycle-with-policy-and-tags, step task#2; 4 fixtures affected).
AWS: each entry of repositories carries the same reachable repositoryUri. Smithy (ECR): DescribeRepositories -> output member repositories (RepositoryList of Repository).
Emulator: every list entry carries the real-AWS dkr.ecr host, rejected as host_not_own.
Why: the list read is the normal way a client discovers where to pull from; the discovery result points at AWS, not the emulator.
Fix: same serializer as cause 1, applied on the list path.

Cause 4 of 4 — GetAuthorizationToken.authorizationData[].proxyEndpoint (fixture api-account-settings-and-authorization-token, step task#6).
AWS: proxyEndpoint is the registry URL to use with the returned token in a 'docker login' command. Smithy (ECR): AuthorizationData$proxyEndpoint — 'The registry URL to use for this authorization token in a docker login command. The Amazon ECR registry URL format is https://aws_account_id.dkr.ecr.region.amazonaws.com'. For an emulator the requirement is that the URL names the registry endpoint the emulator serves.
Emulator: returns 'https://<accountId>.dkr.ecr.<region>.amazonaws.com', rejected as host_not_own.
Why: 'docker login $(proxyEndpoint)' and every credential-helper flow authenticates against real AWS instead of the emulator, so the token the emulator minted is never presented to it and no authenticated push/pull is possible.
Fix: the GetAuthorizationToken handler/serializer (build proxyEndpoint from the emulator's own registry endpoint, scheme included).

Traces: api-image-push-via-layer-upload#setup#1, api-image-push-via-layer-upload#teardown#14, api-repository-lifecycle-with-policy-and-tags#task#2, api-account-settings-and-authorization-token#task#6 (clusters also cover 18 + 18 + 4 + 1 fixture steps respectively).

## Evidence
All four clusters are verdict=mismatch with matching HTTP status (200 -> 200); the divergence is the announced host.

Cause 1 (CreateRepository, api-image-push-via-layer-upload#setup#1): detail '$.repository.repositoryUri: host_not_own (expected "000000000000.dkr.ecr.us-west-2.amazonaws.com/<repositoryName>", got "000000000000.dkr.ecr.us-west-2.amazonaws.com/<repositoryName>")' — the strings are textually identical because the emulator copies AWS's DNS name; the assertion is host ownership, and the emulator does not serve that host. affected_count=18.
Cause 2 (DeleteRepository, api-image-push-via-layer-upload#teardown#14): same detail on '$.repository.repositoryUri'. affected_count=18.
Cause 3 (DescribeRepositories, api-repository-lifecycle-with-policy-and-tags#task#2): same detail on '$.repositories[0].repositoryUri'. affected_count=4.
Cause 4 (GetAuthorizationToken, api-account-settings-and-authorization-token#task#6): detail '$.authorizationData[0].proxyEndpoint: host_not_own (expected "<shaped-not-asserted>", got "https://000000000000.dkr.ecr.us-west-2.amazonaws.com")'.

Expected: every announced registry address (repositoryUri, proxyEndpoint, and any other registry endpoint ECR publishes) is a host and port the emulator listens on, with the account/region-shaped path or prefix preserved. Actual: the real-AWS dkr.ecr.<region>.amazonaws.com hostname, which the emulator does not serve.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Re-replay api-image-push-via-layer-upload step setup#1 (CreateRepository) and teardown#14 (DeleteRepository), api-repository-lifecycle-with-policy-and-tags step task#2 (DescribeRepositories) and api-account-settings-and-authorization-token step task#6 (GetAuthorizationToken).
4. Or by hand: CreateRepository, then read repository.repositoryUri; call GetAuthorizationToken and read authorizationData[0].proxyEndpoint. Both name <account>.dkr.ecr.<region>.amazonaws.com — a host the emulator does not answer on — instead of the emulator's own registry listener.
