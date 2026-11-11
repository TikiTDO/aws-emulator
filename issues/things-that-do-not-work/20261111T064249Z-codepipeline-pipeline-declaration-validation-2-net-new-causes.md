Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codepipeline.create-pipeline-dangling-input-artifact-rejected codepipeline.update-pipeline-invalid-action-declaration-rejected
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline.create-pipeline-dangling-input-artifact-rejected|run#12|AssertionError|assert message == (
Issue-Key: codepipeline.update-pipeline-invalid-action-declaration-rejected|run#12|AssertionError|assert message == (
Issue-Commit: 826b08334102dd43451716f9b2396fd89c307280

# CodePipeline pipeline-declaration validation: 2 net-new causes — InvalidActionDeclarationException wording for a dangling inputArtifacts reference (CreatePipeline) and for an output-artifact count over an action type's maximum (UpdatePipeline)

> _Note: this issue was identified against the code base at commit `826b08334102dd43451716f9b2396fd89c307280`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePipeline

## Description
Two net-new root causes, both in CodePipeline's pipeline-declaration structural validation. In both cases the emulator correctly classifies the refusal as InvalidActionDeclarationException (HTTP 400) but serves its own wording instead of AWS's message text for that error class, so the status and code are right and only the message diverges. Neither cause is covered by the open wording issue 20261029T183239Z-codepipeline-refusal-classification-and-wording-10-net-new.md (job lifecycle states, approval tokens, not-found texts, limits, stop, retry) nor by 20261103T082129Z-codepipeline-refusal-wording-3-net-new-causes-startpipelineexecution.md — those name different operations and error classes.

