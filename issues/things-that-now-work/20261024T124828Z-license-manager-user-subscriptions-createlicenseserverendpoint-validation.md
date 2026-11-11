Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: dc6b9d697210888fba6fd46cd7393ea22255dcd4

# RdsSalCredentialsProvider SecretId is accepted without resolving the Secrets Manager secret

_Filed: 2026-10-24T12:48:28Z_
_Operation: CreateLicenseServerEndpoint | Class: validation_

> _Note: this issue was identified against the code base at commit `dc6b9d697210888fba6fd46cd7393ea22255dcd4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateLicenseServerEndpoint.LicenseServerSettings` is documented as "the type of license server and the Secrets Manager secret that enables administrators to add or remove users associated with the license server": `ServerSettings.RdsSalSettings.RdsSalCredentialsProvider` is a `CredentialsProvider` union whose `SecretsManagerCredentialsProvider.SecretId` names a secret of the caller's account. The handler stores the whole `LicenseServerSettings` structure verbatim and never resolves that secret, so an endpoint can be created against a SecretId no secret exists for: the call answers 200, the endpoint is persisted, and it settles into PROVISIONED as if the RDS license server had been configured with working administrator credentials.

Expected: an endpoint whose credentials secret cannot be resolved is not provisionable — the operation models both ValidationException (400) and ResourceNotFoundException (404) for exactly this kind of unusable reference, and the endpoint's own `LicenseServerEndpointProvisioningStatus` models PROVISIONING_FAILED for a provisioning that cannot complete. A client that mistypes or deletes the secret must be told, not handed a PROVISIONED endpoint.

Actual: the SecretId is opaque text to this emulator. `secretsmanager:CreateSecret` IS implemented in this repo and its state exposes `pub secrets: BTreeMap<String, Secret>`, so the secret store is readable in the shared engine (the pattern EVS uses to read EC2's store in `services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/state/ec2_resources.rs`), and no endpoint ever reaches PROVISIONING_FAILED because `status_message`/failed statuses are never written.

## Evidence

Spec: `CreateLicenseServerEndpointRequest.LicenseServerSettings` (@required) — "The settings include the type of license server and the Secrets Manager secret that enables administrators to add or remove users associated with the license server"; `RdsSalSettings$RdsSalCredentialsProvider` (@required) — "a reference to the credentials provider that's used for RDS license server user administration"; `SecretsManagerCredentialsProvider$SecretId` (@length min 1) — "The ID of the Secrets Manager secret that contains credentials"; `LicenseServerEndpointProvisioningStatus` values include PROVISIONING_FAILED; the operation declares ValidationException (400) and ResourceNotFoundException (404) (smithy_query shape=CreateLicenseServerEndpointRequest/RdsSalSettings/SecretsManagerCredentialsProvider). Handler: `src/ops/create_license_server_endpoint.rs` resolves ONLY `input.identity_provider_arn` (`state.live_identity_provider`) and then stores `settings: input.license_server_settings.clone()` with `provisioning_status: PROVISIONING` + `provisioned_due_at`, never touching the credentials provider; the crate has no dependency on the Secrets Manager service crate. Tests: `tests/all/license_server_endpoints.rs::a_create_outside_the_modelled_constraints_is_refused` covers only modelled constraints and `an_endpoint_for_an_unregistered_provider_is_refused` covers only the provider ARN; no test asserts anything about the SecretId.

## Steps to Reproduce

1. RegisterIdentityProvider for an Active Directory provider with Product=REMOTE_DESKTOP_SERVICES and note the IdentityProviderArn.
2. Without creating any Secrets Manager secret, call CreateLicenseServerEndpoint with that IdentityProviderArn and LicenseServerSettings={ServerType:RDS_SAL, ServerSettings:{RdsSalSettings:{RdsSalCredentialsProvider:{SecretsManagerCredentialsProvider:{SecretId:"no-such-secret"}}}}}.
3. Observe HTTP 200 with a minted LicenseServerEndpointArn; poll ListLicenseServerEndpoints and observe the endpoint settle to PROVISIONED with no StatusMessage. Expected: the unresolvable credentials secret is refused (ValidationException/ResourceNotFoundException) or the endpoint settles into PROVISIONING_FAILED with a StatusMessage.
4. Create a real secret via secretsmanager:CreateSecret and repeat with its id: the emulator answers identically, so a correct request is indistinguishable from the broken one.
