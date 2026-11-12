Issue-Source: trace-sweep-divergence
Issue-Trace: lakeformation-data-cells-filter-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lakeformation-data-cells-filter-lifecycle lakeformation-data-lake-location-registration lakeformation-data-lake-settings-and-principal lakeformation-lf-tag-lifecycle lakeformation-table-permission-grant-revoke
Issue-Shape-Id: com.amazonaws.lakeformation#AWSLakeFormation
Issue-Key: lakeformation|GetDataCellsFilter|2xx->2xx|$.DataCellsFilter.VersionId
Issue-Key: lakeformation|GetDataCellsFilter|2xx->2xx|$.DataCellsFilter.ColumnNames[*]
Issue-Key: lakeformation|GetDataCellsFilter|2xx->2xx|$.DataCellsFilter.RowFilter.AllRowsWildcard
Issue-Key: lakeformation|GetDataCellsFilter|2xx->2xx|$.DataCellsFilter.RowFilter.FilterExpression
Issue-Key: lakeformation|DescribeResource|2xx->2xx|$.ResourceInfo.ExpectedResourceOwnerAccount
Issue-Key: lakeformation|DescribeResource|2xx->2xx|$.ResourceInfo.HybridAccessEnabled
Issue-Key: lakeformation|DescribeResource|2xx->2xx|$.ResourceInfo.LastModified
Issue-Key: lakeformation|DescribeResource|2xx->2xx|$.ResourceInfo.ResourceArn
Issue-Key: lakeformation|DescribeResource|2xx->2xx|$.ResourceInfo.VerificationStatus
Issue-Key: lakeformation|GetDataLakeSettings|2xx->2xx|$.DataLakeSettings.DisallowGrantOnIAMAllowedPrincipals
Issue-Key: lakeformation|GetDataLakeSettings|2xx->2xx|$.DataLakeSettings.SetSourceIdentity
Issue-Key: lakeformation|GetDataLakeSettings|2xx->2xx|$.DataLakeSettings.WhitelistedForExternalDataFiltering
Issue-Key: lakeformation|PutDataLakeSettings|4xx->2xx|$
Issue-Key: lakeformation|PutDataLakeSettings|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: lakeformation|PutDataLakeSettings|4xx->2xx|status
Issue-Key: lakeformation|GetDataLakeSettings|2xx->2xx|$.DataLakeSettings.ReadOnlyAdmins[*]
Issue-Key: lakeformation|GetLFTag|2xx->2xx|$.TagValues[*]
Issue-Key: lakeformation|GrantPermissions|4xx->2xx|$
Issue-Key: lakeformation|GrantPermissions|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: lakeformation|GrantPermissions|4xx->2xx|status
Issue-Commit: 42eb24959c6a4eb33ac3ba3d48b702ee83f6032d

# Lake Formation reads omit modeled members and accept invalid requests

