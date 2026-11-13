Issue-Severity: LOW
Issue-Gap-Class: maintainability

# The Connect-instance registry gate is implemented twice, once per ConnectCampaigns API version

_Operation: CreateCampaign | Class: maintainability_

## Description

Both ConnectCampaigns crates gate on the same fact — whether the account's Amazon
Connect instance registry holds the `connectInstanceId` a request names — and each
states that rule in its own `state.rs`:

* `services/service-com-amazonaws-connectcampaigns-amazonconnectcampaignservice/src/state.rs`
  — `connect_instance_refusal`, `InstanceRefusal`, `INSTANCE_NOT_USABLE_MESSAGE`,
  `INSTANCE_REGISTRY_UNAVAILABLE_MESSAGE`
* `services/service-com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2/src/state.rs`
  — `connect_instance_exists`, `connect_instance_registry_refusal`,
  `InstanceRegistryRefusal`, `INSTANCE_NOT_FOUND_MESSAGE`,
  `INSTANCE_REGISTRY_UNAVAILABLE_MESSAGE`

The two copies read the SAME store through the shared engine
(`service_com_amazonaws_connect_amazonconnectservice::state::service_state`), draw the
same distinction between "the account holds no such instance" (a request fault) and "the
registry could not be read at all" (a server fault), carry the same lock-order rule, and
carry the same refusal sentence — "Connect Instance does not exist or is not enabled to
use Campaigns." — as two separate string constants. The test fixture that writes a
registry record (`register_connect_instance` / `deregister_connect_instance` and the
`Harness` around them, including the full `connect_state::InstanceRecord` literal) is
duplicated the same way in the two crates' `tests/all/common.rs`.

What is NOT shared, and must stay per crate, is the REFUSAL each API version answers
with: the v1 recording carries HTTP 400 `ValidationException` for this class and the V2
recording HTTP 404 `ResourceNotFoundException`, so each crate maps the shared outcome to
the error its own model and recording name.

The fix is response-preserving: hoist the registry READ and its two outcomes into
`services/common` (beside `response_media_type`), leave each crate's own modelled
refusal and message where they are, and give the two test suites one fixture for the
`InstanceRecord` they both write — so the two crates cannot drift on the record's shape
or on the absent-instance vs unreachable-registry distinction. Add one test per former
copy that drives the shared read through an operation of that crate
(`CreateCampaign` in v1, `StartInstanceOnboardingJob` in V2).

## Evidence

`CreateCampaign` of the v1 crate and `CreateCampaign` / `StartInstanceOnboardingJob` /
the instance-integration operations of the V2 crate each call their own crate's copy of
the gate; a grep for `INSTANCE_REGISTRY_UNAVAILABLE_MESSAGE` finds the same constant
declared in both `state.rs` files.

## Steps to Reproduce

1. Read the two `state.rs` spans listed above and compare them line by line.
2. Compare `tests/all/common.rs` in the two crates.
3. Observe that one rule, one registry read and one fixture are stated twice.
