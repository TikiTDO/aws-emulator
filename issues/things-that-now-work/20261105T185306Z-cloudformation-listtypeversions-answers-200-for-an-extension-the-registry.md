Issue-Source: trace-sweep-divergence
Issue-Trace: cloudformation-type-registry-reads
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudformation-type-registry-reads
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|ListTypeVersions|4xx->2xx|$.ErrorResponse
Issue-Key: cloudformation|ListTypeVersions|4xx->2xx|status
Issue-Commit: 04def8971989cc2c0cf0cb3f2a0f2f1403eb68f2

# ListTypeVersions answers 200 for an extension the registry holds no version of, instead of 400 CFNRegistryException

> _Note: this issue was identified against the code base at commit `04def8971989cc2c0cf0cb3f2a0f2f1403eb68f2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTypeVersions

## Description
One net-new wire divergence in the CloudFormation extension (type) registry: the version listing never refuses, because the emulator's registry synthesizes an Amazon-Web-Services-published record for every resource type it knows how to provision.

=== Cause 1 of 1: ListTypeVersions reports a version summary for an extension that has no registered version, instead of the registry's modeled refusal (fixture cloudformation-type-registry-reads, step task#4, operation ListTypeVersions) ===

AWS: Smithy (services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json): ListTypeVersions input Type (enum RegistryType: HOOK|MODULE|RESOURCE), TypeName (@length 10..204, @pattern ^[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}(::MODULE){0,1}$), Arn (TypeArn @length 0..1024), DeprecatedStatus, PublisherId, MaxResults (@range 1..100), NextToken; output TypeVersionSummaries + NextToken; @paginated. The operation models exactly ONE error, CFNRegistryException — errorKind client, httpError 400, awsQueryError code CFNRegistryException httpResponseCode 400 — and the model documents the operation as returning "summary information about the versions of an extension", i.e. only over the extension VERSIONS the account and Region's registry actually holds (a version is written by RegisterType for a private extension or ActivateType for a public one). An extension for which the registry holds no such version record is not listable: the recorded AWS response for this step is HTTP 400 with body ErrorResponse/Error { Type: 'Sender', Code: 'ValidationError'... } — precisely, Error.Type 'Sender', Error.Code 'CFNRegistryException', Error.Message 'The type does not exist', plus a RequestId. Message rule: 'The type does not exist' is entirely FIXED text for this error class — it has no variable parts, and in particular it does NOT echo the requested TypeName/Arn; the RequestId in the recorded body is a per-run value and is not part of the requirement.

Emulator: answers HTTP 200 with a populated ListTypeVersionsResponse and no ErrorResponse element at all (replay detail: "status: expected 400, got 200" and "$.ErrorResponse: missing"). The refusal path exists in the handler (services/.../src/ops/list_type_versions.rs, `type_not_found` at lines 228-230, reached at line 345) but it is never reached, because the extension lookup falls back to a synthetic public catalogue: `crate::state::registry::aws_published` (registry.rs line 1312) returns a StoredExtension for ANY RESOURCE type in `aws_published_resource_types()` (registry.rs line 1336 = template::IMPLEMENTED_RESOURCE_TYPES chained with service_common::cfn_provisioning::NON_PROVISIONABLE_RESOURCE_TYPES) and for the two Hook entries of `PUBLIC_EXTENSION_CATALOG` (registry.rs line 1426), each carrying a fabricated version row. So every AWS-native type name the emulator recognizes reads as an extension with versions, and the only modeled error of the operation can never fire for it.

Why: a client that uses ListTypeVersions to decide whether an extension is registered/activated in this account and Region — the registry-discovery step of activation, publishing and type-configuration flows, and of `cloudformation list-type-versions` in scripts — is told a version exists where AWS refuses the call. It takes the success branch, then reads a DefaultVersionId/VersionId and version ARN that belong to no registry record, and the error branch keyed on CFNRegistryException/HTTP 400 is dead code against this emulator. Error-class dispatch inverts: a non-retriable 400 client refusal becomes a 200.

Fix: the extension-resolution step of the ListTypeVersions handler and the public-catalogue source it reads — the AWS-published fallback must only yield extensions that really have registry version records, so that an extension with none takes the handler's existing CFNRegistryException path, and that refusal's message must be AWS's fixed text 'The type does not exist'.

Traces: cloudformation-type-registry-reads#task#4 (the cluster's only member).

Related open issues, none of which cover this call or carry this replay key: 20261105T132157Z-cloudformation-listtypeversions-stub.md is a BEHAVIORAL stub issue asserting ListTypeVersions answers HTTP 501 as an unmodified scaffold — the code has since drifted (the handler is implemented and the module header reads `//! IMPLEMENTED:`), and the wire replay shows a 200, not a 501, so that issue does not describe this refusal gap. 20261102T023139Z-cloudformation-stack-refactor-execution-stackset-import-validation-and.md owns step task#2 (ListTypes) of this SAME fixture but not task#4. 20261105T132157Z-cloudformation-5-net-new-root-causes-in-the-cf7de2.md and 20261105T032210Z-cloudformation-extension-type-registry-refusals-diverge-registertype-accepts.md cover other registry refusals (DeregisterType, SetTypeConfiguration, BatchDescribeTypeConfigurations, RegisterType) and neither covers ListTypeVersions.

