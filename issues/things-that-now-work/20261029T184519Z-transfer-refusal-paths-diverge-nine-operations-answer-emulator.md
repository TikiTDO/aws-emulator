Issue-Source: trace-sweep-divergence
Issue-Trace: transfer-server-delete-error-paths
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: transfer-server-delete-error-paths transfer-as2-certificate-and-profile-lifecycle transfer-connector-transfer-request-validation transfer-import-invalid-host-key-then-list-unchanged transfer-server-host-key-and-security-policy transfer-resource-tagging-across-types transfer-as2-agreement-lifecycle transfer-webapp-without-identity-center
Issue-Shape-Id: com.amazonaws.transfer#TransferService
Issue-Key: transfer|CreateServer|4xx->4xx|$.Message
Issue-Key: transfer|CreateProfile|4xx->4xx|$.message
Issue-Key: transfer|StartFileTransfer|4xx->4xx|$.Message
Issue-Key: transfer|ListFileTransferResults|4xx->4xx|$.Message
Issue-Key: transfer|ListFileTransferResults|4xx->4xx|$.__type
Issue-Key: transfer|StartRemoteMove|4xx->2xx|$.Message
Issue-Key: transfer|StartRemoteMove|4xx->2xx|$.__type
Issue-Key: transfer|StartRemoteMove|4xx->2xx|status
Issue-Key: transfer|ImportHostKey|4xx->4xx|$.Message
Issue-Key: transfer|TagResource|4xx->4xx|$.Message
Issue-Key: transfer|TagResource|4xx->4xx|$.Resource
Issue-Key: transfer|TagResource|4xx->4xx|$.ResourceType
Issue-Key: transfer|DescribeAgreement|4xx->2xx|$.Message
Issue-Key: transfer|DescribeAgreement|4xx->2xx|$.Resource
Issue-Key: transfer|DescribeAgreement|4xx->2xx|$.ResourceType
Issue-Key: transfer|DescribeAgreement|4xx->2xx|$.__type
Issue-Key: transfer|DescribeAgreement|4xx->2xx|status
Issue-Key: transfer|CreateWebApp|4xx->2xx|$.Message
Issue-Key: transfer|CreateWebApp|4xx->2xx|$.__type
Issue-Key: transfer|CreateWebApp|4xx->2xx|status
Issue-Commit: 3b929b447a52a39dfc36e4a4cbd93099ed5ef8cf

# Transfer refusal paths diverge: nine operations answer emulator-authored messages, the wrong error code, or no refusal at all

