Issue-Source: trace-sweep-divergence
Issue-Trace: config-conformance-pack-create-inspect-delete
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: config-conformance-pack-create-inspect-delete config-conformance-pack-basic-lifecycle config-conformance-pack-compliance-inspection config-conformance-pack-multi-pack-filtering
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config|DescribeConformancePacks|2xx->2xx|$.ConformancePackDetails[*]
Issue-Key: config|DescribeConformancePackCompliance|2xx->2xx|$.ConformancePackRuleComplianceList[*]
Issue-Key: config|DescribeConformancePacks|4xx->2xx|$
Issue-Key: config|DescribeConformancePacks|4xx->2xx|$.__type
Issue-Key: config|DescribeConformancePacks|4xx->2xx|$.message
Issue-Key: config|DescribeConformancePacks|4xx->2xx|status
Issue-Key: config|DescribeConformancePackStatus|2xx->2xx|$.ConformancePackStatusDetails[*]
Issue-Key: config|DescribeConformancePacks|2xx->2xx|$.ConformancePackDetails[*].Type
Issue-Key: config|DescribeConformancePackStatus|2xx->2xx|$.ConformancePackStatusDetails[*].StackArn
Issue-Key: config|DescribeConformancePackStatus|2xx->2xx|$.ConformancePackStatusDetails[*].Type
Issue-Commit: a6cd77094d82d955b2186a9b9b1faf688af70a8f

# Config conformance-pack describe responses omit Type and misreport deployment state

