Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeartifact.describe-repository-missing-permission-denied codeartifact.deploy-domain-behaves-after-cfn-create codeartifact.deploy-repository-behaves-with-external-connection
Issue-Shape-Id: com.amazonaws.codeartifact#CodeArtifactControlPlaneService
Issue-Key: codeartifact.describe-repository-missing-permission-denied|_error#21|AssertionError|raise AssertionError(f"expected the call to be denied, but it SUCCEEDED and returned {result!r}")
Issue-Key: codeartifact.deploy-domain-behaves-after-cfn-create|_deploy_cfn#3|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stacks[0].get("Outputs", [])}
Issue-Key: codeartifact.deploy-repository-behaves-with-external-connection|_deploy_cfn#4|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stacks[0].get("Outputs", [])}
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# CodeArtifact: no IAM authorization on reads, PublishPackageVersion digest mismatch answered 403 SignatureDoesNotMatch, and CFN stack Outputs for CodeArtifact resources carry no OutputValue

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeRepository

## Description
Three distinct root causes, all reached through CodeArtifact, none of them a missing operation implementation (each request is answered, just wrongly, and two of them are refused before any CodeArtifact handler runs).

Cause 1 of 3: CodeArtifact control-plane reads are not authorized at all.
AWS: DescribeRepository requires `codeartifact:DescribeRepository` on the caller's identity; a principal with no grant is refused with AccessDeniedException (HTTP 403, per Smithy: DescribeRepository errors -> AccessDeniedException httpError 403, documented as "The operation did not succeed because of an unauthorized access attempt"). The refusal is distinct from a not-found or a malformed-input refusal, and granting the exact action to the SAME assumed-role session makes the identical call succeed.
Emulator: returns HTTP 200 with the full `repository` description to an assumed-role session whose inline policy grants nothing — the authorization decision is never made.
Why: every negative authorization test a client writes against the emulator passes vacuously. Code that relies on least-privilege roles to fence off a repository (and any test asserting a 403) is validated green against an emulator that would have served the data, so the privilege defect ships.
Fix: the authorization decision in the request pipeline for the CodeArtifact control-plane operations (identity-policy evaluation ahead of the operation handlers), plus the AccessDeniedException path in the read handlers.
Traces: codeartifact.describe-repository-missing-permission-denied.

Cause 2 of 3: the SigV4 payload-hash check rejects PublishPackageVersion whenever `assetSHA256` disagrees with the body.
AWS: `assetSHA256` is bound to the HTTP header `x-amz-content-sha256` (Smithy: PublishPackageVersion.assetSHA256 -> SHA256, httpHeader `x-amz-content-sha256`, required, @length 64..64, @pattern ^[0-9a-f]+$) and `assetContent` is the streaming httpPayload. A declared digest that does not match the uploaded bytes is a MODELED APPLICATION error, not an auth failure: the request authenticates, the service hashes the body, compares, and refuses with ValidationException (HTTP 400, per Smithy: PublishPackageVersion errors -> ValidationException httpError 400). The recorded message names both digests, e.g. `The provided content hashes do not match the calculated content hashes. Hash algorithm 'SHA-256', Expected hash '<declared>', Actual hash '<computed>'` (example wording; the requirement is the 400/ValidationException classification and that the rejected publish stores nothing).
Emulator: answers `SignatureDoesNotMatch` / 'The request signature does not match' — the signature verifier treats the member-bound `x-amz-content-sha256` header as the authoritative payload hash, recomputes the body hash, and fails authentication before the operation is dispatched.
Why: the whole digest-integrity contract of this operation becomes unreachable. A client that uploads a corrupted asset gets an auth error instead of a 400 ValidationException, so it retries/re-signs instead of recomputing its digest; and because the request never reaches the handler, implementing PublishPackageVersion will NOT make this path correct.
Hypothesis: AWS authenticates using the signed header value supplied by the client (which the client also signed with) and defers the body comparison to the application layer, which is why the two checks land in different layers.
Fix: the SigV4 payload-hash verification in the request pipeline — it must not re-derive and enforce the body hash for an operation whose model binds a member to `x-amz-content-sha256` with a streaming payload; the digest comparison belongs in the PublishPackageVersion handler's validation.
Traces: codeartifact.codeartifact-publish-with-mismatched-asset-sha256-rejected-and-version-absent.

