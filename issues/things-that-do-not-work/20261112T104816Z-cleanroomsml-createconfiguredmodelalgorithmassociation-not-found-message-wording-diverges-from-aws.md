Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cleanroomsml.create-association-nonexistent-algorithm-rejected
Issue-Shape-Id: com.amazonaws.cleanroomsml#AWSStarkControlService
Issue-Key: cleanroomsml.create-association-nonexistent-algorithm-rejected|run#21|AssertionError|assert message == f"Configured Model Algorithm: {missing_arn} not found.", f"unexpected message {message!r}"
Issue-Commit: 109b0d2c5ac5e0d1086daa2342a14a3f0aa32bfd

# CreateConfiguredModelAlgorithmAssociation not-found message wording diverges from AWS ResourceNotFoundException text

> _Note: this issue was identified against the code base at commit `109b0d2c5ac5e0d1086daa2342a14a3f0aa32bfd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConfiguredModelAlgorithmAssociation

## Description
The cleanroomsml emulator rejects CreateConfiguredModelAlgorithmAssociation against a configured-model-algorithm ARN that does not exist with the correct HTTP status and the correct error code, but words the ResourceNotFoundException `message` differently from AWS. This issue has ONE cause.

Cause 1 of 1 - CreateConfiguredModelAlgorithmAssociation ResourceNotFoundException message wording.
AWS: Smithy (services/service-com-amazonaws-cleanroomsml-awsstarkcontrolservice): CreateConfiguredModelAlgorithmAssociation is POST /memberships/{membershipIdentifier}/configured-model-algorithm-associations; `configuredModelAlgorithmArn` is @required (@length 20..2048, pattern ^arn:aws[-a-z]*:cleanrooms-ml:[-a-z0-9]+:[0-9]{12}:configured-model-algorithm/[-a-zA-Z0-9_/.]+$) and the operation models ResourceNotFoundException with httpError 404. The message TEXT is recorded behavior only: when the referenced algorithm does not exist AWS answers HTTP 404, error code ResourceNotFoundException, with a `message` of the fixed form "Configured Model Algorithm: <arn> not found." - the fixed text is "Configured Model Algorithm:", a space, then "not found."; the only variable part is the configured-model-algorithm ARN taken verbatim from the request's `configuredModelAlgorithmArn` member. There is no per-run value in the message.
Emulator: returns the right status and the right error code (the check's `code == "ResourceNotFoundException"` assertion passes) but words the message "Configured Model Algorithm <arn> does not exist." - it drops the colon after the resource-type name and substitutes "does not exist." for "not found.". Hypothesis: a single global not-found formatter serves every cleanroomsml resource type, and its one template cannot produce AWS's per-resource-type wording.
Why: the error message is a client-parsed part of the contract. Log scrapers, recorded-contract assertions and branching logic that match AWS's ResourceNotFoundException text for a missing configured model algorithm do not recognise the emulator's message, so a correctly-signalled missing reference reads as an unknown failure against the emulator while passing against AWS.
Fix: the ResourceNotFoundException construction in the cleanroomsml CreateConfiguredModelAlgorithmAssociation handler - or the per-resource-type wording table of the shared not-found message formatter it calls - must emit AWS's per-resource wording instead of one global template.
Traces: behavioral check cleanroomsml.create-association-nonexistent-algorithm-rejected.

Not covered elsewhere: open issue 20261112T092323Z-cleanroomsml-getconfiguredmodelalgorithm-not-found-message-wording-diverges-from-aws.md reports the identical wording defect, but only for GetConfiguredModelAlgorithm, and its replay key is cleanroomsml|GetConfiguredModelAlgorithm|4xx->4xx|$.message; CreateConfiguredModelAlgorithmAssociation is named by neither its cause nor its key. Open issue 20261109T153504Z-cleanroomsml-error-responses-diverge-from-aws-not-found.md bundles six cleanroomsml error causes, but its not-found causes are GetTrainingDataset, GetAudienceModel and DeleteTrainingDataset only, and they diverge in the OPPOSITE direction (emulator says "<Type>: <arn> not found." where AWS says "<Type> <arn> does not exist."). If the formatter is shared, fixing it must satisfy BOTH wordings per resource type.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, check cleanroomsml.create-association-nonexistent-algorithm-rejected ([mismatch]; service pass rate cleanroomsml 3/4).

The check creates an IAM role, a ConfiguredModelAlgorithm, a collaboration and a membership, then calls CreateConfiguredModelAlgorithmAssociation with the algorithm ARN's parent path plus a nonexistent id, and asserts on the refusal:

  assert code == "ResourceNotFoundException"            -> PASSES (status/code are correct)
  assert message == f"Configured Model Algorithm: {missing_arn} not found."  -> FAILS

  AssertionError: unexpected message 'Configured Model Algorithm arn:aws:cleanrooms-ml:us-west-2:000000000000:configured-model-algorithm/00000000000000000000000000000000 does not exist.'

Expected message fixed text: "Configured Model Algorithm:" ... "not found."; its only variable part is the configured-model-algorithm ARN echoed from the request.
Got message fixed text: "Configured Model Algorithm" (no colon) ... "does not exist.".

The ARN, region and account id above are the recorded run's values filling that template and are not themselves the requirement.

The remainder of the check (no association left behind by the rejected call; positive control association created, listed and fetched) was not reached because the assertion aborts the check at this point.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from that checkout.
2. Replay the behavioral suite simparity-behavioral-aws-v1 (run_behavioral_suite); check cleanroomsml.create-association-nonexistent-algorithm-rejected.
3. Create an IAM role assumable by cleanrooms-ml.amazonaws.com, then CreateConfiguredModelAlgorithm with that roleArn; create a cleanrooms collaboration and a membership.
4. Call CreateConfiguredModelAlgorithmAssociation(membershipIdentifier=<membership id>, configuredModelAlgorithmArn=<the algorithm ARN's path with a nonexistent id appended>, name=<any valid name>).
5. Observe HTTP 404 with error code ResourceNotFoundException (correct) but message 'Configured Model Algorithm <arn> does not exist.' where AWS returns 'Configured Model Algorithm: <arn> not found.'.
