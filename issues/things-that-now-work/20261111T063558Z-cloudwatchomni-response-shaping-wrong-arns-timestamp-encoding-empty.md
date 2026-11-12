Issue-Source: trace-sweep-divergence
Issue-Trace: cloudwatchomni-access-profile-and-grant-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudwatchomni-access-profile-and-grant-lifecycle cloudwatchomni-deep-link-and-identity-center-refusals cloudwatchomni-omni-dashboard-update-and-validation cloudwatchomni-view-definition-statement-and-name-rules cloudwatchomni-view-update-and-conflict-errors
Issue-Shape-Id: com.amazonaws.cloudwatchomni#CloudWatchOmniFrontend
Issue-Key: cloudwatchomni|CreateAccessProfile|2xx->2xx|$.accessProfile.arn
Issue-Key: cloudwatchomni|CreateAccessProfile|2xx->2xx|$.accessProfile.createdAt
Issue-Key: cloudwatchomni|CreateAccessProfile|2xx->2xx|$.accessProfile.updatedAt
Issue-Key: cloudwatchomni|CreateAccessGrant|2xx->2xx|$.accessGrant.createdAt
Issue-Key: cloudwatchomni|CreateAccessGrant|2xx->2xx|$.accessGrant.principal.principalAttributes
Issue-Key: cloudwatchomni|CreateAccessGrant|2xx->2xx|$.accessGrant.updatedAt
Issue-Key: cloudwatchomni|GetAccessGrant|2xx->2xx|$.accessGrant.createdAt
Issue-Key: cloudwatchomni|GetAccessGrant|2xx->2xx|$.accessGrant.grantArn
Issue-Key: cloudwatchomni|GetAccessGrant|2xx->2xx|$.accessGrant.principal.principalAttributes
Issue-Key: cloudwatchomni|GetAccessGrant|2xx->2xx|$.accessGrant.updatedAt
Issue-Key: cloudwatchomni|GetDomain|2xx->2xx|$.domain.createdAt
Issue-Key: cloudwatchomni|GetDomain|2xx->2xx|$.domain.customEndpointUrls[*]
Issue-Key: cloudwatchomni|GetDomain|2xx->2xx|$.domain.domainEndpointUrl
Issue-Key: cloudwatchomni|GetDomain|2xx->2xx|$.domain.identityProviderConfiguration
Issue-Key: cloudwatchomni|GetDomain|2xx->2xx|$.domain.updatedAt
Issue-Key: cloudwatchomni|CreateOmniDashboard|2xx->2xx|$.omniDashboard.createdAt
Issue-Key: cloudwatchomni|CreateOmniDashboard|2xx->2xx|$.omniDashboard.tags
Issue-Key: cloudwatchomni|CreateOmniDashboard|2xx->2xx|$.omniDashboard.updatedAt
Issue-Key: cloudwatchomni|CreateView|2xx->2xx|$.arn
Issue-Key: cloudwatchomni|CreateView|2xx->2xx|$.createdAt
Issue-Key: cloudwatchomni|CreateView|2xx->2xx|$.updatedAt
Issue-Commit: 826b08334102dd43451716f9b2396fd89c307280

# cloudwatchomni response shaping wrong: ARNs, Timestamp encoding, empty collections

> _Note: this issue was identified against the code base at commit `826b08334102dd43451716f9b2396fd89c307280`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAccessProfile

## Description
Six replayed CloudWatchOmni success responses (HTTP 200) carry structurally wrong payloads. Three faults recur: (a) resource ARNs are built with an extra space/<spaceId> segment, or without the resource-id segment; (b) every modeled Timestamp member fails the recorded shape assertion and comes back as a bare floating-point number; (c) modeled collection/structure members that are present-but-empty on AWS are omitted entirely. This bundle has SIX causes; all six must be fixed and re-replayed.

