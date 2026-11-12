Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 9cdb13cb194e5f529c19ad9711760ae55b49f7ec

# TagResource and UntagResource rewrite an entity's tag set without restamping the record's LastModifiedDate, which the change-set tag write does

_Filed: 2026-11-12T19:41:34Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `9cdb13cb194e5f529c19ad9711760ae55b49f7ec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

An entity record carries one `tags` field and one `last_modified_date` field. Two paths in this crate write that `tags` field:

1. `apply_changes` (the `StartChangeSet` path): it restamps `last_modified_date` and only then merges the change's `EntityTags` into the record — so a change whose ONLY effect is tagging the entity moves its `LastModifiedDate`.
2. `TagResource` / `UntagResource`: they take the same `Vec<Tag>` through `resource_tags_mut` and overwrite it, and touch nothing else. `LastModifiedDate` keeps the instant of the last change set.

So after `TagResource` has changed what `ListTagsForResource` reports for an entity, `DescribeEntity`, `BatchDescribeEntities` and `ListEntities` all still report the older instant in a member documented as "The last time the entity was modified" — and the same entity, tagged with the same key and value through a change set instead, reports a fresh one. The two tag-write paths of this one crate disagree about whether tagging an entity modifies it.

The gap has a second, observable consequence inside this crate: `LastModifiedDate` is a filter and sort attribute of `ListEntities` (`ENTITY_SORT_BY[0]`, the default `Sort$SortBy`, and the `LastModifiedDate` `DateRange` of every per-type `*Filters` shape), so a `DateRange` window that should include a just-tagged entity excludes it, and the default ordering does not move it.

`UntagResource` has the identical hole on the identical field; both are the same defect and the same fix.

## Evidence

MODEL — the member that goes stale (smithy_query query="shape" shape="EntityDetail"):
```
"LastModifiedDate": {"target": "DateTimeISO8601", ..., "doc": "<p>The last time the entity was modified.</p>"}
```
and (smithy_query query="doc" shape="DescribeEntityResponse" member="LastModifiedDate"): "The last modified date of the entity, in ISO 8601 format (2018-02-27T13:45:22Z)."

SIDE 1 — the change-set tag write RESTAMPS it. services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/state.rs:2128-2130 (`apply_changes`):
```
                entry.revision = entry.revision.saturating_add(1);
                entry.last_modified_date = stamp.clone();
                merge_tags(&mut entry.tags, change.entity_tags);
```
under its own doc, src/state.rs:2086-2088: "A change naming an existing entity mutates that record and bumps its revision". Nothing in that block requires the change to carry details: a change carrying `EntityTags` alone reaches these three lines and moves `last_modified_date`.

SIDE 2 — the tagging surface does NOT. src/ops/tag_resource.rs:194-222:
```
        let Some(held) = store.resource_tags_mut(&resource) else {
            return Err(TagResourceErrorModeled::ResourceNotFoundException(
                crate::state::resource_not_found(&resource),
            )
            .into());
        };
        ...
        let mut merged = held.clone();
        crate::state::merge_tags(&mut merged, input.tags);
        ...
        *held = merged;

        Ok(TagResourceOutput {})
```
and src/ops/untag_resource.rs:198-211:
```
        let Some(held) = store.resource_tags_mut(&resource) else { ... };
        ...
        held.retain(|tag| !input.tag_keys.contains(&tag.key));

        Ok(UntagResourceOutput {})
```
Both reach the record through the accessor that hands out ONLY the tags — src/state.rs:1922-1934:
```
    /// Those same tags, for writing.
    pub fn resource_tags_mut(&mut self, resource: &CatalogResource) -> Option<&mut Vec<Tag>> {
        match resource {
            CatalogResource::Entity { entity_type, id } => self
                .entities
                .get_mut(id)
                .filter(|held| &held.entity_type == entity_type)
                .map(|held| &mut held.tags),
```
so `last_modified_date` is not even reachable from the tagging handlers.

THE FIELD BOTH PATHS SHARE — src/state.rs:1102-1106 (`EntityRecord`): `pub last_modified_date: String,` beside `pub tags: Vec<Tag>,`.

WHAT ELSE READS IT — src/state.rs:1317 (`attribute_values`): `"LastModifiedDate" => return vec![self.last_modified_date.clone()],`, which is what `ListEntities` filters (`EntityCriterion::matches`, src/state.rs:1429-1442) and sorts (`attribute_sort_key`, src/state.rs:1340) on; `ENTITY_SORT_BY` (src/state.rs:233) makes it the default `Sort$SortBy`.

TEST COVERAGE: nothing pins the current behaviour. `grep LastModifiedDate tests/` matches only tests/all/change_set_lifecycle.rs (564-566, 804, 1257, 1290), tests/all/entity_reads.rs:477, tests/all/entity_read_agreement.rs:333, tests/all/sort_order_direction.rs, tests/all/model_declared_sort_attributes.rs, tests/all/filter_validation.rs:278 and tests/all/entity_type_projections.rs — none of which calls TagResource or UntagResource. The tagging tests (tests/all/resource_policy_and_tagging.rs, tests/all/tag_set_uniqueness.rs, tests/all/reserved_tag_keys.rs) never read `LastModifiedDate`. tests/all/change_set_lifecycle.rs:803-804 states the change-set side of the rule: "A change naming an existing entity mutates that entity rather than creating another: the details merge, the revision moves and `LastModifiedDate` is restamped."

## Steps to Reproduce

1. POST /StartChangeSet {"Catalog":"AWSMarketplace","ChangeSet":[{"ChangeType":"CreateProduct","Entity":{"Type":"AmiProduct@1.0"},"Details":"{\"ProductTitle\":\"a product\"}"}]}, wait out CHANGE_SET_PREPARING + CHANGE_SET_APPLYING, and read the created entity id from POST /ListChangeSets -> ChangeSetSummaryList[0].EntityIdList[0].
2. GET /DescribeEntity?catalog=AWSMarketplace&entityId=<id> and record `/LastModifiedDate` (call it T0).
3. Sleep past one second, then POST /TagResource {"ResourceArn":"<the entity's EntityArn>","Tags":[{"Key":"owner","Value":"team-a"}]} — answers 200.
4. POST /ListTagsForResource for the same ARN: the new tag is reported, so the record WAS written.
5. GET /DescribeEntity?catalog=AWSMarketplace&entityId=<id> again: `/LastModifiedDate` is still T0, although the entity was modified after T0 — the member documents "The last time the entity was modified". POST /ListEntities with a `LastModifiedDate` DateRange whose AfterValue is T0 also omits the entity.
6. Now do the same tagging through a change set instead: POST /StartChangeSet with [{"ChangeType":"UpdateInformation","Entity":{"Type":"AmiProduct@1.0","Identifier":"<id>"},"EntityTags":[{"Key":"owner","Value":"team-b"}]}] and wait for SUCCEEDED; GET /DescribeEntity shows `/LastModifiedDate` moved (state.rs:2129) — the same write through the other path restamps it.
7. POST /UntagResource {"ResourceArn":"<EntityArn>","TagKeys":["owner"]} and GET /DescribeEntity: `/LastModifiedDate` is again unchanged by the removal.
