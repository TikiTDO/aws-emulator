Issue-Source: trace-sweep-divergence
Issue-Trace: config-conformance-pack-create-inspect-delete
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: config-conformance-pack-create-inspect-delete
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config|DescribeConformancePackStatus|2xx->2xx|$.ConformancePackStatusDetails[*]
Issue-Commit: 32b3cf39b49b34d906693dfa43539d42886b562b

# config: DescribeConformancePackStatus always reports CREATE_COMPLETE, never the CREATE_FAILED terminal state

> _Note: this issue was identified against the code base at commit `32b3cf39b49b34d906693dfa43539d42886b562b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeConformancePackStatus

## Description
One net-new cause in the AWS Config emulator's conformance-pack lifecycle, surfaced by the wire corpus. The pack deployment path has no failure state at all: every PutConformancePack is settled optimistically as a successful CloudFormation deployment, so DescribeConformancePackStatus can only ever report CREATE_COMPLETE.

Cause 1 of 1: DescribeConformancePackStatus reports ConformancePackState=CREATE_COMPLETE (with a StackArn and no ConformancePackStatusReason) for a pack whose deployment did not succeed, where AWS reports the CREATE_FAILED terminal state (with a ConformancePackStatusReason and no StackArn).
AWS: Smithy (com-amazonaws-configservice-starlingdoveservice) models DescribeConformancePackStatus -> ConformancePackStatusDetailsList of ConformancePackStatusDetail{Type, ConformancePackName (required), ConformancePackId (required), ConformancePackArn (required), ConformancePackState (required, enum ConformancePackState), StackArn (required), ConformancePackStatusReason (optional, @length 0..2000), LastUpdateRequestedTime (required), LastUpdateCompletedTime}. The modeled documentation of ConformancePackStatusDetail$ConformancePackState states that Config sets the state to CREATE_IN_PROGRESS "when a conformance pack creation is in progress for an account", CREATE_COMPLETE "when a conformance pack has been successfully created in your account", CREATE_FAILED "when a conformance pack creation failed in your account", DELETE_IN_PROGRESS and DELETE_FAILED; ConformancePackStatusDetail$ConformancePackStatusReason is modeled as "The reason of conformance pack creation failure". So the state is derived from the real outcome of the underlying CloudFormation deployment, and a pack whose deployment fails is reported as CREATE_FAILED and carries the failure reason. The recorded response for this fixture's pack shows exactly that terminal detail: Type=CUSTOM, ConformancePackName/ConformancePackId/ConformancePackArn as minted, ConformancePackState=CREATE_FAILED, a non-empty ConformancePackStatusReason (asserted as shaped, not by value), LastUpdateRequestedTime and LastUpdateCompletedTime shaped, and NO StackArn member at all — AWS omits StackArn for this failed pack even though the member is modeled required. The pack name, id, ARN, reason text and timestamps are per-run values, not requirements; the requirement is the state value, the presence of the failure reason, and the omission of StackArn in the failed state.
Emulator: the same DescribeConformancePackStatus call answers HTTP 200 (status matches) but with ConformancePackStatusDetails[0].ConformancePackState=CREATE_COMPLETE, a synthesised StackArn (arn:aws:cloudformation:<region>:<account>:stack/awsconfigconforms-<packName>-<packId>/<uuid>), no ConformancePackStatusReason member, and LastUpdateCompletedTime set 0.5 s after LastUpdateRequestedTime. The whole detail is fabricated from the PutConformancePack request: there is no code path that can produce CREATE_FAILED (or DELETE_FAILED), so the deployment outcome is never consulted.
Why: a client that polls DescribeConformancePackStatus for a terminal state — which is the documented way to learn whether a pack deployed — is told a pack that failed to deploy is live and complete. Error handling keyed on CREATE_FAILED is never reached, the ConformancePackStatusReason that explains the failure is never delivered, and automation concludes its compliance coverage is in place when no rules were deployed. The fabricated StackArn compounds this: it points at a CloudFormation stack that does not correspond to a successful deployment, so a client that follows it to CloudFormation gets a second, inconsistent answer.
Fix: the conformance-pack state model and the DescribeConformancePackStatus projection in the config service — PutConformancePack must record the actual outcome of materialising the pack's template (the same deployment path that creates the pack's member AWS::Config::ConfigRule resources) and move the pack to CREATE_FAILED with a ConformancePackStatusReason when that materialisation does not succeed, instead of unconditionally stamping CREATE_COMPLETE; the serializer must then omit StackArn for a pack that has no successfully created stack and emit ConformancePackStatusReason when one is recorded. Open issue 20261107T193246Z-config-4-net-new-causes-custom-lambda-and.md (Cause 2) describes pack member rules never being evaluated after the pack reaches CREATE_COMPLETE, and 20261029T184030Z-config-compliance-and-aggregation-never-converge-conformance-pack.md describes pack compliance non-convergence; neither carries this fixture step's replay key and neither covers the lifecycle state itself, so this divergence must be replayed to confirm any fix.
Traces: config-conformance-pack-create-inspect-delete#task#3.

## Evidence
From the wire replay of trace set simparity-wire-aws-v1 (score 94.0%, 17 mismatched steps). Divergence cluster service=config|DescribeConformancePackStatus|status=200->200|keys=$.ConformancePackStatusDetails[0], verdict=mismatch, affected_count=1.

Fixture config-conformance-pack-create-inspect-delete, step task#3, operation DescribeConformancePackStatus.
Expected status 200, actual status 200 — the divergence is entirely in the body at $.ConformancePackStatusDetails[0].

Expected (recorded AWS response):
  {"Type": "CUSTOM",
   "ConformancePackName": "<packName from the request>",
   "ConformancePackId": "<minted pack id>",
   "ConformancePackArn": "arn:aws:config:<region>:<account>:conformance-pack/<packName>/<packId>",
   "ConformancePackState": "CREATE_FAILED",
   "ConformancePackStatusReason": <present, shaped-not-asserted>,
   "LastUpdateRequestedTime": <shaped-not-asserted>,
   "LastUpdateCompletedTime": <shaped-not-asserted>}
  — note there is NO StackArn member.

Actual (emulator response):
  {"Type": "CUSTOM",
   "ConformancePackName": "<packName>",
   "ConformancePackId": "<packId>",
   "ConformancePackArn": "arn:aws:config:<region>:<account>:conformance-pack/<packName>/<packId>",
   "ConformancePackState": "CREATE_COMPLETE",
   "StackArn": "arn:aws:cloudformation:<region>:<account>:stack/awsconfigconforms-<packName>-<packId>/<uuid>",
   "LastUpdateRequestedTime": <epoch seconds>,
   "LastUpdateCompletedTime": <LastUpdateRequestedTime + 0.5>}
  — no ConformancePackStatusReason member.

Requirement vs per-run values: the requirement is ConformancePackState == CREATE_FAILED for a pack whose deployment failed, a present ConformancePackStatusReason, and StackArn omitted in that state. The concrete pack name, pack id, ARNs, the reason's text, the stack uuid and both timestamps are per-run values and are not part of the requirement (the corpus asserts them as shaped only).

Model citation: Smithy ConformancePackState enum values are CREATE_IN_PROGRESS | CREATE_COMPLETE | CREATE_FAILED | DELETE_IN_PROGRESS | DELETE_FAILED; ConformancePackStatusDetail$ConformancePackState doc maps CREATE_FAILED to "a conformance pack creation failed in your account"; ConformancePackStatusDetail$ConformancePackStatusReason doc is "The reason of conformance pack creation failure."

## Steps to Reproduce
1. Replay fixture config-conformance-pack-create-inspect-delete against the emulator and inspect step task#3.
2. Equivalently, by hand: call PutConformancePack(ConformancePackName=<name>, TemplateBody=<the fixture's pack template>) in a region/account where the pack's deployment cannot succeed (as in the recording), then poll DescribeConformancePackStatus(ConformancePackNames=[<name>]).
3. Observe the emulator answers 200 with ConformancePackStatusDetails[0].ConformancePackState = "CREATE_COMPLETE", a synthesised StackArn and no ConformancePackStatusReason, and that it settles there permanently.
4. AWS instead settles the same pack at ConformancePackState = "CREATE_FAILED" with a ConformancePackStatusReason explaining the failure and no StackArn member.
5. There is no input to the emulator that yields CREATE_FAILED (or DELETE_FAILED), which is the underlying defect: the pack lifecycle has no failure state.
