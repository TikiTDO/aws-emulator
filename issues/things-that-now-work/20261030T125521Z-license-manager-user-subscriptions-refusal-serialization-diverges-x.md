Issue-Source: trace-sweep-divergence
Issue-Trace: license-manager-user-subscriptions-associate-disassociate-user-unknown-instance
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: license-manager-user-subscriptions-associate-disassociate-user-unknown-instance license-manager-user-subscriptions-deregister-identity-provider-not-found license-manager-user-subscriptions-license-server-endpoint-lifecycle-errors license-manager-user-subscriptions-list-associations-and-subscriptions-unregistered-idp license-manager-user-subscriptions-start-stop-product-subscription-unregistered-idp license-manager-user-subscriptions-update-identity-provider-settings-unregistered
Issue-Shape-Id: com.amazonaws.licensemanagerusersubscriptions#LicenseManagerUserSubscriptions
Issue-Key: license-manager-user-subscriptions|AssociateUser|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: license-manager-user-subscriptions|DisassociateUser|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: license-manager-user-subscriptions|DeregisterIdentityProvider|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: license-manager-user-subscriptions|DeleteLicenseServerEndpoint|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: license-manager-user-subscriptions|ListUserAssociations|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: license-manager-user-subscriptions|ListProductSubscriptions|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: license-manager-user-subscriptions|StartProductSubscription|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: license-manager-user-subscriptions|StopProductSubscription|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: license-manager-user-subscriptions|UpdateIdentityProviderSettings|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: license-manager-user-subscriptions|DeregisterIdentityProvider|4xx->4xx|$.message
Issue-Commit: 73df09ca417a0b7206ba1e112ed84e03c7401ae7

# license-manager-user-subscriptions refusal serialization diverges: x-amzn-errortype header and constraint-message pattern rendering

