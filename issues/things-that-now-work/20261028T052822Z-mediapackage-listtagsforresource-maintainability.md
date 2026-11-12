Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 37884bada2128c91024def86659c4326af7ab1cb

# The ARN-to-tag-map resolution rule is written out twice in state.rs, once per borrow kind

_Filed: 2026-10-28T05:28:22Z_
_Operation: ListTagsForResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `37884bada2128c91024def86659c4326af7ab1cb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ComAmazonawsMediapackageMediapackageState::tags` and `::tags_mut` implement the same rule twice: match the `TaggedResource` kind, look the record up in the request's Region by id, keep it only when the stored record's own `arn` equals the `ResourceArn` the caller sent, and project that record's tag map. The two copies differ only in the borrow kind (`channel`/`origin_endpoint` versus `channel_mut`/`origin_endpoint_mut`, `&record.tags` versus `&mut record.tags`). `tags_mut`'s own documentation already says it is "Scoped exactly as [`tags`](Self::tags) is" — a promise the compiler does not keep, so adding a third taggable resource kind, or changing the ARN-match rule, has to be done identically in two places or the read path and the write path will disagree about which resource an ARN resolves to.

The rule can be stated once — e.g. by having the two methods share one resolution of (kind, id) -> the record's key, or by generating both through one macro/helper over the borrow kind — leaving every response byte and every existing test unchanged.

## Evidence

First copy, `services/service-com-amazonaws-mediapackage-mediapackage/src/state.rs:2209-2225`:

    pub fn tags(
        &self,
        region: &str,
        arn: &str,
        target: &TaggedResource,
    ) -> Option<&BTreeMap<String, String>> {
        match target {
            TaggedResource::Channel(id) => self
                .channel(region, id)
                .filter(|record| record.arn == arn)
                .map(|record| &record.tags),
            TaggedResource::OriginEndpoint(id) => self
                .origin_endpoint(region, id)
                .filter(|record| record.arn == arn)
                .map(|record| &record.tags),
        }
    }

Second copy, `src/state.rs:2231-2247`:

    pub fn tags_mut(
        &mut self,
        region: &str,
        arn: &str,
        target: &TaggedResource,
    ) -> Option<&mut BTreeMap<String, String>> {
        match target {
            TaggedResource::Channel(id) => self
                .channel_mut(region, id)
                .filter(|record| record.arn == arn)
                .map(|record| &mut record.tags),
            TaggedResource::OriginEndpoint(id) => self
                .origin_endpoint_mut(region, id)
                .filter(|record| record.arn == arn)
                .map(|record| &mut record.tags),
        }
    }

The doc comment that already claims the two are one rule, `src/state.rs:2230`: "Scoped exactly as [`tags`](Self::tags) is."

Callers: `src/ops/list_tags_for_resource.rs:174` (`state.tags(...)`), `src/ops/tag_resource.rs:194` and `src/ops/untag_resource.rs:199` (`state.tags_mut(...)`).

Test coverage that must stay unchanged: `tests/all/resource_tags.rs` (`list_tags_for_resource_reports_the_tag_set_the_create_stored`, `list_tags_for_resource_reports_what_tag_and_untag_left_behind`, `listing_the_tags_of_an_arn_this_service_cannot_resolve_is_refused`, `a_deleted_resource_has_no_tag_set_to_report`), `tests/all/reserved_and_malformed_tags.rs`, `tests/all/region_isolation.rs` and `tests/all/mutation_persistence.rs`.

## Steps to Reproduce

1. Open services/service-com-amazonaws-mediapackage-mediapackage/src/state.rs and read `tags` at line 2209 and `tags_mut` at line 2231.
2. Observe the same per-kind lookup plus `record.arn == arn` filter spelled out twice, differing only in borrow kind, while `tags_mut`'s doc asserts it is "Scoped exactly as tags is".
3. Note that adding a third `TaggedResource` variant (e.g. a harvest job) or changing the ARN-match rule requires an identical edit in both bodies, with nothing failing if only one is changed.
