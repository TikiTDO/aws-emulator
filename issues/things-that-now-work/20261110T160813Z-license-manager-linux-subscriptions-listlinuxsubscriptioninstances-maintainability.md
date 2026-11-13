Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 40a92b4af209b70ba6c416e4aad3621125e9576d

# Deduplicate the "does this account discover this Region right now" rule, written twice in the state module

_Filed: 2026-11-10T16:08:13Z_
_Operation: ListLinuxSubscriptionInstances | Class: maintainability_

> _Note: this issue was identified against the code base at commit `40a92b4af209b70ba6c416e4aad3621125e9576d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The state module states the same rule twice, in two adjacent functions: "an account discovers exactly the `SourceRegions` its settings name while `LinuxSubscriptionsDiscovery` is Enabled, and NO Region while it is not".

* `discovered_regions()` matches on the settings singleton, gates on `linux_subscriptions_discovery == DISCOVERY_ENABLED`, and collects `source_regions`.
* `discovers_region(region)` matches on the settings singleton AGAIN, gates on `linux_subscriptions_discovery == DISCOVERY_ENABLED` AGAIN, and scans the same `source_regions` for one entry.

The second is exactly `discovered_regions().contains(region)` — same settings branch, same enabled gate, same member — so the enabled gate and the Region membership are each implemented twice and the copies can drift.

That drift is not cosmetic, because the two copies decide the SAME discovery pass from opposite ends and are expected to agree: the per-hop and per-write gate goes through `discovers_region` (src/state/discovery.rs:229, src/state.rs:530, src/state.rs:555) while the pruning step that ends every pass goes through `discovered_regions` (via `retain_discovered_regions`, src/state.rs:596-599) and the pass's plan goes through it too (`discovery_plan`, src/state.rs:650). If a future condition (an `OrganizationIntegration` term, say) were added to one copy only, a pass would write records its own pruning step immediately drops, or keep records no hop refreshes — the exact failure modes two already-fixed issues in this backlog were about.

Fix shape, with no response and no test changes: let `discovers_region` delegate to the one computation (`self.discovered_regions().contains(region)`), so the enabled gate and the Region set are stated once. Set membership and the `any()` scan select the same Regions, so every response and every existing assertion is unchanged.

## Evidence

Copy 1 — services/service-com-amazonaws-licensemanagerlinuxsubscriptions-licensemanagerlinuxsubscriptions/src/state.rs:602-621:
```
    /// The Regions this account discovers right now: the `SourceRegions` its settings name
    /// while `LinuxSubscriptionsDiscovery` is [`DISCOVERY_ENABLED`], and NO Region while it
    /// is not — nor while the account has no settings at all.
    pub fn discovered_regions(&self) -> BTreeSet<String> {
        match self.service_settings.as_ref() {
            Some(settings) if settings.linux_subscriptions_discovery == DISCOVERY_ENABLED => {
                settings
                    .discovery_settings
                    .source_regions
                    .iter()
                    .cloned()
                    .collect()
            }
            _ => BTreeSet::new(),
        }
    }
```

Copy 2 — the same branch, the same gate, the same member, 4 lines later — src/state.rs:623-636:
```
    /// Whether this account discovers `region` right now — the one question each hop of a
    /// pass and each of its writes settles against the settings as they are then.
    pub fn discovers_region(&self, region: &str) -> bool {
        match self.service_settings.as_ref() {
            Some(settings) if settings.linux_subscriptions_discovery == DISCOVERY_ENABLED => {
                settings
                    .discovery_settings
                    .source_regions
                    .iter()
                    .any(|source| source == region)
            }
            _ => false,
        }
    }
```
The doc of copy 2 calls itself "the ONE question each hop of a pass and each of its writes settles against the settings", while copy 1 answers that same question for the plan and for the pruning step.

The two copies decide one pass from both ends — src/state.rs:530 (write gate, copy 2):
```
        if !self.discovers_region(region) {
            return;
        }
```
src/state.rs:596-599 (pruning, copy 1):
```
    pub fn retain_discovered_regions(&mut self) {
        let discovered = self.discovered_regions();
        self.discovered_instances
            .retain(|_, record| discovered.contains(&record.region));
```
and src/state/discovery.rs:229 (hop gate, copy 2):
```
        let discovered_now = state.lock().discovers_region(region);
```

Existing coverage that must stay green and unchanged: tests/all/discovery_switch_off.rs ("an account discovers exactly the `SourceRegions` its settings name while `LinuxSubscriptionsDiscovery` is `Enabled`, and NO Region while it is not", `switching_discovery_off_stops_both_lists_reporting_what_a_pass_had_found`, `an_account_with_no_settings_at_all_discovers_no_region`) and tests/all/discovery_pass_settings_change.rs ("every hop a discovery pass makes and every write it lands is decided by the account's `LinuxSubscriptionsDiscovery` and `SourceRegions` AT THAT INSTANT", `a_settings_change_landing_mid_pass_decides_what_that_pass_writes_and_keeps`).

## Steps to Reproduce

1. Open services/service-com-amazonaws-licensemanagerlinuxsubscriptions-licensemanagerlinuxsubscriptions/src/state.rs at line 602 and read the two functions that follow.
2. Observe that `discovered_regions` (602-621) and `discovers_region` (623-636) both match on `self.service_settings`, both gate on `linux_subscriptions_discovery == DISCOVERY_ENABLED`, and both read `discovery_settings.source_regions`: `discovers_region(r)` is `discovered_regions().contains(r)` written out a second time.
3. Note the callers sit on opposite ends of one discovery pass: src/state/discovery.rs:229 and src/state.rs:530/555 go through `discovers_region`, while src/state.rs:596-599 (`retain_discovered_regions`) and src/state.rs:650 (`discovery_plan`) go through `discovered_regions`.
4. Add a term to the enabled gate in ONE of the two functions (for example require `OrganizationIntegration == Enabled`) and observe nothing in the crate forces the other copy to follow, so the pass's write gate and its pruning step can disagree about the same Region — the pass then writes records its own final pruning drops.