Cause 3 of 3: CloudFormation stack Outputs that resolve CodeArtifact resource attributes are emitted without a value.
AWS: DescribeStacks on a CREATE_COMPLETE stack returns each Output as a member carrying both `OutputKey` and `OutputValue`; `!GetAtt Domain.Name` / `!GetAtt Domain.Arn` / `!GetAtt Repository.Name` on AWS::CodeArtifact::Domain and AWS::CodeArtifact::Repository resolve to the created resource's attribute (recorded behavior only — the CloudFormation Smithy model types Output.OutputValue as an optional member, so the requirement here is the recorded CFN contract that a resolved Output always carries a value).
Emulator: the Output entries come back with `OutputKey` present and `OutputValue` absent, so reading `o["OutputValue"]` raises KeyError — the GetAtt against the CodeArtifact resource types resolved to nothing and the Output was serialized anyway.
Why: any infrastructure-as-code flow that deploys CodeArtifact via CloudFormation and then feeds a stack Output (domain name, repository name, repository ARN) into the next step reads a missing key, so the emulator cannot exercise CFN-provisioned CodeArtifact at all. Serializing a valueless Output instead of failing the stack also hides the unresolved attribute.
Fix: CodeArtifact resource-type attribute resolution (GetAtt) in the CloudFormation stack provisioner, and the Outputs serializer, which should not emit an Output whose value never resolved.
Traces: codeartifact.deploy-domain-behaves-after-cfn-create, codeartifact.deploy-repository-behaves-with-external-connection. (Both checks are declared under service `codeartifact`; the defective component is the CloudFormation side of the CodeArtifact resource types.)

## Evidence
Cause 1 — codeartifact.describe-repository-missing-permission-denied:
  expected the call to be denied (AccessDeniedException, HTTP 403), but it SUCCEEDED and returned
  {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'repository': {'name': 'sim2real-ca-repo-441ba9916471', 'administratorAccount': '000000000000', 'domainName': 'sim2real-ca-3b6f4d4387710', 'domainOwner': '000000000000', 'arn': 'arn:aws:codeartifact:us-west-2:000000000000:repository/sim2real-ca-3b6f4d4387710/sim2real-ca-repo-441ba9916471', 'upstreams': [], 'externalConnections': [], ...}}
  — the caller was an assumed-role session whose inline policy granted only sts:GetCallerIdentity.

Cause 2 — codeartifact.codeartifact-publish-with-mismatched-asset-sha256-rejected-and-version-absent:
  expected ValidationException (HTTP 400) for a digest mismatch
  got      'SignatureDoesNotMatch': 'The request signature does not match'
  Request: publish_package_version(format='generic', assetContent=<8192 bytes>, assetName='payload.bin', assetSHA256=<true sha256 with one hex char changed>). Note the control call with the TRUE digest would be served by the handler, so this is not the unimplemented-operation cause: the mismatched-digest request is refused in the auth layer.

Cause 3 — codeartifact.deploy-domain-behaves-after-cfn-create and codeartifact.deploy-repository-behaves-with-external-connection:
  File "check_codeartifact_deploy_domain_behaves_after_cfn_create.py", line 82, in _deploy_cfn
    outputs = {o["OutputKey"]: o["OutputValue"] for o in stacks[0].get("Outputs", [])}
  KeyError: 'OutputValue'
  Template: AWS::CodeArtifact::Domain (+ AWS::CodeArtifact::Repository with an external connection), Outputs using !GetAtt Domain.Name.

## Steps to Reproduce
Cause 1:
1. iam.create_role with a trust policy for the account root and an inline policy granting only sts:GetCallerIdentity.
2. sts.assume_role into it, build a codeartifact client from the session.
3. create_domain/create_repository with the privileged client, then call describe_repository(domain=D, repository=R) with the restricted session -> observe HTTP 200 instead of AccessDeniedException (403).