> _Note: this issue was identified against the code base at commit `42eb24959c6a4eb33ac3ba3d48b702ee83f6032d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDataCellsFilter, DescribeResource, GetDataLakeSettings, PutDataLakeSettings, GetLFTag, GrantPermissions

## Description
Beyond error wording, the emulator's Lake Formation state and projections diverge in three ways: successful reads omit members AWS always populates (version ids, settings defaults, registration verification state), a mutation is not reflected in the subsequent read, and two requests AWS refuses with 400 are accepted with 200 — which then corrupts the state a later read serves. 8 distinct causes.

Cause 1 of 8 — GetDataCellsFilter does not report VersionId after create.
AWS: Smithy: DataCellsFilter.VersionId targets VersionString ('The ID of the data cells filter version'). Recorded: a freshly created filter reads back with DataCellsFilter.VersionId = the first version ('1'); the value is a monotonic version counter, not a literal.
Emulator: HTTP 200 (status correct) but $.DataCellsFilter.VersionId is absent.
Why: clients that pass the read-back VersionId into optimistic-concurrency flows (or merely display the filter version) get null and cannot version a filter at all.
Fix: the data-cells-filter state in the lakeformation service crate must assign and store a version on create, and the GetDataCellsFilter projection must emit it.
Traces: lakeformation-data-cells-filter-lifecycle#task#5.

Cause 2 of 8 — UpdateDataCellsFilter is not applied: the read still serves the pre-update filter, and the version does not advance.
AWS: Smithy: UpdateDataCellsFilter takes the full DataCellsFilter in TableData; RowFilter is a union-like structure of FilterExpression OR AllRowsWildcard, and ColumnNames is the replacement column list. Recorded: after an update that narrows ColumnNames, switches RowFilter to AllRowsWildcard ({}) and sets FilterExpression to the all-rows form, GetDataCellsFilter returns exactly the updated filter with VersionId advanced to the second version ('2').
Emulator: HTTP 200 returning the ORIGINAL filter — the removed column is still present in ColumnNames, RowFilter.AllRowsWildcard is absent, RowFilter.FilterExpression is still the pre-update predicate, and VersionId is absent (no version advance).
Why: an update silently does nothing while reporting success, so any client that updates a row/column filter and re-reads it believes the narrower filter is in force when the old one is. For a data-access filter this is a security-relevant stale read.
Fix: the UpdateDataCellsFilter handler must replace the stored filter (ColumnNames/ColumnWildcard and RowFilter wholesale, clearing the member not supplied) and bump the stored version; the GetDataCellsFilter projection then serves it.
Traces: lakeformation-data-cells-filter-lifecycle#task#11.

Cause 3 of 8 — DescribeResource projection omits registration state and emits members AWS does not.
AWS: Smithy: ResourceInfo members include RoleArn, HybridAccessEnabled, WithFederation, WithPrivilegedAccess, VerificationStatus (enum) and ExpectedResourceOwnerAccount. Recorded: DescribeResource for an S3 location registered with a role returns ResourceInfo carrying ExpectedResourceOwnerAccount = the caller's account id, HybridAccessEnabled = false (the RegisterResource default), VerificationStatus = 'VERIFIED', and does NOT echo ResourceArn or LastModified.
Emulator: HTTP 200 with ExpectedResourceOwnerAccount, HybridAccessEnabled and VerificationStatus all absent, while adding ResourceArn (the request's ARN) and LastModified as a floating-point epoch number.
Why: a client cannot tell whether a registered location's role actually verified (the field it polls after RegisterResource is null), cannot read back the hybrid-access flag it registered with, and sees two extra members — one of them an epoch float where the model declares a timestamp — which breaks strict deserialisers.
Fix: the DescribeResource projection and the RegisterResource state in the lakeformation service crate — persist the registration flags and owner account, derive VerificationStatus, and stop emitting the unreported members.
Traces: lakeformation-data-lake-location-registration#task#6.

Cause 4 of 8 — GetDataLakeSettings omits the settings defaults AWS always returns.
AWS: recorded behavior only for these member names (the on-disk Smithy DataLakeSettings shape does not declare them): GetDataLakeSettings returns DisallowGrantOnIAMAllowedPrincipals = false, SetSourceIdentity = false and WhitelistedForExternalDataFiltering = [] for a catalog whose settings have just been written without them. Every member is a documented default of the settings document, not a per-run value.
Emulator: HTTP 200 with all three members absent from $.DataLakeSettings.
Why: a read-modify-write of data lake settings (the only supported way to change one field) loses these fields, and a client that treats absent as 'unset' cannot round-trip the settings document.
Fix: the GetDataLakeSettings projection / data-lake-settings state — materialise the defaults when the stored settings do not carry them.
Traces: lakeformation-data-lake-settings-and-principal#task#4.

Cause 5 of 8 — PutDataLakeSettings accepts a principal listed as BOTH a read-only admin and a data lake admin.
AWS: Smithy: PutDataLakeSettings errors InvalidInputException (httpError 400, member Message); DataLakeSettings carries DataLakeAdmins and ReadOnlyAdmins as separate DataLakePrincipalList members. Recorded: HTTP 400 with {"Message": "Principal cannot be both readOnly and LF admin."} — wholly fixed text (the offending principal is not echoed), with x-amzn-errortype set to the modeled error.
Emulator: HTTP 200 with an empty body — the contradictory settings are accepted and written, and no x-amzn-errortype header is sent.
Why: the mutual-exclusion invariant Lake Formation enforces is absent, so a client's misconfiguration silently 'succeeds' and the emulator's admin model drifts from anything AWS would allow (see cause 6, which is the read that then diverges).
Fix: the PutDataLakeSettings validation — reject an identifier appearing in both DataLakeAdmins and ReadOnlyAdmins with InvalidInputException and AWS's fixed message.
Traces: lakeformation-data-lake-settings-and-principal#task#5.

Cause 6 of 8 — GetDataLakeSettings serves the state the refused write left behind, and still omits the defaults.
AWS: recorded behavior only: because the contradictory PutDataLakeSettings of cause 5 is refused, the catalog's ReadOnlyAdmins stays EMPTY, and the read returns DisallowGrantOnIAMAllowedPrincipals = false, SetSourceIdentity = false and WhitelistedForExternalDataFiltering = [].
Emulator: HTTP 200 whose $.DataLakeSettings.ReadOnlyAdmins[0] is the principal from the request AWS refused, with the same three default members missing.
Why: the emulator's settings document reports an admin arrangement AWS never permits, so authorization decisions a client makes from GetDataLakeSettings (who may change the lake) are wrong for the rest of the session.
Fix: same two components as causes 4 and 5 — the PutDataLakeSettings validation (so nothing is written) plus the GetDataLakeSettings projection's defaults.
Traces: lakeformation-data-lake-settings-and-principal#task#6 (cluster also covers task#8).

Cause 7 of 8 — GetLFTag returns TagValues in the wrong order.
AWS: Smithy: GetLFTagResponse.TagValues targets TagValueList (length 1..50). Recorded: after values are added to an LF tag, GetLFTag returns TagValues sorted ascending, so the values appear in lexicographic order rather than in the order they were supplied.
Emulator: HTTP 200 with the two trailing values transposed relative to the recording (the emulator preserves insertion order instead of sorting).
Why: a client that compares the tag's value list by position — diffing desired vs actual tag values, or indexing into TagValues — sees spurious drift and can re-issue pointless UpdateLFTag calls.
Fix: the GetLFTag projection in the lakeformation service crate (or the LF-tag state's value container) must order TagValues the way the service does.
Traces: lakeformation-lf-tag-lifecycle#task#8.

Cause 8 of 8 — GrantPermissions accepts a grant to a principal that does not exist.
AWS: Smithy: GrantPermissions errors InvalidInputException (httpError 400, member Message) and requires Principal (DataLakePrincipal). Recorded: granting to an IAM role ARN that was never created is refused HTTP 400 with {"Message": "Invalid principal, arn: <principalArn>"} — the fixed text is "Invalid principal, arn: " and the only variable part is the DataLakePrincipalIdentifier from the request — and x-amzn-errortype names the modeled error.
Emulator: HTTP 200 with an empty body and no x-amzn-errortype header — the grant is recorded against a non-existent principal.
Why: the emulator accepts permission grants AWS rejects, so a client's typo'd or stale principal is never surfaced and the permission store accumulates entries ListPermissions will report for a principal that cannot exist — tests of the failure path pass against the emulator and fail against AWS.
Fix: the GrantPermissions principal resolution/validation — resolve Principal against IAM state and raise InvalidInputException with AWS's invalid-principal template when it is unknown.
Traces: lakeformation-table-permission-grant-revoke#task#10.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200). All eight clusters are verdict=mismatch.

1. lakeformation-data-cells-filter-lifecycle#task#5 GetDataCellsFilter 200->200: $.DataCellsFilter.VersionId missing (expected '1', got None).
2. lakeformation-data-cells-filter-lifecycle#task#11 GetDataCellsFilter 200->200: $.DataCellsFilter.ColumnNames[1] extra (expected None, got the pre-update column); $.DataCellsFilter.RowFilter.AllRowsWildcard missing (expected {}, got None); $.DataCellsFilter.RowFilter.FilterExpression expected 'TRUE', got the pre-update predicate; $.DataCellsFilter.VersionId missing (expected '2', got None).
3. lakeformation-data-lake-location-registration#task#6 DescribeResource 200->200: ResourceInfo.ExpectedResourceOwnerAccount missing (expected the caller account id); ResourceInfo.HybridAccessEnabled missing (expected False); ResourceInfo.VerificationStatus missing (expected 'VERIFIED'); ResourceInfo.LastModified type_mismatch (expected None, got an epoch float); ResourceInfo.ResourceArn type_mismatch (expected None, got the request ARN).
4. lakeformation-data-lake-settings-and-principal#task#4 GetDataLakeSettings 200->200: DisallowGrantOnIAMAllowedPrincipals missing (expected False); SetSourceIdentity missing (expected False); WhitelistedForExternalDataFiltering missing (expected []).
5. lakeformation-data-lake-settings-and-principal#task#5 PutDataLakeSettings: status expected 400, got 200; body empty_body (expected {'Message': 'Principal cannot be both readOnly and LF admin.'}, got {}); $headers.x-amzn-errortype missing.
6. lakeformation-data-lake-settings-and-principal#task#6 (also task#8) GetDataLakeSettings 200->200: ReadOnlyAdmins[0] extra (expected None, got the refused principal); plus the same three missing default members.
7. lakeformation-lf-tag-lifecycle#task#8 GetLFTag 200->200: TagValues[1] expected 'restricted', got 'confidential'; TagValues[2] expected 'confidential', got 'restricted' (ordering inverted).
8. lakeformation-table-permission-grant-revoke#task#10 GrantPermissions: status expected 400, got 200; body empty_body (expected {'Message': 'Invalid principal, arn: <absent-role-arn>'}, got {}); $headers.x-amzn-errortype missing.

No open issue covers any of these; the only open lakeformation-adjacent issue (20261025T000319Z-glue-iam-lakeformation-s3-a-glue-job-run.md) is about a Glue job run executing no script.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus), or exercise the fixtures directly:
   - lakeformation-data-cells-filter-lifecycle: CreateDataCellsFilter, then GetDataCellsFilter (step task#5) and check DataCellsFilter.VersionId; then UpdateDataCellsFilter narrowing ColumnNames and setting RowFilter.AllRowsWildcard={}, then GetDataCellsFilter again (step task#11) and compare the whole filter plus VersionId.
   - lakeformation-data-lake-location-registration: RegisterResource for an S3 ARN with a role, then DescribeResource (step task#6) and compare ResourceInfo member-by-member.
   - lakeformation-data-lake-settings-and-principal: GetDataLakeSettings after a PutDataLakeSettings (step task#4); then PutDataLakeSettings with the same principal in both DataLakeAdmins and ReadOnlyAdmins (step task#5) and observe 200 instead of 400; then GetDataLakeSettings (step task#6) and observe ReadOnlyAdmins populated.
   - lakeformation-lf-tag-lifecycle: add tag values, then GetLFTag (step task#8) and compare TagValues positionally.
   - lakeformation-table-permission-grant-revoke: GrantPermissions to an IAM role ARN that was never created (step task#10) and observe 200 instead of 400 InvalidInputException.
3. Diff each response against the recorded AWS response for that step.
