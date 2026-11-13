Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 3d1fad8f4871dd362a3fc79086918715269856ce

# The ResourceArn-to-tag-map resolution is written out twice, once per borrow kind, so the read and write halves of the tag surface can drift

_Filed: 2026-10-27T23:37:39Z_
_Operation: ListTagsForResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `3d1fad8f4871dd362a3fc79086918715269856ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The whole rule for turning a tagging operation's `ResourceArn` into the tag map it names — parse the ARN, check the partition/Region/account scope it claims, resolve the detector, then dispatch on which of the six `TaggedSubResource` collections holds the sub-resource — is implemented twice in `src/state.rs`: once in `tagged_resource_tags` (the shared borrow, used by `ListTagsForResource`) and once in `tagged_resource_tags_mut` (the mutable borrow, used by `TagResource` and `UntagResource`). The two bodies are line-for-line the same decision tree, differing only in `get`/`get_mut`, `detector_in`/`detector_in_mut` and `&`/`&mut`.

The doc comment on the read half already asserts the invariant the duplication threatens: "Resolution is exactly [`Self::tagged_resource_tags_mut`]'s — same scope check, same records — so a report and a write cannot disagree about which ARN names which resource." Nothing in the code enforces that; a seventh taggable sub-resource added to `TAGGED_SUB_RESOURCE_SEGMENTS`, or a change to the scope check, has to be made in two places, and if only one copy is updated then `TagResource` will tag a resource `ListTagsForResource` refuses (or vice versa) — precisely the disagreement the comment says cannot happen.

The fix is a single resolution seam the two borrow kinds share (for example one function that resolves the `TaggedResource` plus the scope check, with the per-collection dispatch expressed once). Every response and every existing test stays unchanged: the two functions already return the same resource for the same ARN.

## Evidence

Copy 1 (shared borrow) — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/state.rs:8021-8057:
```
    /// Resolution is exactly [`Self::tagged_resource_tags_mut`]'s — same scope
    /// check, same records — so a report and a write cannot disagree about which
    /// ARN names which resource; ...
    pub fn tagged_resource_tags(&self, region: &str, arn: &str) -> Option<&BTreeMap<String, String>> {
        let resource = parse_tagged_resource_arn(arn)?;
        if resource.partition != PARTITION
            || resource.region != region
            || resource.account_id != ::engine::emulator_account_id()
        { return None; }
        let detector = self.detector_in(region, &resource.detector_id)?;
        let Some((sub_resource, id)) = &resource.sub_resource else { return Some(&detector.tags); };
        match sub_resource {
            TaggedSubResource::Filter => detector.filters.get(id).map(|f| &f.tags),
            TaggedSubResource::IpSet => detector.ip_sets.get(id).map(|s| &s.tags),
            TaggedSubResource::ThreatIntelSet => detector.threat_intel_sets.get(id).map(|s| &s.tags),
            TaggedSubResource::TrustedEntitySet => detector.trusted_entity_sets.get(id).map(|s| &s.tags),
            TaggedSubResource::ThreatEntitySet => detector.threat_entity_sets.get(id).map(|s| &s.tags),
            TaggedSubResource::PublishingDestination => detector.publishing_destinations.get(id).map(|d| &d.tags),
        }
    }
```

Copy 2 (mutable borrow) — src/state.rs:8073-8107:
```
    pub fn tagged_resource_tags_mut(&mut self, region: &str, arn: &str) -> Option<&mut BTreeMap<String, String>> {
        let resource = parse_tagged_resource_arn(arn)?;
        if resource.partition != PARTITION
            || resource.region != region
            || resource.account_id != ::engine::emulator_account_id()
        { return None; }
        let detector = self.detector_in_mut(region, &resource.detector_id)?;
        let Some((sub_resource, id)) = &resource.sub_resource else { return Some(&mut detector.tags); };
        match sub_resource {
            TaggedSubResource::Filter => detector.filters.get_mut(id).map(|f| &mut f.tags),
            TaggedSubResource::IpSet => detector.ip_sets.get_mut(id).map(|s| &mut s.tags),
            TaggedSubResource::ThreatIntelSet => detector.threat_intel_sets.get_mut(id).map(|s| &mut s.tags),
            TaggedSubResource::TrustedEntitySet => detector.trusted_entity_sets.get_mut(id).map(|s| &mut s.tags),
            TaggedSubResource::ThreatEntitySet => detector.threat_entity_sets.get_mut(id).map(|s| &mut s.tags),
            TaggedSubResource::PublishingDestination => detector.publishing_destinations.get_mut(id).map(|d| &mut d.tags),
        }
    }
```

The six-way dispatch is driven by one table that already lives once — src/state.rs:7903-7913 `pub const TAGGED_SUB_RESOURCE_SEGMENTS: [(&str, TaggedSubResource); 6] = [ ("filter", ...), ("ipset", ...), ("threatintelset", ...), ("trustedentityset", ...), ("threatentityset", ...), ("publishingDestination", ...) ];` — so the walk over it is what is duplicated, not the catalogue.

Callers: src/ops/list_tags_for_resource.rs:167 `state.tagged_resource_tags(&region, &input.resource_arn)`; src/ops/tag_resource.rs:162 and src/ops/untag_resource.rs:157 `state.tagged_resource_tags_mut(&region, &input.resource_arn)`.

Existing tests that cover both halves and must stay green unchanged: tests/all/resource_tag_listing.rs, tests/all/resource_tag_addition.rs, tests/all/resource_tag_removal.rs and tests/all/tag_map_projection.rs.

## Steps to Reproduce

1. Open services/service-com-amazonaws-guardduty-guarddutyapiservice/src/state.rs and read `tagged_resource_tags` (lines 8025-8057) and `tagged_resource_tags_mut` (lines 8073-8107) side by side: the ARN parse, the three-part scope check, the detector lookup and the six-arm sub-resource dispatch appear in both, identical apart from the borrow kind.
2. Note the doc comment at src/state.rs:8021-8024 asserting the two resolve identically ("so a report and a write cannot disagree about which ARN names which resource") — an invariant held only by hand.
3. Factor the resolution into one seam the two borrow kinds share, keeping both public signatures.
4. Run tests/all/resource_tag_listing.rs, resource_tag_addition.rs, resource_tag_removal.rs and tag_map_projection.rs: every response and every assertion must be unchanged.
