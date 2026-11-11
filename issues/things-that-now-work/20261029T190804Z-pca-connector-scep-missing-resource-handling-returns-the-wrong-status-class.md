Issue-Source: trace-sweep-divergence
Issue-Trace: pca-connector-scep-error-not-found-resources
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pca-connector-scep-error-not-found-resources pca-connector-scep-error-create-connector-bad-ca
Issue-Shape-Id: com.amazonaws.pcaconnectorscep#PcaConnectorScep
Issue-Key: pca-connector-scep|DeleteConnector|2xx->4xx|$
Issue-Key: pca-connector-scep|DeleteConnector|2xx->4xx|status
Issue-Key: pca-connector-scep|DeleteChallenge|2xx->4xx|$
Issue-Key: pca-connector-scep|DeleteChallenge|2xx->4xx|status
Issue-Key: pca-connector-scep|ListTagsForResource|4xx->4xx|$.message
Issue-Key: pca-connector-scep|ListTagsForResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: pca-connector-scep|ListTagsForResource|4xx->4xx|status
Issue-Key: pca-connector-scep|TagResource|4xx->4xx|$.message
Issue-Key: pca-connector-scep|TagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: pca-connector-scep|TagResource|4xx->4xx|status
Issue-Key: pca-connector-scep|CreateConnector|4xx->2xx|$.Message
Issue-Key: pca-connector-scep|CreateConnector|4xx->2xx|$.ResourceId
Issue-Key: pca-connector-scep|CreateConnector|4xx->2xx|$.ResourceType
Issue-Key: pca-connector-scep|CreateConnector|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: pca-connector-scep|CreateConnector|4xx->2xx|status
Issue-Commit: ef775f68482e6d25f63f0fc1f052163f266c1a25

# Missing-resource handling returns the wrong status class on deletes, tagging and CreateConnector

