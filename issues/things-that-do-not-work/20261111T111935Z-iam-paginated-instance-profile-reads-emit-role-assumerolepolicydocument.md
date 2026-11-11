Issue-Source: trace-sweep-divergence
Issue-Trace: iam-instance-profile-listing-pagination
Issue-Step: task#11
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-instance-profile-listing-pagination
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|ListInstanceProfiles|2xx->2xx|$.ListInstanceProfilesResponse.ListInstanceProfilesResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-e4435dbd/sim2real-probe-iam-ipp-profile-0-1694368d"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfiles|2xx->2xx|$.ListInstanceProfilesResponse.ListInstanceProfilesResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-e4435dbd/sim2real-probe-iam-ipp-profile-1-b04aa8d5"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfiles|2xx->2xx|$.ListInstanceProfilesResponse.ListInstanceProfilesResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-e4435dbd/sim2real-probe-iam-ipp-profile-2-4bed0e69"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfiles|2xx->2xx|$.ListInstanceProfilesResponse.ListInstanceProfilesResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-e4435dbd/sim2real-probe-iam-ipp-profile-3-c7a97d5c"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfilesForRole|2xx->2xx|$.ListInstanceProfilesForRoleResponse.ListInstanceProfilesForRoleResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-e4435dbd/sim2real-probe-iam-ipp-profile-0-1694368d"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfilesForRole|2xx->2xx|$.ListInstanceProfilesForRoleResponse.ListInstanceProfilesForRoleResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-e4435dbd/sim2real-probe-iam-ipp-profile-1-b04aa8d5"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfilesForRole|2xx->2xx|$.ListInstanceProfilesForRoleResponse.ListInstanceProfilesForRoleResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-e4435dbd/sim2real-probe-iam-ipp-profile-2-4bed0e69"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfilesForRole|2xx->2xx|$.ListInstanceProfilesForRoleResponse.ListInstanceProfilesForRoleResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-e4435dbd/sim2real-probe-iam-ipp-profile-3-c7a97d5c"].Roles.member.AssumeRolePolicyDocument
Issue-Commit: e0e2288ca52485323bf2abcb85828afa6106dd90

# iam paginated instance-profile reads emit Role.AssumeRolePolicyDocument as raw JSON instead of percent-encoded

> _Note: this issue was identified against the code base at commit `e0e2288ca52485323bf2abcb85828afa6106dd90`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInstanceProfiles

## Description
2 net-new causes, one shared fault: the Role projection embedded in IAM's instance-profile LIST responses serializes the role's trust policy as raw JSON text instead of the percent-encoded (URL-encoded) form IAM puts on the Query/XML wire. Both causes must be closed before this issue is submitted.

Cause 1 of 2 — ListInstanceProfiles serves InstanceProfiles.member.Roles.member.AssumeRolePolicyDocument unencoded.
AWS: Smithy iam#ListInstanceProfiles (input PathPrefix/Marker/MaxItems, paginated inputToken=Marker outputToken=Marker items=InstanceProfiles pageSize=MaxItems) returns InstanceProfiles -> instanceProfileListType (required); each member embeds iam#InstanceProfile whose Roles members are iam#Role, and iam#Role.AssumeRolePolicyDocument targets policyDocumentType (@length 1-131072, @pattern ^[\u0009\u000A\u000D\u0020-\u00FF]+$, doc "The policy that grants an entity permission to assume the role."). IAM returns policy documents URL-encoded per RFC 3986, so the value arrives as one token containing no literal '{', '"', ':', ',' or space — of the form %7B%22Version%22%3A...%7D — and a client must URL-decode it before parsing JSON. The requirement is the ENCODING applied to whatever trust policy the role actually carries, in every page of the listing; no particular document text is part of the contract. Status is 200 on both sides, so this is purely a serialization defect.
Emulator: the member is emitted as the document's raw JSON text, e.g. {"Version": "2012-10-17", "Statement": [{"Effect": "Allow", "Principal": {"Service": "ec2.amazonaws.com"}, "Action": "sts:AssumeRole"}]}, for every instance profile returned in the page.
Why: a client or SDK that URL-decodes the field — which AWS's contract requires — mangles the raw JSON: percent sequences are absent and any '+' inside the document decodes to a space, so trust-policy parsing, diffing and round-tripping back through CreateRole/UpdateAssumeRolePolicy break, and byte-level comparison against a real AWS response never matches.
Fix: the ListInstanceProfiles response serializer, specifically the shared Role-to-wire projection used by the instance-profile list path — percent-encode policyDocumentType members on the Query/XML wire the same way the single-entity reads must.
Traces: iam-instance-profile-listing-pagination#task#11 (all four profiles in the page diverge on the same member).

