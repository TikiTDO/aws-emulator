Issue-Source: trace-sweep-divergence
Issue-Trace: guardduty-investigation-lifecycle
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: guardduty-investigation-lifecycle guardduty-tagging-created-with-tags guardduty-tagging-round-trip
Issue-Shape-Id: com.amazonaws.guardduty#GuardDutyAPIService
Issue-Key: guardduty|GetInvestigation|2xx->2xx|$.investigation.cloud
Issue-Commit: 221dd3602e5d0e66d71e70da1fb8509b07c948c5

# GuardDuty GetInvestigation fabricates investigation.cloud from endpoint context instead of omitting it

> _Note: this issue was identified against the code base at commit `221dd3602e5d0e66d71e70da1fb8509b07c948c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetInvestigation

## Description
GetInvestigation returns 200 with a correct status line, but the serialized investigation object carries a 'cloud' sub-structure that GuardDuty does not emit for the investigation record being read. 1 cause.

Cause 1 of 1: GetInvestigation invents an investigation.cloud block.
AWS: Investigation.Cloud is an OPTIONAL member (Smithy: com.amazonaws.guardduty#Investigation member Cloud, jsonName 'cloud', target CloudDetails; the required members of Investigation are only InvestigationId/Status/TriggerPrompt/TriggeredBy). Its documentation is 'Details about the cloud environment in which the investigation was performed, including the provider, region, and account', and CloudDetails itself requires provider/region/account — i.e. the structure exists only once the investigation has actually resolved an investigated resource's environment. Recorded behavior only for the omission rule: in the replayed recordings the GetInvestigation body for a freshly created investigation carries NO 'cloud' member at all. Status 200, no error code involved.
Emulator: unconditionally synthesizes the member from its own endpoint/credential context, emitting cloud = {provider: <cloud provider constant>, region: <the emulator's configured region>, account: <the caller's account id>} on every GetInvestigation read. (In the replayed step those values were AWS / us-west-2 / 000000000000; the host region and account id are environment-specific and are not themselves the requirement — the requirement is that the member is absent when the investigation record holds no cloud details.)
Why: a client that uses the presence of 'cloud' as the signal that the investigation has determined which provider/region/account it analysed will treat an unresolved investigation as resolved, and will read a region and account that GuardDuty never asserted for that investigation — for an investigation whose resource lives in another region or another member account, the fabricated values are simply wrong. Round-tripping the read also persists invented environment data.
Fix: the GetInvestigation response serializer and the investigation state record it reads — carry Cloud as an optional field on the stored investigation and emit the 'cloud' member only when that field is populated, instead of deriving it from request/endpoint context at serialization time.
Traces: guardduty-investigation-lifecycle#task#5, guardduty-tagging-created-with-tags#setup#4, guardduty-tagging-round-trip#setup#4.

Related but NOT covering this: open issue 20261029T190006Z-guardduty-read-responses-omit-required-scalars-and-invent-empty.md describes the same emulator defect as its 'Cause 9', but its replay linkage for that cause points at guardduty-investigation-lifecycle#task#4, not the GetInvestigation step that actually diverges (task#5), so the failing step is not gated by it and the replay sweep still reports it as unowned.

## Evidence
Replayed step guardduty-investigation-lifecycle#task#5 (GetInvestigation), verdict mismatch, expected status 200 / actual status 200.
$.investigation.cloud: type_mismatch — expected None (member absent from the recorded body), got a populated CloudDetails object {'provider': 'AWS', 'region': 'us-west-2', 'account': '000000000000'} (provider/region/account filled from the emulator's own endpoint and caller context).
Same cluster, same divergence key service=guardduty|GetInvestigation|status=200->200|keys=$.investigation.cloud, also at guardduty-tagging-created-with-tags#setup#4 and guardduty-tagging-round-trip#setup#4 (3 affected steps).

## Steps to Reproduce
1. Boot the emulator and replay fixture guardduty-investigation-lifecycle: CreateInvestigation on a detector, then GetInvestigation at step task#5.
2. Compare the 200 body's investigation object against the recording: the recorded body has no 'cloud' key; the emulator's body contains cloud.{provider,region,account}.
3. The same fabrication reproduces on the GetInvestigation step of guardduty-tagging-created-with-tags (setup#4) and guardduty-tagging-round-trip (setup#4).