> _Note: this issue was identified against the code base at commit `a6cd77094d82d955b2186a9b9b1faf688af70a8f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeConformancePacks

## Description
Eight distinct causes in the conformance-pack read and lifecycle surface of this service: describe responses drop a member AWS always returns, the deployment state machine always succeeds where AWS fails the deployment, the CloudFormation stack ARN is not AWS-shaped, and a deleted pack is still served. EIGHT CAUSES must be closed.

Cause 1 of 8 - DescribeConformancePacks omits Type from ConformancePackDetail (fixture config-conformance-pack-create-inspect-delete, step task#2).
AWS: recorded behavior only - the on-disk model's ConformancePackDetail has no Type member (Smithy: shape ConformancePackDetail), but the recording shows the live service returning Type for every pack, with the value 'CUSTOM' for a pack created by PutConformancePack (as opposed to a service-linked/managed pack).
Emulator: returns ConformancePackName/Arn/Id/ConformancePackInputParameters/LastUpdateRequestedTime and no Type at all.
Why: a client that branches on pack provenance (custom vs service-linked) gets no value and must treat every pack as unknown; deserializers built from the current API shape see a required-in-practice member absent.
Fix: the ConformancePackDetail serializer plus the pack record in config state - persist the pack's type at PutConformancePack time and emit it as 'CUSTOM' for user-authored packs.

Cause 2 of 8 - DescribeConformancePackCompliance invents a rule-compliance row for a pack that produced no evaluations (fixture config-conformance-pack-basic-lifecycle, step task#2).
AWS: ConformancePackRuleComplianceList is required with @length min 0 (Smithy: DescribeConformancePackCompliance output), i.e. an empty list is the normal answer. The recording shows an empty ConformancePackRuleComplianceList immediately after the pack is put, because no rule evaluation has been produced yet.
Emulator: returns one row per template rule with ComplianceType 'INSUFFICIENT_DATA'.
Why: a client that polls this list until it is non-empty (the normal AWS idiom for "evaluations have landed") terminates immediately against the emulator on data AWS has not produced.
Fix: DescribeConformancePackCompliance's handler/state - derive rows from recorded rule evaluations rather than from the template's rule list.

Cause 3 of 8 - a deleted conformance pack is still returned, forever (fixture config-conformance-pack-basic-lifecycle, step teardown#7).
AWS: DescribeConformancePacks models NoSuchConformancePackException (client, 400) (Smithy: DescribeConformancePacks.errors). The recording shows that after DeleteConformancePack, describing the pack by name answers 400 NoSuchConformancePackException with message "No such conformance pack exists. Conformance Pack(s): [" + name + "]".
Emulator: answers 200 with the full ConformancePackDetails entry; the replay polled for 60s / 61 attempts and the pack never disappeared.
Why: delete is not observable - teardown and reconciliation loops that wait for the pack to vanish never finish against AWS semantics, and a pack believed deleted is still reported as deployed.
Fix: DeleteConformancePack's handler/state - remove (or mark DELETE_COMPLETE and then drop) the pack record so describe raises the modeled 400.

Cause 4 of 8 - DescribeConformancePackStatus reports CREATE_COMPLETE for a deployment AWS fails (fixture config-conformance-pack-create-inspect-delete, step task#3).
AWS: ConformancePackStatusDetail.ConformancePackState is a required enum and ConformancePackStatusReason carries "the reason of conformance pack creation failure" (Smithy: shape ConformancePackStatusDetail). The recording shows this deployment settling in CREATE_FAILED with a ConformancePackStatusReason present (and Type 'CUSTOM').
Emulator: reports CREATE_COMPLETE, no ConformancePackStatusReason, and no Type.
Why: the failure branch of the deployment state machine is unreachable, so clients can never exercise or assert CREATE_FAILED handling; worse, they conclude the pack's rules are deployed when AWS deployed nothing.
Fix: the conformance-pack deployment state machine (PutConformancePack -> status) - validate the template/delivery target and settle to CREATE_FAILED with a reason when deployment cannot succeed.

Cause 5 of 8 - the same Type omission on the multi-pack filtering read path (fixture config-conformance-pack-multi-pack-filtering, step task#4).
AWS: as cause 1 - each ConformancePackDetails entry carries Type 'CUSTOM'.
Emulator: Type missing on the filtered single-pack result (also at task#5).
Why/Fix: as cause 1; this binding exercises the name-filtered path of the serializer.

Cause 6 of 8 - the same Type omission when all packs are listed (fixture config-conformance-pack-multi-pack-filtering, step task#6).
AWS: as cause 1, for every entry of the unfiltered list.
Emulator: both entries are returned without Type, so the whole list diverges.
Why/Fix: as cause 1; this binding exercises the unfiltered multi-entry path.

Cause 7 of 8 - the conformance pack's StackArn is not AWS's CloudFormation stack ARN form (fixture config-conformance-pack-multi-pack-filtering, step task#7).
AWS: ConformancePackStatusDetail.StackArn is required and targets StackArn (Smithy: shape ConformancePackStatusDetail). The recording shows AWS's CloudFormation stack ARN form: arn:aws:cloudformation:<region>:<account>:stack/awsconfigconforms-<packName>-<packId>/<stack-uuid> - the stack NAME embeds both the pack name and the pack id, and the final ARN segment is a CloudFormation stack uuid.
Emulator: arn:aws:cloudformation:<region>:<account>:stack/awsconfigconforms-<packName>/<packId> - the pack id is used as the ARN's stack-id segment and dropped from the stack name. Type is missing on the same entry (also at task#8).
Why: a client that derives the backing CloudFormation stack name from this ARN (to call CloudFormation) builds a name AWS does not have, and ARN parsers that expect a stack uuid read a pack id instead.
Fix: the ConformancePackStatusDetail serializer / the stack identity minted by the deployment path.

Cause 8 of 8 - the same StackArn form and Type omission on the all-packs status read (fixture config-conformance-pack-multi-pack-filtering, step task#9).
AWS: as cause 7, for every entry of the unfiltered ConformancePackStatusDetails list.
Emulator: both entries carry the non-AWS StackArn form and no Type.
Why/Fix: as cause 7; this binding exercises the multi-entry status path.

No existing open issue covers the config conformance-pack surface (the only open config wire issue, 20261022T170950Z-config-error-paths-unenforced-delivery-channel-validation-and.md, is about delivery channels).

Traces: config-conformance-pack-create-inspect-delete#task#2 and #task#3, config-conformance-pack-basic-lifecycle#task#2 and #teardown#7, config-conformance-pack-multi-pack-filtering#task#4, #task#6, #task#7 and #task#9; the compliance row also fails in config-conformance-pack-compliance-inspection#task#2 and the surviving-pack read in config-conformance-pack-create-inspect-delete#teardown#5.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from mainline (emulator_up=true, bootstrap_signal=false).

Cause 1 - DescribeConformancePacks, config-conformance-pack-create-inspect-delete#task#2 (200 on both sides):
  $.ConformancePackDetails[0]: expected entry includes 'Type': 'CUSTOM'; got entry with ConformancePackName/ConformancePackArn/ConformancePackId/ConformancePackInputParameters/LastUpdateRequestedTime only.

Cause 2 - DescribeConformancePackCompliance, config-conformance-pack-basic-lifecycle#task#2 (200 on both sides, 2 steps in cluster):
  $.ConformancePackRuleComplianceList[0]: extra (expected None, got {'ConfigRuleName': '<rule>', 'ComplianceType': 'INSUFFICIENT_DATA'}).

Cause 3 - DescribeConformancePacks, config-conformance-pack-basic-lifecycle#teardown#7 (2 steps in cluster):
  status: expected 400, got 200; poll capped at 60s, error code None not in ['NoSuchConformancePackException'] after 61 attempts.
  $.__type: missing (expected 'NoSuchConformancePackException'); $.message: missing (expected 'No such conformance pack exists. Conformance Pack(s): [<packName>]').

Cause 4 - DescribeConformancePackStatus, config-conformance-pack-create-inspect-delete#task#3 (200 on both sides):
  expected ConformancePackState 'CREATE_FAILED' with ConformancePackStatusReason present and Type 'CUSTOM'; got ConformancePackState 'CREATE_COMPLETE', no ConformancePackStatusReason, no Type.

Cause 5 - DescribeConformancePacks, config-conformance-pack-multi-pack-filtering#task#4 (2 steps in cluster):
  $.ConformancePackDetails[0].Type: missing (expected 'CUSTOM', got None).

Cause 6 - DescribeConformancePacks, config-conformance-pack-multi-pack-filtering#task#6:
  $.ConformancePackDetails[0] and [1] diverge; the only difference in each entry is the absent Type ('CUSTOM').

Cause 7 - DescribeConformancePackStatus, config-conformance-pack-multi-pack-filtering#task#7 (2 steps in cluster):
  $.ConformancePackStatusDetails[0].StackArn: expected 'arn:aws:cloudformation:us-west-2:000000000000:stack/awsconfigconforms-<packName>-<packId>/<stack-uuid>', got 'arn:aws:cloudformation:us-west-2:000000000000:stack/awsconfigconforms-<packName>/<packId>'.
  $.ConformancePackStatusDetails[0].Type: missing (expected 'CUSTOM').

Cause 8 - DescribeConformancePackStatus, config-conformance-pack-multi-pack-filtering#task#9:
  $.ConformancePackStatusDetails[0] and [1] diverge on the same StackArn form and the absent Type.

(Pack names, pack ids, stack uuids and timestamps above are per-run values; the requirements are the member set, the state transition and the ARN form.)

Model grounding (services/service-com-amazonaws-configservice-starlingdoveservice/smithy-model/com-amazonaws-configservice-starlingdoveservice.json): DescribeConformancePacks errors include NoSuchConformancePackException (400); ConformancePackStatusDetail requires ConformancePackState and StackArn and defines ConformancePackStatusReason; DescribeConformancePackCompliance output requires ConformancePackRuleComplianceList (min 0).

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from mainline.
2. Replay the wire corpus simparity-wire-aws-v1.
3. Cause 1: config-conformance-pack-create-inspect-delete#task#2 - DescribeConformancePacks after PutConformancePack; Type is absent.
4. Cause 2: config-conformance-pack-basic-lifecycle#task#2 - DescribeConformancePackCompliance right after the put; a fabricated INSUFFICIENT_DATA row appears.
5. Cause 3: config-conformance-pack-basic-lifecycle#teardown#7 - DeleteConformancePack then DescribeConformancePacks by name; the pack is still returned 200.
6. Cause 4: config-conformance-pack-create-inspect-delete#task#3 - DescribeConformancePackStatus; state is CREATE_COMPLETE instead of CREATE_FAILED with a reason.
7. Causes 5 and 6: config-conformance-pack-multi-pack-filtering#task#4 and #task#6 - filtered and unfiltered DescribeConformancePacks; Type absent.
8. Causes 7 and 8: config-conformance-pack-multi-pack-filtering#task#7 and #task#9 - DescribeConformancePackStatus; StackArn form wrong and Type absent.
9. Re-replay all eight fixture#step bindings to confirm the fix.
