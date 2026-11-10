Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-delete-launch-template-versions-partial-failure
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-delete-launch-template-versions-partial-failure ec2-launch-template-versioning
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|stub:DeleteLaunchTemplateVersions
Issue-Commit: 91a1d12adf13e91eb189076c1b2883c7fc3c7ce7

# Implement ec2 DeleteLaunchTemplateVersions: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `91a1d12adf13e91eb189076c1b2883c7fc3c7ce7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteLaunchTemplateVersions

## Description
`DeleteLaunchTemplateVersions` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_launch_template_versions.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `DeleteLaunchTemplateVersions` call, so each one unblocks only when `DeleteLaunchTemplateVersions` works: `ec2-delete-launch-template-versions-partial-failure#task#5`, `ec2-launch-template-versioning#task#7`, `ec2-launch-template-versioning#task#8`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_launch_template_versions.rs`. Replaying the corpus, the emulator refused the `DeleteLaunchTemplateVersions` call: status: expected 200, got 501
$.DeleteLaunchTemplateVersionsResponse: missing (expected {'requestId': '<shaped-not-asserted>', 'successfullyDeletedLaunchTemplateVersionSet': {'item': {'launchTemplateId': 'lt-0000000000000001c', 'launchTemplateName': 'sim2real-ltv-pf-911c8d43', 'versionNumber': '2'}}, 'unsuccessfullyDeletedLaunchTemplateVersionSet': {'item': {'launchTemplateId': 'lt-0000000000000001c', 'launchTemplateName': 'sim2real-ltv-pf-911c8d43', 'responseError': {'code': 'launchTemplateVersionDoesNotExist', 'message': 'The launch template version does not exist.'}, 'versionNumber': '99'}}}, got None)
$headers.content-type: value_mismatch (expected 'text/xml;charset=UTF-8', got 'text/xml')

## Steps to Reproduce
Boot the ec2 emulator
Call DeleteLaunchTemplateVersions and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
