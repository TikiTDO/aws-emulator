Issue-Source: trace-sweep-divergence
Issue-Trace: iam-instance-profile-listing-pagination
Issue-Step: task#11
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-instance-profile-listing-pagination
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|ListInstanceProfiles|2xx->2xx|$.ListInstanceProfilesResponse.ListInstanceProfilesResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-79e6a3c8/sim2real-probe-iam-ipp-profile-0-346051dd"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfiles|2xx->2xx|$.ListInstanceProfilesResponse.ListInstanceProfilesResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-79e6a3c8/sim2real-probe-iam-ipp-profile-1-3f1d88b7"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfiles|2xx->2xx|$.ListInstanceProfilesResponse.ListInstanceProfilesResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-79e6a3c8/sim2real-probe-iam-ipp-profile-2-432a1a12"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfiles|2xx->2xx|$.ListInstanceProfilesResponse.ListInstanceProfilesResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-79e6a3c8/sim2real-probe-iam-ipp-profile-3-b1daa7ae"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfilesForRole|2xx->2xx|$.ListInstanceProfilesForRoleResponse.ListInstanceProfilesForRoleResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-79e6a3c8/sim2real-probe-iam-ipp-profile-0-346051dd"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfilesForRole|2xx->2xx|$.ListInstanceProfilesForRoleResponse.ListInstanceProfilesForRoleResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-79e6a3c8/sim2real-probe-iam-ipp-profile-1-3f1d88b7"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfilesForRole|2xx->2xx|$.ListInstanceProfilesForRoleResponse.ListInstanceProfilesForRoleResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-79e6a3c8/sim2real-probe-iam-ipp-profile-2-432a1a12"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfilesForRole|2xx->2xx|$.ListInstanceProfilesForRoleResponse.ListInstanceProfilesForRoleResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-79e6a3c8/sim2real-probe-iam-ipp-profile-3-b1daa7ae"].Roles.member.AssumeRolePolicyDocument
Issue-Commit: 03f875ec5891a5d63db09920b5ca7a9cbf214550

# The shared Role wire projection skips RFC 3986 encoding of trust policies in instance-profile listings (2 causes)

> _Note: this issue was identified against the code base at commit `03f875ec5891a5d63db09920b5ca7a9cbf214550`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInstanceProfiles

## Description
2 net-new causes sharing ONE fault: the Role projection embedded in IAM's instance-profile LIST responses writes the role's trust policy as raw JSON text instead of the percent-encoded (URL-encoded) form IAM puts on the Query/XML wire. Both causes must be closed before this issue is submitted.

Cause 1 of 2 - ListInstanceProfiles serves InstanceProfiles.member.Roles.member.AssumeRolePolicyDocument unencoded.
AWS: Smithy (services/service-com-amazonaws-iam-awsidentitymanagementv20100508/smithy-model/com-amazonaws-iam-awsidentitymanagementv20100508.json) - iam#ListInstanceProfiles returns InstanceProfiles -> iam#instanceProfileListType (required); each member is iam#InstanceProfile whose required Roles list holds iam#Role, and iam#Role.AssumeRolePolicyDocument targets iam#policyDocumentType (@length min 1 max 131072, @pattern ^[\u0009\u000A\u000D\u0020-\u00FF]+$, doc "The policy that grants an entity permission to assume the role."). The model does not state the transfer encoding, so for that part: recorded behavior only - the recording shows IAM returning the member as a single percent-encoded token containing no literal '{', '"', ':', ',' or space (shape %7B%22Version%22%3A...%7D), i.e. URL-encoded per RFC 3986, which a client must URL-decode before parsing JSON. The requirement is the ENCODING applied to whatever trust policy the role actually carries, on every page of the listing; the document text itself and the generated profile names/ARNs are per-run values, not part of the contract. Status is 200 on both sides and no error code or message is involved, so this is purely a serialization defect.
Emulator: emits the document's raw JSON text for every profile in the page, e.g. {"Version": "2012-10-17", "Statement": [{"Effect": "Allow", "Principal": {"Service": "ec2.amazonaws.com"}, "Action": "sts:AssumeRole"}]} - unencoded, with literal braces, quotes, colons and spaces.
Why: a client or SDK that URL-decodes the field, as AWS's wire contract requires, mangles raw JSON - percent sequences are absent and any '+' inside the document decodes to a space - so trust-policy parsing, diffing and round-tripping back through CreateRole/UpdateAssumeRolePolicy break, and byte-comparison against a real AWS response never matches.
Fix: the ListInstanceProfiles response serializer, specifically the shared Role-to-wire projection used by the instance-profile list path: percent-encode policyDocumentType members on the Query/XML wire exactly as the single-entity reads do.
Traces: iam-instance-profile-listing-pagination#task#11 (all four profiles in the page diverge on the same member).