> _Note: this issue was identified against the code base at commit `ef775f68482e6d25f63f0fc1f052163f266c1a25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteConnector

## Description
Shared fault: when a referenced resource is absent, the emulator picks the wrong HTTP status class. It raises 404 where AWS succeeds (idempotent deletes), raises 404 where AWS raises a 400 tagging-layer error, and succeeds where AWS raises 404. All five causes are pca-connector-scep.

Smithy: pca-connector-scep (restJson1). DeleteConnector (DELETE /connectors/{ConnectorArn}) and DeleteChallenge (DELETE /challenges/{ChallengeArn}) are modeled with output Unit and http code 202. CreateConnector (POST /connectors, code 202) lists ResourceNotFoundException among its errors. ListTagsForResourceRequest.ResourceArn / TagResourceRequest.ResourceArn target plain String with no pattern; ResourceNotFoundException is httpError=404, BadRequestException is httpError=400. 5 causes.

Cause 1 of 5: DeleteConnector on a connector that does not exist must be idempotent.
  AWS: 202 Accepted with an empty JSON body `{}` and no error header — deleting an absent connector is a no-op success (recorded behavior; the model gives the op output Unit with code 202).
  Emulator: 404 with body {"__type":"ResourceNotFoundException","Message":"Connector <arn> could not be found","ResourceId":"<arn>","ResourceType":"Connector"}.
  Why: teardown and cleanup code (CDK/CloudFormation deletes, test fixtures, retried deletes after a timeout) treats delete as idempotent. Against the emulator the second delete, or a delete of something already gone, hard-fails and aborts the teardown.
  Fix: the DeleteConnector handler — its missing-key branch must return the 202/empty-body success instead of constructing a not-found error.
  Traces: pca-connector-scep-error-not-found-resources#task#9.

Cause 2 of 5: DeleteChallenge on a challenge that does not exist must be idempotent.
  AWS: 202 Accepted, empty JSON body `{}`, no error header.
  Emulator: 404 ResourceNotFoundException with ResourceType "Challenge".
  Why: as cause 1, for challenge teardown — and challenges are implicitly removed when their connector is deleted, so an explicit cleanup delete lands on an absent challenge routinely.
  Fix: the DeleteChallenge handler's missing-key branch.
  Traces: pca-connector-scep-error-not-found-resources#task#10.

Cause 3 of 5: ListTagsForResource on an absent resource is a 400 tagging-layer error, not the modeled 404.
  AWS: 400 with x-amzn-errortype `NotFoundException` and body member `message` (lowercase) = "Resource was not found" — a fixed string, carrying no ARN. The tagging endpoints answer from the tag subsystem, which reports absence with its own 400 error rather than the service's 404 ResourceNotFoundException.
  Emulator: 404 with x-amzn-errortype ResourceNotFoundException and a capital-M `Message` describing the connector.
  Why: both the status class and the error-code string are wrong, so a client's catch/branch on NotFoundException never fires, a 4xx-vs-404 retry policy classifies the call differently, and the body member name differs in case so even a message read fails.
  Fix: the tagging path's not-found branch (ListTagsForResource handler / the shared tag-resource lookup), which must emit the 400 NotFoundException shape with a lowercase `message`.
  Traces: pca-connector-scep-error-not-found-resources#task#6.

Cause 4 of 5: TagResource on an absent resource — same divergence as cause 3.
  AWS: 400, x-amzn-errortype `NotFoundException`, `message` = "Resource was not found".
  Emulator: 404 ResourceNotFoundException.
  Why: as cause 3, on the tag-write path; tagging a resource that was concurrently deleted is misclassified.
  Fix: the same shared tagging not-found branch, reached from TagResource.
  Traces: pca-connector-scep-error-not-found-resources#task#7.

Cause 5 of 5: CreateConnector never checks that the referenced private CA exists.
  AWS: 404 ResourceNotFoundException when CertificateAuthorityArn names a certificate authority that does not exist — `Message` = "CertificateAuthorityArn was not found: <caArn>.", `ResourceId` = the supplied CA ARN, `ResourceType` = "CertificateAuthorityArn" (the input member name), x-amzn-errortype ResourceNotFoundException. ResourceNotFoundException is a modeled error of CreateConnector.
  Emulator: 202 Accepted with a created-connector body — the connector is accepted and registered, with no existence check on the CA and none of the four error fields present.
  Why: the worst of the five — a misconfigured CA ARN silently "succeeds", so client code that depends on create-time validation proceeds to issue challenges against a connector that could never work on AWS, and the error path is untestable.
  Fix: CreateConnector's validation/handler — resolve CertificateAuthorityArn against acm-pca state before admitting the connector, and raise the modeled 404 naming the input member as ResourceType.
  Traces: pca-connector-scep-error-create-connector-bad-ca#task#1.

Related but NOT covering cause 5: open issue 20261024T071525Z-pca-connector-scep-createconnector-wrong-behavior.md ("CreateConnector never produces a FAILED connector: the referenced private CA and VPC endpoint are never checked") describes the asynchronous FAILED-status path; it carries no replay key for this fixture step and does not cover the synchronous 404 ResourceNotFoundException required here.

## Evidence
Replay against trace set simparity-wire-aws-v1, 5 mismatch clusters:

- pca-connector-scep-error-not-found-resources#task#9 DeleteConnector: status expected 202, got 404; body expected {} , got {'__type': 'ResourceNotFoundException', 'Message': 'Connector <arn> could not be found', 'ResourceId': '<arn>', 'ResourceType': 'Connector'}
- pca-connector-scep-error-not-found-resources#task#10 DeleteChallenge: status expected 202, got 404; body expected {}, got {'__type': 'ResourceNotFoundException', ..., 'ResourceType': 'Challenge'}
- pca-connector-scep-error-not-found-resources#task#6 ListTagsForResource: status expected 400, got 404; $.message expected 'Resource was not found', got None (absent); $headers.x-amzn-errortype expected 'NotFoundException', got 'ResourceNotFoundException'
- pca-connector-scep-error-not-found-resources#task#7 TagResource: identical to task#6 (status 400->404, $.message 'Resource was not found' missing, errortype NotFoundException -> ResourceNotFoundException)
- pca-connector-scep-error-create-connector-bad-ca#task#1 CreateConnector: status expected 404, got 202; $.Message expected 'CertificateAuthorityArn was not found: <caArn>.', got None; $.ResourceId expected '<caArn>', got None; $.ResourceType expected 'CertificateAuthorityArn', got None; $headers.x-amzn-errortype expected 'ResourceNotFoundException', got None

Requirements: statuses 202 (idempotent delete), 400 + NotFoundException (tagging), 404 + ResourceNotFoundException (CreateConnector); fixed message text 'Resource was not found' and '<member> was not found: <arn>.'. The ARNs/UUIDs in the recording are per-run values that fill the templates, not part of them.

## Steps to Reproduce
1. Boot the emulator and replay fixtures pca-connector-scep-error-not-found-resources and pca-connector-scep-error-create-connector-bad-ca from trace set simparity-wire-aws-v1.
2. task#9/task#10 send DELETE /connectors/{arn} and DELETE /challenges/{arn} for ARNs that were never created: expect 202 with body {}, observe 404 ResourceNotFoundException.
3. task#6/task#7 send GET /tags/{arn} and POST /tags/{arn} for an absent resource: expect 400 with x-amzn-errortype NotFoundException and {"message":"Resource was not found"}, observe 404 ResourceNotFoundException.
4. pca-connector-scep-error-create-connector-bad-ca#task#1 sends POST /connectors with a well-formed CertificateAuthorityArn for a CA that does not exist: expect 404 ResourceNotFoundException naming CertificateAuthorityArn, observe 202 and a created connector.
