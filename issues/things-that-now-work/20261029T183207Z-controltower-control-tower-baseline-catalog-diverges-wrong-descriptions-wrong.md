Issue-Source: trace-sweep-divergence
Issue-Trace: controltower-baseline-catalog-discovery
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: controltower-baseline-catalog-discovery controltower-catalog-list-baselines
Issue-Shape-Id: com.amazonaws.controltower#AWSControlTowerApis
Issue-Key: controltower|ListBaselines|2xx->2xx|$.baselines[name="AWSControlTowerBaseline"].description
Issue-Key: controltower|ListBaselines|2xx->2xx|$.baselines[name="AuditBaseline"].description
Issue-Key: controltower|ListBaselines|2xx->2xx|$.baselines[name="BackupAdminBaseline"].arn
Issue-Key: controltower|ListBaselines|2xx->2xx|$.baselines[name="BackupAdminBaseline"].description
Issue-Key: controltower|ListBaselines|2xx->2xx|$.baselines[name="BackupBaseline"].arn
Issue-Key: controltower|ListBaselines|2xx->2xx|$.baselines[name="BackupBaseline"].description
Issue-Key: controltower|ListBaselines|2xx->2xx|$.baselines[name="BackupCentralVaultBaseline"].arn
Issue-Key: controltower|ListBaselines|2xx->2xx|$.baselines[name="BackupCentralVaultBaseline"].description
Issue-Key: controltower|ListBaselines|2xx->2xx|$.baselines[name="CentralConfigBaseline"]
Issue-Key: controltower|ListBaselines|2xx->2xx|$.baselines[name="CentralSecurityRolesBaseline"]
Issue-Key: controltower|ListBaselines|2xx->2xx|$.baselines[name="ConfigBaseline"]
Issue-Key: controltower|ListBaselines|2xx->2xx|$.baselines[name="IdentityCenterBaseline"].description
Issue-Key: controltower|ListBaselines|2xx->2xx|$.baselines[name="LogArchiveBaseline"].arn
Issue-Key: controltower|ListBaselines|2xx->2xx|$.baselines[name="LogArchiveBaseline"].description
Issue-Key: controltower|GetBaseline|2xx->2xx|$.description
Issue-Key: controltower|GetBaseline|2xx->2xx|$.name
Issue-Key: controltower|ListBaselines|2xx->2xx|$.baselines[arn="arn
Issue-Commit: 2f933b71caa2a46f81d276f12cbb60da2946d084

# Control Tower baseline catalog diverges: wrong descriptions, wrong ARN-to-name mapping and missing AWS-managed baselines

> _Note: this issue was identified against the code base at commit `2f933b71caa2a46f81d276f12cbb60da2946d084`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListBaselines

## Description
The emulator serves its own invented baseline catalog instead of the AWS-managed one. The catalog is AWS-owned, identical in every account, and a client discovers baselines by name/ARN and then enables them, so every member of it (name, ARN and description) is a wire requirement rather than an emulator choice. Three causes, all rooted in the BASELINE_CATALOG constant the controltower crate resolves reads against.

Cause 1 of 3 — ListBaselines omits AWS-managed baselines and re-words every description (fixture controltower-baseline-catalog-discovery, step task#1).
AWS: Smithy: ListBaselines -> ListBaselinesOutput.baselines (required, list of Baseline{arn,name,description}); GetBaseline.arn @pattern ^arn:[a-z-]+:controltower:[a-z0-9-]*:[0-9]{0,12}:baseline/[A-Z0-9]{16}$. The names and descriptions themselves are recorded behavior only: the recording shows the catalog containing, among others, CentralSecurityRolesBaseline ('Sets up central resources for security monitoring within your organization.'), CentralConfigBaseline, ConfigBaseline, LogArchiveBaseline ('Sets up a central repository for logs of API activities and resource configurations from accounts in your organization.'), BackupCentralVaultBaseline ('Sets up a central AWS Backup vault in your organization.'), BackupAdminBaseline and BackupBaseline, with AuditBaseline described as 'Sets up resources to monitor security and compliance of accounts in your organization.', IdentityCenterBaseline as 'Sets up shared resources for AWS Identity Center, which prepares the AWSControlTowerBaseline to set up Identity Center access for accounts.' and AWSControlTowerBaseline as 'Sets up resources and mandatory controls for member accounts within the target OU, required for AWS Control Tower governance.' These description strings are wholly fixed text — they contain no request- or state-derived values.
Emulator: several of those entries are absent from the listing entirely (CentralSecurityRolesBaseline, CentralConfigBaseline, ConfigBaseline are reported missing), and every description that is present is emulator-authored prose, e.g. AuditBaseline is described as 'Sets up the cross-account audit resources AWS Control Tower requires in the audit account.' and AWSControlTowerBaseline as 'Sets up the resources and permissions AWS Control Tower requires to govern the accounts of a target organizational unit.' Status is 200 in both.
Why: a client that selects a baseline by name cannot find the one it needs, and anything that surfaces the catalog to a user (console-like listings, documentation generators, policy checks keyed on the description) renders text AWS never emits.
Fix: the baseline catalog seed (BASELINE_CATALOG) in the controltower crate's state module, which ListBaselines projects.
Traces: controltower-baseline-catalog-discovery#task#1.

Cause 2 of 3 — GetBaseline resolves a baseline ARN to the wrong catalog entry (fixture controltower-baseline-catalog-discovery, step task#2).
AWS: Smithy: GetBaseline.baselineIdentifier (required, BaselineArn) -> GetBaselineOutput{arn required, name required, description}; the identifier must resolve to the one baseline that bears it, so the returned name/description belong to the requested ARN. In the recording the requested ARN resolves to AuditBaseline, description 'Sets up resources to monitor security and compliance of accounts in your organization.'
Emulator: the same ARN resolves to a different baseline — name 'AWSControlTowerBaseline', description 'Sets up the resources and permissions AWS Control Tower requires to govern the accounts of a target organizational unit.' — answered 200.
Why: the ARN-to-baseline mapping is not AWS's, so a client that read an ARN out of ListBaselines (or that hard-codes a documented baseline ARN) gets a different baseline back, and a subsequent EnableBaseline applies the wrong baseline to its target.
Fix: the ARN assignment in the same catalog constant, and GetBaseline's lookup against it.
Traces: controltower-baseline-catalog-discovery#task#2.

Cause 3 of 3 — the catalog's ARNs are not the AWS ones, so baselines are unfindable by ARN (fixture controltower-catalog-list-baselines, step task#1).
AWS: recorded behavior only — the catalog pairs fixed ARNs with fixed names, e.g. LogArchiveBaseline with arn:aws:controltower:<region>::baseline/J8HX46AHS5MIKQPD, BackupCentralVaultBaseline with .../3WPD0NA6TJ9AOMU2 and CentralSecurityRolesBaseline with .../A6EEAE10F08193F2 (the region segment is request-derived; the 16-character uppercase id segment is fixed per baseline, and the account segment is empty for these AWS-managed baselines).
Emulator: the listing answers 200 but mints different id segments for the same names (LogArchiveBaseline served as .../2V6DZ8QKR4JT7MWN, BackupCentralVaultBaseline as .../WM8FJ6YHL3QS5XKB), so a lookup keyed on the AWS ARN finds nothing: the baselines at the AWS ARNs are reported missing from the response.
Why: baseline ARNs are stable, documented identifiers that clients and templates embed; a client enabling a baseline by ARN gets ResourceNotFound or silently enables a different baseline.
Fix: the per-baseline ARN id segments in the catalog constant.
Traces: controltower-catalog-list-baselines#task#1.

## Evidence
Replay of trace set simparity-wire-aws-v1 (controltower, 3 clusters):

1) ListBaselines, controltower-baseline-catalog-discovery#task#1, expected 200 / actual 200 — $.baselines[name="AuditBaseline"].description: expected 'Sets up resources to monitor security and compliance of accounts in your organization.', got 'Sets up the cross-account audit resources AWS Control Tower requires in the audit account.'; $.baselines[name="CentralSecurityRolesBaseline"]: missing (expected {arn: arn:aws:controltower:us-west-2::baseline/A6EEAE10F08193F2, description 'Sets up central resources for security monitoring within your organization.'}); $.baselines[name="LogArchiveBaseline"].arn: expected 'arn:aws:controltower:us-west-2::baseline/J8HX46AHS5MIKQPD', got 'arn:aws:controltower:us-west-2::baseline/2V6DZ8QKR4JT7MWN'; $.baselines[name="BackupCentralVaultBaseline"].arn: expected '.../3WPD0NA6TJ9AOMU2', got '.../WM8FJ6YHL3QS5XKB'; also $.baselines[name="CentralConfigBaseline"] and $.baselines[name="ConfigBaseline"] missing, and IdentityCenterBaseline / AWSControlTowerBaseline / BackupAdminBaseline / BackupBaseline descriptions mismatched.

2) GetBaseline, controltower-baseline-catalog-discovery#task#2, expected 200 / actual 200 — $.name: expected 'AuditBaseline', got 'AWSControlTowerBaseline'; $.description: expected 'Sets up resources to monitor security and compliance of accounts in your organization.', got 'Sets up the resources and permissions AWS Control Tower requires to govern the accounts of a target organizational unit.'

3) ListBaselines, controltower-catalog-list-baselines#task#1, expected 200 / actual 200 — entries keyed by ARN report missing for arn:aws:controltower:us-west-2::baseline/A6EEAE10F08193F2 (CentralSecurityRolesBaseline), .../J8HX46AHS5MIKQPD (LogArchiveBaseline) and .../3WPD0NA6TJ9AOMU2 (BackupCentralVaultBaseline); .../4T4HA1KMO10S6311 and .../17BSJV3IGJ2QSGA2 and .../LN25R72TTG6IGPTQ carry emulator-authored descriptions.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Fixture controltower-baseline-catalog-discovery, step task#1: POST /list-baselines and compare each baselines[] entry's name, arn and description against the recorded response — several names are absent and every description differs.
3. Same fixture, step task#2: POST /get-baseline with the recorded baselineIdentifier and observe the response names a different baseline (AWSControlTowerBaseline rather than AuditBaseline).
4. Fixture controltower-catalog-list-baselines, step task#1: POST /list-baselines and look up the recorded AWS baseline ARNs (e.g. arn:aws:controltower:us-west-2::baseline/J8HX46AHS5MIKQPD) in the response — they are not present.
