Issue-Source: trace-sweep-divergence
Issue-Trace: codeartifact-domain-lifecycle
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codeartifact-domain-lifecycle codeartifact-domain-repository-lifecycle codeartifact-domain-tag-lifecycle codeartifact-get-authorization-token-workflow codeartifact-list-repositories-prefix-and-pagination codeartifact-package-group-error-paths codeartifact-package-group-lifecycle-and-hierarchy codeartifact-repository-error-paths codeartifact-repository-tag-lifecycle codeartifact-list-domains-pagination codeartifact-domain-permissions-policy-lifecycle codeartifact-repository-permissions-policy-lifecycle
Issue-Shape-Id: com.amazonaws.codeartifact#CodeArtifactControlPlaneService
Issue-Key: codeartifact|CreateDomain|2xx->2xx|$.domain.s3BucketArn
Issue-Key: codeartifact|DescribeDomain|2xx->2xx|$.domain.s3BucketArn
Issue-Key: codeartifact|DeleteDomain|2xx->2xx|$.domain.s3BucketArn
Issue-Key: codeartifact|CreateDomain|2xx->2xx|$.domain.encryptionKey
Issue-Key: codeartifact|ListDomains|2xx->2xx|$.domains[arn="arn
Issue-Key: codeartifact|DeleteDomain|2xx->2xx|$.domain.encryptionKey
Issue-Key: codeartifact|PutDomainPermissionsPolicy|2xx->2xx|$.policy.revision
Issue-Key: codeartifact|PutRepositoryPermissionsPolicy|2xx->2xx|$.policy.revision
Issue-Key: codeartifact|DeletePackageGroup|2xx->4xx|$
Issue-Key: codeartifact|DeletePackageGroup|2xx->4xx|status
Issue-Key: codeartifact|AssociateExternalConnection|4xx->4xx|$.message
Issue-Commit: 6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492

# CodeArtifact domain, policy and package-group resource fields and idempotent delete diverge

> _Note: this issue was identified against the code base at commit `6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
This issue bundles FIVE distinct net-new root causes found by the simparity-wire-aws-v1 wire replay, all in CodeArtifact's domain/policy/package-group surface: three are derived response FIELDS whose form is wrong (so every read of the resource repeats them), one is an idempotent delete that answers 404 where AWS answers 200, and one is a re-worded 404 message. A fixer must close all five before submitting; each cause names its own failing fixture step. Generated names, key ids, revisions and account numbers quoted below are one run's values filling a template — the FORM and the derivation rule are the requirement, never the literal value.

=== Cause 1 of 5: the domain's s3BucketArn carries an extra random segment ===
AWS: Smithy DomainDescription.s3BucketArn is an Arn — "The Amazon Resource Name (ARN) of the Amazon S3 bucket that is used to store package assets in the domain" — reported by CreateDomain, DescribeDomain, DeleteDomain and the domain readers. Recorded behavior: the value is of the form arn:aws:s3:::assets-<domainOwner account id>-<region> and has NO further segment; it is stable for the domain's whole lifetime (create, describe and delete all report the same string).
Emulator: it appends a random hex segment — arn:aws:s3:::assets-<account>-<region>-<12 hex chars> — minted by crate::state::asset_bucket_arn(&region, &input.domain) at src/ops/create_domain.rs (the s3_bucket_arn field of the stored Domain), and every later projection repeats it.
Why: the bucket ARN is what callers derive S3 permissions, bucket-policy conditions and asset-location assertions from; an ARN with an extra component does not match any policy or test written against AWS's two-part suffix.
Fix: crate::state::asset_bucket_arn in src/state.rs — compose exactly assets-<owner>-<region> with no extra segment.
NOTE on this cause's evidence: the recorded expectation is from a live account, so the account id in it (787052242323) can never equal the emulator's (000000000000); that part of the difference is environmental and is NOT the finding. The finding is the EXTRA segment, which no recorded value has.
Traces: codeartifact-domain-lifecycle#task#1 (CreateDomain; cluster covers 9 steps across the domain fixtures), codeartifact-domain-lifecycle#task#2 (DescribeDomain), codeartifact-domain-lifecycle#teardown#4 (DeleteDomain; cluster covers 9 teardown steps).

=== Cause 2 of 5: the domain reports an encryptionKey that is not the key bound to the domain ===
AWS: DomainDescription.encryptionKey is "The ARN of an Key Management Service (KMS) key associated with a domain", and CreateDomain.encryptionKey is the key the caller names ("a key ID, a key Amazon Resource Name (ARN), a key alias, or a key alias ARN"). The domain's reported key is the key it is actually encrypted with, resolved to a key ARN, and every later read (ListDomains' DomainSummary, DeleteDomain's description) reports that SAME key.
Emulator: the key reported for the domain is a different KMS key ARN from the one the replayed step bound to that domain, and the substituted value is then repeated consistently by ListDomains and DeleteDomain. Hypothesis: src/ops/create_domain.rs falls through to crate::state::default_encryption_key_arn(&region, &input.domain) — a key ARN the emulator synthesises per domain name — instead of resolving/echoing the key the request named, so the domain is reported against a key no KMS store ever created.
Why: callers read this field to grant kms:Decrypt / kms:CreateGrant on the right key, to assert that a domain was created with the CMK they passed, and to correlate a domain with its key in audit output. Pointing at a key the caller never named breaks all three, and the error is invisible until a key-scoped policy fails.
Fix: the encryption_key derivation in src/ops/create_domain.rs plus crate::state::default_encryption_key_arn in src/state.rs — a request that names a key must be reported with that key (resolved to its ARN); only a request naming none may fall back, and the fallback must be a key the emulator's KMS actually holds.
Traces: codeartifact-list-domains-pagination#setup#2 (CreateDomain), codeartifact-list-domains-pagination#task#4 (ListDomains; cluster also covers #task#5), codeartifact-list-domains-pagination#teardown#7 (DeleteDomain).

=== Cause 3 of 5: a resource policy's revision is a 32-character hex digest instead of AWS's base64 digest ===
AWS: ResourcePolicy.revision is PolicyRevision @length(1..100) @pattern(^\S+$), "The current revision of the resource policy", returned by PutDomainPermissionsPolicy and PutRepositoryPermissionsPolicy and accepted back as the policyRevision precondition. Recorded behavior: the token is a 44-character standard-base64 string ending in '=' (a base64-encoded SHA-256 digest), identical in form for domain policies and repository policies.
Emulator: a 32-character lowercase hex string (an MD5-shaped digest) — minted by crate::state::attached_policy(&self.arn, document, self.policy_attachments), called from Domain::attach_policy and Repository::attach_policy in src/state.rs, and derived from a monotonically advancing attachment counter rather than from the document.
Why: clients persist a revision and pass it back as the optimistic-concurrency precondition, and some validate or log its form. A token of a different alphabet and length is not interchangeable with the service's, and a counter-derived token also changes when the SAME document is re-attached, where a digest-derived one does not.
Fix: crate::state::attached_policy in src/state.rs — derive the revision as base64 of the SHA-256 digest of the stored policy document, so both Put*PermissionsPolicy handlers mint a token of AWS's form.
Traces: codeartifact-domain-permissions-policy-lifecycle#task#3 (PutDomainPermissionsPolicy), codeartifact-repository-permissions-policy-lifecycle#task#4 (PutRepositoryPermissionsPolicy).

=== Cause 4 of 5: DeletePackageGroup on an absent group is refused 404 instead of answering 200 ===
AWS: DeletePackageGroup (DELETE /v1/package-group) is @idempotent and its output's packageGroup member is optional. Recorded behavior: deleting a package group that does not exist (here, one never created) answers HTTP 200 with an EMPTY JSON body {} — no packageGroup, no error — i.e. the delete is idempotent in the strong sense and a repeat/no-op delete is a success.
Emulator: HTTP 404 with {'__type': 'ResourceNotFoundException', 'message': "Package group not found. Package group '<pattern>' in domain '<domain>' owned by account '<account>' does not exist.", 'resourceId': '<pattern>'} — raised by the existence guard in src/ops/delete_package_group.rs.
Why: idempotent deletes are what clean-up code, retried provisioning and teardown scripts rely on; against the emulator the second (or speculative) delete raises where AWS succeeds, so a retry after a timeout fails the whole workflow and teardown has to swallow an error AWS never produces.
Fix: src/ops/delete_package_group.rs — an absent (non-root) group must produce the 200 empty-body success the idempotent contract requires, not the modeled ResourceNotFoundException; keep the root-group ValidationException refusal.
Traces: codeartifact-package-group-error-paths#task#5.

=== Cause 5 of 5: AssociateExternalConnection's not-found message is the emulator's own sentence ===
AWS: AssociateExternalConnection (POST /v1/repository/external-connection) takes externalConnection as a free string (the supported values are listed in the member's documentation: public:npmjs, public:pypi, public:maven-central, ...) and models ResourceNotFoundException httpError 404. Recorded behavior: a connection name that is not one the service offers is refused 404 with the service's own message, which the replay carries shape-asserted rather than literally — so the required sentence is the one in the recorded step, not the emulator's.
Emulator: 404 (correct status) with 'The external connection '<name>' does not exist' — a sentence composed by the handler from crate::state::EXTERNAL_CONNECTIONS, which does not satisfy the recorded message assertion.
Why: the message is the only part of this refusal that tells a caller whether the NAME is unsupported or the repository/domain was not found; a client matching or displaying the service sentence diverges.
Fix: the unknown-connection branch of src/ops/associate_external_connection.rs — emit the sentence the recorded step carries (take it verbatim from codeartifact-repository-error-paths#task#7's recorded response) instead of the emulator's own.
Traces: codeartifact-repository-error-paths#task#7.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator booted from this checkout: 344 steps, 272 passed, 72 mismatched, score 79.1%, 30 divergence clusters, all service "codeartifact". No cluster below carries stub_path or owned_by. Verdicts verbatim from the replay output:

Cause 1 — [mismatch] codeartifact-domain-lifecycle#task#1 CreateDomain, 200 -> 200 (9 steps in cluster: also codeartifact-domain-repository-lifecycle#task#1, codeartifact-domain-tag-lifecycle#setup#1, codeartifact-get-authorization-token-workflow#setup#1, codeartifact-list-repositories-prefix-and-pagination#setup#1, codeartifact-package-group-error-paths#setup#1, codeartifact-package-group-lifecycle-and-hierarchy#setup#1, codeartifact-repository-error-paths#setup#1, codeartifact-repository-tag-lifecycle#setup#1): $.domain.s3BucketArn: value_mismatch (expected 'arn:aws:s3:::assets-787052242323-us-west-2', got 'arn:aws:s3:::assets-000000000000-us-west-2-8ab37f6af033'). Same key and same shape for [mismatch] codeartifact-domain-lifecycle#task#2 DescribeDomain and [mismatch] codeartifact-domain-lifecycle#teardown#4 DeleteDomain (9 teardown steps in that cluster). The account segment differs because the recording is from a live account; the emulator-attributable part is the trailing '-8ab37f6af033' segment, which appears in no recorded value.

Cause 2 — [mismatch] codeartifact-list-domains-pagination#setup#2 CreateDomain, 200 -> 200: $.domain.encryptionKey: value_mismatch (expected 'arn:aws:kms:us-west-2:000000000000:key/4222cbfa-e635-008a-e94a-327c402928c2', got 'arn:aws:kms:us-west-2:000000000000:key/314cc66b-e181-29b5-c46f-ae80efc43bbb'). [mismatch] codeartifact-list-domains-pagination#task#4 ListDomains, 200 -> 200 (2 steps): $.domains[arn="arn:aws:codeartifact:us-west-2:000000000000:domain/<domain-b>"].encryptionKey: the same expected/actual pair. [mismatch] codeartifact-list-domains-pagination#teardown#7 DeleteDomain, 200 -> 200: same key, same pair. Both sides are in the emulator's own account 000000000000 (unlike Cause 1's live-account ARN), so the expected key is the one bound during this replay, not a live-run artifact; the emulator reports a different key and repeats it consistently afterwards.

Cause 3 — [mismatch] codeartifact-domain-permissions-policy-lifecycle#task#3 PutDomainPermissionsPolicy, 200 -> 200: $.policy.revision: value_mismatch (expected 'EyPrTkTl3ui3QHCl4CDJXECOsAsV+85VZoMWAVPgkDk=', got '5f6bfb161569dfa4a5af18640fb91592'). [mismatch] codeartifact-repository-permissions-policy-lifecycle#task#4 PutRepositoryPermissionsPolicy, 200 -> 200: $.policy.revision: value_mismatch (expected 'vbFs1ZG1ODLjKJmVpQQVghZUuOq4A58W6iCoAHcfQDQ=', got '303eaac554b49f76f11643249bdcb8c7'). The digests themselves are per-document values; the divergence is the FORM — 44-char base64 ending '=' versus 32-char hex — and it is the same on both policy-bearing resources.

Cause 4 — [mismatch] codeartifact-package-group-error-paths#task#5 DeletePackageGroup, status: expected 200, got 404; $: value_mismatch (expected {}, got {'__type': 'ResourceNotFoundException', 'message': "Package group not found. Package group '/npm/<group>-missing/*' in domain '<domain>' owned by account '000000000000' does not exist.", 'resourceId': '/npm/<group>-missing/*'}).

Cause 5 — [mismatch] codeartifact-repository-error-paths#task#7 AssociateExternalConnection, 404 -> 404: $.message: value_mismatch (expected '<shaped-not-asserted>', got "The external connection 'public:does-not-exist' does not exist").

## Steps to Reproduce
Boot the emulator from this checkout and replay simparity-wire-aws-v1 (each cause's fixture#step is listed under Traces / Issue-Trace-Ids). By hand:
Cause 1: create_domain(domain=D) -> domain.s3BucketArn must be exactly arn:aws:s3:::assets-<owner>-<region> (three colons, two suffix components, no trailing random segment); describe_domain(domain=D) and delete_domain(domain=D) must report the identical string.
Cause 2: create a KMS key K; create_domain(domain=D, encryptionKey=<K's ARN>) -> domain.encryptionKey must be K's key ARN; list_domains() and delete_domain(domain=D) must report the same key. Observe a different, synthesised key ARN.
Cause 3: create_domain(domain=D); put_domain_permissions_policy(domain=D, policyDocument=P) -> policy.revision must be a 44-character base64 token ending '=' (base64 of the document's SHA-256), and re-attaching the identical document must mint the same token; repeat with create_repository + put_repository_permissions_policy. Observe a 32-character hex counter-derived token.
Cause 4: create_domain(domain=D); delete_package_group(domain=D, packageGroup='/npm/never-created/*') -> expect HTTP 200 with an empty JSON body {}; observe 404 ResourceNotFoundException. The same must hold for a second delete of a group that was just deleted.
Cause 5: create_domain(domain=D) and create_repository(domain=D, repository=R); associate_external_connection(domain=D, repository=R, externalConnection='public:does-not-exist') -> expect 404 with the service's message as recorded in codeartifact-repository-error-paths#task#7; observe "The external connection 'public:does-not-exist' does not exist".
