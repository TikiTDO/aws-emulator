Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock.apply-guardrail-denied-topic-named-in-assessment bedrock.deploy-guardrail-via-cloudformation-enforces-denied-topic bedrock.apply-guardrail-pii-anonymize-masks-output
Issue-Key: bedrock.apply-guardrail-denied-topic-named-in-assessment|run#12|AssertionError|assert resp["action"] == "GUARDRAIL_INTERVENED", (
Issue-Key: bedrock.deploy-guardrail-via-cloudformation-enforces-denied-topic|run#18|AssertionError|assert control_resp["action"] == "GUARDRAIL_INTERVENED", control_resp
Issue-Key: bedrock.apply-guardrail-pii-anonymize-masks-output|run#12|AssertionError|assert resp.get("actionReason") == "Guardrail masked.", (
Issue-Commit: f0242a1930b25bb78bdea7a3bf1c4ec4cb6cc2ae

# bedrock ApplyGuardrail: DENY topic policy is never evaluated (action=NONE, no topicPolicy assessment) and actionReason is never serialized

> _Note: this issue was identified against the code base at commit `f0242a1930b25bb78bdea7a3bf1c4ec4cb6cc2ae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ApplyGuardrail

## Description
Two independent root causes in the ApplyGuardrail evaluation/response path of the bedrock guardrail feature, both grounded in behavioral checks listed under the `bedrock` service heading. A fixer must close BOTH causes before submitting.

Neither cause is covered by an open issue that carries these checks' replay keys. Related but distinct open issues: 20261024T031932Z-bedrock-runtime-inline-guardrail-error-taxonomy-applyguardrail-and.md and 20261024T094806Z-bedrock-runtime-applyguardrail-answers-the-unmodeled-code-notimplemented.md concern ApplyGuardrail ERROR codes, not the 200 evaluation result; 20261029T171601Z-bedrock-converse-serves-a-constant-notice-string-inline.md concerns Converse/Invoke inline guardrails, not ApplyGuardrail; 20261111T062636Z-bedrock-runtime-applyguardrail-response-omits-actionreason-output-blockedresponse.md describes the same actionReason omission from the wire corpus (see Cause 2).

Cause 1 of 2: a guardrail with a DENY topic policy accepts and round-trips the topic, but ApplyGuardrail never evaluates it — on-topic content is passed through as action=NONE with no topicPolicy assessment.
AWS: Smithy (com.amazonaws.bedrockruntime) ApplyGuardrail -> ApplyGuardrailResponse{usage @required (GuardrailUsage), action @required (GuardrailAction, enum NONE|GUARDRAIL_INTERVENED), actionReason String, outputs @required, assessments @required, guardrailCoverage}. GuardrailAssessment.topicPolicy -> GuardrailTopicPolicyAssessment{topics @required} of GuardrailTopic{name @required, type @required (GuardrailTopicType, enum DENY), action @required (GuardrailTopicPolicyAction, enum BLOCKED|NONE), detected Boolean}. For a guardrail created with topicPolicyConfig.topicsConfig = [{name, definition, examples, type: DENY}], content that is on that denied topic makes AWS answer HTTP 200 with action='GUARDRAIL_INTERVENED' and assessments[0].topicPolicy.topics containing an entry whose name is the CONFIGURED topic name, type='DENY', action='BLOCKED', detected=true; for source='INPUT' the intervened response's outputs carry the guardrail's configured blockedInputMessaging (recorded behavior only — `outputs`/`blockedResponse` content is derived from the guardrail configuration, never from any single recording). Off-topic content on the same guardrail version stays action='NONE'. The behaviour is identical whether the guardrail was created through the Bedrock control plane or deployed as an AWS::Bedrock::Guardrail CloudFormation resource, and identical for 'DRAFT' and a published version.
Emulator: HTTP 200 with action='NONE', outputs=[], and assessments[0] carrying ONLY invocationMetrics and appliedGuardrailDetails — the topicPolicy member is absent entirely — for content squarely on the configured DENY topic. usage.topicPolicyUnits=1 and assessments[0].invocationMetrics.usage.topicPolicyUnits=1 are nevertheless reported, i.e. the response bills a topic-policy evaluation that did not happen. Observed for a published version (guardrailVersion='1') and for guardrailVersion='DRAFT', and for both a Bedrock-API-created guardrail and a CloudFormation-deployed one, so it is the evaluator and not the resource-creation path.
Why: the topic policy fails OPEN. A client that routes prompts through ApplyGuardrail as its policy gate forwards exactly the content the guardrail exists to refuse, and a client that inspects assessments[0].topicPolicy.topics to log/report which topic fired gets a KeyError on a missing member instead of the BLOCKED entry. Because action and the assessment agree with each other, there is no signal at all that enforcement is absent.
Fix: the ApplyGuardrail handler's policy-evaluation stage — the topic-policy matcher must run against the guardrail's stored topicsConfig and drive both the GuardrailAction and the GuardrailTopicPolicyAssessment in the response serializer (and the units it already bills).
Traces: bedrock.apply-guardrail-denied-topic-named-in-assessment; bedrock.deploy-guardrail-via-cloudformation-enforces-denied-topic (whose positive control, a guardrail created directly through the Bedrock API, fails the same way, so the divergence is not CloudFormation-specific).

Cause 2 of 2: ApplyGuardrail omits actionReason on every verdict, so a masking intervention is indistinguishable from a blocking one.
AWS: Smithy models ApplyGuardrailResponse.actionReason as an optional String ('The reason for the action taken when harmful content is detected.'). Recorded behavior: AWS populates it with fixed text that is the whole value and carries no request- or state-derived part — 'Guardrail masked.' when the intervention anonymised/masked content, and 'No action.' when the evaluation did not intervene. Since both a masking and a blocking verdict carry the same action='GUARDRAIL_INTERVENED', actionReason is the only member that tells the caller whether its text was rewritten or refused.
Emulator: on a source='OUTPUT' evaluation against a guardrail whose sensitiveInformationPolicyConfig.piiEntitiesConfig is [{type: EMAIL, action: ANONYMIZE}], the emulator does return action='GUARDRAIL_INTERVENED' but $.actionReason is absent (botocore surfaces None) where AWS returns 'Guardrail masked.'. The check stops at that assertion, so the remaining PII expectations on the same response (piiEntities entry type=EMAIL/action=ANONYMIZED/match/detected, the masked `outputs` text, and usage.sensitiveInformationPolicyUnits being 1 on OUTPUT and 0 on INPUT) were never exercised and are NOT claimed here.
Why: a caller cannot branch on 'masked' vs 'blocked' vs 'no action' without re-deriving the verdict from the raw assessment, and a typed SDK yields null on a member AWS always sends.
Fix: the ApplyGuardrail handler's response builder — set actionReason from the verdict the evaluator reached, on the intervened (masked/blocked) and the no-action paths alike.
Traces: bedrock.apply-guardrail-pii-anonymize-masks-output.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replay: 79/153 passed. The three checks below are listed under '### bedrock' and are marked neither 'ALREADY FILED' nor 'OWNED by'.

Cause 1 — [mismatch] bedrock.apply-guardrail-denied-topic-named-in-assessment:
  AssertionError: on-topic input must be blocked by the DENY topic; action='NONE' assessments=[{'invocationMetrics': {'guardrailProcessingLatency': 2, 'usage': {'topicPolicyUnits': 1, 'contentPolicyUnits': 0, 'wordPolicyUnits': 0, 'sensitiveInformationPolicyUnits': 0, 'sensitiveInformationPolicyFreeUnits': 0, 'contextualGroundingPolicyUnits': 0}, 'guardrailCoverage': {...}}, 'appliedGuardrailDetails': {...'guardrailVersion': '1', 'guardrailOrigin': ['REQUEST'], 'guardrailOwnership': 'SELF'}}]
  Expected: action='GUARDRAIL_INTERVENED' and assessments[0].topicPolicy.topics containing {name: <configured topic name>, type: 'DENY', action: 'BLOCKED', detected: true}. Got: action='NONE', no topicPolicy member, outputs=[].

Cause 1 — [mismatch] bedrock.deploy-guardrail-via-cloudformation-enforces-denied-topic:
  AssertionError on the POSITIVE CONTROL (guardrail created directly via bedrock.create_guardrail, topic 'MedicalDosageAdvice', type DENY; ApplyGuardrail guardrailVersion='DRAFT', source='INPUT', 74 characters of on-topic content):
  HTTPStatusCode 200, 'action': 'NONE', 'outputs': [], 'assessments': [{'invocationMetrics': {'usage': {'topicPolicyUnits': 1, ...}, 'guardrailCoverage': {'textCharacters': {'guarded': 74, 'total': 74}}}, 'appliedGuardrailDetails': {'guardrailVersion': 'DRAFT', ...}}] — again no topicPolicy assessment.

Cause 2 — [mismatch] bedrock.apply-guardrail-pii-anonymize-masks-output:
  AssertionError: a masking intervention must be distinguishable from a blocking one by actionReason; got None
  Expected: $.actionReason == 'Guardrail masked.' (fixed text, whole value). Got: member absent. The preceding assertion action=='GUARDRAIL_INTERVENED' PASSED, so the masking verdict itself was reached.

Model citations used: com.amazonaws.bedrockruntime#ApplyGuardrailResponse{usage @required, action @required, actionReason String, outputs @required, assessments @required, guardrailCoverage}; #GuardrailAction = [NONE, GUARDRAIL_INTERVENED]; #GuardrailTopicPolicyAssessment{topics @required}; #GuardrailTopic{name @required, type @required, action @required, detected}; #GuardrailTopicType = [DENY]; #GuardrailTopicPolicyAction = [BLOCKED, NONE]; #GuardrailUsage{topicPolicyUnits @required, ...}.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Cause 1: bedrock.create_guardrail(name=..., blockedInputMessaging=..., blockedOutputsMessaging=..., topicPolicyConfig={'topicsConfig': [{'name': <TOPIC>, 'definition': <definition>, 'examples': [<on-topic example>], 'type': 'DENY'}]}); poll get_guardrail until status=='READY'; optionally create_guardrail_version and wait for that version to be READY.
3. bedrock-runtime.apply_guardrail(guardrailIdentifier=<id>, guardrailVersion='DRAFT' or the published version, source='INPUT', content=[{'text': {'text': <text on the denied topic>}}]).
4. Observe HTTP 200 with action='NONE', outputs=[] and assessments[0] without a topicPolicy member, while usage.topicPolicyUnits==1. AWS returns action='GUARDRAIL_INTERVENED' with topicPolicy.topics naming the configured topic (type DENY, action BLOCKED, detected true). Repeating the same shape through an AWS::Bedrock::Guardrail CloudFormation stack reproduces it identically (check bedrock.deploy-guardrail-via-cloudformation-enforces-denied-topic).
5. Cause 2: bedrock.create_guardrail(..., sensitiveInformationPolicyConfig={'piiEntitiesConfig': [{'type': 'EMAIL', 'action': 'ANONYMIZE'}]}); wait READY; create_guardrail_version and wait READY.
6. bedrock-runtime.apply_guardrail(guardrailIdentifier=<id>, guardrailVersion=<version>, source='OUTPUT', content=[{'text': {'text': 'Please email me at <an email address> tomorrow.'}}]).
7. Observe action=='GUARDRAIL_INTERVENED' but no actionReason member in the 200 body; AWS sends actionReason='Guardrail masked.' (and 'No action.' for a non-intervening evaluation).
