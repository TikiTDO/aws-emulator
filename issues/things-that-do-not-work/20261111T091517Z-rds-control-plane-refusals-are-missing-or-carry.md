Issue-Source: trace-sweep-divergence
Issue-Trace: rds-cluster-parameter-group-mutation-envelopes
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-cluster-parameter-group-mutation-envelopes rds-identifier-validation-and-lowercasing rds-refarch-aur-create-cluster-production-settings-refusals
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|ModifyDBClusterParameterGroup|4xx->2xx|$.ErrorResponse
Issue-Key: rds|ModifyDBClusterParameterGroup|4xx->2xx|status
Issue-Key: rds|CreateGlobalCluster|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|CreateDBCluster|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 34a59b28db3b6549e829977bc135282939da6978

# rds: control-plane refusals are missing or carry emulator-authored message text (3 causes)

> _Note: this issue was identified against the code base at commit `34a59b28db3b6549e829977bc135282939da6978`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyDBClusterParameterGroup

## Description
Three net-new wire causes where an rds request AWS refuses is either accepted or refused with the emulator's own wording. Each is a separate cluster in the replay and must be closed on its own.

Cause 1 of 3: ModifyDBClusterParameterGroup accepts a parameter name the group's engine family does not define.
AWS: a Parameters entry whose ParameterName is not a parameter of the group's DB parameter group family is refused with HTTP 400 and the awsQuery error envelope ErrorResponse/Error with Type 'Sender' and Code 'InvalidParameterValue'; the message's fixed text is 'Could not find parameter with name: ' followed by the ParameterName taken verbatim from the request (the recording shows it for the unknown name the fixture sends). Smithy models only DBParameterGroupNotFoundFault and InvalidDBParameterGroupStateFault for this operation, so the unknown-parameter refusal is the service-common InvalidParameterValue — recorded behavior for the code and status.
Emulator: the request succeeds with HTTP 200 and a normal DBClusterParameterGroupNameMessage; no ErrorResponse is produced and the unknown name is silently accepted into the group.
Why: a client that mis-spells a parameter, or targets a parameter from the wrong engine family, is told the modification landed. The bogus parameter is then reported back by subsequent reads, and the real misconfiguration is only discovered when the cluster does not behave as configured. This cause is NOT covered by the open issue '20261109T205028Z-rds-invalid-input-and-not-found-refusals-are.md' (a different set of four requests); the replay reports this step as unowned.
Fix: the ModifyDBClusterParameterGroup handler's input validation — resolve each Parameters[].ParameterName against the group's family parameter catalog before mutating state, and raise InvalidParameterValue (400, Sender) naming the offending parameter.
Traces: rds-cluster-parameter-group-mutation-envelopes#task#5.

Cause 2 of 3: CreateGlobalCluster's refusal of an unusable GlobalClusterIdentifier is worded as a generic identifier-pattern complaint.
AWS: GlobalClusterIdentifier is constrained in the model (Smithy: CreateGlobalCluster.GlobalClusterIdentifier -> GlobalClusterIdentifier, @length min 1 max 255, @pattern ^[A-Za-z][0-9A-Za-z-:._]*$), but the identifier the fixture sends is refused by the service with HTTP 400 and a message whose fixed text is 'Invalid global cluster identifier: ' followed by the supplied identifier value, with no clause about letters, digits, hyphens or consecutive hyphens.
Emulator: the status and error class match, but the message is the emulator's generic pattern sentence 'The parameter GlobalClusterIdentifier is not a valid identifier. Identifiers must begin with a letter; must contain only ASCII letters, digits, and hyphens; and must not end with a hyphen or contain two consecutive hyphens.' — text AWS does not emit for this refusal, and it mis-states the cause (the identifier it rejects satisfies that character rule).
Why: a client or operator reading the message is told the identifier has illegal characters when the identifier is in fact unacceptable for a different reason, and any test or log scraper matching AWS's 'Invalid global cluster identifier: <id>' never matches.
Fix: the CreateGlobalCluster identifier validation — emit AWS's fixed 'Invalid global cluster identifier: ' text with the request's identifier interpolated, rather than the shared pattern-violation sentence.
Traces: rds-identifier-validation-and-lowercasing#task#8.