> _Note: this issue was identified against the code base at commit `73df09ca417a0b7206ba1e112ed84e03c7401ae7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateUser

## Description
SHARED FAULT (10 causes, all in the license-manager-user-subscriptions crate's REFUSAL SERIALIZATION, not in any one operation handler): on every path replayed here the emulator already picks the right HTTP status and the right error class, but the bytes it writes for the refusal diverge from the recording. Causes 1-9 are the same defect in the error-response HEADER serializer, reproduced once per operation that reaches the Unauthorized front door (nine separate fixture steps, nine separate replay keys); cause 10 is the shared constraint-violation MESSAGE renderer. A fixer must close all TEN causes, and must re-replay all ten fixture steps listed in Traces: a single header-serializer edit is expected to clear causes 1-9, but each is its own recorded step and none of them is exercised by the others.

Causes 1-9 share this AWS/Why/Fix analysis (repeated per cause below only for the operation-specific expected/actual):
AWS: recorded behavior only for the refusal class. Smithy (services/service-com-amazonaws-licensemanagerusersubscriptions-licensemanagerusersubscriptions): protocol restJson1, authScheme sigv4; UnauthorizedException is NOT in the model at all - no operation binds it and the only service-common error is InternalServerException - so this refusal comes from the service's front door, above the modeled operation errors. In every recording of that refusal the response carries the error-type header x-amzn-errortype TWICE with the value UnauthorizedException, which an HTTP client that joins repeated headers observes as the single string 'UnauthorizedException, UnauthorizedException'. The requirement is the header SET the service emits on this refusal class (the error-type header repeated, value UnauthorizedException - entirely fixed text, nothing interpolated from the request or state), not merely that the value parses to the right error name. Hypothesis: on the real service the sigv4/front-door denial stamps the error-type header and the restJson1 error serializer stamps it again, so the refusal leaves with two occurrences.
Emulator: HTTP 401 and the right error name, but x-amzn-errortype is emitted ONCE ('UnauthorizedException'), so the header set does not match the recording.
Why: a client or SDK that reads the raw header set - error-class extraction that splits the error-type header on ',', middleware that asserts on the header list, or a proxy/test fixture that compares response headers - sees a different refusal envelope than AWS sends for this service. Code and status being right makes this silent: the divergence only shows up in the one place (the header) that SDK error deserializers key on.
Fix: the crate's shared error-response header serialization for the front-door Unauthorized refusal (the component that stamps x-amzn-errortype on a 401), NOT the nine operation handlers - they already select status 401 and the UnauthorizedException class correctly.

Cause 1 of 10 - AssociateUser, unknown EC2 instance, front-door Unauthorized refusal. Emulator: 401 with x-amzn-errortype 'UnauthorizedException'; recorded: 401 with the header twice ('UnauthorizedException, UnauthorizedException'). Traces: license-manager-user-subscriptions-associate-disassociate-user-unknown-instance#task#1.

Cause 2 of 10 - DisassociateUser, same fixture. Emulator: 401, header once; recorded: header twice. Traces: license-manager-user-subscriptions-associate-disassociate-user-unknown-instance#task#2 (its cluster also covers task#3 of the same fixture).

Cause 3 of 10 - DeregisterIdentityProvider against an identity provider that is not registered. The recorded step asserts the error-type header by SHAPE rather than by literal value, and the emulator's single-valued 'UnauthorizedException' does not satisfy that assertion - the same header-set divergence as causes 1-2, reported as a shape mismatch instead of a value mismatch. Status 401 matches. Traces: license-manager-user-subscriptions-deregister-identity-provider-not-found#task#2.

Cause 4 of 10 - DeleteLicenseServerEndpoint on the license-server endpoint lifecycle error path. Same shape-asserted error-type header; emulator sends it once. Status 401 matches. Traces: license-manager-user-subscriptions-license-server-endpoint-lifecycle-errors#task#5.

Cause 5 of 10 - ListUserAssociations against an unregistered identity provider. Emulator: 401, header once; recorded: header twice. Traces: license-manager-user-subscriptions-list-associations-and-subscriptions-unregistered-idp#task#1.

Cause 6 of 10 - ListProductSubscriptions, same fixture. Emulator: 401, header once; recorded: header twice. Traces: license-manager-user-subscriptions-list-associations-and-subscriptions-unregistered-idp#task#2.

Cause 7 of 10 - StartProductSubscription against an unregistered identity provider. Emulator: 401, header once; recorded: header twice. Traces: license-manager-user-subscriptions-start-stop-product-subscription-unregistered-idp#task#1.

Cause 8 of 10 - StopProductSubscription, same fixture. Emulator: 401, header once; recorded: header twice. Traces: license-manager-user-subscriptions-start-stop-product-subscription-unregistered-idp#task#2.

Cause 9 of 10 - UpdateIdentityProviderSettings against an identity provider that was never registered. Emulator: 401, header once; recorded: header twice. Traces: license-manager-user-subscriptions-update-identity-provider-settings-unregistered#task#1.

Cause 10 of 10 - the pattern-constraint violation message quotes the regular expression WITH its ^ and $ anchors.
AWS: Smithy: DeregisterIdentityProviderRequest.IdentityProviderArn targets Arn with @pattern ^arn:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,63}:[a-zA-Z0-9-\.]{1,510}/[a-zA-Z0-9-\.]{1,510}$; the violation class is ValidationException (errorKind client, httpError 400 defaulted). The recorded refusal body is the standard framework constraint sentence - fixed text '1 validation error detected: Value ' / ' at ' / ' failed to satisfy constraint: Member must satisfy regular expression pattern: ' - whose variable parts are the rejected value and the member name, both copied from the request (the recording's 'arn:aws:license-manager-user-subscriptions:us-west-2:000000000000:identity-provider/d-2eabeceeaa/VISUAL_STUDIO_PROFESSIONAL' and 'identityProviderArn' fill the template, they are not part of it) and the pattern itself, rendered WITHOUT the leading ^ and trailing $ anchors.
Emulator: identical status (400), class, fixed text, value and member name, but it interpolates the modeled pattern verbatim, anchors included ('...regular expression pattern: ^arn:...$').
Why: the message is part of the error-class contract; a client that parses or compares the quoted pattern - or re-compiles it to pre-validate input - gets a regex the service never emits, and the divergence applies to EVERY pattern-constraint refusal this service can produce (Arn, Directory ^(d|sd)-[0-9a-f]{10}$, SecurityGroup ^sg-..., Subnet ^subnet-...), not just this member.
Fix: the shared constraint-violation message renderer used by this crate's input validation - strip the anchors when interpolating a @pattern - not the DeregisterIdentityProvider handler.
Traces: license-manager-user-subscriptions-deregister-identity-provider-not-found#task#3.

Related open issues, none of which carries any of these ten replay keys: 20261029T234938Z-license-manager-user-subscriptions-400-refusal-bodies-are.md describes the anchor-rendering fault of cause 10 in prose (as its cause 2) but its replay keys bind only ListInstances and RegisterIdentityProvider $.message steps, so deregister-identity-provider-not-found#task#3 is never re-replayed by it; 20261030T034340Z-license-manager-user-subscriptions-refusal-fidelity-iam-not.md is behavioral and covers filter/directory wording, missing IAM authorization and StopProductSubscription's error class, not response headers or pattern rendering. The resolved issue 20261029T234938Z-license-manager-user-subscriptions-unauthorized-refusals-answer-http.md fixed the STATUS of these refusals (400 -> 401); the status now matches and only the header set still diverges, so it does not cover causes 1-9.

## Evidence
Replay of wire trace set simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up=true, 37 steps, 23 passed, 14 mismatched, score 62.2%, 13 divergence clusters. Three clusters were already owned by open issues and are excluded here; the ten below are net-new.

Causes 1-9 (header set on the 401 Unauthorized refusal) - expected status 401, actual status 401 in every case; the only diverging key is $headers.x-amzn-errortype:
1. license-manager-user-subscriptions-associate-disassociate-user-unknown-instance#task#1 AssociateUser - value_mismatch: expected 'UnauthorizedException, UnauthorizedException', got 'UnauthorizedException'.
2. license-manager-user-subscriptions-associate-disassociate-user-unknown-instance#task#2 DisassociateUser - value_mismatch: expected 'UnauthorizedException, UnauthorizedException', got 'UnauthorizedException' (cluster affects task#2 and task#3).
3. license-manager-user-subscriptions-deregister-identity-provider-not-found#task#2 DeregisterIdentityProvider - shape_mismatch: expected '<shaped-not-asserted>', got 'UnauthorizedException'.
4. license-manager-user-subscriptions-license-server-endpoint-lifecycle-errors#task#5 DeleteLicenseServerEndpoint - shape_mismatch: expected '<shaped-not-asserted>', got 'UnauthorizedException'.
5. license-manager-user-subscriptions-list-associations-and-subscriptions-unregistered-idp#task#1 ListUserAssociations - value_mismatch: expected 'UnauthorizedException, UnauthorizedException', got 'UnauthorizedException'.
6. license-manager-user-subscriptions-list-associations-and-subscriptions-unregistered-idp#task#2 ListProductSubscriptions - same expected/actual.
7. license-manager-user-subscriptions-start-stop-product-subscription-unregistered-idp#task#1 StartProductSubscription - same expected/actual.
8. license-manager-user-subscriptions-start-stop-product-subscription-unregistered-idp#task#2 StopProductSubscription - same expected/actual.
9. license-manager-user-subscriptions-update-identity-provider-settings-unregistered#task#1 UpdateIdentityProviderSettings - same expected/actual.
No body ($.message) divergence was reported on any of these nine steps, so the status, error name and refusal text already match; the header set is the whole divergence.

Cause 10: license-manager-user-subscriptions-deregister-identity-provider-not-found#task#3 DeregisterIdentityProvider - expected status 400, actual 400; $.message value_mismatch. Expected "1 validation error detected: Value 'arn:aws:license-manager-user-subscriptions:us-west-2:000000000000:identity-provider/d-2eabeceeaa/VISUAL_STUDIO_PROFESSIONAL' at 'identityProviderArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,63}:[a-zA-Z0-9-\.]{1,510}/[a-zA-Z0-9-\.]{1,510}"; got the identical sentence with the pattern rendered as ^arn:...$ - the two anchor characters are the only difference.

Model evidence (services/service-com-amazonaws-licensemanagerusersubscriptions-licensemanagerusersubscriptions/smithy-model/com-amazonaws-licensemanagerusersubscriptions-licensemanagerusersubscriptions.json): sdkId 'License Manager User Subscriptions', protocol restJson1, authScheme sigv4, 17 operations; UnauthorizedException appears nowhere in the model (per-op errors are AccessDeniedException/ConflictException/ResourceNotFoundException/ServiceQuotaExceededException/ThrottlingException/ValidationException, service-common error InternalServerException only), confirming the 401 refusal is a front-door envelope rather than a modeled operation error; ValidationException is errorKind client with httpError 400 (defaulted); DeregisterIdentityProviderRequest.IdentityProviderArn targets Arn whose @pattern carries the ^...$ anchors.

## Steps to Reproduce
1. git clone the repo (mainline) and boot the emulator built from that checkout.
2. Replay wire trace set simparity-wire-aws-v1 (replay_wire_corpus).
3. Representative (cause 1): fixture license-manager-user-subscriptions-associate-disassociate-user-unknown-instance, step task#1 - POST /user/AssociateUser on the path that reaches the Unauthorized front door. The response is 401 with the right error name, but dump the RAW response headers: the emulator emits x-amzn-errortype once, while the recording carries it twice (joined by an HTTP client as 'UnauthorizedException, UnauthorizedException').
4. Causes 2-9: repeat step 3 for each recorded step and compare the error-type header set, not just its parsed value - associate-disassociate-user-unknown-instance#task#2 (DisassociateUser, POST /user/DisassociateUser), deregister-identity-provider-not-found#task#2 (DeregisterIdentityProvider, POST /identity-provider/DeregisterIdentityProvider), license-server-endpoint-lifecycle-errors#task#5 (DeleteLicenseServerEndpoint, POST /license-server/DeleteLicenseServerEndpoint), list-associations-and-subscriptions-unregistered-idp#task#1 and #task#2 (POST /user/ListUserAssociations, POST /user/ListProductSubscriptions), start-stop-product-subscription-unregistered-idp#task#1 and #task#2 (POST /user/StartProductSubscription, POST /user/StopProductSubscription), update-identity-provider-settings-unregistered#task#1 (POST /identity-provider/UpdateIdentityProviderSettings).
5. Cause 10: fixture license-manager-user-subscriptions-deregister-identity-provider-not-found, step task#3 - POST /identity-provider/DeregisterIdentityProvider with an IdentityProviderArn that violates the Arn @pattern. Status 400 and the sentence's fixed text match; the quoted pattern in $.message is rendered with ^ and $, which the recorded body omits. Confirm the same for any other pattern-constrained member (DirectoryId, SecurityGroupId, Subnets) to verify the fix is in the shared renderer.
6. After fixing, re-replay all ten steps above (they are listed in Issue-Trace / Issue-Trace-Ids) - the nine header steps are distinct recorded units and are not exercised by one another.
