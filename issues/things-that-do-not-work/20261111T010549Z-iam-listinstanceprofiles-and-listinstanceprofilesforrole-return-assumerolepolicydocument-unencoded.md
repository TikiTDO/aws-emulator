Issue-Source: trace-sweep-divergence
Issue-Trace: iam-instance-profile-listing-pagination
Issue-Step: task#11
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-instance-profile-listing-pagination
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|ListInstanceProfiles|2xx->2xx|$.ListInstanceProfilesResponse.ListInstanceProfilesResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-b8af4533/sim2real-probe-iam-ipp-profile-0-6cbae30c"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfiles|2xx->2xx|$.ListInstanceProfilesResponse.ListInstanceProfilesResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-b8af4533/sim2real-probe-iam-ipp-profile-1-0307e731"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfiles|2xx->2xx|$.ListInstanceProfilesResponse.ListInstanceProfilesResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-b8af4533/sim2real-probe-iam-ipp-profile-2-2bf827ae"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfiles|2xx->2xx|$.ListInstanceProfilesResponse.ListInstanceProfilesResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-b8af4533/sim2real-probe-iam-ipp-profile-3-60b9e5cf"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfilesForRole|2xx->2xx|$.ListInstanceProfilesForRoleResponse.ListInstanceProfilesForRoleResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-b8af4533/sim2real-probe-iam-ipp-profile-0-6cbae30c"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfilesForRole|2xx->2xx|$.ListInstanceProfilesForRoleResponse.ListInstanceProfilesForRoleResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-b8af4533/sim2real-probe-iam-ipp-profile-1-0307e731"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfilesForRole|2xx->2xx|$.ListInstanceProfilesForRoleResponse.ListInstanceProfilesForRoleResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-b8af4533/sim2real-probe-iam-ipp-profile-2-2bf827ae"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfilesForRole|2xx->2xx|$.ListInstanceProfilesForRoleResponse.ListInstanceProfilesForRoleResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-b8af4533/sim2real-probe-iam-ipp-profile-3-60b9e5cf"].Roles.member.AssumeRolePolicyDocument
Issue-Commit: 7eb0bd3bda5e5fd14f4b01ea8dd2daf93fb4b55d

# iam ListInstanceProfiles and ListInstanceProfilesForRole return AssumeRolePolicyDocument unencoded

