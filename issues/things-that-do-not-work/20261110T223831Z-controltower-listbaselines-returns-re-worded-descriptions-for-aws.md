Issue-Source: trace-sweep-divergence
Issue-Trace: controltower-baseline-catalog-discovery
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: controltower-baseline-catalog-discovery controltower-catalog-list-baselines
Issue-Shape-Id: com.amazonaws.controltower#AWSControlTowerApis
Issue-Key: controltower|ListBaselines|2xx->2xx|$.baselines[name="BackupAdminBaseline"].description
Issue-Key: controltower|ListBaselines|2xx->2xx|$.baselines[name="BackupBaseline"].description
Issue-Key: controltower|ListBaselines|2xx->2xx|$.baselines[name="CentralConfigBaseline"].description
Issue-Key: controltower|ListBaselines|2xx->2xx|$.baselines[name="ConfigBaseline"].description
Issue-Key: controltower|ListBaselines|2xx->2xx|$.baselines[arn="arn:aws:controltower:us-west-2::baseline/1QBGH2G48YVGDQ3Y"].description
Issue-Key: controltower|ListBaselines|2xx->2xx|$.baselines[arn="arn:aws:controltower:us-west-2::baseline/APO9ATVPBKFRRGLK"].description
Issue-Key: controltower|ListBaselines|2xx->2xx|$.baselines[arn="arn:aws:controltower:us-west-2::baseline/H6C5JFCJJ3CPU3J5"].description
Issue-Key: controltower|ListBaselines|2xx->2xx|$.baselines[arn="arn:aws:controltower:us-west-2::baseline/YX7VMZML5IG8EJUD"].description
Issue-Commit: 593a364106701e97ef06e9977601052b67c4adfb

# controltower ListBaselines returns re-worded descriptions for AWS-managed baseline catalog entries

