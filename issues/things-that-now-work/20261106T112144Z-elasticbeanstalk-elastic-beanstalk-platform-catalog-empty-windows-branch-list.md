Issue-Source: trace-sweep-divergence
Issue-Trace: elasticbeanstalk-catalog-list-platform-branches
Issue-Step: task#13
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticbeanstalk-catalog-list-platform-branches elasticbeanstalk-catalog-list-platform-versions elasticbeanstalk-platform-catalog-and-account-quotas
Issue-Shape-Id: com.amazonaws.elasticbeanstalk#AWSElasticBeanstalkService
Issue-Key: elasticbeanstalk|ListPlatformBranches|2xx->2xx|$.ListPlatformBranchesResponse.ListPlatformBranchesResult.PlatformBranchSummaryList
Issue-Key: elasticbeanstalk|ListPlatformVersions|2xx->2xx|$.ListPlatformVersionsResponse.ListPlatformVersionsResult.PlatformSummaryList.member[*]
Issue-Key: elasticbeanstalk|DescribePlatformVersion|2xx->2xx|$.DescribePlatformVersionResponse.DescribePlatformVersionResult.PlatformDescription.CustomAmiList.member[*].ImageId
Issue-Commit: 20004884b35d4e30d0a51c1edcbd16198ad55508

# Elastic Beanstalk platform catalog: empty Windows branch list, duplicated/mislabelled ListPlatformVersions members, blank CustomAmi ImageId (3 causes)

> _Note: this issue was identified against the code base at commit `20004884b35d4e30d0a51c1edcbd16198ad55508`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPlatformBranches

## Description
THREE net-new root causes in the elasticbeanstalk emulator's AWS-managed platform catalog, all served with HTTP 200 so nothing signals the divergence to a client. THREE causes must be closed before submitting. Each cause has its own fixture step, listed under its Traces line.