> _Note: this issue was identified against the code base at commit `3b929b447a52a39dfc36e4a4cbd93099ed5ef8cf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateServer

## Description
Nine net-new refusal divergences in the transfer emulator share one fault: the refusal path is written by the emulator rather than reproduced from the service. Three sub-forms occur — the status/code is right but the Message is emulator prose, the error class itself is wrong, or the operation does not refuse at all and answers 200. Each cause must be closed before submitting; there are NINE.

Note: the recorded corpus returns HTTP 400 for this service's ResourceNotFoundException/InvalidRequestException envelopes (the model declares ResourceNotFoundException httpError 404); the recorded 400 is the requirement here, consistent with the already-resolved issue 'Transfer not-found errors answer HTTP 404 and echo the identifier instead of the recorded 400 generic not-found'. The open issue 'Transfer input validation, authorization and reference data diverge...' and 'Transfer host-key and web-app planes still refuse valid calls...' cover different operations and do not cover any cause below.

Cause 1 of 9 — CreateServer with EndpointType=VPC and no EndpointDetails.
AWS: Smithy: CreateServer.EndpointType -> EndpointType enum, EndpointDetails -> EndpointDetails (both optional in the model, so the cross-member rule is service-side); error InvalidRequestException (errorKind client). Recorded response: HTTP 400, Message 'EndpointDetails required for VPC'. The whole message is fixed text for this error class — it has no variable slot.
Emulator: HTTP 400 with the right code but Message 'EndpointDetails must be specified when EndpointType is VPC'.
Why: clients and test suites that assert on the service's refusal text (or log/triage on it) see a string AWS never emits, so a VPC-endpoint misconfiguration is reported with wording no AWS runbook matches.
Fix: CreateServer's input validation — the EndpointType/EndpointDetails cross-member check must emit the service's fixed string, not a rebuilt sentence.
Traces: transfer-server-delete-error-paths#task#5.

Cause 2 of 9 — CreateProfile with an out-of-set ProfileType.
AWS: Smithy: CreateProfile.ProfileType required, target enum ProfileType with values LOCAL, PARTNER; error InvalidRequestException. Recorded response: HTTP 400, message "1 validation error detected: Value at 'profileType' failed to satisfy constraint: Member must satisfy enum value set: [PARTNER, LOCAL]". Fixed text: the '<n> validation error(s) detected: Value at '<member>' failed to satisfy constraint: Member must satisfy enum value set: [...]' template. Variable parts: the member name taken from the request (profileType, lowerCamel wire spelling) and the enum value list. The value list is rendered in the SERVICE's order, PARTNER then LOCAL, which is NOT the model's declaration order.
Emulator: same status and template but the set is printed in model-declaration order: '[LOCAL, PARTNER]'.
Why: an exact-match assertion on the refusal text fails, and a client that parses the advertised set to build a retry sees a different ordering than the service produces. This is the same ordering fault already resolved for CreateWorkflow ('CreateWorkflow enum-constraint ValidationException message lists the WorkflowStepType value set in model-declaration order instead of AWS's order'), unfixed for ProfileType.
Fix: CreateProfile's validation — the enum value-set renderer must use the service's published order per enum rather than iterating the model shape.
Traces: transfer-as2-certificate-and-profile-lifecycle#task#12.

Cause 3 of 9 — StartFileTransfer with neither SendFilePaths nor RetrieveFilePaths.
AWS: Smithy: StartFileTransfer.SendFilePaths / RetrieveFilePaths both optional, target FilePaths @length min 1 max 10; error InvalidRequestException. Recorded response: HTTP 400, Message 'Empty send file list' — entirely fixed text for this class.
Emulator: HTTP 400 with Message 'SendFilePaths or RetrieveFilePaths must be specified'.
Why: the emulator describes the rule in its own words; a caller asserting the service's refusal (or matching it in error handling) does not recognise the string.
Fix: StartFileTransfer's input validation.
Traces: transfer-connector-transfer-request-validation#task#6.

Cause 4 of 9 — ListFileTransferResults against an AS2 connector: WRONG ERROR CLASS.
AWS: Smithy: ListFileTransferResults(ConnectorId, TransferId) with errors InvalidRequestException and ResourceNotFoundException. Recorded response: HTTP 400, __type 'InvalidRequestException', Message 'ListFileTransferResults is not supported for AS2 connectors' — fixed text; the operation is unsupported for AS2 connectors regardless of whether the transfer id exists.
Emulator: HTTP 400 but __type 'ResourceNotFoundException' with Message 'Unknown transfer' — it looks the transfer id up first and reports it missing.
Why: the connector-protocol gate is reported as a missing resource, so a client retries or re-creates the transfer instead of learning the call is not supported for AS2; code branching on __type takes the wrong branch.
Fix: ListFileTransferResults' handler — gate on the connector's protocol (AS2 vs SFTP) BEFORE resolving the transfer id, and raise InvalidRequestException.
Traces: transfer-connector-transfer-request-validation#task#7.

Cause 5 of 9 — StartRemoteMove against an AS2 connector: NO REFUSAL AT ALL.
AWS: Smithy: StartRemoteMove(ConnectorId, SourcePath, TargetPath) -> MoveId, errors include InvalidRequestException. Recorded response: HTTP 400, __type 'InvalidRequestException', Message 'Remote move not supported for AS2 connectors' — fixed text. Remote move is an SFTP-connector operation only.
Emulator: HTTP 200 with a MoveId — the move is accepted for an AS2 connector.
Why: the client believes a remote move was started on a connector that cannot perform one, then polls a move that will never exist; an unsupported-protocol bug in the caller is masked entirely.
Fix: StartRemoteMove's handler — add the connector-protocol gate before allocating a MoveId.
Traces: transfer-connector-transfer-request-validation#task#9.

Cause 6 of 9 — ImportHostKey with malformed host-key material.
AWS: Smithy: ImportHostKey.HostKeyBody required, target HostKey @length 0..4096 (sensitive); error InvalidRequestException. Recorded response: HTTP 400, Message 'Invalid host key format' — fixed text with no variable slot; it does not name the member or the expected encoding.
Emulator: HTTP 400 with Message 'HostKeyBody must be a private key in OpenSSH format', i.e. emulator prose that names the member and asserts a narrower encoding rule than the service states.
Why: callers matching the service's refusal text miss it, and the invented wording implies OpenSSH is the only accepted encoding, which is a rule AWS does not state in this message.
Fix: ImportHostKey's validation — emit the service's fixed string for malformed key material.
Traces: transfer-import-invalid-host-key-then-list-unchanged#task#4 (also affects transfer-server-host-key-and-security-policy#task#2 in the same cluster).

Cause 7 of 9 — TagResource on an unknown server ARN: all three not-found members wrong.
AWS: Smithy: TagResource(Arn required, Tags required); error ResourceNotFoundException whose members Message, Resource and ResourceType are ALL required. Recorded response: HTTP 400 with Message 'Unknown server', Resource = the bare server id from the request ARN, ResourceType = 'Server'. Fixed text: 'Unknown server' — the noun is the resource kind decoded from the ARN, not a generic word. Variable parts: Resource is the ARN's resource IDENTIFIER (the s-... id), not the whole ARN; ResourceType is the ARN's resource kind in TitleCase.
Emulator: HTTP 400 with Message 'Unknown resource', Resource = the full ARN as sent, ResourceType = 'Resource'.
Why: ResourceNotFoundException is modelled with Resource/ResourceType precisely so a client can tell WHICH kind of object was missing; a caller tagging several resource types cannot distinguish them, and anything keying on Resource as an identifier gets an ARN.
Fix: TagResource's handler/serializer for the tag plane — parse the ARN's resource type and id and populate the three members per-kind instead of emitting a single generic triple.
Traces: transfer-resource-tagging-across-types#task#13.

Cause 8 of 9 — DescribeAgreement for an agreement id that does not exist: NO REFUSAL AT ALL.
AWS: Smithy: DescribeAgreement(AgreementId required @pattern ^a-([0-9a-f]{17})$, ServerId required) -> Agreement required; errors include ResourceNotFoundException (Message/Resource/ResourceType all required). Recorded response: HTTP 400, __type 'ResourceNotFoundException', Message 'Unknown agreement' (fixed text), Resource = the requested agreement id, ResourceType = 'Agreement'.
Emulator: HTTP 200 — it answers successfully for an agreement id that was never created (the fixture asks for one after the lifecycle deleted/never created it).
Why: a deleted or mistyped agreement reads back as existing, so cleanup verification and negative tests pass against state that is gone; the client never learns the id is invalid.
Fix: DescribeAgreement's handler — look the agreement up in state and raise the not-found error when absent.
Traces: transfer-as2-agreement-lifecycle#task#11.

Cause 9 of 9 — CreateWebApp with no Identity Center application permission: NO REFUSAL AT ALL.
AWS: Smithy: CreateWebApp(IdentityProviderDetails required -> WebAppIdentityProviderDetails) -> WebAppId; errors include AccessDeniedException (errorKind client). Recorded response: HTTP 400, __type 'AccessDeniedException', Message of the standard IAM authorization-failure form: 'User: <calling-principal-arn> is not authorized to perform: sso:CreateApplication on resource: <sso application resource arn> because no resource-based policy allows the sso:CreateApplication action'. Fixed text: the 'User: ... is not authorized to perform: ... on resource: ... because no resource-based policy allows the ... action' template plus the action name sso:CreateApplication, which is the action CreateWebApp performs on the caller's behalf. Variable parts: the calling principal ARN (from the signed request's identity) and the Identity Center application resource ARN (built from the instance referenced by the request).
Emulator: HTTP 200 with a WebAppId — the web app is created although the caller has no Identity Center application permission.
Why: the Identity Center precondition for web apps is silently skipped, so a deployment that cannot work in a real account appears to succeed; nothing surfaces the missing sso:CreateApplication grant.
Fix: CreateWebApp's handler/authorization step — perform the sso:CreateApplication authorization check against the calling principal before provisioning, and render the standard AccessDenied message with the principal and resource filled from request and state.
Traces: transfer-webapp-without-identity-center#task#4.

## Evidence
All nine causes come from replay of simparity-wire-aws-v1 (score 82.1%, 25 diverged steps). Expected = recorded AWS response, Actual = emulator.

1. CreateServer / transfer-server-delete-error-paths#task#5 — status 400 both; $.Message expected 'EndpointDetails required for VPC', got 'EndpointDetails must be specified when EndpointType is VPC'.
2. CreateProfile / transfer-as2-certificate-and-profile-lifecycle#task#12 — status 400 both; $.message expected "1 validation error detected: Value at 'profileType' failed to satisfy constraint: Member must satisfy enum value set: [PARTNER, LOCAL]", got the same text with '[LOCAL, PARTNER]'.
3. StartFileTransfer / transfer-connector-transfer-request-validation#task#6 — status 400 both; $.Message expected 'Empty send file list', got 'SendFilePaths or RetrieveFilePaths must be specified'.
4. ListFileTransferResults / transfer-connector-transfer-request-validation#task#7 — status 400 both; $.__type expected 'InvalidRequestException', got 'ResourceNotFoundException'; $.Message expected 'ListFileTransferResults is not supported for AS2 connectors', got 'Unknown transfer'.
5. StartRemoteMove / transfer-connector-transfer-request-validation#task#9 — status expected 400, got 200; $.__type expected 'InvalidRequestException', got None; $.Message expected 'Remote move not supported for AS2 connectors', got None.
6. ImportHostKey / transfer-import-invalid-host-key-then-list-unchanged#task#4 (cluster also covers transfer-server-host-key-and-security-policy#task#2) — status 400 both; $.Message expected 'Invalid host key format', got 'HostKeyBody must be a private key in OpenSSH format'.
7. TagResource / transfer-resource-tagging-across-types#task#13 — status 400 both; $.Message expected 'Unknown server', got 'Unknown resource'; $.Resource expected the bare server id, got the full 'arn:aws:transfer:...:server/<id>'; $.ResourceType expected 'Server', got 'Resource'.
8. DescribeAgreement / transfer-as2-agreement-lifecycle#task#11 — status expected 400, got 200; $.__type expected 'ResourceNotFoundException', got None; $.Message expected 'Unknown agreement', got None; $.Resource expected the requested agreement id, got None; $.ResourceType expected 'Agreement', got None.
9. CreateWebApp / transfer-webapp-without-identity-center#task#4 — status expected 400, got 200; $.__type expected 'AccessDeniedException', got None; $.Message expected the 'User: ... is not authorized to perform: sso:CreateApplication on resource: ... because no resource-based policy allows the sso:CreateApplication action' text, got None.

Model grounding (services/service-com-amazonaws-transfer-transferservice/smithy-model/com-amazonaws-transfer-transferservice.json): ProfileType enum = [LOCAL, PARTNER]; ResourceNotFoundException requires Message, Resource and ResourceType; CreateServer/CreateWebApp both declare AccessDeniedException and InvalidRequestException; StartRemoteMove and ListFileTransferResults both declare InvalidRequestException.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Re-replay the nine bound steps:
   - transfer-server-delete-error-paths#task#5 (CreateServer EndpointType=VPC, no EndpointDetails)
   - transfer-as2-certificate-and-profile-lifecycle#task#12 (CreateProfile with an out-of-set ProfileType)
   - transfer-connector-transfer-request-validation#task#6 (StartFileTransfer with no SendFilePaths/RetrieveFilePaths)
   - transfer-connector-transfer-request-validation#task#7 (ListFileTransferResults on an AS2 connector)
   - transfer-connector-transfer-request-validation#task#9 (StartRemoteMove on an AS2 connector)
   - transfer-import-invalid-host-key-then-list-unchanged#task#4 (ImportHostKey with malformed HostKeyBody)
   - transfer-resource-tagging-across-types#task#13 (TagResource on an unknown server ARN)
   - transfer-as2-agreement-lifecycle#task#11 (DescribeAgreement for a nonexistent agreement id)
   - transfer-webapp-without-identity-center#task#4 (CreateWebApp with no Identity Center application permission)
4. Compare status, __type, Message (and Resource/ResourceType where modelled) against the recorded response for each.
