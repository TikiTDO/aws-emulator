Issue-Source: trace-sweep-divergence
Issue-Trace: codeartifact-domain-lifecycle
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codeartifact-domain-lifecycle codeartifact-domain-repository-lifecycle codeartifact-domain-tag-lifecycle codeartifact-get-authorization-token-workflow codeartifact-list-repositories-prefix-and-pagination codeartifact-package-group-error-paths codeartifact-package-group-lifecycle-and-hierarchy codeartifact-repository-error-paths codeartifact-repository-tag-lifecycle
Issue-Shape-Id: com.amazonaws.codeartifact#CodeArtifactControlPlaneService
Issue-Key: codeartifact|CreateDomain|2xx->2xx|$.domain.s3BucketArn
Issue-Key: codeartifact|DescribeDomain|2xx->2xx|$.domain.s3BucketArn
Issue-Key: codeartifact|DeleteDomain|2xx->2xx|$.domain.s3BucketArn
Issue-Commit: 09fb1164602e6ebdd2bd6e78b61b67ab44fe1e77

# CodeArtifact domain s3BucketArn is derived from the caller account instead of the service-owned asset bucket

> _Note: this issue was identified against the code base at commit `09fb1164602e6ebdd2bd6e78b61b67ab44fe1e77`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
THREE root causes, all one shared fault, found by replaying the simparity-wire-aws-v1 wire corpus against the emulator booted from this checkout: every operation that returns a DomainDescription renders s3BucketArn from the CALLER's account id, whereas the service renders the ARN of the CodeArtifact-owned, region-scoped asset bucket, whose account component is NOT the caller's. A fixer must close all three causes (they are three handlers sharing one DomainDescription projection) before submitting. (All domain names, account ids and regions quoted below are one run's values filling a template - only the fixed structure and the rule for the variable parts are the requirement.)

=== Cause 1 of 3: CreateDomain's returned domain.s3BucketArn carries the caller account instead of the asset-bucket account ===
AWS: Smithy CreateDomain (POST /v1/domain, success 200) returns CreateDomainResult{domain: DomainDescription}. DomainDescription.s3BucketArn targets Arn (@length 1..1011, @pattern '^\\S+$') and is documented as "The Amazon Resource Name (ARN) of the Amazon S3 bucket that is used to store package assets in the domain" - note it is the ARN of the bucket that STORES the assets, a bucket CodeArtifact itself owns, distinct from DomainDescription.owner (AccountId, @pattern '^[0-9]{12}$'), which is "The Amazon Web Services account ID that owns the domain". The model does not state the bucket-naming rule, so the value is recorded behavior only: the recording returns s3BucketArn of the form 'arn:aws:s3:::assets-<assetBucketAccount>-<region>', where <region> is the domain's region and <assetBucketAccount> is a 12-digit account that is NOT the domain owner. The same recorded response passes on domain.owner and domain.arn, both of which carry the caller account - i.e. the recording carries TWO different account ids in one response, which is what establishes that the s3BucketArn account component is not the caller's.
Emulator: status 200 and every other DomainDescription member match; s3BucketArn is built by substituting the caller's own account id into the bucket name, yielding 'arn:aws:s3:::assets-000000000000-us-west-2' where the recording has 'arn:aws:s3:::assets-787052242323-us-west-2'. Fixed text of the value (the requirement): 'arn:aws:s3:::assets-' + <assetBucketAccount> + '-' + <region>. Variable parts: the region, taken from the endpoint the domain was created against, and the asset-bucket account, which is a CodeArtifact-owned constant for that region - NOT a value read from the request or from the caller's identity. Hypothesis: the DomainDescription projection formats the bucket name from the same account field it uses for owner/arn, so the two can never differ.
Why: s3BucketArn is how a client discovers where a domain's package assets live. A client that reads it to grant a bucket policy, to scope an IAM statement, to audit asset storage, or merely to assert that the asset bucket is not in its own account, gets an ARN naming a bucket in the caller's account - a bucket that does not exist and that the service does not use. Because the wrong value is derived from the caller, it is self-consistent and silently plausible: it looks like a valid ARN and only fails when something tries to resolve or authorize against it.
Fix: the DomainDescription construction used by the CreateDomain handler, and the domain state that stores/derives the asset-bucket identity, in the codeartifact service crate - the asset-bucket account must be a region-scoped service-owned constant held separately from the domain owner, not the request's account id.
Traces: codeartifact-domain-lifecycle#task#1, with 8 further cluster members on the same cause (codeartifact-domain-repository-lifecycle#task#1, codeartifact-domain-tag-lifecycle#setup#1, codeartifact-get-authorization-token-workflow#setup#1, codeartifact-list-repositories-prefix-and-pagination#setup#1, codeartifact-package-group-error-paths#setup#1, codeartifact-package-group-lifecycle-and-hierarchy#setup#1, codeartifact-repository-error-paths#setup#1, codeartifact-repository-tag-lifecycle#setup#1).

=== Cause 2 of 3: DescribeDomain's returned domain.s3BucketArn carries the caller account instead of the asset-bucket account ===
AWS: Smithy DescribeDomain (GET /v1/domain, success 200, httpQuery members domain + domainOwner) returns DescribeDomainResult{domain: DomainDescription}, the same shape as CreateDomain, so the same s3BucketArn rule applies: a read of an existing domain reports the asset bucket the service actually stores that domain's assets in, 'arn:aws:s3:::assets-<assetBucketAccount>-<region>', whose account component is not the domain owner. Recorded behavior only for the naming rule; the status (200) and the error set (403/500/404/429/400) are modeled.
Emulator: status 200 and the rest of the DomainDescription match; s3BucketArn again substitutes the caller account - 'arn:aws:s3:::assets-000000000000-us-west-2' instead of 'arn:aws:s3:::assets-787052242323-us-west-2'. So the defect is not confined to the create path: it is the projection every read shares, meaning the wrong value is also what a client re-reading the domain sees.
Why: DescribeDomain is the canonical way to discover a domain's asset bucket after the fact (the create call's response is usually not retained). A client that resolves the bucket at use time rather than at create time gets the same non-existent caller-account bucket, so no amount of re-reading recovers the correct value.
Fix: the same DomainDescription projection, reached from the DescribeDomain handler in the codeartifact service crate.
Traces: codeartifact-domain-lifecycle#task#2 (the only member of its cluster).