Cause 2 of 2 — the same unencoded document on ListInstanceProfilesForRole.
AWS: Smithy iam#ListInstanceProfilesForRole (RoleName required, @length 1-64, @pattern ^[\w+=,.@-]+$; errors NoSuchEntity 404, ServiceFailure 500) returns the SAME instanceProfileListType, so the embedded Role.AssumeRolePolicyDocument is percent-encoded exactly as in cause 1.
Emulator: identical divergence — raw JSON for every profile returned for the role, status 200 vs 200.
Why: as cause 1; a caller that reaches the profile set by role rather than by path hits the same decode failure, so repairing only the ListInstanceProfiles path leaves this read broken.
Fix: the ListInstanceProfilesForRole response serializer / the same shared Role projection; this is a second replay unit that must go green.
Traces: iam-instance-profile-listing-pagination#task#13.

Ownership: the replay reports owned_by empty and stub_path empty for BOTH clusters, so no open issue carries their replay keys (the previously filed issues pin per-run generated profile ARNs in their Issue-Key lines, which do not match this run's generated names). Several open issues describe a similar fault without owning these steps and should be cross-checked when fixing: '20261111T010549Z-iam-listinstanceprofiles-and-listinstanceprofilesforrole-return-assumerolepolicydocument-unencoded.md', '20261110T144755Z-iam-instance-profile-list-projections-never-percent-encode.md', '20261110T101948Z-iam-assumerolepolicydocument-is-not-percent-encoded-in-instance-profile.md', '20261110T050411Z-iam-instance-profile-listing-pages-serve-assumerolepolicydocument-unencoded.md'.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (97 divergence clusters; 95 already owned, these 2 net-new).

Cause 1 — iam-instance-profile-listing-pagination#task#11, operation ListInstanceProfiles, expected status 200, actual status 200. For each profile in the page (.../sim2real-probe-iam-ipp-e4435dbd/sim2real-probe-iam-ipp-profile-0-1694368d, -1-b04aa8d5, -2-4bed0e69, -3-c7a97d5c):
  $.ListInstanceProfilesResponse.ListInstanceProfilesResult.InstanceProfiles.member[Arn=...].Roles.member.AssumeRolePolicyDocument: value_mismatch
    expected '%7B%22Version%22%3A%222012-10-17%22%2C%22Statement%22%3A%5B%7B%22Effect%22%3A%22Allow%22%2C%22Principal%22%3A%7B%22Service%22%3A%22ec2.amazonaws.com%22%7D%2C%22Action%22%3A%22sts%3AAssumeRole%22%7D%5D%7D'
    got     '{"Version": "2012-10-17", "Statement": [{"Effect": "Allow", "Principal": {"Service": "ec2.amazonaws.com"}, "Action": "sts:AssumeRole"}]}'
(The profile names and the account/path segments above are per-run generated values; the requirement is the percent-encoding of the member, not these strings.)

Cause 2 — iam-instance-profile-listing-pagination#task#13, operation ListInstanceProfilesForRole, expected status 200, actual status 200: the identical value_mismatch on $.ListInstanceProfilesForRoleResponse.ListInstanceProfilesForRoleResult.InstanceProfiles.member[Arn=...].Roles.member.AssumeRolePolicyDocument for the same four profiles.

No status, error code or error message diverges in either cluster — both are 200/200 and the only diverging key is the AssumeRolePolicyDocument member.

Model grounding (smithy_query on services/service-com-amazonaws-iam-awsidentitymanagementv20100508/smithy-model/com-amazonaws-iam-awsidentitymanagementv20100508.json): ListInstanceProfiles and ListInstanceProfilesForRole both return InstanceProfiles -> instanceProfileListType (required); iam#Role.AssumeRolePolicyDocument -> policyDocumentType, @length 1-131072, @pattern ^[\u0009\u000A\u000D\u0020-\u00FF]+$.

## Steps to Reproduce
1. git_clone the repo; the emulator is built and booted from that checkout.
2. Call replay_wire_corpus() for trace set simparity-wire-aws-v1.
3. Replay fixture iam-instance-profile-listing-pagination: create a role whose trust policy allows ec2.amazonaws.com to sts:AssumeRole, create several instance profiles under a common path, and add the role to each profile.
4. Step task#11: POST Action=ListInstanceProfiles (with PathPrefix and MaxItems so the listing pages) and read InstanceProfiles.member.Roles.member.AssumeRolePolicyDocument — the emulator returns raw JSON where AWS returns the percent-encoded document.
5. Step task#13: POST Action=ListInstanceProfilesForRole with that RoleName and read the same member — same raw JSON.
6. URL-decode either value as an AWS SDK does: the decode is a no-op and the document is still raw JSON, whereas a real AWS response decodes to the JSON document.
