Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 086f796f1e6ea795d98417e1d50d205591b703fd

# A default policy keeps PolicyDetails.ResourceTypes, so target_resource_types() reports two types and a read-modify-write update of the policy is refused

_Filed: 2026-10-27T18:40:59Z_
_Operation: CreateLifecyclePolicy | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `086f796f1e6ea795d98417e1d50d205591b703fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PolicyDetails$ResourceTypes` is modelled `[Custom snapshot policies only]`, and `ResourceTypeValuesList` is `@length(min: 1, max: 1)` — one type. A DEFAULT policy states its target as `DefaultPolicy` / `PolicyDetails$ResourceType`, which the write path resolves into the record's single `default_policy_type` field.

`CreateLifecyclePolicy` nevertheless accepts `PolicyDetails.ResourceTypes` on a DEFAULT-policy request and persists it verbatim: the `[Default policies only]` refusal is the only kind-restriction the crate enforces and it runs in the opposite direction (`if !is_default`), `materialized_policy_details` returns a default policy's structure untouched, and `DefaultPolicyConfiguration::taken_from_policy_details` moves the six default-only members out of the structure but not `resource_types`. The stored record therefore holds the target type TWICE: once in `default_policy_type` and once inside `policy_details.resource_types`.

Three observable consequences follow from that one stored contradiction:

1. `LifecyclePolicyRecord::target_resource_types()` pushes `default_policy_type` AND extends with `details.resource_types`, so it returns `["VOLUME", "VOLUME"]` for such a policy — a two-element list for a member the model caps at one, and a direct contradiction of the function's own doc ("A DEFAULT policy targets the one type `default_policy_type` holds — the single stored copy of it").
2. `GetLifecyclePolicy` reports a DEFAULT policy whose `PolicyDetails` carries both `ResourceType: "VOLUME"` and `ResourceTypes: ["VOLUME"]`, i.e. a custom-snapshot-only member on a `SIMPLIFIED` configuration.
3. Sending that reported configuration back is REFUSED. `immutable_configuration_violation` compares the stated `ResourceTypes` against `target_resource_types()`, so `["VOLUME"] != ["VOLUME", "VOLUME"]` and the update answers 400 `InvalidRequestException` "The parameter PolicyDetails.ResourceTypes cannot be updated. The policy's target resource type is VOLUME, VOLUME and VOLUME was requested." The service refuses the exact configuration it just reported, and the message states a stored value that cannot exist under the model, breaking read-modify-write for the policy permanently.

The same accumulation makes a DEFAULT policy that holds no `default_policy_type` (created with `PolicyDetails: {"PolicyLanguage": "SIMPLIFIED"}`) silently acquire a target type through `ResourceTypes`: `target_resource_types()` is empty, so the `!stored.is_empty()` guard lets the update through, and afterwards `GET /policies?resourceTypes=INSTANCE` selects a policy whose every read reports no `ResourceType` — the very outcome the helper's doc says the comparison exists to prevent.

## Evidence

The model restricts the member to custom policies and to one element — `smithy_query(query="shape", shape="PolicyDetails")`, member `ResourceTypes`: `"length": {"min": 1, "max": 1}`, doc "<b>[Custom snapshot policies only]</b> The target resource type for snapshot and AMI lifecycle policies. Use <code>VOLUME </code>to create snapshots of individual volumes or use <code>INSTANCE</code> to create multi-volume snapshots from the volumes for an instance."

The create stores a default policy's structure untouched — `services/service-com-amazonaws-dlm-dlm-20180112/src/state.rs:335-344`:
```
pub fn materialized_policy_details(
    mut details: PolicyDetails,
    default_policy: bool,
) -> PolicyDetails {
    if default_policy {
        // A DEFAULT policy's configuration, which carries none of the custom
        // members stamped below.
        details.policy_language = Some(SIMPLIFIED_POLICY_LANGUAGE.to_string());
        return details;
    }
```
and the resolver that moves default-only members out of it does not move `resource_types` — `src/state.rs:446-456`:
```
    pub fn taken_from_policy_details(details: &mut PolicyDetails) -> Self {
        Self {
            resource_type: details.resource_type.take(),
            create_interval: details.create_interval.take(),
            ...
```
so `src/ops/create_lifecycle_policy.rs:386-388` (`let mut policy_details = input.policy_details.map(|details| crate::state::materialized_policy_details(details, is_default));`) and `src/ops/create_lifecycle_policy.rs:443` (`store.insert_policy(&region, record);`) persist a second copy. The only kind-restriction refusal the crate runs points the other way — `src/ops/create_lifecycle_policy.rs:235-243`:
```
        if !is_default {
            let mut refused = stated_in_body.stated_names("");
            refused.extend(
                crate::state::DefaultPolicyOnlyParameters::in_policy_details(
                    input.policy_details.as_ref(),
                )
                .stated_names(crate::state::POLICY_DETAILS_PREFIX),
            );
```

