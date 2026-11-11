Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena.data-catalog-parameters-map-round-trip
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena.data-catalog-parameters-map-round-trip|run#23|AssertionError|assert code == "InvalidRequestException", (
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# Athena UpdateDataCatalog refuses with NotImplemented instead of InvalidRequestException for a deleted catalog (1 cause)

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateDataCatalog

## Description
Cause 1 of 1: `UpdateDataCatalog` is not served at all, so the DataCatalog lifecycle contract cannot be exercised past the update step.

Related open issue: 20261104T163600Z-athena-updatedatacatalog-stub.md ("Implement athena UpdateDataCatalog: unimplemented scaffold stub") describes the same handler being a scaffold, but its Issue-Trace-Ids list does not carry this check's replay key (`athena.data-catalog-parameters-map-round-trip`), so that issue does not cover this failure and this one records it.

AWS:      Smithy (com.amazonaws.athena#UpdateDataCatalog): input members Name (CatalogNameString, required, @length 1..256) and Type (DataCatalogType enum, required), optional Description and Parameters; modeled errors are InvalidRequestException (client, HTTP 400) and InternalServerException (server, HTTP 500). AWS refuses an UpdateDataCatalog against a catalog name that does not exist (here: one that was just deleted with DeleteDataCatalog) with HTTP 400, error code `InvalidRequestException`. Recorded message template: fixed text `DataCatalog ` + variable part (the `Name` value from the request) + fixed text ` does not exist.` — note AWS words the *read* refusal differently (GetDataCatalog on the same deleted name answers `DataCatalog <Name> was not found.`, which the emulator already gets right in this same check), so the two messages are not interchangeable. The variable part is always taken from the request, never a literal from any one run.

Emulator: the call is refused with HTTP 501 and error code `NotImplemented`, message `operation UpdateDataCatalog is scaffolded but not implemented in this emulator`. The handler `services/service-com-amazonaws-athena-amazonathena/src/ops/update_data_catalog.rs` still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `crate::scaffolded::Scaffolded::not_implemented(..)`, so nothing is read or written. The earlier steps of the check pass: CreateDataCatalog persists a HIVE catalog, GetDataCatalog round-trips the Parameters map (a long `metadata-function` Lambda ARN and `sdk-version`) unaltered, DeleteDataCatalog removes it, and GetDataCatalog on the deleted name correctly answers InvalidRequestException with the `was not found.` wording. Only the UpdateDataCatalog step diverges.

Why:     a client cannot update a data catalog at all, and error-handling paths break twice over: code that catches `InvalidRequestException` to detect "catalog gone / recreate it" sees an unmodeled `NotImplemented` (an error shape absent from the operation's modeled error set) and either crashes or retries a 501 as if it were a transient server fault. Catalog-management tooling (and CloudFormation-style converge loops that update an existing catalog rather than recreating it) cannot complete against the emulator.

Fix:      implement the UpdateDataCatalog handler in `services/service-com-amazonaws-athena-amazonathena/src/ops/update_data_catalog.rs`: resolve `Name` in the service's data-catalog state, apply `Type`/`Description`/`Parameters` to the stored catalog on success, and when the name resolves to nothing raise the modeled InvalidRequestException (HTTP 400) with the `DataCatalog <Name> does not exist.` message built from the request's `Name`. Remove the file's scaffold header only once the handler is real.

Traces:   athena.data-catalog-parameters-map-round-trip (mismatch: `AssertionError: expected InvalidRequestException updating a deleted catalog, got NotImplemented`).

## Evidence
Failing check: athena.data-catalog-parameters-map-round-trip [mismatch]

Replay output (tail):
  File "check_athena_data_catalog_parameters_map_round_trip.py", line 68, in run
    assert code == "InvalidRequestException", (
  AssertionError: expected InvalidRequestException updating a deleted catalog, got NotImplemented

Expected (AWS): UpdateDataCatalog(Name=<deleted catalog name>, Type="HIVE", Parameters={...}) -> HTTP 400, Error.Code="InvalidRequestException", Error.Message="DataCatalog <Name> does not exist." (fixed text "DataCatalog " ... " does not exist."; <Name> is the request's Name member). Modeled errors for the operation are exactly InvalidRequestException (400) and InternalServerException (500).

Actual (emulator): HTTP 501, Error.Code="NotImplemented", Error.Message="operation UpdateDataCatalog is scaffolded but not implemented in this emulator".

Surrounding steps in the same check PASS, which isolates the divergence to UpdateDataCatalog: CreateDataCatalog(Type="HIVE", Parameters={metadata-function: <long lambda ARN>, sdk-version: "1.0"}) succeeds, GetDataCatalog returns the Parameters map byte-identical, DeleteDataCatalog succeeds, and GetDataCatalog on the deleted name returns InvalidRequestException with "DataCatalog <Name> was not found." as AWS words it.

## Steps to Reproduce
1. Boot the athena emulator from this checkout.
2. athena.create_data_catalog(Name=<fresh name>, Type="HIVE", Parameters={"metadata-function": "arn:aws:lambda:<region>:123456789012:function:<long name>", "sdk-version": "1.0"}).
3. athena.get_data_catalog(Name=<name>) — observe Type and Parameters round-trip unchanged (passes today).
4. athena.delete_data_catalog(Name=<name>).
5. athena.get_data_catalog(Name=<name>) — observe InvalidRequestException "DataCatalog <name> was not found." (passes today).
6. athena.update_data_catalog(Name=<name>, Type="HIVE", Parameters={...}) — observe HTTP 501 NotImplemented instead of HTTP 400 InvalidRequestException "DataCatalog <name> does not exist.".

Replay key: athena.data-catalog-parameters-map-round-trip
