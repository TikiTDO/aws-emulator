Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: e113b6e45c2c86d79f2618efd25a6c62e8ae6901

# The "but not both" refusal never compares the two places: a default-only body parameter is refused whenever PolicyDetails is present, even when the structure does not state it

_Filed: 2026-10-28T15:50:30Z_
_Operation: UpdateLifecyclePolicy | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `e113b6e45c2c86d79f2618efd25a6c62e8ae6901`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule `CreateLifecyclePolicy` documents — "If you create a default policy, you can specify the request parameters either in the request body, or in the PolicyDetails request structure, but not both" — is enforced in `UpdateLifecyclePolicy::handle` from the REQUEST-BODY half alone. The handler builds `duplicated` from `input.default_policy_only_body_parameters().stated_names("")` and refuses as soon as that list is non-empty and a `PolicyDetails` is present, without ever asking what the structure states: `crate::state::DefaultPolicyOnlyParameters::in_policy_details(..)` — the helper the crate already keeps for the structure half, and which the SAME rule's resource-type arm in `CreateLifecyclePolicy::handle` does consult — is never called on this path.

Consequence: a request that states each `[Default policies only]` parameter in exactly ONE place is refused with 400. For a default policy, `PATCH /policies/{id}` with `{"CopyTags": true, "PolicyDetails": {"PolicyLanguage": "SIMPLIFIED", "ResourceType": "VOLUME"}}` answers `InvalidRequestException` whose Message asserts a fact the request contradicts ("a default policy specifies CopyTags either in the request body or in PolicyDetails, but not both" — the structure states no `CopyTags`) and whose `MutuallyExclusiveParameters` lists `["PolicyDetails", "CopyTags"]`, i.e. names as "parameters that cannot be provided together" a pair the crate itself combines elsewhere: `CreateLifecyclePolicy` accepts the body-level default-policy parameter `DefaultPolicy` beside a `PolicyDetails` structure (pinned by a test) and refuses the pair only when the structure ALSO states the same member (`ResourceType`). One rule, two implementations — per-member for the resource type, per-request for the other six parameters — so two requests that are alike in the relevant respect (one body parameter beside a structure that does not state it) get opposite answers.

The mismatch also leaves the resolver the crate wrote for this rule unreachable: `DefaultPolicyConfiguration::or` exists to decide which side stated each member, but under the coarse refusal no accepted request ever states members on both sides, so its fallback never merges anything.

The defect is the refusal being raised without comparing the two places (and reporting a `Message`/`MutuallyExclusiveParameters` that misdescribe the request); the same block is written a second time in `CreateLifecyclePolicy::handle`, so a fix belongs in both call sites.

## Evidence

The update refuses from the body half alone — `services/service-com-amazonaws-dlm-dlm-20180112/src/ops/update_lifecycle_policy.rs:260-270`:
```
        if policy.default_policy && input.policy_details.is_some() {
            let duplicated = input.default_policy_only_body_parameters().stated_names("");
            if !duplicated.is_empty() {
                return Err(
                    UpdateLifecyclePolicyErrorModeled::InvalidRequestException(
                        crate::state::configuration_stated_twice(&duplicated),
                    )
                    .into(),
                );
            }
        }
```
The SAME rule's other arm, in the sibling handler, DOES compare the two places — `src/ops/create_lifecycle_policy.rs:208-222`:
```
        if is_default && input.policy_details.is_some() {
            let mut duplicated = stated_in_body.stated_names("");
            // The resource type is one of those parameters: `DefaultPolicy` and
            // `PolicyDetails$ResourceType` carry the same documentation, so a
            // request naming both states it in both places. The refusal names it
            // by the BODY member the caller used, the alternative to the
            // structure.
            if input.default_policy.is_some()
                && crate::state::DefaultPolicyOnlyParameters::in_policy_details(
                    input.policy_details.as_ref(),
                )
                .resource_type
            {
                duplicated.insert(0, crate::state::DEFAULT_POLICY_MEMBER.to_string());
            }
```
The structure-half helper that the update path never calls — `src/state.rs:1021-1034`:
```
    pub fn in_policy_details(details: Option<&PolicyDetails>) -> Self {
        let Some(details) = details else {
            return Self::default();
        };
        Self {
            resource_type: details.resource_type.is_some(),
            create_interval: details.create_interval.is_some(),
            ...
```
The message and payload the coarse refusal emits — `src/state.rs:1121-1132`:
```
pub fn configuration_stated_twice(body_parameters: &[String]) -> InvalidRequestException {
    let mut parameters = vec![POLICY_DETAILS_MEMBER.to_string()];
    parameters.extend(body_parameters.iter().cloned());
    parameters_cannot_be_combined(
        format!(
            "a default policy specifies {} either in the request body or in \
             {POLICY_DETAILS_MEMBER}, but not both",
            body_parameters.join(", ")
        ),
        parameters,
    )
}
```
and what `MutuallyExclusiveParameters` is supposed to mean — `src/state.rs:962-966`: "`MutuallyExclusiveParameters` is \"the request included parameters that cannot be provided together\", so it lists the REQUEST's own parameters — the names a caller passed to the operation."

