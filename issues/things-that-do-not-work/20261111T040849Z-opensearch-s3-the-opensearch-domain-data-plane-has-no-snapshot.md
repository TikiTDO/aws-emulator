Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch__s3.snapshot-writes-objects-under-the-declared-base-path-and-delete-removes-them
Issue-Key: opensearch__s3.snapshot-writes-objects-under-the-declared-base-path-and-delete-removes-them|_register_repo#15|AssertionError|raise AssertionError(
Issue-Commit: c0ced5d18fa4eb624de2042a998eca41dbf9ea10

# The OpenSearch domain data plane has no _snapshot handler, so registering an S3 snapshot repository is refused

> _Note: this issue was identified against the code base at commit `c0ced5d18fa4eb624de2042a998eca41dbf9ea10`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PUT /_snapshot/{repository} (OpenSearch domain data plane)

## Description
One net-new root cause.

Cause 1 of 1: the domain's data-plane endpoint does not route the snapshot APIs, so an S3 snapshot repository cannot be registered and no snapshot can be taken or deleted.
AWS: recorded behavior only — the snapshot APIs are domain data-plane REST calls, not Smithy operations. The recording shows PUT /_snapshot/<repo> with {"type":"s3","settings":{"bucket":...,"base_path":...,"role_arn":...}} accepted (HTTP 200 {"acknowledged":true}), a subsequent snapshot writing objects under the declared base_path, and the snapshot delete removing exactly those objects.
Emulator: HTTP 400 with body {'error': 'no handler found for uri [/_snapshot/<repo>] and method [PUT]'} — the route does not exist, which is also not the transient IAM-propagation refusal the check retries on, so the run stops there.
Why: snapshot/restore to S3 is the supported backup path for a domain; a client gets a 400 that looks like a malformed request, cannot register a repository at all, and no snapshot objects are ever written or cleaned up.
Fix: the OpenSearch domain data-plane router — implement the _snapshot repository and snapshot endpoints (register/get/snapshot/delete) and back the s3 repository type with the declared bucket, base_path and role.
Traces: opensearch__s3.snapshot-writes-objects-under-the-declared-base-path-and-delete-removes-them.

## Evidence
Check opensearch__s3.snapshot-writes-objects-under-the-declared-base-path-and-delete-removes-them [mismatch]:
AssertionError: registering the s3 snapshot repository was refused with HTTP 400, which is not the IAM-propagation refusal this retry is for: {'error': 'no handler found for uri [/_snapshot/<repo>] and method [PUT]'}

Expected: HTTP 200 {"acknowledged": true}, then snapshot objects under the declared base_path and their removal on snapshot delete.
Actual: HTTP 400, no such route.

## Steps to Reproduce
1. Create an OpenSearch domain, a bucket and a role the domain can assume for snapshots.
2. PUT https://<domain endpoint>/_snapshot/<repo> with body {"type":"s3","settings":{"bucket":"<bucket>","base_path":"<prefix>","role_arn":"<role>"}}.
3. Observe HTTP 400 {'error': 'no handler found for uri [/_snapshot/<repo>] and method [PUT]'}.