Related but NOT covering: open issue 20261029T235806Z-elasticbeanstalk-elastic-beanstalk-platform-branch-catalog-returns-wrong-branch.md covers ListPlatformBranches for ten OTHER PlatformName filters (task#1-#4, #6-#8, #10-#12); it does not carry the Windows filter (task#13), ListPlatformVersions, or DescribePlatformVersion. The resolved issue 20261029T235806Z-elasticbeanstalk-elastic-beanstalk-platform-catalog-omits-platform-versions-and.md is closed and covers nothing here.

Cause 1 of 3: the PlatformName 'Windows' filter of ListPlatformBranches returns no platform branches at all.
AWS: Smithy ListPlatformBranches(Filters: SearchFilters, MaxRecords @range min 1, NextToken) -> ListPlatformBranchesResult{PlatformBranchSummaryList, NextToken}; PlatformBranchSummary{PlatformName, BranchName, LifecycleState (PlatformBranchLifecycleState: beta|supported|deprecated|retired), BranchOrder (integer, default 0), SupportedTierList (WebServer/Standard|Worker/SQS/HTTP)}. The branch inventory itself is service data, so for this filter the requirement is the recorded catalog: the 'Windows' platform publishes legacy IIS-on-Windows-Server branches — 'Legacy - IIS 7.5 running on 64bit Windows Server' and 'Legacy - IIS 8 running on 64bit Windows Server', both LifecycleState 'Retired', BranchOrder 9700, SupportedTierList {WebServer/Standard} (recorded behavior only for the inventory; the member shape is modeled).
Emulator: PlatformBranchSummaryList is serialized as an EMPTY element — the replay reports $.ListPlatformBranchesResponse.ListPlatformBranchesResult.PlatformBranchSummaryList as a type_mismatch, expected a {'member': [...]} list of two summaries, got '' (an empty string, i.e. no member elements at all). So the emulator's catalog holds no 'Windows' branch, and the empty list is additionally emitted as a valueless element rather than an absent/empty wrapper carrying no member children.
Why: a client enumerating branches for the Windows platform concludes the platform does not exist in the region and cannot select a branch, so every Windows deployment path looks impossible; a client parsing the response as a list also sees a scalar where a list is modeled.
Fix: the platform-branch catalog data and the PlatformName filter applied to it in the ListPlatformBranches handler, plus the empty-list serialization of PlatformBranchSummaryList.
Traces: elasticbeanstalk-catalog-list-platform-branches#task#13.

Cause 2 of 3: ListPlatformVersions serves each catalogued platform version twice and drops PlatformLifecycleState on the duplicate, so every odd list position names the wrong platform version.
AWS: Smithy ListPlatformVersions(Filters: PlatformFilters, MaxRecords @range min 1, NextToken) -> ListPlatformVersionsResult{PlatformSummaryList, NextToken}; PlatformSummary{PlatformArn, PlatformOwner, PlatformStatus, PlatformCategory, OperatingSystemName, OperatingSystemVersion, SupportedTierList, SupportedAddonList, PlatformLifecycleState, PlatformVersion, PlatformBranchName, PlatformBranchLifecycleState}. PlatformLifecycleState is documented 'Possible values: recommended | empty — If an empty value is returned, the platform version is supported but isn't the recommended one for its branch', and each branch has exactly one recommended version. The version inventory is service data, so the requirement is the recorded catalog: one PlatformSummary per published platform version, each distinct, with PlatformArn/PlatformVersion/PlatformBranchName consistent with one another and PlatformLifecycleState 'Recommended' on the branch's recommended version only.
Emulator: the list length matches but all 34 odd positions (member[1], member[3], ... member[67]) diverge while the even positions match. At each odd position the emulator reports a different platform version from the recording and carries NO PlatformLifecycleState member at all: at member[1] the recording has PlatformBranchName '.NET 8 running on 64bit Amazon Linux 2023', PlatformVersion '3.11.9', PlatformArn .../platform/.NET 8 running on 64bit Amazon Linux 2023/3.11.9 and PlatformLifecycleState 'Recommended', while the emulator answers PlatformBranchName '.NET 10 running on 64bit Amazon Linux 2023', PlatformVersion '3.4.0' with the PlatformLifecycleState member absent; at member[3] the recording has 'Corretto 11 running on 64bit Amazon Linux 2023' / '4.12.9'. Hypothesis: the handler emits every catalogued version twice — the second copy, at the odd index, losing the Recommended marker — so AWS's second (non-recommended) version of each branch is never served and the odd half of the list is duplicates.
Why: a client that pages this catalog to pick a platform version sees half the catalog as duplicates and never sees the older supported versions AWS publishes; because the duplicate omits PlatformLifecycleState, 'pick the recommended version for this branch' and 'is this version recommended?' both answer wrongly, and the PlatformArn it would then pass to CreateEnvironment/DescribePlatformVersion names a version that is not the one the client chose.
Fix: the platform-version catalog the ListPlatformVersions handler serves, its per-branch expansion/de-duplication and the PlatformLifecycleState it stamps on each summary.
Traces: elasticbeanstalk-catalog-list-platform-versions#task#1.

Cause 3 of 3: DescribePlatformVersion returns a CustomAmiList entry whose ImageId is the empty string.
AWS: Smithy DescribePlatformVersion(PlatformArn) -> DescribePlatformVersionResult{PlatformDescription}; PlatformDescription.CustomAmiList: list<CustomAmi>, CustomAmi{VirtualizationType, ImageId} ('The ID of the image used to create the custom AMI'). AWS populates every listed CustomAmi with an EC2 AMI id for its VirtualizationType (an 'ami-' prefixed identifier); the fixture asserts the member's shape rather than one run's id, since the id itself is per-environment data.
Emulator: $.DescribePlatformVersionResponse.DescribePlatformVersionResult.PlatformDescription.CustomAmiList.member[1].ImageId is '' — the second CustomAmi entry is emitted with its VirtualizationType but an empty ImageId, where the recording has an AMI id of the expected shape (reported as expected '<shaped-not-asserted>', got '').
Why: a client that reads CustomAmiList to launch or validate against the platform's AMI for a given virtualization type receives an empty identifier and either fails validation or issues an EC2 call with no image id; a listed CustomAmi with no ImageId is never something AWS returns.
Fix: the platform-description catalog data behind DescribePlatformVersion — the CustomAmiList entries built in the DescribePlatformVersion handler / the platform catalog held in crate::state must carry an ImageId for every entry they emit.
Traces: elasticbeanstalk-platform-catalog-and-account-quotas#task#4.

## Evidence
All three clusters: expected HTTP 200, actual HTTP 200 — only the body diverges, so a client gets no signal.

Cause 1 (fixture elasticbeanstalk-catalog-list-platform-branches, step task#13, operation ListPlatformBranches, 1 affected step):
  $.ListPlatformBranchesResponse.ListPlatformBranchesResult.PlatformBranchSummaryList: type_mismatch — expected {'member': [{'PlatformName': 'Windows', 'BranchName': 'Legacy - IIS 7.5 running on 64bit Windows Server', 'LifecycleState': 'Retired', 'BranchOrder': '9700', 'SupportedTierList': {'member': 'WebServer/Standard'}}, {'PlatformName': 'Windows', 'BranchName': 'Legacy - IIS 8 running on 64bit Windows Server', 'LifecycleState': 'Retired', 'BranchOrder': '9700', 'SupportedTierList': {'member': 'WebServer/Standard'}}], got ''.

Cause 2 (fixture elasticbeanstalk-catalog-list-platform-versions, step task#1, operation ListPlatformVersions, 1 affected step): 34 diverging keys, exactly member[1], member[3], member[5] ... member[67] of $.ListPlatformVersionsResponse.ListPlatformVersionsResult.PlatformSummaryList.
  member[1]: type_mismatch — expected {'PlatformCategory': '.NET Core', 'PlatformOwner': 'AWSElasticBeanstalk', 'OperatingSystemVersion': '2023', 'PlatformArn': 'arn:aws:elasticbeanstalk:us-west-2::platform/.NET 8 running on 64bit Amazon Linux 2023/3.11.9', 'PlatformVersion': '3.11.9', 'SupportedTierList': {'member': ['WebServer/Standard','Worker/SQS/HTTP']}, 'SupportedAddonList': {'member': ['Log/CloudWatch','Log/S3','Monitoring/Healthd','WorkerDaemon/SQSD']}, 'PlatformBranchLifecycleState': 'Supported', 'PlatformStatus': 'Ready', 'OperatingSystemName': 'Amazon Linux', 'PlatformBranchName': '.NET 8 running on 64bit Amazon Linux 2023', 'PlatformLifecycleState': 'Recommended'}, got the same shape MINUS PlatformLifecycleState and naming a different version: {'PlatformArn': 'arn:aws:elasticbeanstalk:us-west-2::platform/.NET 10 running on 64bit Amazon Linux 2023/3.4.0', 'PlatformVersion': '3.4.0', 'PlatformBranchName': '.NET 10 running on 64bit Amazon Linux 2023', 'PlatformBranchLifecycleState': 'Supported', ...}.
  member[3]: type_mismatch — expected PlatformBranchName 'Corretto 11 running on 64bit Amazon Linux 2023', PlatformVersion '4.12.9', PlatformLifecycleState present.
  The requirement is the structure of the catalog — one distinct summary per published version, PlatformArn/PlatformVersion/PlatformBranchName mutually consistent, PlatformLifecycleState 'Recommended' on the branch's recommended version — not the particular version strings or the region/account in the ARN of this run.

Cause 3 (fixture elasticbeanstalk-platform-catalog-and-account-quotas, step task#4, operation DescribePlatformVersion, 1 affected step):
  $.DescribePlatformVersionResponse.DescribePlatformVersionResult.PlatformDescription.CustomAmiList.member[1].ImageId: value_mismatch — expected '<shaped-not-asserted>' (an AMI id of the modeled shape), got ''.
  The specific AMI id is not the requirement; a non-empty, well-shaped ImageId on every emitted CustomAmi is.

## Steps to Reproduce
1. Boot the emulator and point an AWS client at it (region us-west-2).
2. Cause 1: eb.list_platform_branches(Filters=[{'Attribute':'PlatformName','Operator':'=','Values':['Windows']}]). Observe HTTP 200 with PlatformBranchSummaryList serialized as an empty element (no member children) instead of the two 'Legacy - IIS ... running on 64bit Windows Server' Retired branches. Re-replay fixture elasticbeanstalk-catalog-list-platform-branches step task#13.
3. Cause 2: eb.list_platform_versions() (no filters) and walk PlatformSummaryList. Compare member-for-member with the recording: every odd index diverges — it repeats the preceding version's branch/version/ARN and carries no PlatformLifecycleState, while AWS's entry there is the next published version (with 'Recommended' where it is its branch's recommended version). Re-replay fixture elasticbeanstalk-catalog-list-platform-versions step task#1.
4. Cause 3: eb.describe_platform_version(PlatformArn=<an AWS-managed platform ARN from the catalog>) and read PlatformDescription.CustomAmiList. Observe entry [1] carries VirtualizationType but ImageId ''. Re-replay fixture elasticbeanstalk-platform-catalog-and-account-quotas step task#4.
