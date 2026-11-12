Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4affd7a378830b3c6d8042007351e66db953803d

# CreateDataTransformationProfile answers 501 for the StarterProfile and SampleData source variants

_Filed: 2026-10-24T00:03:07Z_
_Operation: CreateDataTransformationProfile | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4affd7a378830b3c6d8042007351e66db953803d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Two of the four variants of the modelled `CreateDataTransformationProfileSource` union — `StarterProfile` (valid for SourceFormat CCDA) and `SampleData` (valid for SourceFormat CSV) — are refused by the otherwise implemented handler with `Scaffolded::not_implemented(..)`, i.e. HTTP 501, after the source-format check passes. Expected: a well-formed request naming a starter profile (or a sample-data S3 URI) creates a DRAFT profile at version 0 and answers 201 with ProfileId/Version/SourceFormat/TargetFormat/ProfileName/LastUpdatedAt, which a later GetDataTransformationProfile reads back. Actual: HTTP 501 with no profile created, so two of the operation's four documented creation paths are unusable even though the operation is implemented for the other two.

## Evidence

Spec: the union `CreateDataTransformationProfileSource` declares ProfileMapping, ExistingVersionedProfileId, StarterProfile and SampleData; `Source` is `@required` on CreateDataTransformationProfileRequest and the operation models no 501. Handler: `src/ops/create_data_transformation_profile.rs` `initial_mapping()` lines ~368-385 return `crate::scaffolded::Scaffolded::not_implemented(...)` for both variants. Tests: `tests/all/data_transformation_profile.rs` exercises the ProfileMapping / ExistingVersionedProfileId variants only.

## Steps to Reproduce

1. Call CreateDataTransformationProfile with SourceFormat=CCDA, ProfileName=p and Source={"StarterProfile":{"StarterProfileName":"<any>"}}
2. Observe HTTP 501 instead of 201 with a new DRAFT ProfileId
3. Repeat with SourceFormat=CSV and Source={"SampleData":{"S3Uri":"s3://bucket/key.csv"}} and observe the same 501
