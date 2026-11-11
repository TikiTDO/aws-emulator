Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 7a00204bd93c8ed9dd88a2629fdcaac9e0c865d8

# CreateProbe never checks the MERGED tag set against the per-resource tag ceiling its sibling TagResource enforces

_Filed: 2026-11-08T01:01:22Z_
_Operation: CreateProbe | Class: validation_

> _Note: this issue was identified against the code base at commit `7a00204bd93c8ed9dd88a2629fdcaac9e0c865d8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateProbe` is the only path in this crate that MERGES two caller-supplied tag maps into one resource's single tag map: it unions `input.tags` with `input.probe.tags` and stores the result as the new probe's whole tag map. Each half is individually bounded by the modelled `TagMap` `@length(min: 0, max: 200)` at the request boundary, so the union can carry up to 400 pairs, and the handler applies no count check to it.

The crate already owns the rule for exactly this situation. `crate::state::check_tag_count` judges the MERGED set a write would leave on a resource, and `TagResource` calls it before storing anything; `CreateProbe` calls `check_tag_keys` on both halves (the reserved-prefix rule) but never `check_tag_count`, so the one merge path that can actually overshoot the ceiling is the one path that skips it.

Two observable consequences of the stored over-size map:

1. The probe's `tags` member is projected straight out of the record by `probe_report!`, so `CreateProbeOutput`/`GetProbeOutput`/`GetMonitorOutput.probes[].tags`/`ListTagsForResourceOutput.tags` all report more entries than the modelled `TagMap` maximum the crate's own constant doc calls the most a response can carry.
2. Every later `TagResource` against that probe's ARN is refused with HTTP 400 `ValidationException` — including a write that only OVERWRITES keys the probe already carries, which `check_tag_count` is documented to never refuse — because `stored.len()` alone is already past the bound. The probe becomes permanently untaggable by a state the service itself created.

## Evidence

The merge, with no count check (services/service-com-amazonaws-networkmonitor-networkmonitor/src/ops/create_probe.rs:241-271):
```
        // Both `tags` and `probe.tags` are documented as the pairs assigned to
        // the probe, so the probe gets their union; ...
        let requested_tags: ... = input.tags.unwrap_or_default().into_iter().collect();
        crate::state::check_tag_keys(... member_path("tags") ...)?;
        let probe_tags: ... = input.probe.tags.unwrap_or_default().into_iter().collect();
        crate::state::check_tag_keys(... probe_member_path(... "tags") ...)?;
        let mut tags = requested_tags;
        tags.extend(probe_tags);
        let spec = crate::state::ProbeSpec { ... tags };
```
(`grep check_tag_count` over the whole crate returns only `state.rs` and `tag_resource.rs` — never `create_probe.rs`.)

The sibling that DOES apply the rule to the merged set (services/service-com-amazonaws-networkmonitor-networkmonitor/src/ops/tag_resource.rs:261-272):
```
        // The merged set is what the resource will carry, so it is the set the
        // modelled `TagMap` bound applies to — checked before anything is
        // written, so a refused call stores none of the pairs it carried.
        crate::state::check_tag_count(
            &crate::state::member_path("tags"),
            tags,
            input.tags.keys().map(String::as_str),
        )
        .map_err(validation)?;
```

The helper's own statement of the rule (services/service-com-amazonaws-networkmonitor-networkmonitor/src/state.rs:886-894):
```
/// Refuse a tag map that would leave a resource carrying more pairs than
/// [`MAX_TAGS_PER_RESOURCE`], for every operation that MERGES tags into a
/// resource the store already holds (`TagResource`).
...
/// the ceiling bounds what the resource WILL carry, so
/// the count judged is the MERGED set's
```
and why the number is not an arbitrary emulator cap (state.rs:263-266):
```
/// Its VALUE is the modelled `TagMap`'s own `@length(min: 0, max: 200)`, which
/// is the most pairs any response can carry: a resource holding more could not
/// be reported.
```

The Smithy model gives both halves that same bound, so neither is refused at the boundary: `CreateProbeInput$tags` -> `"tags": {"target": "TagMap", "length": {"min": 0, "max": 200}, "type": "map"}` and `ProbeInput$tags` -> `"tags": {"target": "TagMap", "length": {"min": 0, "max": 200}, "type": "map"}` (smithy_query shape=CreateProbeInput,ProbeInput).

Test coverage: the rule is pinned for `TagResource` only — tests/all/tag_count_ceiling_refusal.rs:11-19 "THE RULE: the per-resource tag maximum is a MODELLED `@length` bound the handler applies itself — to the MERGED set the write would store rather than to the request's own map" — and no test exercises the union. tests/all/resource_tags.rs:511-528 drives `CreateProbe` with `probe.tags` of `many_tags(MAX_TAGS)` (200) but sends NO top-level `tags` member (body at line 531-534), and tests/all/monitor_and_probe.rs:655-679 exercises the union with only three pairs, so nothing justifies or pins the unchecked merge.

## Steps to Reproduce

1. CreateMonitor `{"monitorName":"tag-union"}`.
2. POST /monitors/tag-union/probes with a body whose top-level `tags` carries 200 distinct pairs (`req-000`..`req-199`) and whose `probe.tags` carries 200 further DISTINCT pairs (`prb-000`..`prb-199`), with a valid `sourceArn`, `destination`, `protocol`:`ICMP`, `packetSize`:56. Each map is inside the modelled `TagMap` maximum, so the front door accepts the body and `handle` runs.
3. Observe HTTP 200 and a `tags` map of 400 entries in the response — past the modelled `TagMap` `@length(max: 200)` that state.rs:263-266 calls "the most pairs any response can carry".
4. GET /tags/{probeArn} (ListTagsForResource) and GET /monitors/tag-union: both report the same 400-entry map.
5. POST /tags/{probeArn} (TagResource) with a single pair whose key the probe ALREADY carries — an overwrite, which check_tag_count is documented never to refuse. It is refused with HTTP 400 `ValidationException` "1 validation error detected: Value '400 entries' at 'tags' failed to satisfy constraint: Member must have length less than or equal to 200": the probe the service itself created can no longer be tagged at all.
6. Compare with the same merged count reached through TagResource (tests/all/tag_count_ceiling_refusal.rs), which is refused before anything is stored.