The doc that the resulting read contradicts — `src/state.rs:639-655`:
```
    /// Every resource type this policy targets, for the `resourceTypes` filter.
    ///
    /// A DEFAULT policy targets the one type [`Self::default_policy_type`] holds
    /// — the single stored copy of it, which is also the one every read reports —
    /// and a CUSTOM policy the types its configuration's `ResourceTypes` names.
    pub fn target_resource_types(&self) -> Vec<String> {
        let mut types: Vec<String> = Vec::new();
        if let Some(default_type) = self.default_policy_type.as_ref() {
            types.push(default_type.clone());
        }
        if let Some(details) = self.policy_details.as_ref() {
            if let Some(resource_types) = details.resource_types.as_ref() {
                types.extend(resource_types.iter().cloned());
            }
        }
        types
    }
```

The comparison that then refuses the service's own output, whose comment asserts the one-element invariant that no longer holds — `src/state.rs:1239-1251`:
```
    if let Some(stated) = details.resource_types.as_ref() {
        // `ResourceTypeValuesList` is `@length(min: 1, max: 1)`, so the member
        // states exactly one type and the lists are compared as they stand.
        let stored = policy.target_resource_types();
        if !stored.is_empty() && *stated != stored {
            return Some(configuration_member_cannot_be_updated(
                RESOURCE_TYPES_MEMBER,
                "target resource type",
                &stated.join(", "),
                &stored.join(", "),
            ));
        }
    }
```
reached from `src/ops/update_lifecycle_policy.rs:279-285`, under a comment stating the opposite outcome (`src/ops/update_lifecycle_policy.rs:271-275`): "A configuration restating the value the policy already holds for one of them states the configuration it has, so it is accepted and changes nothing".

The read path keeps the stray member — `src/state.rs:545-560` (`effective_policy_details`) clones the stored structure and overwrites only `policy_language`, `policy_type`, `resource_type`, the two intervals, `copy_tags`, `extend_deletion`, `cross_region_copy_targets` and `exclusions`; `resource_types` is never cleared.

Test coverage: no test sends `ResourceTypes` on a default policy. `tests/all/policy_identity_immutability.rs:208-210` states the split the rows follow — "True when it is a DEFAULT policy, which states its target as `PolicyDetails$ResourceType` rather than as `ResourceTypes`" — and `default_policy_details` (`tests/all/policy_identity_immutability.rs:270-278`) uses only `"ResourceType": target`. Every `ResourceTypes` occurrence under `tests/` sits in a CUSTOM policy's configuration, so this path is unpinned.

## Steps to Reproduce

1. POST /policies (SigV4-signed for dlm) with {"ExecutionRoleArn": "arn:aws:iam::<emulator account>:role/AWSDataLifecycleManagerDefaultRole", "Description": "default volume policy", "State": "ENABLED", "DefaultPolicy": "VOLUME", "PolicyDetails": {"PolicyLanguage": "SIMPLIFIED", "ResourceTypes": ["VOLUME"]}} — observe HTTP 200 and note the PolicyId (the create accepts the [Custom snapshot policies only] member on a SIMPLIFIED configuration)
2. GET /policies/{PolicyId} — Policy.PolicyDetails reports BOTH "ResourceType": "VOLUME" and "ResourceTypes": ["VOLUME"], so the default policy reports a custom-only member and the target type twice
3. PATCH /policies/{PolicyId} with {"PolicyDetails": <exactly the structure step 2 reported>} — observe HTTP 400 InvalidRequestException with Message "The parameter PolicyDetails.ResourceTypes cannot be updated. The policy's target resource type is VOLUME, VOLUME and VOLUME was requested.": the configuration the service itself reported is refused, naming a two-element stored value that ResourceTypeValuesList's @length(min: 1, max: 1) forbids, and the policy can no longer be updated through a read-modify-write
4. Repeat step 1 with "PolicyDetails": {"PolicyLanguage": "SIMPLIFIED"} and no DefaultPolicy, then PATCH /policies/{PolicyId} with {"PolicyDetails": {"PolicyLanguage": "SIMPLIFIED", "ResourceTypes": ["INSTANCE"]}} — observe HTTP 200, then GET /policies?resourceTypes=INSTANCE returns the policy while GET /policies/{PolicyId} reports no PolicyDetails.ResourceType at all, so the list filter selects it under a type no read of it reports
