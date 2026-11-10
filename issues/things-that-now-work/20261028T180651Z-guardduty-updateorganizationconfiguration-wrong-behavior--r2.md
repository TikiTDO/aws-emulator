Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 5cb591631d698c9098c00693287b08686d77b0d1

# UpdateOrganizationConfiguration never applies an ALL auto-enablement to the organization's existing member accounts, unlike CreateMembers which calls the crate's own auto-enable writer

_Filed: 2026-10-28T18:06:51Z_
_Operation: UpdateOrganizationConfiguration | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `5cb591631d698c9098c00693287b08686d77b0d1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model documents `ALL` (both for `autoEnableOrganizationMembers` and for a per-feature `OrganizationFeatureConfiguration.AutoEnable`) as covering the accounts that are ALREADY in the organization, not just the ones that join later — that is exactly what distinguishes `ALL` from `NEW`, and the model even says how long propagating it to the member accounts takes.

The handler writes only the detector's own `organization` record: `configuration.auto_enable_members`, `apply_org_data_sources`, `apply_org_feature_configurations`. It never touches `detector.members`, and nothing else in the crate propagates the configuration afterwards (there is no due-instant for it, so no later read settles one either). The result is that switching an opt-in feature's auto-enable to `ALL` leaves every existing member's stored feature list exactly as it was, so `GetMemberDetectors` reports the feature `DISABLED` while `DescribeOrganizationConfiguration` reports `autoEnable: ALL` for the same organization.

This is not a capability the crate lacks: `crate::state::apply_organization_auto_enable(member, &organization, now)` is the member-side writer for precisely this rule, and `CreateMembers` calls it for NEW and for EXISTING members. So two paths in this crate disagree about the same fact: an idempotent re-`CreateMembers` of an account the organization already holds applies the auto-enablement, while the operation that SETS the auto-enablement does not. The fix is to run the existing helper over the detector's associated members on the write path, under the instant this handler already ticks.

## Evidence

MODEL — smithy_query(query="doc",shape="UpdateOrganizationConfigurationRequest",member="AutoEnableOrganizationMembers"): "ALL: Indicates that all accounts in the organization have GuardDuty enabled automatically. This includes NEW accounts that join the organization and accounts that may have been suspended or removed from the organization in GuardDuty." / "It may take up to 24 hours to update the configuration for all the member accounts." and, for the opposite direction, "When you update the auto-enable setting from ALL or NEW to NONE, this action doesn't disable the corresponding option for your existing accounts." The same sentences are modelled per feature — smithy_query(query="doc",shape="OrganizationFeatureConfiguration",member="AutoEnable"): "ALL: Indicates that all accounts in the organization have the feature enabled automatically. This includes NEW accounts that join the organization ... It may take up to 24 hours to update the configuration for all the member accounts."

HANDLER — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/update_organization_configuration.rs:225-242, the whole write:
```
        if let Some(value) = members {
            configuration.auto_enable_members = value.to_string();
        } else if let Some(flag) = input.auto_enable {
            configuration.auto_enable_members =
                crate::state::org_status_for_flag(flag).to_string();
        }
        // The deprecated `DataSources` view is applied first so an explicit
        // `Features` entry for the same switch wins.
        crate::state::apply_org_data_sources(configuration, input.data_sources.as_ref());
        crate::state::apply_org_feature_configurations(configuration, input.features.as_ref());
        ...
        detector.updated_at_millis = now;
```
No member record is read or written on any path (the file's only `detector` uses are `detector.organization` at :207 and `detector.updated_at_millis` at :242).

THE HELPER THIS PATH SKIPS — src/state.rs:5854-5858: `pub fn apply_organization_auto_enable(\n    member: &mut MemberRecord,\n    organization: &OrganizationConfiguration,\n    now: i64,\n) {` with its doc at src/state.rs:5837-5843: "Write the organization's auto-enable settings onto one member account's stored configuration. / A feature the organization auto-enables ... is ENABLED for the account, together with every additional configuration that feature auto-enables."

THE SIBLING THAT CALLS IT — src/ops/create_members.rs:274-280: "// An EXISTING member takes the organization's auto-enable / // settings too, which is what `irrespective of the accounts / // being new or existing members` says." followed by `crate::state::apply_organization_auto_enable(existing, &organization, now);`, and again for a new member at create_members.rs:312-316.

TEST COVERAGE — tests/all/member_organization_auto_enable.rs:291-333 (`an_auto_enabled_feature_reaches_an_existing_member_too`) only observes the propagation AFTER a second `CreateMembers`: it calls `update_ok_in(... organization_body(&[(feature, auto_enable)]))` at :316-322 and then, at :326, "// The same account again: \"irrespective of the accounts being new or / // existing members\". record_member(&router, &detector_id, &account, email, &case).await;" before reading the member. No test reads the member between the update and that second create, so nothing pins the current non-propagation, and applying the helper in the update would leave this test passing.

## Steps to Reproduce

1. CreateDetector in us-east-1 (enable=true) and note the detectorId.
2. CreateMembers with accountDetails=[{accountId: 111122223333, email: member@example.com}] → 200, unprocessedAccounts=[].
3. GetMemberDetectors accountIds=[111122223333] → the opt-in feature (e.g. EBS_MALWARE_PROTECTION) reports DISABLED, as a fresh member does.
4. UpdateOrganizationConfiguration with autoEnableOrganizationMembers=ALL and features=[{name: EBS_MALWARE_PROTECTION, autoEnable: ALL}] → 200.
5. DescribeOrganizationConfiguration → autoEnableOrganizationMembers=ALL and the feature's autoEnable=ALL.
6. GetMemberDetectors accountIds=[111122223333] again. Expected per the model ("all accounts in the organization have the feature enabled automatically"): the existing member's feature is ENABLED. Actual: still DISABLED — the organization record changed and no member did.
7. Call CreateMembers again for the same account with the same email (a no-op for the relationship) and repeat GetMemberDetectors: now the feature reports ENABLED, because create_members.rs:280 runs `apply_organization_auto_enable` that the update never ran.
