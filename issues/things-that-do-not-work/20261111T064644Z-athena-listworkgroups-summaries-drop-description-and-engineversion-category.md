Issue-Source: trace-sweep-divergence
Issue-Trace: athena-data-catalog-crud-lifecycle
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: athena-data-catalog-crud-lifecycle athena-workgroup-list-contains-created athena-workgroup-lifecycle-and-query-execution
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena|ListWorkGroups|2xx->2xx|$.WorkGroups[Name="primary"].Description
Issue-Key: athena|ListWorkGroups|2xx->2xx|$.WorkGroups[Name="primary"].EngineVersion.Category
Issue-Key: athena|ListWorkGroups|2xx->2xx|$.WorkGroups[Name="sim2real-conv-athena-wg-6e79a49c"].Description
Issue-Key: athena|ListWorkGroups|2xx->2xx|$.WorkGroups[Name="sim2real-conv-athena-wg-6e79a49c"].EngineVersion.Category
Issue-Key: athena|ListWorkGroups|4xx->4xx|$.Message
Issue-Commit: f456950adb04cb9e7696776dd0db02907fd8875a

# Athena ListWorkGroups summaries drop Description and EngineVersion.Category, and its malformed-token refusal is re-worded (2 causes)

> _Note: this issue was identified against the code base at commit `f456950adb04cb9e7696776dd0db02907fd8875a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListWorkGroups

## Description
Two net-new root causes in Athena ListWorkGroups. Both must be closed before this issue is submitted.

Cause 1 of 2 — every WorkGroupSummary in the list omits Description and EngineVersion.Category (fixtures athena-data-catalog-crud-lifecycle step task#1 for the built-in 'primary' workgroup, and athena-workgroup-list-contains-created step task#2 for a client-created workgroup).
AWS: Smithy: ListWorkGroups output WorkGroups -> WorkGroupsList of WorkGroupSummary, whose members are Name, State, Description (WorkGroupDescriptionString, @length 0-1024, 'The workgroup description'), CreationTime and EngineVersion. AWS emits Description for every summary — the empty string when the workgroup carries no description, not an absent member — and the recordings additionally show EngineVersion carrying a Category member ('Presto'); Category is absent from the published model, so it is recorded behavior only. The description text is whatever was supplied at CreateWorkGroup (empty string when none); Category reflects the engine family backing the effective engine version.
Emulator: both members are missing from the summaries (Description expected '', got None; EngineVersion.Category expected 'Presto', got None), for the built-in workgroup and for created ones alike.
Why: a client listing workgroups to render or match them cannot distinguish 'no description' from 'not reported' and sees a null where AWS guarantees a string, so code that reads .Description directly raises; code that routes on the engine family (Presto vs Spark workgroups) finds no Category and cannot classify a workgroup without a second GetWorkGroup call.
Fix: the ListWorkGroups response serializer / the workgroup summary projection, which must always emit Description (defaulting to '') and the EngineVersion Category alongside the selected/effective versions.
Traces: athena-data-catalog-crud-lifecycle#task#1, athena-workgroup-list-contains-created#task#2.

Cause 2 of 2 — the refusal for a malformed pagination token carries emulator-authored text (fixture athena-workgroup-lifecycle-and-query-execution, step task#11).
AWS: ListWorkGroups refuses an unparseable NextToken with HTTP 400 InvalidRequestException and Message 'The nextPageToken is malformed: <token>' — fixed text 'The nextPageToken is malformed: ' followed by the token value copied from the request. Smithy: ListWorkGroups input NextToken (Token, @length 1-1024), modeled error InvalidRequestException (httpError 400, members AthenaErrorCode and Message); the message wording is recorded behavior.
Emulator: the status and error class are right but Message is 'Invalid NextToken is provided' — different fixed text and the offending token is not echoed.
Why: the token value is the only diagnostic a caller has for a corrupted or cross-operation token; a client (or an operator reading logs) that matches AWS's wording or extracts the token from it gets neither.
Fix: the ListWorkGroups pagination-token validation in the handler, where the InvalidRequestException message is composed.
Traces: athena-workgroup-lifecycle-and-query-execution#task#11.

Related open issues that do NOT cover these two causes: '20261104T063259Z-athena-workgroup-and-data-catalog-reads-omit-modeled.md' and '20261109T161941Z-athena-getworkgroup-omits-systemtablestatuses-and-enforceworkgroupconfiguration-and-over.md' are about GetWorkGroup/GetDataCatalog reads, not the ListWorkGroups summary projection; '20261104T163600Z-athena-conflict-not-found-and-output-location-refusals.md' covers other re-worded refusals, and '20261102T132146Z-glue-pagination-emits-a-raw-name-nexttoken-ignores.md' is Glue. The replay report attributes none of these clusters to an open issue.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout.

Cause 1 — fixture athena-data-catalog-crud-lifecycle, step task#1, ListWorkGroups, 200 -> 200:
  $.WorkGroups[Name="primary"].Description: missing — expected '' (empty string), got None
  $.WorkGroups[Name="primary"].EngineVersion.Category: missing — expected 'Presto', got None
  Same divergence for a client-created workgroup at athena-workgroup-list-contains-created#task#2 ($.WorkGroups[Name="<created-wg>"].Description expected '', got None; .EngineVersion.Category expected 'Presto', got None). The workgroup names in the selectors are per-run values; the requirement is that both members are present on every summary.

Cause 2 — fixture athena-workgroup-lifecycle-and-query-execution, step task#11, ListWorkGroups, 400 -> 400:
  $.Message: expected 'The nextPageToken is malformed: notavalidnexttoken', got 'Invalid NextToken is provided'
  Fixed text: 'The nextPageToken is malformed: '; variable part: the NextToken value from the request (here 'notavalidnexttoken').

## Steps to Reproduce
1. git_clone() the project and boot the emulator built from the checkout.
2. replay_wire_corpus() over trace set simparity-wire-aws-v1.
3. Cause 1: replay fixture athena-data-catalog-crud-lifecycle to step task#1 (ListWorkGroups with no workgroups created yet, returning the built-in 'primary'), and fixture athena-workgroup-list-contains-created to step task#2 (ListWorkGroups after CreateWorkGroup). Observe each WorkGroupSummary without a Description member and without EngineVersion.Category.
4. Cause 2: replay fixture athena-workgroup-lifecycle-and-query-execution to step task#11 — call ListWorkGroups with NextToken='notavalidnexttoken'. Observe 400 InvalidRequestException with Message 'Invalid NextToken is provided' instead of 'The nextPageToken is malformed: notavalidnexttoken'.
