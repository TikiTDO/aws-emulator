Issue-Source: trace-sweep-divergence
Issue-Trace: glue-aorigin-search-tables-resolves-the-database
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-aorigin-search-tables-resolves-the-database glue-tsd-search-tables-route
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|SearchTables|2xx->2xx|$.TableList[*]
Issue-Key: glue|SearchTables|2xx->2xx|$.TableList[*].DatabaseName
Issue-Key: glue|SearchTables|2xx->2xx|$.TableList[*].StorageDescriptor.Location
Issue-Key: glue|SearchTables|4xx->2xx|$.Message
Issue-Key: glue|SearchTables|4xx->2xx|$.__type
Issue-Key: glue|SearchTables|4xx->2xx|status
Issue-Key: glue|SearchTables|2xx->4xx|$.TableList
Issue-Key: glue|SearchTables|2xx->4xx|status
Issue-Commit: c588c92daf57694e44caee6ddcc17c51be576a2d

# Glue SearchTables returns the wrong result set for filters, comparators and search text

> _Note: this issue was identified against the code base at commit `c588c92daf57694e44caee6ddcc17c51be576a2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SearchTables

## Description
SearchTables matches the wrong tables: it drops results that AWS returns, returns results AWS excludes, and never applies the comparator rule that makes a malformed filter a refusal. Five causes in one matching engine.

Cause 1 of 5: a SearchTables filter that selects one of two same-named tables resolves to the wrong one and returns only one row. AWS: Smithy SearchTables takes Filters (SearchPropertyPredicates) and SearchText and returns a TableList; the recording shows TWO tables, the first belonging to database '...-srch-a-...' and the second to '...-srch-b-...', each with its own StorageDescriptor.Location. Emulator: returns a single table and it is the '-b-' one, i.e. the predicate selected the wrong candidate and the second match was dropped. Why: a client enumerating same-named tables across databases sees one arbitrary table instead of all matches, so a catalog-wide search silently loses data. Fix: the SearchTables handler's predicate matching and result assembly. Traces: glue-aorigin-search-tables-resolves-the-database#task#9.

Cause 2 of 5: SearchTables accepts a comparator applied to a non-time field. AWS: SearchTables models InvalidInputException (httpError 400); the recording shows 400 InvalidInputException with `Message` 'Comparator can only apply to time fields' — entirely fixed text, no field name interpolated. Emulator: answers 200 with a result set. Why: a mis-built filter silently returns results instead of being refused, so the client trusts a match that AWS would never have made. Fix: SearchTables' filter validation. Traces: glue-aorigin-search-tables-resolves-the-database#task#10 (cluster also covers glue-tsd-search-tables-route#task#7, #task#8, #task#9).

Cause 3 of 5: a SearchTables call that AWS answers with an empty result set is refused. AWS: the recording shows 200 with `TableList` present and empty ([]). Emulator: 400, with no TableList at all. Why: "no matches" is a normal outcome; turning it into a client error breaks any search loop, and even on success the emulator elides the empty list rather than emitting it. Fix: the SearchTables handler (no-match path) and its output serializer. Traces: glue-aorigin-search-tables-resolves-the-database#task#11.

Cause 4 of 5: SearchTables drops a matching table from a two-row result. AWS: the recording shows TableList[1] present — a fully populated Table in database '...-tsd-zz-...' named 'andes_3_0_data'. Emulator: TableList[1] absent. Why: the search is under-inclusive across databases, so results depend on which database the emulator happened to scan. Fix: the SearchTables handler's catalog-wide scan. Traces: glue-tsd-search-tables-route#task#11.

Cause 5 of 5: SearchTables returns a table AWS excludes. AWS: the recording shows an EMPTY TableList for this search. Emulator: returns one table ('andes_3_0_data' in database '...-tsd-srch-...'). Why: over-inclusive matching, the mirror of cause 4 — a client acts on a table that does not satisfy the search. Fix: the SearchTables handler's predicate/SearchText evaluation. Traces: glue-tsd-search-tables-route#task#12.

Related open issue (does not cover these steps): 20261029T201859Z-glue-reads-report-stale-or-wrong-resource-state.md.

## Evidence
Replay of trace set simparity-wire-aws-v1, service glue.

Cause 1 — glue-aorigin-search-tables-resolves-the-database#task#9 SearchTables, 200/200: $.TableList[0].DatabaseName value_mismatch (expected the '-srch-a-' database, got the '-srch-b-' one); $.TableList[0].StorageDescriptor.Location value_mismatch ('.../a/' vs '.../b/'); $.TableList[1] missing (expected a full Table in the '-srch-b-' database).
Cause 2 — glue-aorigin-search-tables-resolves-the-database#task#10 SearchTables: status expected 400, got 200; $.__type missing (expected 'InvalidInputException'); $.Message missing (expected 'Comparator can only apply to time fields'). 4 steps in cluster.
Cause 3 — glue-aorigin-search-tables-resolves-the-database#task#11 SearchTables: status expected 200, got 400; $.TableList missing (expected []).
Cause 4 — glue-tsd-search-tables-route#task#11 SearchTables, 200/200: $.TableList[1] missing (expected the 'andes_3_0_data' Table in the '-tsd-zz-' database).
Cause 5 — glue-tsd-search-tables-route#task#12 SearchTables, 200/200: $.TableList[0] extra (expected no results, got 'andes_3_0_data' in the '-tsd-srch-' database).

## Steps to Reproduce
Replay the wire corpus and inspect these glue steps:
1. glue-aorigin-search-tables-resolves-the-database#task#9 — SearchTables across two databases holding the same table name.
2. glue-aorigin-search-tables-resolves-the-database#task#10 — SearchTables with a comparator on a non-time field.
3. glue-aorigin-search-tables-resolves-the-database#task#11 — SearchTables whose filter matches nothing.
4. glue-tsd-search-tables-route#task#11 — SearchTables expected to return two tables.
5. glue-tsd-search-tables-route#task#12 — SearchTables expected to return none.
