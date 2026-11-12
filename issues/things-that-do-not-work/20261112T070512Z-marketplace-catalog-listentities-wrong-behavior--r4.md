Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0ae6f58da9eea98f3529421bd2884e28b9157587

# Entity Visibility is judged against a cross-type union, so ListEntities serves "Unavailable" in a *ProductSummary whose modelled enum excludes it

_Filed: 2026-11-12T07:05:12Z_
_Operation: ListEntities | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0ae6f58da9eea98f3529421bd2884e28b9157587`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

An entity's `Visibility` is accepted, stored and reported against ONE hand-written set, `crate::state::ENTITY_VISIBILITY_VALUES`, which is the UNION of the model's five per-type `*VisibilityString` enums. "Unavailable" is declared by exactly one of them (`DataProductVisibilityString`), so for the other four product types the union admits a value that type's own enum forbids.

Concretely: a succeeded `APPLY` change set whose change carries `Details = {"ProductTitle":"x","Visibility":"Unavailable"}` on `Entity.Type = "SaaSProduct@1.0"` is NOT failed (`details_defects` only consults the union, and `summary_member_contracts("SaaSProduct")` is empty), `detail_visibility` stores "Unavailable" on the record, and `ListEntities` then reports it twice: in `EntitySummary$Visibility` (unconstrained) and in `EntitySummary$SaaSProductSummary$Visibility`, whose target `SaaSProductVisibilityString` enumerates only `Limited | Public | Restricted | Draft`. The same holds for `AmiProduct`, `ContainerProduct` and `MachineLearningProduct`.

This contradicts the crate's own stated rule and its own per-type machinery: `EntityRecord::summary_name` states "a value a summary member cannot hold is reported by no projection of this record" and names `Visibility` as following that two-sided rule; every OTHER enumerated summary member is judged per type through `summary_member_contracts` + `choice(...)` (e.g. `("State", choice(OFFER_STATE_VALUES))`); and the crate already owns the per-type dispatch idiom for exactly this kind of per-type enum in `entity_type_sort_by_values`, plus the generated per-type constants `SAA_S_PRODUCT_VISIBILITY_STRING_VALUES` / `AMI_PRODUCT_VISIBILITY_STRING_VALUES` / `CONTAINER_PRODUCT_VISIBILITY_STRING_VALUES` / `MACHINE_LEARNING_PRODUCT_VISIBILITY_STRING_VALUES` — none of which any code path consults. A side effect of the same gap: such an entity can never be selected by its own type's `*VisibilityFilter`, because that filter's `ValueList` is boundary-validated against the per-type enum and so can never carry "Unavailable".

## Evidence

MODEL (smithy_query query="shape" shape="SaaSProductVisibilityString"): `{"shape_id": "...#SaaSProductVisibilityString", "type": "enum", "values": ["Limited", "Public", "Restricted", "Draft"]}` — and `DataProductVisibilityString` is the only one that adds `"Unavailable"`: `"values": ["Limited", "Public", "Restricted", "Unavailable", "Draft"]`. `EntitySummary$SaaSProductSummary` targets `SaaSProductSummary`, whose `Visibility` targets `SaaSProductVisibilityString`.

THE CRATE'S OWN GENERATED PER-TYPE CONSTANTS, never consulted:
- `services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/ops/types/shared_types.rs:244`: `pub const SAA_S_PRODUCT_VISIBILITY_STRING_VALUES: &[&str] = &["Draft", "Limited", "Public", "Restricted"];`
- `shared_types.rs:168`: `pub const DATA_PRODUCT_VISIBILITY_STRING_VALUES: &[&str] = &["Draft", "Limited", "Public", "Restricted", "Unavailable"];`
(grep for `SAA_S_PRODUCT_VISIBILITY_STRING_VALUES` across `src/` returns only its definition.)

THE UNION THAT REPLACES THEM — `src/state.rs:372-377`:
```
/// The `Visibility` values an entity may carry: the union of the model's five
/// `*VisibilityString` enums (`AmiProductVisibilityString`, ...).
pub const ENTITY_VISIBILITY_VALUES: &[&str] =
    &["Draft", "Limited", "Public", "Restricted", "Unavailable"];