## Evidence
Cause 1 — fixture cloudformation-type-registry-reads, step task#4, operation ListTypeVersions, cluster divergence_key `service=cloudformation|ListTypeVersions|status=400->200|keys=$.ErrorResponse,status`, verdict mismatch, affected_count 1:
  expected: HTTP 400 with
    $.ErrorResponse.Error.Type    = 'Sender'
    $.ErrorResponse.Error.Code    = 'CFNRegistryException'
    $.ErrorResponse.Error.Message = 'The type does not exist'   (fixed text in full; no variable parts)
    $.ErrorResponse.RequestId     = <per-run value, not asserted as a requirement>
  actual:   HTTP 200, $.ErrorResponse missing (None) — the emulator serves a ListTypeVersionsResponse instead of the refusal.
  replay detail (verbatim): "status: expected 400, got 200\n$.ErrorResponse: missing (expected {'Error': {'Type': 'Sender', 'Code': 'CFNRegistryException', 'Message': 'The type does not exist'}, 'RequestId': '9b2a0321-1bb5-426f-ba30-0780dffa46a0'}, got None)"

Model consulted: ListTypeVersions — errors: [CFNRegistryException, errorKind client, httpError 400, awsQueryError {code CFNRegistryException, httpResponseCode 400}]; it is the operation's ONLY modeled error, so this status/code pair is the only refusal AWS can serve here.

Code path that makes the refusal unreachable (file/line citations):
  services/service-com-amazonaws-cloudformation-cloudformation/src/ops/list_type_versions.rs:343-345 — `stored.or(published.as_ref()).ok_or_else(|| type_not_found(identifier.named()))?`
  .../src/ops/list_type_versions.rs:332-342 — the `published` fallback, consulted whenever the account registry holds nothing
  .../src/state/registry.rs:1312-1322 — `aws_published()` returns a record for any name in the published catalogues
  .../src/state/registry.rs:1336-1345 — `aws_published_resource_types()` = IMPLEMENTED_RESOURCE_TYPES + NON_PROVISIONABLE_RESOURCE_TYPES
  .../src/state/registry.rs:1426-1440 — `PUBLIC_EXTENSION_CATALOG` (AWS::Hooks::GuardHook, AWS::Hooks::LambdaHook)
(host, port, request id and the registry ARNs/timestamps in any single run are environment-specific and are not themselves the requirement)

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus simparity-wire-aws-v1 (replay_wire_corpus). The cluster `service=cloudformation|ListTypeVersions|status=400->200|keys=$.ErrorResponse,status` reports fixture cloudformation-type-registry-reads, step task#4.
2. Or exercise it directly: call ListTypeVersions for an extension this account and Region holds no registered/activated version of (the fixture's registry-reads walk does this after enumerating the registry with ListTypes at task#2) — e.g. ListTypeVersions(Type='RESOURCE', TypeName=<an AWS-native resource type name the emulator provisions, which no RegisterType/ActivateType call in this account ever added a version for>).
3. Expect HTTP 400 with Error.Type 'Sender', Error.Code 'CFNRegistryException', Error.Message 'The type does not exist'. Observe instead HTTP 200 carrying TypeVersionSummaries built from the synthetic AWS-published record, and no ErrorResponse.
4. Re-replay cloudformation-type-registry-reads#task#4 to confirm a fix.