> _Note: this issue was identified against the code base at commit `7eb0bd3bda5e5fd14f4b01ea8dd2daf93fb4b55d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInstanceProfiles

## Description
Two net-new causes, one shared fault: the instance-profile list projections serialize the embedded role's trust policy as raw JSON instead of the percent-encoded form IAM puts on the wire. Both must be closed before this issue is submitted.

Cause 1 of 2: ListInstanceProfiles serves InstanceProfiles.member.Roles.member.AssumeRolePolicyDocument as raw JSON.
AWS: Smithy iam#ListInstanceProfiles returns InstanceProfiles -> instanceProfileListType (required), whose members embed iam#Role, whose AssumeRolePolicyDocument targets policyDocumentType (@length 1-131072, @pattern ^[\u0009\u000A\u000D\u0020-\u00FF]+$). IAM returns policy documents URL (percent) encoded per RFC 3986, so the trust policy arrives as a single token with no literal braces, quotes, colons, commas or spaces — e.g. %7B%22Version%22%3A%222012-10-17%22%2C%22Statement%22%3A%5B...%5D%7D — and a client must URL-decode before parsing JSON. The requirement is the encoding of whatever document the role actually carries, not any one recorded document's text.
Emulator: the member is emitted as the raw JSON text of the document ({"Version": "2012-10-17", "Statement": [{"Effect": "Allow", "Principal": {"Service": "ec2.amazonaws.com"}, "Action": "sts:AssumeRole"}]}), unencoded, for every profile in every page of the listing. Status is 200 on both sides; only this member diverges.
Why: an SDK or client that URL-decodes the field (what AWS's contract requires) mangles the raw JSON — percent sequences are absent and '+' characters inside a document would decode to spaces — so trust-policy comparison, diffing and round-tripping through CreateRole/UpdateAssumeRolePolicy all break; XML-level string comparison against a real AWS response never matches.
Fix: the ListInstanceProfiles response serializer / the shared Role-to-wire projection used by the instance-profile list path (percent-encode policyDocumentType members on the Query/XML wire).
Traces: iam-instance-profile-listing-pagination#task#11 (all four profiles in the page diverge on the same member).

Cause 2 of 2: the same unencoded document on ListInstanceProfilesForRole.
AWS: Smithy iam#ListInstanceProfilesForRole(RoleName required) returns the same instanceProfileListType, so its embedded Role.AssumeRolePolicyDocument is percent-encoded exactly as in cause 1.
Emulator: identical divergence — raw JSON for every profile returned for the role.
Why: as cause 1; a client that reaches the profile set by role rather than by path hits the same decode failure, so fixing only the ListInstanceProfiles path leaves this read broken.
Fix: the ListInstanceProfilesForRole response serializer / the same shared Role projection; this trace is the second replay unit that must go green.
Traces: iam-instance-profile-listing-pagination#task#13.

The replay reports owned_by empty and stub_path empty for both clusters, so no open issue carries their replay keys. Several open issues describe a similar fault without owning these steps and should be cross-checked when fixing: '20261109T145343Z-iam-instance-profile-listing-pages-return-assumerolepolicydocument-as.md', '20261110T050411Z-iam-instance-profile-listing-pages-serve-assumerolepolicydocument-unencoded.md', '20261110T101948Z-iam-assumerolepolicydocument-is-not-percent-encoded-in-instance-profile.md', '20261110T144755Z-iam-instance-profile-list-projections-never-percent-encode.md'.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout:

1) iam-instance-profile-listing-pagination#task#11, operation ListInstanceProfiles, status 200 vs 200. For each profile in the page (…/sim2real-probe-iam-ipp-profile-0-6cbae30c, -1-0307e731, -2-2bf827ae, -3-60b9e5cf):
  $.ListInstanceProfilesResponse.ListInstanceProfilesResult.InstanceProfiles.member[Arn=…].Roles.member.AssumeRolePolicyDocument: value_mismatch
    expected '%7B%22Version%22%3A%222012-10-17%22%2C%22Statement%22%3A%5B%7B%22Effect%22%3A%22Allow%22%2C%22Principal%22%3A%7B%22Service%22%3A%22ec2.amazonaws.com%22%7D%2C%22Action%22%3A%22sts%3AAssumeRole%22%7D%5D%7D'
    got '{"Version": "2012-10-17", "Statement": [{"Effect": "Allow", "Principal": {"Service": "ec2.amazonaws.com"}, "Action": "sts:AssumeRole"}]}'

2) iam-instance-profile-listing-pagination#task#13, operation ListInstanceProfilesForRole, status 200 vs 200: the identical value_mismatch on $.ListInstanceProfilesForRoleResponse.ListInstanceProfilesForRoleResult.InstanceProfiles.member[Arn=…].Roles.member.AssumeRolePolicyDocument for the same four profiles.

Model grounding (smithy_query, com-amazonaws-iam-awsidentitymanagementv20100508.json): ListInstanceProfiles and ListInstanceProfilesForRole both return InstanceProfiles -> instanceProfileListType (required); iam#Role.AssumeRolePolicyDocument -> policyDocumentType, @length 1-131072, @pattern ^[\u0009\u000A\u000D\u0020-\u00FF]+$, doc "The policy that grants an entity permission to assume the role."

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
3. Replay fixture iam-instance-profile-listing-pagination: create a role with an ec2.amazonaws.com trust policy, create several instance profiles under a path, add the role to each.
4. Step task#11: POST Action=ListInstanceProfiles (with PathPrefix/MaxItems paging) and read InstanceProfiles.member.Roles.member.AssumeRolePolicyDocument — it is raw JSON, not percent-encoded.
5. Step task#13: POST Action=ListInstanceProfilesForRole with that RoleName and read the same member — same raw JSON.