Cause 1 of 6 - CreateAccessProfile ARN format and Timestamp encoding (fixture cloudwatchomni-access-profile-and-grant-lifecycle, step task#3).
AWS: Smithy model com.amazonaws.cloudwatchomni#AccessProfile - member arn is @required, target Arn (@length 1..2048, @pattern ^arn:.+$); members createdAt/updatedAt are @required, target Timestamp. The recording shows the access-profile ARN shaped arn:aws:cloudwatch:<region>:<account-id>:access-profile/<profileId> - the profile id is the only resource-path segment, with no space segment (see the asserted ARN in the same fixture's GetAccessProfile step, Cause 3's sibling). Protocol is rpcv2Cbor (service CloudWatchOmniFrontend, protocols: [rpcv2Cbor]), so a Timestamp member must be serialized as an RFC 8949 tag(1) epoch date-time value, not as an untagged number. Region, account id, profile id and the instants themselves are per-run values; the ARN template and the timestamp encoding are the requirement.
Emulator: returns $.accessProfile.arn = arn:aws:cloudwatch:<region>:<account-id>:space/<spaceId>/access-profile/<profileId> (an extra space/<spaceId> segment), and $.accessProfile.createdAt / $.accessProfile.updatedAt as plain decimal numbers (replay saw 1794378701.673033), which fail the recorded Timestamp shape assertion. Hypothesis: the CBOR serializer writes Timestamp members as a bare double instead of tag(1).
Why: a client that parses the ARN to recover the profile id, or that passes the ARN to IAM/another API, gets a resource identifier AWS never issues and no equality match against a live-recorded ARN. A client SDK decoding the response gets a number where its generated model expects an instant - either a deserialization error or a nonsense date.
Fix: the CreateAccessProfile handler's ARN construction, and the shared rpcv2Cbor response serializer's Timestamp encoding, in the cloudwatchomni service crate.
Traces: cloudwatchomni-access-profile-and-grant-lifecycle#task#3.

Cause 2 of 6 - CreateAccessGrant omits empty principalAttributes and mis-encodes Timestamps (fixture cloudwatchomni-access-profile-and-grant-lifecycle, step task#7).
AWS: Smithy com.amazonaws.cloudwatchomni#AccessGrant - principal is @required (target AccessGrantPrincipal); createdAt/updatedAt are @required Timestamps. The recording shows $.accessGrant.principal.principalAttributes present as an empty list when the grant has no attributes - AWS serializes the empty list rather than dropping the member.
Emulator: $.accessGrant.principal.principalAttributes is absent (expected [], got nothing) and createdAt/updatedAt are bare numbers as in Cause 1.
Why: a client that iterates principalAttributes without a null guard faults on an absent member, and the response is not round-trip equal to AWS's for the same request.
Fix: the CreateAccessGrant handler's principal serialization (emit modeled empty lists/maps rather than skipping them) plus the shared Timestamp encoding.
Traces: cloudwatchomni-access-profile-and-grant-lifecycle#task#7.

Cause 3 of 6 - GetAccessGrant grantArn format, empty principalAttributes, Timestamps (fixture cloudwatchomni-access-profile-and-grant-lifecycle, step task#8).
AWS: Smithy AccessGrant.grantArn is @required, target Arn (@pattern ^arn:.+$). The recording asserts the grant ARN shaped arn:aws:cloudwatch:<region>:<account-id>:access-grant/<grantId> - grant id as the sole resource-path segment, no space segment.
Emulator: returns arn:aws:cloudwatch:<region>:<account-id>:space/<spaceId>/access-grant/<grantId>, omits principal.principalAttributes, and emits bare-number timestamps.
Why: same as Cause 1 - the ARN a client stores from a Get call does not match the one AWS returns, so ARN-keyed lookups and policy references built from it are wrong.
Fix: the GetAccessGrant handler / the access-grant ARN builder shared with CreateAccessGrant.
Traces: cloudwatchomni-access-profile-and-grant-lifecycle#task#8.

Cause 4 of 6 - GetDomain omits identityProviderConfiguration, mis-encodes Timestamps, and builds endpoint URLs from the wrong template (fixture cloudwatchomni-deep-link-and-identity-center-refusals, step task#7).
AWS: Smithy com.amazonaws.cloudwatchomni#Domain - domainEndpointUrl is @required String, customEndpointUrls a StringList, identityProviderConfiguration target IdentityProviderConfiguration, createdAt/updatedAt @required Timestamps. Recorded behavior only for the URL template: the domain endpoint is https://<domainId>.cloudwatch-omni.global.app.aws and each custom endpoint is https://<domainName>.cloudwatch-omni.global.app.aws - a global (non-regional) app.aws host. The domain id and name are per-run values; the host suffix cloudwatch-omni.global.app.aws is the requirement. identityProviderConfiguration is recorded present as an empty structure for an IAM-only domain.
Emulator: serves https://<domainId>.cloudwatch-omni.<region>.amazonaws.com and https://<domainName>.cloudwatch-omni.<region>.amazonaws.com (regional amazonaws.com suffix), omits identityProviderConfiguration entirely, and returns bare-number createdAt/updatedAt.
Why: the endpoint URL is the value a console/deep-link client navigates to; a regional amazonaws.com host is not the domain's user-facing endpoint on AWS, so any client that stores or redirects to it sends users to an address the service does not publish.
Fix: the GetDomain handler's endpoint-URL construction and its Domain serializer (emit the modeled empty structure), plus the shared Timestamp encoding.
Traces: cloudwatchomni-deep-link-and-identity-center-refusals#task#7.

Cause 5 of 6 - CreateOmniDashboard omits empty tags and mis-encodes Timestamps (fixture cloudwatchomni-omni-dashboard-update-and-validation, step task#3).
AWS: Smithy com.amazonaws.cloudwatchomni#OmniDashboard - tags target TagMap (@length max 50); createdAt/updatedAt are @required Timestamps. The recording shows $.omniDashboard.tags present as an empty map when the dashboard was created without tags.
Emulator: omits $.omniDashboard.tags and returns bare-number createdAt/updatedAt.
Why: a client reading dashboard.tags without a null guard faults; tag-diffing logic sees 'member absent' instead of 'no tags'.
Fix: the CreateOmniDashboard handler's OmniDashboard serializer plus the shared Timestamp encoding.
Traces: cloudwatchomni-omni-dashboard-update-and-validation#task#3.

Cause 6 of 6 - CreateView ARN omits the view id segment and mis-encodes Timestamps (fixture cloudwatchomni-view-definition-statement-and-name-rules, step task#4; also reproduced by cloudwatchomni-view-update-and-conflict-errors#task#1).
AWS: Smithy CreateViewResponse - arn is @required String, createdAt/updatedAt @required Timestamps. The recording shows the view ARN shaped arn:aws:cloudwatch:<region>:<account-id>:view/<viewName>/<viewId> - BOTH the view name and the generated view id appear in the resource path. The name comes from the request, the id is generated per run.
Emulator: returns arn:aws:cloudwatch:<region>:<account-id>:view/<viewName> - the trailing view-id segment is missing - and bare-number createdAt/updatedAt.
Why: the ARN is not the identifier AWS issues, so a client that stores it and later re-derives the view id, or compares it to an ARN from a live account, gets a value that cannot be resolved.
Fix: the CreateView handler's ARN construction (shared with GetView/ListViews) plus the shared Timestamp encoding.
Traces: cloudwatchomni-view-definition-statement-and-name-rules#task#4.

## Evidence
Cause 1 - expected 200 / got 200, $.accessProfile.arn expected shape arn:aws:cloudwatch:<region>:<acct>:access-profile/<profileId>, got 'arn:aws:cloudwatch:us-west-2:000000000000:space/7f293f84-9ab2-4607-9ebc-520abdcaea7b/access-profile/9eb6a222-ad27-4a47-bb79-1a29b6a5a12e'; $.accessProfile.createdAt/updatedAt failed the Timestamp shape assertion, got 1794378701.673033.
Cause 2 - $.accessGrant.principal.principalAttributes: missing (expected [], got None); createdAt/updatedAt got 1794378701.702197.
Cause 3 - $.accessGrant.grantArn expected 'arn:aws:cloudwatch:us-west-2:000000000000:access-grant/5cca2ff5-74f8-4095-acef-bfe6eb81f205', got 'arn:aws:cloudwatch:us-west-2:000000000000:space/7f293f84-9ab2-4607-9ebc-520abdcaea7b/access-grant/5cca2ff5-74f8-4095-acef-bfe6eb81f205'; principalAttributes missing; timestamps numeric.
Cause 4 - $.domain.domainEndpointUrl expected 'https://d-a0cahpywvfvpvy59ffs4136pr.cloudwatch-omni.global.app.aws', got 'https://d-a0cahpywvfvpvy59ffs4136pr.cloudwatch-omni.us-west-2.amazonaws.com'; $.domain.customEndpointUrls[0] expected 'https://<name>.cloudwatch-omni.global.app.aws', got 'https://<name>.cloudwatch-omni.us-west-2.amazonaws.com'; $.domain.identityProviderConfiguration: missing (expected {}, got None); createdAt/updatedAt got 1794378707.403454.
Cause 5 - $.omniDashboard.tags: missing (expected {}, got None); createdAt/updatedAt got 1794378705.762587.
Cause 6 - $.arn expected 'arn:aws:cloudwatch:us-west-2:000000000000:view/view.sim2realprobecwo55dd424d/af009426-8020-4c42-8233-9ed43983b4d9', got 'arn:aws:cloudwatch:us-west-2:000000000000:view/view.sim2realprobecwo55dd424d'; createdAt/updatedAt got 1794378703.362238.

## Steps to Reproduce
Replay the wire corpus simparity-wire-aws-v1 against the emulator and inspect these steps: cloudwatchomni-access-profile-and-grant-lifecycle#task#3 (CreateAccessProfile), #task#7 (CreateAccessGrant), #task#8 (GetAccessGrant); cloudwatchomni-deep-link-and-identity-center-refusals#task#7 (GetDomain); cloudwatchomni-omni-dashboard-update-and-validation#task#3 (CreateOmniDashboard); cloudwatchomni-view-definition-statement-and-name-rules#task#4 (CreateView). Each returns 200 with the ARN / Timestamp / empty-collection divergences above.
