Issue-Source: trace-sweep-divergence
Issue-Trace: codeartifact-package-group-hierarchy-and-association
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codeartifact-package-group-hierarchy-and-association codeartifact-package-group-lifecycle-and-hierarchy
Issue-Shape-Id: com.amazonaws.codeartifact#CodeArtifactControlPlaneService
Issue-Key: codeartifact|ListPackageGroups|2xx->2xx|$.packageGroups[*].description
Issue-Key: codeartifact|ListPackageGroups|2xx->2xx|$.packageGroups[arn="arn
Issue-Commit: b8c19aa97befdc8bca399d11adb954dfddf4cae8

# CodeArtifact root package group is listed with no description

> _Note: this issue was identified against the code base at commit `b8c19aa97befdc8bca399d11adb954dfddf4cae8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPackageGroups

## Description
The implicit root package group ('/*') that CodeArtifact creates with every domain is projected by ListPackageGroups with NO description member, where AWS reports the service-assigned description of the root group. Two divergence clusters share this one root cause (the projection of the root group's stored record); both are listed below and BOTH must be closed. Generated domain names, group patterns and ARNs quoted here are one run's values filling a template — the requirement is the FIELD and its value for the root group, never the literal surrounding names.

=== Cause 1 of 2: ListPackageGroups' first page omits the root group's description ===
AWS: Smithy PackageGroupSummary.description is a Description (@length 0..1000, @pattern ^\P{C}*$), documented as "The description of the package group", and is a member of PackageGroupSummaryList returned by ListPackageGroups (POST /v1/package-groups, 200). CodeArtifact creates the root package group '/*' implicitly when a domain is created; recorded behavior: the root group's summary carries description = "Default root package group" — fixed text assigned by the service, identical for every domain, not derived from the request.
Emulator: the root group's summary has the member absent entirely ($.packageGroups[0].description missing, expected 'Default root package group', got None). Every other member of the summary matches, so the record exists but carries no description.
Why: a client that reads the listing to display or diff package groups, or that asserts the root group is the service-created default, sees an unset description where AWS always sets one; code that distinguishes the root default from a caller-created group by its description cannot do so, and a round-trip of listing output back into UpdatePackageGroup drops the description.
Fix: the root package group record the domain-creation path seeds in crate::state (its description must be the service's fixed "Default root package group") and/or the summary projection Domain::package_group_summary that services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/src/ops/list_package_groups.rs calls — the field must be populated for the root group.
Traces: codeartifact-package-group-hierarchy-and-association#task#2 (ListPackageGroups, 200 -> 200).

=== Cause 2 of 2: the same omission on the ARN-addressed root group entry in the lifecycle fixture ===
AWS: same model member and same recorded behavior as Cause 1 — the root group entry, identified here by its package-group ARN (arn:aws:codeartifact:<region>:<account>:package-group/<domain>/%2a, i.e. the URL-encoded '/*' pattern), carries description = "Default root package group".
Emulator: that entry's description member is missing (expected 'Default root package group', got None), after a listing that also contains caller-created groups — so the omission is specific to the root group, not to the listing projection as a whole.
Why: same as Cause 1; this cluster additionally shows the defect survives in a domain that holds several groups, so a fixer must verify the root group's description in a populated domain, not only in a fresh one.
Fix: same component as Cause 1; verify with a domain holding caller-created groups alongside the root group.
Traces: codeartifact-package-group-lifecycle-and-hierarchy#task#7 (ListPackageGroups, 200 -> 200).

Related but NOT covering this: the open issue 20261029T203338Z-codeartifact-domain-policy-and-package-group-resource-fields.md bundles other CodeArtifact domain/policy/package-group field defects (s3BucketArn form, encryptionKey, policy revision form, idempotent DeletePackageGroup, AssociateExternalConnection message). Neither cluster here is owned by it (the replay reports owned_by empty for both), and it says nothing about the root group's description.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator booted from this checkout: 344 steps, 281 passed, 63 mismatched, 0 errored, 0 blocked, score 81.7%, 28 divergence clusters, every cluster service "codeartifact". These are the only two clusters with an EMPTY owned_by and an empty stub_path; all 26 others are already owned by open issues. Verdicts verbatim from the replay output:

Cause 1 — [mismatch] codeartifact-package-group-hierarchy-and-association#task#2 ListPackageGroups, expected 200, actual 200, affected_count 1: "$.packageGroups[0].description: missing (expected 'Default root package group', got None)". divergence_key: service=codeartifact|ListPackageGroups|status=200->200|keys=$.packageGroups[0].description.

Cause 2 — [mismatch] codeartifact-package-group-lifecycle-and-hierarchy#task#7 ListPackageGroups, expected 200, actual 200, affected_count 1: "$.packageGroups[arn=\"arn:aws:codeartifact:us-west-2:000000000000:package-group/sim2real-probe-codeartifact-caart-dom-d2e6581f/%2a\"].description: missing (expected 'Default root package group', got None)". divergence_key: service=codeartifact|ListPackageGroups|status=200->200|keys=$.packageGroups[arn=\"arn.

In both clusters the status and every asserted sibling member of the summary match; the sole diverging key is the root group's description. The expected value "Default root package group" is fixed text the service assigns to the implicit root group — it contains no request-, run- or account-specific part (the domain name and account in Cause 2's selector are only how the entry is addressed).

## Steps to Reproduce
Boot the emulator from this checkout and replay simparity-wire-aws-v1; the two failing steps are codeartifact-package-group-hierarchy-and-association#task#2 and codeartifact-package-group-lifecycle-and-hierarchy#task#7. By hand:
Cause 1: create_domain(domain=D); list_package_groups(domain=D) -> the single entry is the implicit root group (pattern '/*') and its summary must carry description = "Default root package group". Observe the description member absent.
Cause 2: create_domain(domain=D); create_package_group(domain=D, packageGroup='/npm/G/*') (and any further groups); list_package_groups(domain=D) -> locate the entry whose arn ends in package-group/<D>/%2a (the root group) and read its description; it must again be "Default root package group". Observe it absent while the caller-created groups' own descriptions are reported.
