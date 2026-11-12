Issue-Source: trace-sweep-divergence
Issue-Trace: connectcases-template-lifecycle
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: connectcases-template-lifecycle
Issue-Shape-Id: com.amazonaws.connectcases#AmazonConnectCases
Issue-Key: connectcases|ListTemplates|2xx->2xx|$.templates[name="sim2real-probe-cc-template-4280d530"].arn
Issue-Key: connectcases|ListTemplates|2xx->2xx|$.templates[name="sim2real-probe-cc-template-4280d530"].id
Issue-Key: connectcases|ListTemplates|2xx->2xx|$.templates[name="sim2real-probe-cc-template-4280d530"].tagPropagationConfigurations
Issue-Commit: 681ba8e3ce79605e692c1fa7f993944edfaadaa9

# connectcases TemplateSummary projection never emits identity members for ListTemplates

> _Note: this issue was identified against the code base at commit `681ba8e3ce79605e692c1fa7f993944edfaadaa9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTemplates

## Description
ONE cause.

Cause 1 of 1: the TemplateSummary elements ListTemplates serializes carry no template identity (no id, no arn) and omit tagPropagationConfigurations, so a template the emulator lists cannot be resolved back to a template id or ARN. Fixture connectcases-template-lifecycle, step task#6.

AWS: Smithy (model services/service-com-amazonaws-connectcases-amazonconnectcases/smithy-model/com-amazonaws-connectcases-amazonconnectcases.json): ListTemplates = POST /domains/{domainId}/templates-list, 200, output ListTemplatesResponse.templates -> TemplateSummaryList (@required, @length max 100) + nextToken. com.amazonaws.connectcases#TemplateSummary declares templateId (@required, @length 1..500, 'The unique identifier for the template'), templateArn (@required, @length 1..500, 'The Amazon Resource Name (ARN) of the template'), name (@required, @length 1..100, pattern ^.*[\S]$), status (@required) and tagPropagationConfigurations -> TagPropagationConfigurationList (@length max 1). So every listed summary is self-describing: identity is a required member, not an optional one. For the wire KEY NAMES `id` and `arn` the model says nothing — recorded behavior only: the recorded 200 body carries each summary's identifier under `id` and its full ARN under `arn`, the ARN having the general form arn:aws:cases:<region>:<account-id>:domain/<domainId>/template/<templateId>, where region, account id, domain id and template id all come from the request and the service's own state. The recording also shows tagPropagationConfigurations serialized as an empty list [] for a template created with no propagation configuration — AWS emits the empty list rather than omitting the member. No error is involved: the recorded status is 200 and the emulator's status also 200.

Emulator: on the same 200 response all three members are absent from the summary element. Replay reports, for the element selected by its name, `.arn: missing (... got None)`, `.id: missing (... got None)` and `.tagPropagationConfigurations: missing (expected [], got None)`, with no `extra` keys — so the emulator is not emitting the identity under the alternative modeled spellings templateId/templateArn either; it simply never projects identity into the summary. Everything else in the element, and the status, match. There is no status, error-code or error-message divergence to quote for this cause; the defect is purely the 200 body's projection.

Why: a client that lists templates and then uses one is broken outright. There is no identifier to feed to GetTemplate/UpdateTemplate/DeleteTemplate or to CreateCase(templateId=...), and no ARN for tagging, IAM or resource-reference flows, so list-then-use — the primary way templates are consumed — cannot be completed against the emulator even though it works on AWS. Because templateId/templateArn are @required, a strict typed SDK deserializer can reject the entire page rather than just lose two fields. tagPropagationConfigurations arriving as null instead of [] forces callers that iterate it to special-case an absent member.

Fix: the ListTemplates response serializer / TemplateSummary projection in the connectcases service crate — the code that maps a stored template into a summary element. Populate each element's identifier and ARN from the stored template state (ARN composed from the configured region and account, the request's domainId and the template's id), using the key names the recorded wire shows for this operation, and serialize tagPropagationConfigurations as [] when the template has no propagation configuration instead of skipping the key. GetTemplate reads the same stored template and must stay consistent with it.

NOTE ON EXISTING ISSUES: four open issues describe this same ListTemplates gap — 20261109T223410Z-connectcases-listtemplates-summaries-omit-the-template-id-arn.md, 20261110T113333Z-connectcases-listtemplates-summary-projection-drops-id-arn-and.md, 20261110T194709Z-connectcases-listtemplates-element-lacks-id-arn-and-empty.md and 20261111T051856Z-connectcases-listtemplates-summaries-omit-template-id-arn-and.md — and the open issue 20261109T181134Z-connectcases-responses-serialize-timestamps-as-epoch-numbers-and.md covers the same null-vs-empty hazard for other operations. None of them owns this cluster: their replay keys pin the summary entry by the per-run GENERATED template name, which differs on every recording, so this run's cluster came back with owned_by EMPTY and would otherwise go unexercised. Whichever of these is fixed first closes the rest; a fixer should verify against the fixture/step named here rather than against a name-bound key.

Traces: connectcases-template-lifecycle#task#6 (the only member of the cluster, affected_count=1).

## Evidence
Replay summary: trace_set_id=simparity-wire-aws-v1, emulator_up=true (/__admin/health answered 200, data listener accepts), steps_total=182, passed=117, mismatched=56, blocked=9, score_pct=64.3, 22 divergence clusters (all service=connectcases). 21 of the 22 clusters are reported owned_by an open issue; this is the only unowned one.

Cluster (verdict=mismatch, expected_status=200, actual_status=200, stub_path empty, owned_by EMPTY, affected_count=1):
  service:        connectcases
  fixture_id:     connectcases-template-lifecycle
  step_key:       task#6
  operation:      ListTemplates
  divergence_key: service=connectcases|ListTemplates|status=200->200|keys=$.templates[name="sim2real-probe-cc-template-4280d530"].arn,$.templates[name="sim2real-probe-cc-template-4280d530"].id,$.templates[name="sim2real-probe-cc-template-4280d530"].tagPropagationConfigurations
  detail (verbatim):
    $.templates[name="sim2real-probe-cc-template-4280d530"].arn: missing (expected 'arn:aws:cases:us-west-2:000000000000:domain/7b8eb872-803a-4e0e-97e3-9cbffa1945c3/template/dd681d73-6a90-452a-b627-d1c7cedd88cb', got None)
    $.templates[name="sim2real-probe-cc-template-4280d530"].id: missing (expected 'dd681d73-6a90-452a-b627-d1c7cedd88cb', got None)
    $.templates[name="sim2real-probe-cc-template-4280d530"].tagPropagationConfigurations: missing (expected [], got None)

The template name sim2real-probe-cc-template-4280d530, the domain id 7b8eb872-803a-4e0e-97e3-9cbffa1945c3, the template uuid dd681d73-6a90-452a-b627-d1c7cedd88cb and the literal ARN above are PER-RUN values of this one recording and are NOT the requirement.

Expected vs actual, stated as the requirement: AWS -> every element of `templates` carries the template's identifier, the template's ARN of the form arn:aws:cases:<region>:<account-id>:domain/<domainId>/template/<templateId> (all four substituted from the request and the emulator's own state), and tagPropagationConfigurations serialized as [] when the template has no propagation configuration, alongside name and status. Emulator -> the element carries name (which is how the entry was matched) and status, but no identifier under any key, no ARN under any key, and no tagPropagationConfigurations key at all. Status agrees on both sides (200 vs 200) and no error body is involved, so there is no diverging HTTP status, error code or error message for this cause.

Model citation: com.amazonaws.connectcases#TemplateSummary.templateId @required @length 1..500; TemplateSummary.templateArn @required @length 1..500; TemplateSummary.name @required @length 1..100 pattern ^.*[\S]$; TemplateSummary.status @required; TemplateSummary.tagPropagationConfigurations -> TagPropagationConfigurationList @length max 1; ListTemplatesResponse.templates @required @length max 100; http POST /domains/{domainId}/templates-list, code 200. The `id` and `arn` wire key names are recorded behavior only.

## Steps to Reproduce
1. git clone the repo, boot the emulator built from that checkout, and replay the simparity-wire-aws-v1 wire corpus.
2. Read the cluster for fixture connectcases-template-lifecycle, step task#6 (operation ListTemplates, service connectcases): verdict mismatch, 200 vs 200, three missing body keys.

Equivalent by hand, inspecting the RAW JSON body rather than an SDK's parsed view:
3. CreateDomain(name=<unique>) and poll GetDomain until the domain is Active; note <domainId>.
4. CreateTemplate(domainId=<domainId>, name=<unique-template-name>, status='Active') with no tagPropagationConfigurations; note the returned template identifier <templateId>.
5. POST /domains/<domainId>/templates-list (ListTemplates) with no filters.
6. Locate the element of `templates` whose name equals <unique-template-name>. It MUST carry: the template identifier equal to <templateId>; the template ARN arn:aws:cases:<region>:<account-id>:domain/<domainId>/template/<templateId>; and tagPropagationConfigurations present as []. Today all three keys are absent from the element.
7. Confirm the consequence: take the identifier from the listing and call GetTemplate(domainId=<domainId>, templateId=<id from listing>). Against the emulator there is no identifier in the listing to pass, so the list-then-get flow cannot be completed at all.
