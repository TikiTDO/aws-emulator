Issue-Source: trace-sweep-divergence
Issue-Trace: connectcases-template-lifecycle
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: connectcases-template-lifecycle
Issue-Shape-Id: com.amazonaws.connectcases#AmazonConnectCases
Issue-Key: connectcases|ListTemplates|2xx->2xx|$.templates[name="sim2real-probe-cc-template-ff722eba"].arn
Issue-Key: connectcases|ListTemplates|2xx->2xx|$.templates[name="sim2real-probe-cc-template-ff722eba"].id
Issue-Key: connectcases|ListTemplates|2xx->2xx|$.templates[name="sim2real-probe-cc-template-ff722eba"].tagPropagationConfigurations
Issue-Commit: f3853c2ce3831f59f19a9d4fb097e3f9d5244398

# connectcases ListTemplates summaries omit template id, arn and tagPropagationConfigurations

> _Note: this issue was identified against the code base at commit `f3853c2ce3831f59f19a9d4fb097e3f9d5244398`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTemplates

## Description
ONE cause.

Cause 1 of 1: ListTemplates returns template summaries that are missing the template identifier, the template ARN and tagPropagationConfigurations (fixture connectcases-template-lifecycle, step task#6).

AWS: Smithy (services/service-com-amazonaws-connectcases-amazonconnectcases/smithy-model/com-amazonaws-connectcases-amazonconnectcases.json, protocol restJson1): ListTemplates = POST /domains/{domainId}/templates-list, output ListTemplatesResponse.templates -> TemplateSummaryList (@length max 100, required). TemplateSummary models templateId (REQUIRED, @length 1-500), templateArn (REQUIRED, @length 1-500), name (REQUIRED, @length 1-100, pattern ^.*[\S]$), status (REQUIRED) and tagPropagationConfigurations (TagPropagationConfigurationList, @length max 1). On the recorded wire the identifier and ARN members appear under the key names `id` and `arn` (recorded behavior only for the key naming), and tagPropagationConfigurations is PRESENT as an empty list [] for a template with no propagation configuration. The ARN has the general form arn:aws:cases:<region>:<account-id>:domain/<domainId>/template/<templateId> — region, account id, domain id and template id all come from the request and the emulator's own state, so only that shape is the requirement, never the recording's literal values. Every entry AWS returns is therefore self-describing: a listed template can be fed straight back into GetTemplate/CreateCase.

Emulator: the 200 response body's summary entry for the just-created template carries neither the identifier nor the ARN under any key, and omits tagPropagationConfigurations. The replay diff reports only `missing` keys and no `extra` keys for that entry, so the emulator is not emitting the identifier/ARN under the alternative modeled spellings (templateId/templateArn) either — it simply never projects them. Status matches (200 vs 200); the defect is the body.

Why: a client that lists templates and then uses one is broken outright. There is no identifier to pass to GetTemplate, UpdateTemplate, DeleteTemplate or CreateCase(templateId=...), and no ARN for tagging/IAM/resource-reference flows; the required-member contract is violated, so a strict typed SDK deserializer can also reject the whole page. List-then-use is the primary way templates are consumed, and it works on AWS while failing against the emulator.

Fix: ListTemplates' summary projection — populate the identifier and ARN of each summary from the stored template state (ARN composed from the configured region/account, the request's domainId and the template id) and serialize tagPropagationConfigurations as [] rather than skipping the key, using the key names the recorded wire shows for this operation.

NOTE ON AN EXISTING ISSUE: the open issue '20261109T181134Z-connectcases-responses-serialize-timestamps-as-epoch-numbers-and.md' describes this same ListTemplates gap as its Cause 7, but its replay keys pin the summary entry by the per-run generated template name (sim2real-probe-cc-template-7d842d0e), so this run's cluster (name sim2real-probe-cc-template-ff722eba) did not match and the replay reported it as UNOWNED. It is filed here so the cause carries a replay key that is not tied to one run's generated name; if that issue is fixed first this one closes with it, and the fixer should verify with the fixture/step named below rather than with a name-bound key.

Traces: connectcases-template-lifecycle#task#6 (the only member of the cluster; affected_count=1).

## Evidence
Replay summary: trace_set_id=simparity-wire-aws-v1, emulator_up=true (/__admin/health 200), 182 steps, 117 passed, 56 mismatched, 9 blocked, score 64.3%, 22 divergence clusters (all connectcases).

Cluster (verdict=mismatch, expected_status=200, actual_status=200, stub_path empty, owned_by EMPTY, affected_count=1):
  fixture_id: connectcases-template-lifecycle
  step_key:   task#6
  operation:  ListTemplates
  divergence_key: service=connectcases|ListTemplates|status=200->200|keys=$.templates[name="sim2real-probe-cc-template-ff722eba"].arn,$.templates[name="sim2real-probe-cc-template-ff722eba"].id,$.templates[name="sim2real-probe-cc-template-ff722eba"].tagPropagationConfigurations
  detail:
    $.templates[name="sim2real-probe-cc-template-ff722eba"].arn: missing (expected an ARN of the form arn:aws:cases:<region>:<account-id>:domain/<domainId>/template/<templateId>; the recording's literal arn:aws:cases:us-west-2:000000000000:domain/7d34dd80-e576-4c0b-9906-b41e40064315/template/c002be0e-42a4-4129-8535-e6715cf91071 is a per-run value, not the requirement) — got None
    $.templates[name="sim2real-probe-cc-template-ff722eba"].id: missing (expected the created template's identifier; the recording's c002be0e-42a4-4129-8535-e6715cf91071 is per-run) — got None
    $.templates[name="sim2real-probe-cc-template-ff722eba"].tagPropagationConfigurations: missing (expected [], got None)

Expected vs actual, as a requirement: AWS -> each templates[] entry contains the template's identifier, its ARN (region/account/domainId/templateId substituted from the request and state) and tagPropagationConfigurations=[] alongside name and status. Emulator -> entry contains the name (it matched, which is how the entry was selected) and status but no identifier, no ARN and no tagPropagationConfigurations key. No status or error-code divergence: both sides are 200 with no error body.

Model citation: TemplateSummary.templateId required @length 1-500; TemplateSummary.templateArn required @length 1-500; TemplateSummary.tagPropagationConfigurations -> TagPropagationConfigurationList @length max 1; ListTemplatesResponse.templates required.

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 wire corpus against the emulator and read the cluster for fixture connectcases-template-lifecycle, step task#6 (operation ListTemplates).

Manually, inspecting the RAW JSON body rather than an SDK's parsed view:
2. CreateDomain(name=<unique>) and poll GetDomain until the domain is Active; note <domainId>.
3. CreateTemplate(domainId=<domainId>, name=<unique-template-name>, status='Active') -> note the returned template identifier <templateId>.
4. POST /domains/<domainId>/templates-list (ListTemplates) with no filters.
5. Locate the entry whose name equals <unique-template-name>. It MUST carry: the template identifier equal to <templateId>; the template ARN arn:aws:cases:<region>:<account-id>:domain/<domainId>/template/<templateId>; tagPropagationConfigurations present as []. Today all three keys are absent from the entry.
6. Confirm the consequence: take the identifier from the listing and call GetTemplate(domainId=<domainId>, templateId=<id from listing>). Against the emulator there is no id in the listing to pass, so the list-then-get flow cannot be completed at all.
