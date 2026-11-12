Issue-Source: trace-sweep-divergence
Issue-Trace: iam-oidc-provider-lifecycle
Issue-Step: task#12
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-oidc-provider-lifecycle iam-outbound-web-identity-federation iam-outbound-web-identity-federation-lifecycle
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|EnableOutboundWebIdentityFederation|2xx->4xx|$.EnableOutboundWebIdentityFederationResponse
Issue-Key: iam|EnableOutboundWebIdentityFederation|2xx->4xx|status
Issue-Key: iam|GetOutboundWebIdentityFederationInfo|2xx->4xx|$.GetOutboundWebIdentityFederationInfoResponse
Issue-Key: iam|GetOutboundWebIdentityFederationInfo|2xx->4xx|status
Issue-Key: iam|DisableOutboundWebIdentityFederation|2xx->4xx|$.DisableOutboundWebIdentityFederationResponse
Issue-Key: iam|DisableOutboundWebIdentityFederation|2xx->4xx|status
Issue-Key: iam|GetOutboundWebIdentityFederationInfo|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: iam|GetOutboundWebIdentityFederationInfo|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|GetOutboundWebIdentityFederationInfo|4xx->4xx|status
Issue-Key: iam|GetOutboundWebIdentityFederationInfo|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|EnableOutboundWebIdentityFederation|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: iam|EnableOutboundWebIdentityFederation|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|EnableOutboundWebIdentityFederation|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|EnableOutboundWebIdentityFederation|4xx->4xx|status
Issue-Key: iam|DisableOutboundWebIdentityFederation|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: iam|DisableOutboundWebIdentityFederation|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|DisableOutboundWebIdentityFederation|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|DisableOutboundWebIdentityFederation|4xx->4xx|status
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# Outbound web identity federation actions answer InvalidAction unknown action

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
EnableOutboundWebIdentityFederation

## Description
Cause 1 of 1: the three outbound-web-identity-federation actions are not dispatched, so both their success contract and their two modelled account-state refusals are unreachable.

AWS (recorded behavior only - these operations are not in the checked-out Smithy model, so the contract is taken from the recorded responses): EnableOutboundWebIdentityFederation returns 200 with EnableOutboundWebIdentityFederationResult/IssuerIdentifier (a generated per-account issuer URL); GetOutboundWebIdentityFederationInfo returns 200 with GetOutboundWebIdentityFederationInfoResult{IssuerIdentifier, JwtVendingEnabled 'true'}; DisableOutboundWebIdentityFederation returns 200 with an empty result. When the feature is off, Get and Disable are refused 404 with Code FeatureDisabled and Message 'Outbound identity federation is disabled for account <accountId>' (Disable on an already-off account: 'Outbound identity federation is already disabled for account <accountId>'); when it is already on, Enable is refused 409 with Code FeatureEnabled and Message 'Outbound identity federation is already enabled for account <accountId>'. The fixed text is as quoted; the only variable part is the 12-digit account id of the caller.

Emulator: all three actions answer 400 with Code InvalidAction and Message 'unknown action: <Action>' - the feature flag is never stored, so neither the enabled nor the disabled branch can be observed.

Why: a client enabling outbound federation gets a 400 on a call AWS answers 200, never receives an IssuerIdentifier, and can never distinguish 'feature off' (FeatureDisabled 404) from 'already on' (FeatureEnabled 409) - the two states the whole feature is driven by.

Fix: route the three actions in the IAM action dispatcher and add per-account outbound-federation state (issuer identifier + enabled flag) that the handlers read and write, including the FeatureDisabled/FeatureEnabled guard clauses.

Traces: iam-oidc-provider-lifecycle#task#12 (Enable), #task#13 (Get), #task#14 (Disable); iam-outbound-web-identity-federation#task#3 (Get, feature off); iam-outbound-web-identity-federation-lifecycle#task#3, #task#6 (Enable while enabled), #task#8 (Disable while disabled).

## Evidence
Expected (recorded AWS) for EnableOutboundWebIdentityFederation: HTTP 200, EnableOutboundWebIdentityFederationResponse/EnableOutboundWebIdentityFederationResult/IssuerIdentifier. Actual: HTTP 400, Code InvalidAction, Message 'unknown action: EnableOutboundWebIdentityFederation'.
Expected for GetOutboundWebIdentityFederationInfo with the feature off: HTTP 404, Code FeatureDisabled, Message 'Outbound identity federation is disabled for account 000000000000'. Actual: HTTP 400, Code InvalidAction, Message 'unknown action: GetOutboundWebIdentityFederationInfo'.
Expected for EnableOutboundWebIdentityFederation when already enabled: HTTP 409, Code FeatureEnabled, Message 'Outbound identity federation is already enabled for account 000000000000'. Actual: HTTP 400 Code InvalidAction.
Expected for DisableOutboundWebIdentityFederation when already disabled: HTTP 404, Code FeatureDisabled, Message 'Outbound identity federation is already disabled for account 000000000000'. Actual: HTTP 400 Code InvalidAction.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay iam-outbound-web-identity-federation-lifecycle: task#3 GetOutboundWebIdentityFederationInfo before enabling -> 400 InvalidAction instead of 404 FeatureDisabled; task#4 Enable -> 400 instead of 200 with IssuerIdentifier; task#6 Enable again -> 400 instead of 409 FeatureEnabled; task#8 Disable twice -> 400 instead of 404 FeatureDisabled.
3. Replay iam-oidc-provider-lifecycle#task#12..#task#14 for the same three actions in the OIDC lifecycle.