The crate states the rule PER MEMBER — `tests/all/default_policy_configuration_precedence.rs:6-10`:
```
//! The rule under test: a default policy's `[Default policies only]`
//! configuration may be stated in the request body or inside the `PolicyDetails`
//! structure, and the two are ALTERNATIVES — one request may not state a member
//! in both places, ...
```
and every case of `an_update_may_not_state_the_configuration_twice` states the member in BOTH places (`tests/all/default_policy_configuration_precedence.rs:581-590`: `patch["PolicyDetails"][member] = value.clone(); patch[member] = value.clone();`), so the one-place-only request is unpinned.

A body-level default-policy parameter beside a `PolicyDetails` is accepted elsewhere in the crate — `tests/all/create_policy_kind_materialization.rs:292-295`:
```
            (
                "naming DefaultPolicy beside a structure stating no language".to_string(),
                json!({"DefaultPolicy": default_type, "PolicyDetails": {}}),
            ),
```
And the resolver written for the rule, whose merge the coarse refusal makes unreachable — `src/state.rs:477-481`: "A request that states the same member in both places is refused ([`configuration_stated_twice`]), so at most one side states each member and the precedence never decides anything a caller can observe." (`DefaultPolicyConfiguration::or`, called only from `src/ops/create_lifecycle_policy.rs:413`).

## Steps to Reproduce

1. POST /policies (SigV4-signed for dlm) with {"ExecutionRoleArn": "arn:aws:iam::<emulator account>:role/AWSDataLifecycleManagerDefaultRole", "Description": "default volume policy", "State": "ENABLED", "DefaultPolicy": "VOLUME", "CreateInterval": 2, "RetainInterval": 9} — HTTP 200, note the PolicyId (a DEFAULT policy whose configuration was stated in the request body)
2. PATCH /policies/{PolicyId} with {"CopyTags": true, "PolicyDetails": {"PolicyLanguage": "SIMPLIFIED", "ResourceType": "VOLUME"}} — the request states CopyTags in exactly one place and the structure states no CopyTags
3. Observe HTTP 400 InvalidRequestException, Code InvalidParameter, Message "a default policy specifies CopyTags either in the request body or in PolicyDetails, but not both" and MutuallyExclusiveParameters ["PolicyDetails", "CopyTags"] — a refusal whose stated reason the request contradicts, produced by src/ops/update_lifecycle_policy.rs:260-270 without consulting DefaultPolicyOnlyParameters::in_policy_details
4. Compare with the per-member arm of the same rule: POST /policies with {"...", "DefaultPolicy": "VOLUME", "PolicyDetails": {}} is accepted (pinned by tests/all/create_policy_kind_materialization.rs:292-295), and is refused only when the structure ALSO states ResourceType (src/ops/create_lifecycle_policy.rs:215-222) — the comparison the other six parameters never get
5. Repeat step 2 against CreateLifecyclePolicy — {"...", "DefaultPolicy": "VOLUME", "CopyTags": true, "PolicyDetails": {"PolicyLanguage": "SIMPLIFIED"}} — and observe the same refusal from the duplicated block at src/ops/create_lifecycle_policy.rs:208-228
