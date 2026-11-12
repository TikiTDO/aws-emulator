Issue-Source: trace-sweep-divergence
Issue-Trace: cleanroomsml-get-operations-not-found
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cleanroomsml-get-operations-not-found
Issue-Shape-Id: com.amazonaws.cleanroomsml#AWSStarkControlService
Issue-Key: cleanroomsml|GetConfiguredModelAlgorithm|4xx->4xx|$.message
Issue-Commit: 7d841236d87a4a0bdd7ae60e4f2e3cff95974158

# GetConfiguredModelAlgorithm not-found message wording diverges from AWS ResourceNotFoundException text

> _Note: this issue was identified against the code base at commit `7d841236d87a4a0bdd7ae60e4f2e3cff95974158`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetConfiguredModelAlgorithm

## Description
The cleanroomsml emulator answers GetConfiguredModelAlgorithm for an absent resource with the correct HTTP status and error shape, but words the ResourceNotFoundException `message` differently from AWS. This issue has ONE cause.

Cause 1 of 1 - GetConfiguredModelAlgorithm ResourceNotFoundException message wording.
AWS: Smithy (services/service-com-amazonaws-cleanroomsml-awsstarkcontrolservice): GetConfiguredModelAlgorithm is GET /configured-model-algorithms/{configuredModelAlgorithmArn} (configuredModelAlgorithmArn is @required, @httpLabel, @length 20..2048, pattern ^arn:aws[-a-z]*:cleanrooms-ml:[-a-z0-9]+:[0-9]{12}:configured-model-algorithm/[-a-zA-Z0-9_/.]+$) and models ResourceNotFoundException with httpError 404. The message TEXT is recorded behavior only: AWS returns HTTP 404 with a `message` of the fixed form "Configured Model Algorithm: <arn> not found." - the fixed text is "Configured Model Algorithm:", a space, then "not found."; the only variable part is the configured-model-algorithm ARN, taken verbatim from the request's configuredModelAlgorithmArn httpLabel. There is no per-run value in the message.
Emulator: returns the correct 404 and the correct error shape, but the message reads "Configured Model Algorithm <arn> does not exist." - it drops the colon after the resource-type name and substitutes "does not exist." for "not found.". Hypothesis: a shared not-found message formatter is used for every cleanroomsml resource type, and this operation's AWS wording differs from the other resource types' ("<Type> <arn> does not exist."), so one template cannot serve both.
Why: the error message is a client-parsed part of the contract. Log scrapers, recorded-contract assertions and branching logic that match AWS's ResourceNotFoundException text for a configured model algorithm do not recognise the emulator's message, so a correctly-signalled missing resource reads as an unknown failure against the emulator while passing against AWS.
Fix: the ResourceNotFoundException construction in the cleanroomsml GetConfiguredModelAlgorithm handler (or the per-resource-type wording table in the shared not-found message formatter it calls) must emit AWS's per-resource wording rather than one global template.
Traces: cleanroomsml-get-operations-not-found#task#1 (the whole cluster; affected_count 1).

Not covered elsewhere: the open issue 20261109T153504Z-cleanroomsml-error-responses-diverge-from-aws-not-found.md bundles six cleanroomsml error-fidelity causes, but its not-found causes are GetTrainingDataset, GetAudienceModel and DeleteTrainingDataset only, and they diverge in the OPPOSITE direction (emulator emits "<Type>: <arn> not found." where AWS words "<Type> <arn> does not exist."). GetConfiguredModelAlgorithm is named by none of its causes and none of its replay keys, so this divergence is net-new.

## Evidence
Replay of trace set simparity-wire-aws-v1 (5 fixtures, 33 steps, 26 passed, score 78.8%), divergence key service=cleanroomsml|GetConfiguredModelAlgorithm|status=404->404|keys=$.message.

Fixture cleanroomsml-get-operations-not-found, step task#1, GetConfiguredModelAlgorithm, status expected 404 and got 404 (status and error shape agree; only the message text diverges):
  $.message expected 'Configured Model Algorithm: arn:aws:cleanrooms-ml:us-west-2:000000000000:configured-model-algorithm/00000000-0000-0000-0000-000000000000 not found.'
  $.message got      'Configured Model Algorithm arn:aws:cleanrooms-ml:us-west-2:000000000000:configured-model-algorithm/00000000-0000-0000-0000-000000000000 does not exist.'

Fixed text of the expected message: "Configured Model Algorithm:" ... "not found.". Variable part: the configured-model-algorithm ARN, echoed from the request path. The ARN, region and account id shown above are the recorded run's values filling that template and are not themselves the requirement.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from that checkout.
2. Replay the wire corpus simparity-wire-aws-v1 (replay_wire_corpus).
3. Fixture cleanroomsml-get-operations-not-found, step task#1: GET /configured-model-algorithms/{configuredModelAlgorithmArn} with a well-formed configured-model-algorithm ARN that was never created.
4. Observe HTTP 404 with the correct ResourceNotFoundException shape, but body message 'Configured Model Algorithm <arn> does not exist.' where AWS returns 'Configured Model Algorithm: <arn> not found.'.
