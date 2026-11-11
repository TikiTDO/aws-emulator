Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 9a3c64d3d31babcda2c35aaf4483db5c8583df25

# ListRecords can emit a NextToken longer than the modelled 8192-character maximum, which the service's own input validator then refuses, dead-ending pagination

_Filed: 2026-11-11T18:27:41Z_
_Operation: ListRecords | Class: pagination_

> _Note: this issue was identified against the code base at commit `9a3c64d3d31babcda2c35aaf4483db5c8583df25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListRecords` builds its page token by hex-encoding the 16-hex-digit per-process key, a colon, and the RAW record identifier the next page resumes at (`encode_next_token`). The token therefore grows with the identifier: its length is `2 * (17 + identifier.len())` characters. The model bounds `ListRecordsNextToken` at `@length 1..8192` on BOTH the response member and the request member, while a record identifier is a `ValueAsString` the model bounds at `len 0..358400` — and `prepare_record` stores whatever the identifier feature carried, with no length reduction. So any record identifier of 4080 characters or more (well inside what PutRecord accepts) makes this operation hand back a token of 8194+ characters: it violates the modelled bound on the member it is serialized into, and when the paginating client sends it back the framework's own `@length` check refuses the request with HTTP 400 before the handler runs (pinned by this crate's own size-bound test). The second and later pages of such a feature group are unreachable: the caller is left with a 400 and no way to resume, even though the handler's cursor logic itself is correct.

The same encoding also means the token is linear in the identifier, so the bound is crossed silently — nothing in the handler checks the token it is about to return against the bound its own module header reproduces.

## Evidence

Model, `ListRecordsResponse$NextToken` (smithy_query query="shape" shape="ListRecordsResponse"): `"NextToken": {"target": "ListRecordsNextToken", "length": {"min": 1, "max": 8192}, "doc": "<p>A token to resume pagination if the response includes more record identifiers than <code>MaxResults</code>.</p>"}` — and the identical bound on the request member `ListRecordsRequest$NextToken`. The crate reproduces both in its own handler header: services/service-com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime/src/ops/list_records.rs:69-70 ("* `NextToken` (ListRecordsNextToken, len 1..8192) — A token to resume pagination of ListRecords results.") and :74 ("* `NextToken` (ListRecordsNextToken, len 1..8192) — A token to resume pagination if the ...").

The code that can exceed it — services/.../src/ops/list_records.rs:296-298:
```
fn encode_next_token(identifier: &str) -> String {
    hex::encode(format!("{:016x}:{identifier}", page_token_key()).as_bytes())
}
```
and its caller services/.../src/ops/list_records.rs:263-264: `let next_token = next_identifier.map(|identifier| encode_next_token(&identifier));`. The identifier it embeds is the stored one — services/.../src/state.rs:252-254 (`/// \`RecordIdentifierValueAsString\` — the value of the group's record-identifier feature in this record.` / `pub identifier: String,`), set verbatim from the posted feature value at services/.../src/state.rs:385-392 — and `RecordIdentifierValueAsString`/`ValueAsString` is modelled `len 0..358400` (OPERATIONS.md:50, GetRecordRequest in the model).

That the framework refuses the oversized token on the NEXT call is pinned by this crate's own test — services/.../tests/all/size_bound_statements.rs:526-533:
```
            case: "ListRecords with a NextToken one character past the maximum",
            ...
            expect: Expect::Outside("Member must have length less than or equal to 8192"),
```
(with `let token_past_the_maximum = "t".repeat(8193);` at :276).

Test coverage of the gap: none. The pagination test uses two-character identifiers only — services/.../tests/all/online_store_record_lifecycle.rs:627 `let written = ["C1", "C2", "C3", "C4", "C5"];` — and no test in the crate writes a record identifier longer than 151 characters (grep for `repeat(` in tests/ returns only 151/65/8192/8193 cases, none of them a record identifier).

## Steps to Reproduce

1. Create feature group `customers` with an OnlineStore through the SageMaker control plane (as tests/all/online_store_record_lifecycle.rs does).
2. PutRecord twice into `customers`, each with a `customer_id` value of 5000 characters (distinct), plus an event-time feature. Both are accepted: `ValueAsString` is modelled `len 0..358400`.
3. POST /FeatureGroup/customers/ListRecords with `{"MaxResults": 1}` — the response carries one identifier and a `NextToken` of 2 * (17 + 5000) = 10034 characters, past the modelled `ListRecordsNextToken` maximum of 8192.
4. POST /FeatureGroup/customers/ListRecords with that `NextToken` — the request is refused with HTTP 400 "at 'nextToken' failed to satisfy constraint: Member must have length less than or equal to 8192" (the bound pinned by tests/all/size_bound_statements.rs:526-533), so the second page can never be fetched.
