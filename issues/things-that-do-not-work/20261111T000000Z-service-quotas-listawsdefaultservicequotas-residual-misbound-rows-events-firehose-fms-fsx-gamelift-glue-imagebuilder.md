Issue-Source: fix-residue
Issue-Trace: service-quotas-catalog-list-aws-default-service-quotas-events
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: service-quotas-catalog-list-aws-default-service-quotas-events service-quotas-catalog-list-aws-default-service-quotas-firehose service-quotas-catalog-list-aws-default-service-quotas-fms service-quotas-catalog-list-aws-default-service-quotas-fsx service-quotas-catalog-list-aws-default-service-quotas-gamelift service-quotas-catalog-list-aws-default-service-quotas-glue service-quotas-catalog-list-aws-default-service-quotas-imagebuilder
Issue-Shape-Id: com.amazonaws.servicequotas#ServiceQuotasV20190624

# ListAWSDefaultServiceQuotas still misbinds the rows no recorded value is quoted for, for events, firehose, fms, fsx, gamelift, glue and imagebuilder

## Operation
ListAWSDefaultServiceQuotas

## Description
RESIDUE of the fix for `20261110T235811Z-service-quotas-listawsdefaultservicequotas-events-firehose-fms-fsx-gamelift-glacier-glue.md`
(now under `issues/things-that-now-work/`). That report bundled ten `ServiceCode`s. Three of
them — `glacier`, `groundstation` and `guardduty` — replay clean after the fix, and in the
other seven every row whose AWS-published facts the report QUOTES is now published under the
`QuotaCode` the recording binds it to. What is left is the rows the report lists without
quoting their recorded values: the emulator answers HTTP 200 and the row under those codes
still carries a `QuotaName`, `Value`, `Unit`, `Adjustable`, `Period`, `UsageMetric` or
`Description` of another quota, or a `Description` worded by the emulator rather than by the
service.

THE RULE (unchanged, and the one the fix applied to the quoted rows). A row published under a
`QuotaCode` carries the `QuotaName`, `Value`, `Unit`, `Adjustable`, `GlobalQuota`, `Period`,
`UsageMetric` and `Description` of the quota AWS publishes UNDER THAT CODE — the text as the
service words it, never a paraphrase and never a neighbouring quota's — and a quota whose own
code no source states is published under no code at all. Smithy fixes the row SHAPE
(`ServiceQuota`: `QuotaCode` @length 1..128, `QuotaName`, `Value` @range 0..10000000000,
`Unit`, `Adjustable`, `GlobalQuota`, `UsageMetric`, `Period`, `QuotaContext`, `Description`
@length 0..350); the per-code CONTENT is recorded behaviour, so the recorded
`ListAWSDefaultServiceQuotas` response of each `ServiceCode` is the reference.

WHAT A FIXER NEEDS, AND WHY THIS IS FILED SEPARATELY. The remaining rows cannot be authored
from anything in this checkout: the replay tool answers "recorded values withheld" for these
clusters, and the original report quotes AWS's values only for the rows already fixed. Closing
this needs the recorded rows themselves (the `QuotaName`, `Value`, `Unit`, `Adjustable`,
`Period`, `UsageMetric` and `Description` each of the codes below publishes). Authoring a
plausible sentence or code pairing instead is the defect itself and was rejected at review.

## Evidence
Replay of trace set simparity-wire-aws-v1 after the fix: `glacier`, `groundstation` and
`guardduty` pass; the seven fixtures below still diverge on body content only (expected 200,
actual 200). The codes still reported, per `ServiceCode`:

- events (fixture service-quotas-catalog-list-aws-default-service-quotas-events, step task#1):
  L-39263BDC, L-3C47459F, L-595D6D42, L-5D1D13E3, L-8331B29E, L-9B653E91, L-D082A4F6,
  L-E2110C81, L-FB1C3A6D. L-3C47459F's recorded row carries a `Period`, so the quota under it
  is a rate rather than the archive plane the catalog serves; L-D082A4F6 is published under no
  row at all, because the event-bus plane it had carried belongs to L-658A4FD9.
- firehose: L-17D4EADE, L-7681C790, L-937C879B — `Description` TEXT only. The three rows are
  the data-plane rates of one delivery stream ("Rate of records" 500000.0, "Rate of Put
  requests" 2000.0, "Rate of data" 5.0), now published with no `Description` rather than one
  the emulator words itself; the recorded sentences are what is missing.
- fms: L-07FCB28B, L-0B28E140, L-1608788B, L-1E778CA5, L-3BC4947E, L-3DCEAE01, L-450C679C,
  L-49265FF2, L-4E3D82C8, L-5AB0AC42, L-72AC804F, L-743D9B7E, L-74591874, L-7AB3A351,
  L-7F63BF51, L-941CF907, L-98694ACE, L-9E030C62, L-BCA8AD34, L-C6CF2DBB, L-C80E7B9E,
  L-CA0A307F, L-DE1D154D, L-F4032167.
- fsx: L-0CD18A5D, L-15D9FE87, L-4EDE4065, L-57578687, L-59D0763F, L-5B89F9CE, L-60836D3E,
  L-736F3D6F, L-7D5FDD38, L-84EAF187, L-88479C21, L-8F1B9C74, L-901C77F5, L-9AFA1F09,
  L-AD2FC696, L-C48231E5, L-C5F860DD, L-C8640C82, L-CD5E0524, L-DD0F7417, L-E24B4DE4,
  L-E94C1C19, L-FD6F2F22. L-4EDE4065, L-7D5FDD38 and L-8F1B9C74 are published under no row,
  because the planes they had carried belong to L-FD89CA8A, L-C28C1403 and the codes the
  recording binds them to.
- gamelift: L-11948650, L-22451070, L-24BE0A39 (`Description` and the value of the timeout),
  L-293B0017, L-3A43EF3C, L-51AF299A, L-55650DB7, L-593688D9, L-67477D57, L-73F6E300,
  L-8AE49BBD, L-8D885299, L-8EBC4E87, L-90D24F1B, L-9F9DE0B2, L-A8C1B434 (`Description`),
  L-BB62CF1D, L-BFEEB817, L-C30AA854, L-C6F4238C, L-CCBBB4DA, L-E8D8BD94, L-EC03D793,
  L-FDDD1260.
- glue: L-04CEE988, L-071ABE08, L-08F3B322, L-096DBB95, L-11FA2C1A (`Description`), L-1DD415D5
  (`Description`), L-2C3F5401, L-2E9FB93F, L-337244C9, L-3890A802, L-4071B0E3, L-4256D6D2,
  L-5E4153CA, L-64A88F0A, L-76EC689B, L-785C1F39, L-7DD7C33A, L-83192DBF, L-83A59AA6,
  L-87F5991C, L-94D025B7, L-A457CBAB, L-ACAC12B6, L-AD871090, L-B78896B9, L-B8497671,
  L-C3952F6C, L-CB69DFA0, L-D987EC31, L-DBA56E2F, L-E15CE20A, L-E6C807E7, L-EAC30D24,
  L-EEC98450, L-F1653A6D, L-F1AAB534, L-F545C262, L-F574AED9, L-F953935E, L-FB210BD9,
  L-FEBBFA7A. L-337244C9 is published under no row, because the account-wide crawler plane it
  had carried belongs to L-11FA2C1A.
- imagebuilder: L-0DF2752F, L-10D22E0D, L-1DF98342, L-31E5726E, L-59EC76F6, L-5E848392,
  L-6105A4EE, L-69312229, L-7A0E01E3, L-84F5AB81, L-9B183655, L-A26B27A7, L-A58CFBED,
  L-B588F8CA, L-BA4D191B, L-D5665818, L-D9E1BB3C, L-E49825CC. L-10D22E0D is published under no
  row, because the infrastructure-configuration plane it had carried belongs to L-D9EF98D9.

The rows the fix DID correct, and must stay corrected, are pinned in
`services/service-com-amazonaws-servicequotas-servicequotasv20190624/tests/all/aws_quota_metadata_per_code.rs`;
the codes published under no row are named per service in
`tests/all/aws_minted_catalog_coverage.rs` and in the row tables of
`src/state/quota_rows.rs`.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. For each `ServiceCode` above, POST `ListAWSDefaultServiceQuotas` with
   `{"ServiceCode": "<code>"}` to the service-quotas endpoint and compare `$.Quotas`, keyed by
   `QuotaCode`, against the recorded response.
3. A `ServiceCode` is clean when every returned row's `QuotaName`, `Description`, `Value`,
   `Unit`, `Adjustable`, `GlobalQuota`, `Period` and `UsageMetric` match the recorded row for
   the SAME `QuotaCode`, and no recorded `QuotaCode` is absent.