Cause 3 of 3: CreateDBCluster's refusal of an out-of-range Serverless v2 capacity is worded as a range complaint instead of AWS's engine-version refusal.
AWS: when ServerlessV2ScalingConfiguration.MaxCapacity exceeds the maximum ACU the cluster's engine version supports (Smithy: CreateDBCluster.ServerlessV2ScalingConfiguration -> MinCapacity/MaxCapacity, DoubleOptional), the request is refused with HTTP 400 and a message whose fixed text is 'Invalid scaling configuration. The capacity values greater than <max> ACUs are not supported by the current engine version <version>. Please refer to this doc https://docs.aws.amazon.com/AmazonRDS/latest/AuroraUserGuide/aurora-serverless-v2.requirements.html for instructions on fixing the issue.' — the variable parts are the supported ACU ceiling and the request's engine version; it does not quote the rejected value.
Emulator: the status and error class match, but the message is emulator-authored: "Serverless v2 capacity value <value> is not valid. The valid scaling range for this cluster is <min> to <max>. To change the cluster's scaling range, use the ModifyDBCluster call." — it interpolates the rejected value, omits the engine version and the documentation link, and points at ModifyDBCluster, which is not AWS's advice for a create-time refusal.
Why: the refusal no longer tells the client which engine version imposes the ceiling, so the actionable remediation (raise the engine version, or lower MaxCapacity) is lost, and message-matching clients and the related open issue '20261111T084329Z-rds-four-control-plane-validation-and-representation-divergences.md' (a different Serverless cause) do not cover this wording.
Fix: the CreateDBCluster ServerlessV2ScalingConfiguration validation — emit AWS's fixed invalid-scaling-configuration text with the engine-version ceiling and the request's engine version interpolated.
Traces: rds-refarch-aur-create-cluster-production-settings-refusals#task#2 (same cluster also covers task#4 and task#5 of that fixture).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout.

Cause 1 — rds-cluster-parameter-group-mutation-envelopes#task#5, ModifyDBClusterParameterGroup:
  status: expected 400, got 200
  $.ErrorResponse: missing (expected {'Error': {'Type': 'Sender', 'Code': 'InvalidParameterValue', 'Message': 'Could not find parameter with name: rdsenv_no_such_parameter'}, ...}, got None)
  ('rdsenv_no_such_parameter' is the ParameterName the request sends; the RequestId in the recording is a per-run value and is not a requirement.)

Cause 2 — rds-identifier-validation-and-lowercasing#task#8, CreateGlobalCluster, status 400 vs 400:
  $.ErrorResponse.Error.Message: value_mismatch
    expected: 'Invalid global cluster identifier:  <the GlobalClusterIdentifier sent by the request>'
    got:      'The parameter GlobalClusterIdentifier is not a valid identifier. Identifiers must begin with a letter; must contain only ASCII letters, digits, and hyphens; and must not end with a hyphen or contain two consecutive hyphens.'
  (Fixed text = 'Invalid global cluster identifier: '; variable part = the identifier from the request.)

Cause 3 — rds-refarch-aur-create-cluster-production-settings-refusals#task#2, CreateDBCluster, status 400 vs 400 (cluster of 3: task#2, task#4, task#5):
  $.ErrorResponse.Error.Message: value_mismatch
    expected: 'Invalid scaling configuration. The capacity values greater than 256.0 ACUs are not supported by the current engine version 17.9. Please refer to this doc https://docs.aws.amazon.com/AmazonRDS/latest/AuroraUserGuide/aurora-serverless-v2.requirements.html for instructions on fixing the issue.'
    got:      "Serverless v2 capacity value 300.0 is not valid. The valid scaling range for this cluster is 0.0 to 256.0. To change the cluster's scaling range, use the ModifyDBCluster call."
  (Fixed text is the two sentences plus the doc link; 256.0 and 17.9 are the ACU ceiling and the engine version for this request, not literals.)

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Cause 1: replay fixture rds-cluster-parameter-group-mutation-envelopes step task#5 — ModifyDBClusterParameterGroup with a Parameters entry naming a parameter the family does not define answers 200 instead of 400 InvalidParameterValue 'Could not find parameter with name: <name>'.
4. Cause 2: replay rds-identifier-validation-and-lowercasing step task#8 — CreateGlobalCluster with the fixture's unusable GlobalClusterIdentifier answers 400 with the generic identifier-pattern sentence instead of 'Invalid global cluster identifier: <id>'.
5. Cause 3: replay rds-refarch-aur-create-cluster-production-settings-refusals step task#2 (and task#4, task#5) — CreateDBCluster with a ServerlessV2ScalingConfiguration.MaxCapacity above the engine version's ACU ceiling answers 400 with the emulator's range sentence instead of AWS's invalid-scaling-configuration text naming the engine version and the doc link.