> _Note: this issue was identified against the code base at commit `593a364106701e97ef06e9977601052b67c4adfb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListBaselines

## Description
The AWS Control Tower baseline catalog is AWS-owned and identical in every account and region: each entry pairs a fixed baseline name with a fixed baseline ARN and a fixed, AWS-authored description string. The emulator's catalog seed carries its own prose for the AWS Backup and AWS Config baselines, so every reader of the catalog -- whether it indexes the response by name or by ARN -- gets description text AWS never emits. Two replay clusters, one per index key, are the same seed defect.

Cause 1 of 2: ListBaselines indexed BY NAME, fixture controltower-baseline-catalog-discovery, step task#1.
AWS: Smithy: ListBaselines is POST /list-baselines, http code 200, output ListBaselinesOutput.baselines (required, list of BaselineSummary{arn required, name required, description}); BaselineSummary.description is documented only as 'A summary description of a Baseline' and carries no constraint, so the exact strings are recorded behavior only. The recording shows, at status 200, that the entry named BackupAdminBaseline is described as 'Sets up AWS Backup Audit Manager.', BackupBaseline as 'Sets up a local AWS Backup vault and attaches multiple AWS Backup plans.', CentralConfigBaseline as 'Sets up central resources for compliance monitoring and auditing within your organization using AWS Config.' and ConfigBaseline as 'Sets up AWS Config resources for member accounts within the target OU, required for Detective Controls enablement. The resources set up are a subset of resources of AWSControlTowerBaseline.'. These description strings are wholly FIXED text: they contain no request-derived and no state-derived part, and no per-run value (no id, account, region, timestamp or request id) appears in them, so the whole string is the requirement.
Emulator: the call answers 200 with the right names present, but each of those four descriptions is replaced by emulator-authored prose -- BackupAdminBaseline gets 'Sets up a delegated administrator account for AWS Backup in your organization.', BackupBaseline gets 'Sets up backup resources for member accounts within the target OU.', CentralConfigBaseline gets 'Sets up central resources for AWS Config in your organization.' and ConfigBaseline gets 'Sets up resources for AWS Config in accounts in your organization.'.
Why: a client that surfaces the catalog to an operator, or that matches on the documented description to pick which baseline to enable on a target OU, reads text AWS does not produce; the BackupAdminBaseline case is actively misleading, because the emulator's wording describes delegated-administrator setup while AWS's baseline of that name sets up AWS Backup Audit Manager, so an automation choosing a baseline from the catalog can enable the wrong one.
Fix: the baseline catalog seed in the controltower crate's state module -- the constant table ListBaselines projects into BaselineSummary -- must hold AWS's fixed description string per baseline rather than re-worded prose; no change to the serializer or to the operation's handler logic is needed.
Traces: controltower-baseline-catalog-discovery#task#1.

Cause 2 of 2: ListBaselines indexed BY ARN, fixture controltower-catalog-list-baselines, step task#1.
AWS: Smithy: same operation and output shape as Cause 1; the ARN member resolves to BaselineArn with @pattern ^arn:[a-z-]+:controltower:[a-z0-9-]*:[0-9]{0,12}:baseline/[A-Z0-9]{16}$, and GetBaseline takes exactly that ARN as its required baselineIdentifier, which is what makes the ARN -- and therefore the description reached through it -- a stable public identifier rather than a per-run value. In such an ARN only the partition and region segments are environment-derived and the account segment is empty for AWS-managed baselines. The description strings are the same four fixed strings cited in Cause 1 ('Sets up AWS Backup Audit Manager.', 'Sets up a local AWS Backup vault and attaches multiple AWS Backup plans.', 'Sets up central resources for compliance monitoring and auditing within your organization using AWS Config.' and the ConfigBaseline/Detective Controls string); recorded behavior only, with no variable parts.
Emulator: the call answers 200, but when the response is indexed by the baseline ARN the same four entries carry the same four re-worded strings as in Cause 1 -- the AWS Backup Audit Manager baseline is described as a delegated-administrator setup, the local-vault baseline as OU-member backup resources, and both AWS Config baselines with shortened emulator prose.
Why: identical client impact to Cause 1, reached through the ARN-keyed path that landing-zone automation and templates use; it is listed as its own cause because it is a separate replay cluster with its own key, so a fix validated only against the name-keyed fixture leaves this one unexercised.
Fix: same component as Cause 1 -- the catalog seed -- confirming that one corrected table serves both the name-keyed and the ARN-keyed read.
Traces: controltower-catalog-list-baselines#task#1.

Related open issue: '20261106T180615Z-controltower-listbaselines-keyed-by-baseline-arn-returns-neither-the.md' reports the ARN-keyed catalog from the missing-id-segment angle, and '20261029T183207Z-controltower-control-tower-baseline-catalog-diverges-wrong-descriptions-wrong.md' the name-keyed side. Neither carries the replay keys of the two clusters above (the replay reports both as unowned), so they are filed here and must be re-replayed in their own right.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up=true, bootstrap_signal=false, 20 steps total, 14 passed, 6 mismatched, score 70%, 5 divergence clusters -- all service controltower. Three clusters (ListTagsForResource, TagResource, UntagResource ARN validation) are owned by open issue 20261029T183207Z-controltower-control-tower-tag-plane-arn-validation-is-skipped.md and are not re-filed. The two clusters below are unowned.

Cause 1 -- ListBaselines, fixture controltower-baseline-catalog-discovery, step task#1, expected status 200 / actual status 200 (status matches; body diverges):
- $.baselines[name="BackupAdminBaseline"].description: expected 'Sets up AWS Backup Audit Manager.', got 'Sets up a delegated administrator account for AWS Backup in your organization.'
- $.baselines[name="BackupBaseline"].description: expected 'Sets up a local AWS Backup vault and attaches multiple AWS Backup plans.', got 'Sets up backup resources for member accounts within the target OU.'
- $.baselines[name="CentralConfigBaseline"].description: expected 'Sets up central resources for compliance monitoring and auditing within your organization using AWS Config.', got 'Sets up central resources for AWS Config in your organization.'
- $.baselines[name="ConfigBaseline"].description: expected 'Sets up AWS Config resources for member accounts within the target OU, required for Detective Controls enablement. The resources set up are a subset of resources of AWSControlTowerBaseline.', got 'Sets up resources for AWS Config in accounts in your organization.'
Divergence key: service=controltower|ListBaselines|status=200->200|keys=$.baselines[name="BackupAdminBaseline"].description,$.baselines[name="BackupBaseline"].description,$.baselines[name="CentralConfigBaseline"].description,$.baselines[name="ConfigBaseline"].description

Cause 2 -- ListBaselines, fixture controltower-catalog-list-baselines, step task#1, expected status 200 / actual status 200:
- $.baselines[arn="arn:aws:controltower:<region>::baseline/H6C5JFCJJ3CPU3J5"].description: expected 'Sets up AWS Backup Audit Manager.', got 'Sets up a delegated administrator account for AWS Backup in your organization.'
- $.baselines[arn="arn:aws:controltower:<region>::baseline/APO9ATVPBKFRRGLK"].description: expected 'Sets up a local AWS Backup vault and attaches multiple AWS Backup plans.', got 'Sets up backup resources for member accounts within the target OU.'
- $.baselines[arn="arn:aws:controltower:<region>::baseline/YX7VMZML5IG8EJUD"].description: expected 'Sets up central resources for compliance monitoring and auditing within your organization using AWS Config.', got 'Sets up central resources for AWS Config in your organization.'
- $.baselines[arn="arn:aws:controltower:<region>::baseline/1QBGH2G48YVGDQ3Y"].description: expected 'Sets up AWS Config resources for member accounts within the target OU, required for Detective Controls enablement. The resources set up are a subset of resources of AWSControlTowerBaseline.', got 'Sets up resources for AWS Config in accounts in your organization.'
Divergence key: service=controltower|ListBaselines|status=200->200|keys=$.baselines[arn=...].description (four entries). The region segment shown as <region> is environment-derived (us-west-2 in this run) and is not part of the requirement; the 16-character id segment and the description text are.

Smithy cited (services/service-com-amazonaws-controltower-awscontroltowerapis/smithy-model/com-amazonaws-controltower-awscontroltowerapis.json): ListBaselines http POST /list-baselines code 200, readonly, paginated(items=baselines); ListBaselinesOutput.baselines required; BaselineSummary{arn required String, name required String, description String -- 'A summary description of a Baseline', unconstrained}; GetBaseline.baselineIdentifier required BaselineArn @pattern ^arn:[a-z-]+:controltower:[a-z0-9-]*:[0-9]{0,12}:baseline/[A-Z0-9]{16}$. The model does not fix the description strings, so they are taken from the recorded responses.

## Steps to Reproduce
1. Clone the repository and boot the emulator built from this checkout.
2. Replay trace set simparity-wire-aws-v1.
3. Cause 1: replay fixture controltower-baseline-catalog-discovery, step task#1 -- POST /list-baselines with the recorded ListBaselines input. Index the returned baselines[] by its name member and compare the description member of BackupAdminBaseline, BackupBaseline, CentralConfigBaseline and ConfigBaseline against the recorded expected response. Each returns emulator-authored prose instead of AWS's fixed description string, while the status is 200 either way.
4. Cause 2: replay fixture controltower-catalog-list-baselines, step task#1 -- the same POST /list-baselines call. Index baselines[] by its arn member instead and compare the description at baseline/H6C5JFCJJ3CPU3J5, baseline/APO9ATVPBKFRRGLK, baseline/YX7VMZML5IG8EJUD and baseline/1QBGH2G48YVGDQ3Y. The same four re-worded strings appear, confirming one shared catalog seed behind both index keys.
5. Optionally call POST /get-baseline (GetBaseline) with one of those ARNs to see the same re-worded description served through the single-baseline read.
6. Both causes must be re-replayed after a fix: correcting only the name-keyed fixture leaves the ARN-keyed cluster unexercised.