Cause 2:
1. create_domain(domain=D); create_repository(domain=D, repository=R).
2. blob = 8192 arbitrary bytes; wrong = sha256(blob).hexdigest() with one hex character changed.
3. publish_package_version(domain=D, repository=R, format='generic', namespace='sim2real', package=P, packageVersion='1.0.0', assetContent=blob, assetName='payload.bin', assetSHA256=wrong) -> observe SignatureDoesNotMatch instead of a 400 ValidationException naming the declared and calculated hashes.

Cause 3:
1. cloudformation.create_stack with a template declaring AWS::CodeArtifact::Domain (and AWS::CodeArtifact::Repository) and an Output whose Value is !GetAtt Domain.Name.
2. Wait for CREATE_COMPLETE, then describe_stacks and read Stacks[0]['Outputs'] -> each entry has OutputKey but no OutputValue.

## Resolution

Causes 1 and 3 are implemented and pinned by tests; cause 2 is NOT implemented and stays
open work, recorded here so it remains discoverable:

- Cause 1 (CodeArtifact control-plane reads are not authorized) is implemented: the IAM
  decision lives in
  `services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/src/service/authorization.rs`
  and is taken at the top of every handler that addresses a repository,
  `src/ops/describe_repository.rs` among them, so a principal allowed by neither its own
  identity policies nor the repository's permissions policy is refused with the modelled
  `AccessDeniedException` (403). The same rule now covers the operations whose subject is
  named by an ARN rather than by name members — `ListTagsForResource`, `TagResource` and
  `UntagResource` — through `authorize_arn_addressed_action`, which reaches the decision
  against the domain, repository or package group the ARN resolved to; it is pinned by
  `tests/all/arn_addressed_authorization.rs`. `ListDomains` and `ListRepositories` stay
  ungated: their action acts on no resource, so the resource the decision and the refusal
  name is not determined by the request.
- Cause 3 (CloudFormation stack Outputs for CodeArtifact resources carry no value, and the
  resources exist in no service) is implemented: both resource types are now provisioned in
  CodeArtifact through its own API and carry their published return values
  (`services/service-com-amazonaws-cloudformation-cloudformation/src/state/provisioner.rs`
  and `.../src/state/template.rs`), covered by
  `core/router/tests/all/cloudformation_provisions_codeartifact_resources.rs`.
- Cause 2 (a `PublishPackageVersion` whose `assetSHA256` disagrees with the uploaded bytes
  is answered `403 SignatureDoesNotMatch` instead of the modelled `400
  ValidationException`) is BLOCKED in the read-only core. The handler half is already
  right: `src/ops/publish_package_version.rs` hashes the received bytes, compares them with
  the declared digest and raises `ValidationException` with AWS's sentence, storing
  nothing — it is simply never reached. The request is refused before any service code
  runs, by `fn payload` in `vendor/server/src/request_authentication.rs` ("a declared hash
  is not trusted, the body is hashed instead", so a declared `x-amz-content-sha256` is
  honoured only when it is a marker containing `-`), called from the signature check
  `vendor/smithy-server-runtime/src/multi_service_router.rs` runs for the MODELLED
  operation before its `run`. A client signs the canonical request with the
  `x-amz-content-sha256` value it sent, so a member-bound digest that disagrees with the
  body makes the emulator's recomputed signature differ. No service seam reaches that
  decision: `can_handle` never sees the body, and `override_refusal` can only reword a
  refusal already decided. The core change needed is in that `payload` function: take a
  declared `x-amz-content-sha256` that is a 64-character lowercase hex digest as the signed
  payload hash (`SignableBody::Precomputed`) rather than re-deriving it from the body, as
  AWS does. With that one change the request authenticates and the comparison already
  written in the handler answers the modelled 400. The cause is carried on its own, with
  the replay key it owns and the core change it needs, by the open issue this change files
  beside it:
  `issues/things-that-do-not-work/20261101T084500Z-codeartifact-publishpackageversion-digest-mismatch-refused-as-signaturedoesnotmatch.md`,
  so it stays open work and remains discoverable. Only causes 1 and 3's traces and keys are
  kept in the header above: cause 2's trace id and `Issue-Key` live in that open issue, so a
  still-failing key is not recorded here as working.
