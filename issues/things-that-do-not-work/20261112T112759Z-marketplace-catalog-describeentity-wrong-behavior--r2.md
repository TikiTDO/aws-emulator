Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4d88b772f7d66c5a6078a51075b00faa51fddaa1

# DescribeEntity reports a bare EntityType, not the documented EntityType@Version form

_Filed: 2026-11-12T11:27:59Z_
_Operation: DescribeEntity | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4d88b772f7d66c5a6078a51075b00faa51fddaa1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DescribeEntityResponse$EntityType` is documented as carrying the entity's type AT ITS SCHEMA VERSION ("in the format of EntityType@Version"), and `EntityDetail$EntityType` (the member the batch read reports the same value in) carries the identical sentence. The handler reports the BARE type instead.

The schema version is submitted on every change: `StartChangeSet` not only accepts `Change$Entity$Type` at a version, it REQUIRES one — `crate::state::check_entity_type_reference` refuses a bare type name as "unqualified" (state.rs:4245-4253), so a caller cannot create an entity without naming a version. Applying the change then throws that version away (`entity_type_of`, state.rs:2109) and the `EntityRecord` keeps only the bare type, so both `DescribeEntity` (describe_entity.rs:209) and `BatchDescribeEntities` (batch_describe_entities.rs:213) report e.g. `AmiProduct` where the member documents `AmiProduct@1.0`. The version is still available in the store — `ChangeRecord::entity_type` holds the whole submitted reference — so nothing prevents the entity reads from reporting the documented form.

The crate is already willing to put the versioned reference in a member that targets the `EntityType` shape: `ChangeRecord::to_summary` reports `Entity$Type` verbatim as submitted (state.rs:934), i.e. `AmiProduct@1.0` in a member whose modelled `@pattern` is `^[a-zA-Z]+$`. So the two entity reads are the only projections that drop the version, and they are the two whose members the model documents as versioned.

## Evidence

Smithy model (smithy_query query="doc" shape="DescribeEntityResponse" member="EntityType"):
  "The named type of the entity, in the format of <code>EntityType@Version</code>."
and the same sentence on the batch read's member (smithy_query query="shape" shape="EntityDetail"):
  EntityType: "The entity type of the entity, in the format of <code>EntityType@Version</code>."

The handler's own doc header repeats it — services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/ops/describe_entity.rs:64-65:
  "//! * `EntityType` (EntityType, len 1..255, pattern ^[a-zA-Z]+$) — The named type of the
   //!   entity, in the format of EntityType@Version."

What the code returns — describe_entity.rs:209:
  "entity_type: Some(entity.entity_type.clone()),"
and batch_describe_entities.rs:213:
  "entity_type: Some(entity.entity_type.clone()),"

Where the version is dropped — state.rs:2105-2109:
  "// `Change$Entity$Type` is a REFERENCE to an entity type, optionally
   // at a schema version (`DataProduct@1.0`); the entity itself is of
   // the type that reference names, so the record and its ARN carry
   // the type, never the version the request addressed it through.
   let entity_type = entity_type_of(&change.entity_type).to_string();"
(state.rs:4172-4173: "pub fn entity_type_of(reference: &str) -> &str { reference.split('@').next().unwrap_or(reference) }")

The version IS required on input — state.rs:4245-4248:
  "pub fn check_entity_type_reference(reference: &str) -> Result<(), ValidationException> {
       if !reference.contains(ENTITY_TYPE_VERSION_SEPARATOR) {
           return Err(unqualified_entity_type(reference));"

The crate already reports a versioned value in an `EntityType`-targeted member — state.rs:930-936 (`ChangeRecord::to_summary`):
  "entity: Some(Entity { r#type: self.entity_type.clone(), identifier: self.entity_identifier.clone() }),"
where `ChangeRecord::entity_type` is the submitted reference (`AmiProduct@1.0`), so the `^[a-zA-Z]+$` pattern on `EntityType` is not what stops the entity reads from naming the version.

Test coverage: tests/all/request_validation.rs:761-768 pins the bare type with the reason "the stored entity is of the type the reference names", and tests/all/documented_entity_types.rs:316-323 asserts `/EntityType` equals the bare type; tests/all/entity_read_agreement.rs:333-345 only asserts the batch read and the single read agree, so it is unaffected by the value chosen. The reason those tests state is about the stored record and its ARN (ARN's `^[a-zA-Z0-9:*/-]+$` genuinely admits no `@`), and it is the reason the member doc quoted above contradicts for the response member.

## Steps to Reproduce

1. StartChangeSet with Catalog=AWSMarketplace and one change: {"ChangeType":"CreateProduct","Entity":{"Type":"AmiProduct@1.0"},"Details":"{\"ProductTitle\":\"a product\"}"} — note the version is mandatory: sending "AmiProduct" is refused with ValidationException by check_entity_type_reference.
2. Wait for the change set to reach SUCCEEDED (DescribeChangeSet), then read the created entity id from the change set summary (ListChangeSets -> EntityIdList).
3. GET /DescribeEntity?catalog=AWSMarketplace&entityId=<id> and read /EntityType: it answers "AmiProduct", while DescribeEntityResponse$EntityType is documented "in the format of EntityType@Version" (expected "AmiProduct@1.0").
4. POST /BatchDescribeEntities with EntityRequestList=[{"Catalog":"AWSMarketplace","EntityId":"<id>"}] and read /EntityDetails/<id>/EntityType: same bare value, for a member carrying the same documented format.
5. GET /DescribeChangeSet?catalog=AWSMarketplace&changeSetId=<id> and read /ChangeSet/0/Entity/Type: it answers "AmiProduct@1.0", showing the version is persisted and that the crate already serves it in an EntityType-targeted member.