Cause 2 of 2 - the same unencoded document on ListInstanceProfilesForRole.
AWS: Smithy iam#ListInstanceProfilesForRole (RoleName required, @length 1-64, @pattern ^[\w+=,.@-]+$) returns the SAME iam#instanceProfileListType, so the embedded Role.AssumeRolePolicyDocument is percent-encoded exactly as in cause 1 (recorded behavior only for the encoding itself; status 200).
Emulator: identical divergence - raw JSON for every profile returned for the role, status 200 vs 200.
Why: as cause 1; a caller that reaches the profile set by role rather than by path hits the same decode failure, so repairing only the ListInstanceProfiles path leaves this read broken.
Fix: the ListInstanceProfilesForRole response serializer / the same shared Role projection - a second replay unit that must go green.
Traces: iam-instance-profile-listing-pagination#task#13.

Ownership: the replay reports owned_by EMPTY and stub_path EMPTY for both clusters, so no open issue carries their replay keys - the previously filed look-alikes pin per-run generated profile ARNs in their Issue-Key lines and those names differ every run. These open issues describe the same fault without owning these steps and should be cross-checked (and likely closed together) when fixing: '20261111T111935Z-iam-paginated-instance-profile-reads-emit-role-assumerolepolicydocument.md', '20261111T010549Z-iam-listinstanceprofiles-and-listinstanceprofilesforrole-return-assumerolepolicydocument-unencoded.md', '20261110T144755Z-iam-instance-profile-list-projections-never-percent-encode.md', '20261110T101948Z-iam-assumerolepolicydocument-is-not-percent-encoded-in-instance-profile.md', '20261110T050411Z-iam-instance-profile-listing-pages-serve-assumerolepolicydocument-unencoded.md'.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: 1695 steps, 1412 passed, 282 mismatched, 1 blocked, score 83.3%, 97 divergence clusters - 95 already owned by open issues, 0 stub-blocked, these 2 net-new.

Cause 1 - fixture iam-instance-profile-listing-pagination, step task#11, operation ListInstanceProfiles, expected status 200, actual status 200, verdict mismatch, affected_count 1. For each instance profile in the page (generated names .../sim2real-probe-iam-ipp-79e6a3c8/sim2real-probe-iam-ipp-profile-0-346051dd, -1-3f1d88b7, -2-432a1a12, -3-b1daa7ae):
  $.ListInstanceProfilesResponse.ListInstanceProfilesResult.InstanceProfiles.member[Arn=...].Roles.member.AssumeRolePolicyDocument: value_mismatch
    expected '%7B%22Version%22%3A%222012-10-17%22%2C%22Statement%22%3A%5B%7B%22Effect%22%3A%22Allow%22%2C%22Principal%22%3A%7B%22Service%22%3A%22ec2.amazonaws.com%22%7D%2C%22Action%22%3A%22sts%3AAssumeRole%22%7D%5D%7D'
    got     '{"Version": "2012-10-17", "Statement": [{"Effect": "Allow", "Principal": {"Service": "ec2.amazonaws.com"}, "Action": "sts:AssumeRole"}]}'
The account id, path and profile names above are per-run generated values; the requirement is that the member is percent-encoded, not these strings.

Cause 2 - fixture iam-instance-profile-listing-pagination, step task#13, operation ListInstanceProfilesForRole, expected status 200, actual status 200, verdict mismatch, affected_count 1: the identical value_mismatch on $.ListInstanceProfilesForRoleResponse.ListInstanceProfilesForRoleResult.InstanceProfiles.member[Arn=...].Roles.member.AssumeRolePolicyDocument for the same four profiles.

No HTTP status, error code or error message diverges in either cluster (200 vs 200); the only diverging key is the AssumeRolePolicyDocument member.

Model grounding (smithy_query, query=shape on policyDocumentType/Role/InstanceProfile): iam#Role.AssumeRolePolicyDocument -> policyDocumentType with @length 1-131072 and @pattern ^[\u0009\u000A\u000D\u0020-\u00FF]+$; iam#InstanceProfile.Roles is a required roleListType, shared by both list operations.

## Steps to Reproduce
1. git_clone() the repo; the emulator is built and booted from that checkout.
2. Call replay_wire_corpus() for trace set simparity-wire-aws-v1.
3. Fixture iam-instance-profile-listing-pagination sets up: a role whose trust policy allows ec2.amazonaws.com to sts:AssumeRole, several instance profiles under one common path, and the role added to each profile.
4. Step task#11: POST Action=ListInstanceProfiles with that PathPrefix and a MaxItems that makes the listing page, then read InstanceProfiles.member.Roles.member.AssumeRolePolicyDocument - the emulator returns raw JSON where AWS returns the percent-encoded document.
5. Step task#13: POST Action=ListInstanceProfilesForRole with that RoleName and read the same member - same raw JSON.
6. URL-decode either returned value the way an AWS SDK does: the decode is a no-op and the value is still raw JSON, whereas a real AWS response decodes into the JSON trust policy.