```
ACCEPTED with it — `src/state.rs:3199-3209` (`details_defects`, the verdict that fails a change set):
```
if let Some(value) = members.get(ENTITY_VISIBILITY_DETAIL_MEMBER) {
    if value.as_str().is_none_or(|text| !ENTITY_VISIBILITY_VALUES.contains(&text)) {
```
STORED with it — `src/state.rs:2750-2755` (`detail_visibility`): `ENTITY_VISIBILITY_VALUES.contains(&value).then(|| value.to_string())`.
SERVED unconditionally — `src/state.rs:1133-1136` (`product_summary`, feeding every `*ProductSummary`): `.then(|| (self.summary_name(), Some(self.visibility.clone())))`.

BOTH SIDES OF THE IN-CRATE CONTRADICTION:
- `src/state.rs:1236-1245` (`EntityRecord::summary_name` doc): "Judged here as well as on the write path, exactly as an entity's `Visibility` is ([`details_defects`] fails the change set and [`detail_visibility`] drops the value): a value a summary member cannot hold is reported by no projection of this record."
- `src/state.rs:3090-3104` (`summary_member_contracts`): "The five `*ProductSummary` shapes report only the entity's name and its visibility, which [`entity_name_contract`] and [`ENTITY_VISIBILITY_VALUES`] bound for every type" — and the function returns `&[]` for the product types, so no per-type visibility contract exists, while `src/state.rs:3035` shows the per-type treatment every other enumerated member gets: `("State", choice(OFFER_STATE_VALUES)),`.
- `src/state.rs:1458-1463` (`entity_type_sort_by_values` doc) proves the crate already has the per-type dispatch idiom for exactly this kind of enum: "This answers those constants themselves, so a value added to or dropped from one of those enums moves the set this service sorts that type on with it", followed by `match entity_type { "SaaSProduct" => SAA_S_PRODUCT_SORT_BY_VALUES, ... }`.

TEST COVERAGE: no test pins "Unavailable" anywhere (grep for `Unavailable` across the crate matches only `src/state.rs:377`, `shared_types.rs:168` and the raw model). `tests/all/documented_entity_types.rs:303-306` creates entities with `"Visibility": "Draft"` / `"Restricted"` only, so the per-type rule is untested rather than pinned.

## Steps to Reproduce

1. POST /StartChangeSet with Catalog=AWSMarketplace, Intent=APPLY, ChangeSet=[{"ChangeType":"CreateProduct","Entity":{"Type":"SaaSProduct@1.0"},"Details":"{\"ProductTitle\":\"a saas product\",\"Visibility\":\"Unavailable\"}"}] — the call is accepted.
2. Wait out CHANGE_SET_PREPARING + CHANGE_SET_APPLYING, then GET /DescribeChangeSet?catalog=AWSMarketplace&changeSetId=<id> — Status is SUCCEEDED with no ErrorDetailList, although "Unavailable" is not a value `SaaSProductVisibilityString` declares.
3. POST /ListEntities with {"Catalog":"AWSMarketplace","EntityType":"SaaSProduct"} — the summary reports `Visibility: "Unavailable"` AND `SaaSProductSummary.Visibility: "Unavailable"`, a value outside that member's modelled enum.
4. POST /ListEntities with {"Catalog":"AWSMarketplace","EntityType":"SaaSProduct","EntityTypeFilters":{"SaaSProductFilters":{"Visibility":{"ValueList":["Unavailable"]}}}} — refused at the boundary, because the per-type enum does not admit the value the entity now carries.
5. Repeat step 1 with Entity.Type "DataProduct@1.0" to see the one type whose enum really does declare "Unavailable": same acceptance, but in contract.