=== Cause 3 of 3: DeleteDomain's returned domain.s3BucketArn carries the caller account instead of the asset-bucket account ===
AWS: Smithy DeleteDomain (DELETE /v1/domain, success 200, httpQuery members domain + domainOwner) returns DeleteDomainResult{domain: DomainDescription} - the deleted domain's final description, including s3BucketArn under the same rule: 'arn:aws:s3:::assets-<assetBucketAccount>-<region>' with a non-caller account component. Recorded behavior only for the naming rule.
Emulator: status 200, correct deletion semantics, but the echoed description again reports 'arn:aws:s3:::assets-000000000000-us-west-2' where the recording has 'arn:aws:s3:::assets-787052242323-us-west-2'.
Why: the delete response is what cleanup and audit tooling records as the domain's last known state, including which bucket held its assets. Reporting a caller-account bucket there writes a wrong fact into exactly the record that outlives the domain.
Fix: the same DomainDescription projection, reached from the DeleteDomain handler in the codeartifact service crate - fixing the shared projection should close all three causes at once, but each must be re-replayed, because each is a separate cluster.
Traces: codeartifact-domain-lifecycle#teardown#4, with 8 further cluster members on the same cause (codeartifact-domain-repository-lifecycle#teardown#10, codeartifact-domain-tag-lifecycle#teardown#7, codeartifact-get-authorization-token-workflow#teardown#4, codeartifact-list-repositories-prefix-and-pagination#teardown#10, codeartifact-package-group-error-paths#teardown#8, codeartifact-package-group-lifecycle-and-hierarchy#teardown#23, codeartifact-repository-error-paths#teardown#9, codeartifact-repository-tag-lifecycle#teardown#9).