Cause 1 of 2: CreatePipeline — an action whose inputArtifacts names an artifact no preceding action outputs.
AWS: refuses with InvalidActionDeclarationException, HTTP 400 (Smithy: com.amazonaws.codepipeline#InvalidActionDeclarationException, errorKind client, httpError 400, doc "The action declaration was specified in an invalid format."; the message text is recorded behavior only — the model constrains only message @length 1..5000). The recorded message is "Input Artifact Bundle <artifactName> in action <actionName> is not declared as Output Artifact Bundle in any of the preceding actions". Fixed text: "Input Artifact Bundle ", " in action ", " is not declared as Output Artifact Bundle in any of the preceding actions". Variable parts: the artifact name taken from the offending action's inputArtifacts[].name, and the action's name — both from the request. AWS names no stage in this message.
Emulator: same code and status, but the message is emulator-worded: "action <actionName> in stage <stageName> declares the input artifact <artifactName>, which no earlier action outputs" (replay: "action DeployAction in stage Deploy declares the input artifact NoSuchArtifact, which no earlier action outputs"). It inverts the sentence, adds the stage name AWS does not mention, and uses "input artifact" instead of AWS's "Input Artifact Bundle"/"Output Artifact Bundle" vocabulary.
Why: a client that matches or surfaces the refusal message — CI tooling, template linters, operators reading a CreatePipeline failure — cannot recognise the dangling-artifact condition, and anything keying on AWS's "Input Artifact Bundle ... is not declared as Output Artifact Bundle" text falls through to a generic failure path even though the pipeline was rejected for exactly that reason.
Fix: the CreatePipeline handler's pipeline-declaration validation (the artifact-graph check that resolves each action's inputArtifacts against preceding actions' outputArtifacts), where the InvalidActionDeclarationException message is composed.
Traces: codepipeline.create-pipeline-dangling-input-artifact-rejected. The check's control path passes (a valid declaration creates, the rejected pipeline is absent from ListPipelines, GetPipeline answers PipelineNotFoundException), so only the message diverges.

Cause 2 of 2: UpdatePipeline — an action declaring more output artifacts than its action type permits (a Manual Approval action, maximum 0, declaring 1).
AWS: refuses with InvalidActionDeclarationException, HTTP 400 (same modeled shape as above; message text is recorded behavior only). The recorded message is "Action <actionName> declares <declaredCount> output artifacts which is more than the maximum count (<maximumCount>)". Fixed text: "Action ", " declares ", " output artifacts which is more than the maximum count (", ")". Variable parts: the action's name and the number of outputArtifacts it declares, both from the request, and the action type's permitted maximum from the action type's artifact bounds. AWS does not quote the action name and does not restate the actionTypeId.
Emulator: same code and status, but the message is emulator-worded: "ActionType (Category: '<category>', Provider: '<provider>', Owner: '<owner>', Version: '<version>') in action '<actionName>' allows at most <maximumCount> output artifact(s), but the action declares <declaredCount>" (replay: "ActionType (Category: 'Approval', Provider: 'Manual', Owner: 'AWS', Version: '1') in action 'ApprovalAction' allows at most 0 output artifact(s), but the action declares 1"). It leads with a rendered actionTypeId tuple AWS never includes, quotes the action name, and reorders declared vs maximum.
Why: a client validating a pipeline update against the documented per-action-type artifact bounds gets an unrecognisable message; tooling that parses the declared and maximum counts out of AWS's text to report which action to fix reads nothing, and the actionTypeId tuple the emulator prepends makes the message look like an action-type lookup failure rather than an artifact-count violation.
Fix: the UpdatePipeline handler's pipeline-declaration validation (the per-action input/output artifact count bounds check shared with CreatePipeline), where the InvalidActionDeclarationException message is composed.
Traces: codepipeline.update-pipeline-invalid-action-declaration-rejected. The rest of that check passes — the refusal is classified InvalidActionDeclarationException (not InvalidStructureException), the stored definition and version are left unchanged, and a later well-formed update still succeeds and bumps the version — so only the message diverges.

## Evidence
Cause 1 — codepipeline.create-pipeline-dangling-input-artifact-rejected (CreatePipeline, Deploy action with inputArtifacts [{name: NoSuchArtifact}] that no preceding action outputs):
  expected error code: InvalidActionDeclarationException (matched)
  expected message: "Input Artifact Bundle NoSuchArtifact in action DeployAction is not declared as Output Artifact Bundle in any of the preceding actions"
  actual message:   "action DeployAction in stage Deploy declares the input artifact NoSuchArtifact, which no earlier action outputs"
  (assertion: check_codepipeline_create_pipeline_dangling_input_artifact_rejected.py line 198)
  Here "NoSuchArtifact" and "DeployAction" are values from the request; the rest of the expected message is fixed text.

Cause 2 — codepipeline.update-pipeline-invalid-action-declaration-rejected (UpdatePipeline, Approval/AWS/Manual/1 action 'ApprovalAction' given outputArtifacts [{name: ApprovalOutput}]):
  expected error code: InvalidActionDeclarationException (matched — not InvalidStructureException)
  expected message: "Action ApprovalAction declares 1 output artifacts which is more than the maximum count (0)"
  actual message:   "ActionType (Category: 'Approval', Provider: 'Manual', Owner: 'AWS', Version: '1') in action 'ApprovalAction' allows at most 0 output artifact(s), but the action declares 1"
  (assertion: check_codepipeline_update_pipeline_invalid_action_declaration_rejected.py line 71)
  Here "ApprovalAction" and "1" come from the request and "0" is the action type's output-artifact maximum; the rest of the expected message is fixed text.

Smithy (codepipeline, com-amazonaws-codepipeline-codepipeline-20150709.json): InvalidActionDeclarationException — errorKind client, httpError 400 (defaulted), member message @length 1..5000, doc "The action declaration was specified in an invalid format." The model fixes the code and status (both correct here) and says nothing about the message text, so the expected wording above is recorded AWS behavior.

## Steps to Reproduce
Cause 1:
1. Create a versioned S3 artifact-store bucket and an IAM role CodePipeline can assume.
2. CreatePipeline with a Source stage whose action outputs artifact "SourceOutput" and a Deploy stage whose action declares inputArtifacts [{"name": "NoSuchArtifact"}] (an artifact no preceding action outputs).
3. Observe the ClientError: code is InvalidActionDeclarationException with HTTP 400 (correct), but the message is "action DeployAction in stage Deploy declares the input artifact NoSuchArtifact, which no earlier action outputs" instead of "Input Artifact Bundle NoSuchArtifact in action DeployAction is not declared as Output Artifact Bundle in any of the preceding actions".

Cause 2:
1. CreatePipeline with a Source stage (S3 source, outputArtifacts [{"name": "SourceOutput"}]) and an Approve stage holding one action 'ApprovalAction' of type Approval/AWS/Manual/1 with no outputArtifacts.
2. GetPipeline, add outputArtifacts [{"name": "ApprovalOutput"}] to the Approval action, and call UpdatePipeline with that declaration.
3. Observe the ClientError: code is InvalidActionDeclarationException with HTTP 400 (correct), but the message is "ActionType (Category: 'Approval', Provider: 'Manual', Owner: 'AWS', Version: '1') in action 'ApprovalAction' allows at most 0 output artifact(s), but the action declares 1" instead of "Action ApprovalAction declares 1 output artifacts which is more than the maximum count (0)".

Replay both with: codepipeline.create-pipeline-dangling-input-artifact-rejected, codepipeline.update-pipeline-invalid-action-declaration-rejected