No open issue carries any of these three replay keys (the replay output reports owned_by empty and stub_path empty for all three clusters). The open CodeArtifact issues about GetRepositoryEndpoint hosts, package-group descriptions, failedVersions messages and package-version error statuses are different fields and different operations.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator booted from this checkout: 34 fixtures, 344 steps, 305 passed, 39 mismatched, 0 errored, 0 blocked, score 88.7%, 14 divergence clusters - all on service "codeartifact". Three of those clusters carry neither stub_path nor owned_by and are the three causes below (18 affected steps across 9 fixtures). Verdicts verbatim from the replay output:

[mismatch] codeartifact-domain-lifecycle#task#1 CreateDomain, status 200 -> 200, affected_count 9:
  $.domain.s3BucketArn: value_mismatch (expected 'arn:aws:s3:::assets-787052242323-us-west-2', got 'arn:aws:s3:::assets-000000000000-us-west-2')

[mismatch] codeartifact-domain-lifecycle#task#2 DescribeDomain, status 200 -> 200, affected_count 1:
  $.domain.s3BucketArn: value_mismatch (expected 'arn:aws:s3:::assets-787052242323-us-west-2', got 'arn:aws:s3:::assets-000000000000-us-west-2')

[mismatch] codeartifact-domain-lifecycle#teardown#4 DeleteDomain, status 200 -> 200, affected_count 9:
  $.domain.s3BucketArn: value_mismatch (expected 'arn:aws:s3:::assets-787052242323-us-west-2', got 'arn:aws:s3:::assets-000000000000-us-west-2')

What is the requirement: the value's structure 'arn:aws:s3:::assets-<assetBucketAccount>-<region>', and the rule that <assetBucketAccount> is a CodeArtifact-owned, region-scoped account NOT equal to the domain owner / caller account. What is NOT the requirement: the literal digits of either account. The decisive observation is internal to a single recorded response: domain.owner and domain.arn in these same steps carry the caller account and PASS, while s3BucketArn carries a different account - so a projection that derives the bucket name from the caller account can never match, whatever the caller account happens to be.

Status (200) and every other DomainDescription member (name, owner, arn, status, createdTime, repositoryCount, assetSizeBytes) match AWS; only s3BucketArn diverges.

## Steps to Reproduce
Boot the emulator from this checkout and replay simparity-wire-aws-v1; the failing steps are codeartifact-domain-lifecycle#task#1 (CreateDomain), codeartifact-domain-lifecycle#task#2 (DescribeDomain) and codeartifact-domain-lifecycle#teardown#4 (DeleteDomain), plus the 15 further members listed per cause.

By hand, with a codeartifact client pointed at the emulator:
1. create_domain(domain=D) -> read domain.owner, domain.arn and domain.s3BucketArn from the response.
   -> expect domain.s3BucketArn of the form 'arn:aws:s3:::assets-<assetBucketAccount>-<region>' where <assetBucketAccount> != domain.owner and != the account component of domain.arn.
   -> observe 'arn:aws:s3:::assets-<callerAccount>-<region>', i.e. the same account as domain.owner.
2. describe_domain(domain=D) -> same assertion on domain.s3BucketArn; observe the same caller-account bucket.
3. delete_domain(domain=D) -> same assertion on the echoed domain.s3BucketArn; observe the same caller-account bucket.
4. Control (shows the fault is the derivation, not the constant): in all three responses, assert only that the account component of s3BucketArn differs from domain.owner. That assertion fails on the emulator for every domain, in every region, for any caller account.
