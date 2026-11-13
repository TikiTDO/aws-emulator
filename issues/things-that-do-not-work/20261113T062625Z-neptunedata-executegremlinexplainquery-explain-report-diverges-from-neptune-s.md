Issue-Source: trace-sweep-divergence
Issue-Trace: neptunedata-query-languages-live-cluster-round-trip
Issue-Step: task#21
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: neptunedata-query-languages-live-cluster-round-trip
Issue-Shape-Id: com.amazonaws.neptunedata#AmazonNeptuneDataplane
Issue-Key: neptunedata|ExecuteGremlinExplainQuery|2xx->2xx|$
Issue-Key: neptunedata|ExecuteGremlinExplainQuery|2xx->2xx|$headers.content-type
Issue-Commit: 4a722088e6b3ccbbd9bb4b06bd9860c6050768ab

# neptunedata ExecuteGremlinExplainQuery explain report diverges from Neptune's report

> _Note: this issue was identified against the code base at commit `4a722088e6b3ccbbd9bb4b06bd9860c6050768ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteGremlinExplainQuery

## Description
One net-new root cause on the neptunedata data plane, reached against a running, answering emulator (the step was served with HTTP 200; this is not an unreachability or a 501 scaffold path). It is NOT covered by the open issue 20261109T174758Z-neptunedata-data-plane-responses-diverge-from-neptune-engine.md, which carries the replay keys for GetEngineStatus (task#18) and ExecuteFastReset (task#27/task#28) only and names no ExecuteGremlinExplainQuery key.

Cause 1 of 1: ExecuteGremlinExplainQuery answers HTTP 200 with a text report whose layout is not the Gremlin explain report Neptune emits — the banner title is not centred, the per-explain header lines (Explain Time, Engine Version) are absent, the whole "Converted Traversal" section and its "+ not converted into Neptune steps:" trailer are absent, the "Predicates" trailer and its count are absent, the Neptune step list is printed in a different step syntax, and the pushdown the "Optimized Traversal" section reports is not the plan Neptune reports for the same traversal.
AWS: Smithy (com.amazonaws.neptunedata) models ExecuteGremlinExplainQuery POST /gremlin/explain -> 200, input {gremlinQuery (String, jsonName "gremlin", required)}, output {output targeting ReportAsText, @httpPayload, @mediaType "text/plain", blob} — i.e. the model fixes only that the payload is a text blob, so the report's internal layout is recorded behavior only. The recording shows Neptune's report as: the rule line of asterisks, then the title "Neptune Gremlin Explain" INDENTED/centred under that rule, then the rule again; a "Query String" section (heading + "============" + the submitted query); an "Original Traversal" section (heading + "==================" + the compiled TinkerPop step list in brackets); then two header lines, "Explain Time: <instant the explain ran>" and "Engine Version: <the engine version of the cluster serving it>"; then a "Converted Traversal" section (heading + "===================" + "Neptune steps:" + the bracketed Neptune step list + a "+ not converted into Neptune steps: <steps>," line naming the steps left to the step iterators); then an "Optimized Traversal" section in the same form carrying the optimized plan; and finally a "Predicates" section (heading + "==========" + "# of predicates: <count>"). Inside a Neptune step list each entry is written INDENTED with no "+- " bullet, each entry ends with a comma, a NeptuneGraphQueryStep's annotations are written as a bare brace group ("}, {path=[...], maxVarId=N}", not "annotations={...}"), the label pattern is written in the VL(...) form with the label value as an angle-bracketed term inside a ContainsFilter and an explicit "project distinct ?1" (e.g. "VL(?1, <~label>, ?2, <~>) . project distinct ?1 . ContainsFilter(?2 in (<label>)) ."), and a terminal count is PUSHED DOWN in the optimized plan into a NeptuneCountGlobalStep rather than left in the step list. In the recording's own example the fixed text is everything listed above; the variable parts are the submitted query, the step list and patterns derived from it, the explain instant, the engine version, and the predicate count — the recording's specific traversal, its 2026-11-05 timestamp and its 1.4.8.0.R1 engine version are that run's values, not part of the requirement.
Emulator: the report is built by crate::state::gremlin_plan::gremlin_explain_report and emits the title line flush-left ("Neptune Gremlin Explain" with no indentation), has NO "Explain Time:" line, NO "Engine Version:" line, NO "Converted Traversal" section and no "+ not converted into Neptune steps:" line, and NO "Predicates"/"# of predicates:" trailer; its single "Optimized Traversal" list prefixes every entry with "+- ", writes "annotations={path=[...], maxVarId=N}", renders the label pattern as "(?1, <~label>, \"<value>\", <~>) . project ?1 ." with a quoted string literal and a Vertex(?1):GraphNode path entry, and leaves the terminal CountGlobalStep as a listed step instead of reporting the count pushdown. The cluster's divergence key also names $headers.content-type for this response, i.e. the response media type diverges here as it does on the other neptunedata responses (Hypothesis: the same missing charset parameter / media-type selection reported as Cause 2 of 20261109T174758Z-neptunedata-data-plane-responses-diverge-from-neptune-engine.md, which does not carry this step's key; the model marks this payload @mediaType text/plain, so the explain response must announce the text media type Neptune announces rather than the service's default JSON one).
Why: the explain report is the whole product of this operation, and the tools that consume it (the Neptune workbench's %%gremlin explain magic, query-tuning scripts, CI assertions on a plan) split it on those section headings and read the header lines. Against the emulator such a client finds no "Converted Traversal" and no "Predicates" section at all, cannot read the explain time or the engine version the plan was produced by, sees a step syntax its plan parser does not match, and is told a terminal count was not pushed down when Neptune reports that it was — so a tuning decision taken from the emulator's report is taken from a plan Neptune never produces.
Fix: the explain-report renderer, crate::state::gremlin_plan::gremlin_explain_report and the plan builder it prints (the step printer and the pattern printer feeding it) — it must emit the centred banner, the Explain Time and Engine Version header lines (the version taken from the engine-status record's engine version rather than hard-coded), both the Converted and the Optimized step sections with the not-converted trailer, Neptune's step-entry syntax and pattern form, the count pushdown in the optimized plan, and the predicate-count trailer; plus the response's content-type for this operation's text payload in the service's response serializer.
Traces: neptunedata-query-languages-live-cluster-round-trip#task#21 (src/ops/execute_gremlin_explain_query.rs:249-266, src/state/gremlin_plan.rs:100-101, 1122-1174).

## Evidence
Cause 1 — fixture neptunedata-query-languages-live-cluster-round-trip, step task#21, ExecuteGremlinExplainQuery, expected 200 / actual 200, whole payload diverges ($: value_mismatch); the cluster's divergence key is service=neptunedata|ExecuteGremlinExplainQuery|status=200->200|keys=$,$headers.content-type.

Recorded (AWS) report, for a traversal of the form g.V().hasLabel('<label>').count():
  *******************************************************
  <spaces>Neptune Gremlin Explain          <- title indented/centred
  *******************************************************
  Query String / ============ / <the submitted query>
  Original Traversal / ================== / [GraphStep(vertex,[]), HasStep([~label.eq(<label>)]), CountGlobalStep]
  Explain Time: <instant>
  Engine Version: <cluster engine version>
  Converted Traversal / =================== / Neptune steps: / [
      NeptuneGraphQueryStep(Vertex) {
          JoinGroupNode {
              PatternNode[VL(?1, <~label>, ?2, <~>) . project distinct ?1 . ContainsFilter(?2 in (<label>)) .]
          }, {path=[Vertex(?1):GraphStep], maxVarId=3}
      },
      NeptuneTraverserConverterStep
  ]
  + not converted into Neptune steps: CountGlobalStep,
  Optimized Traversal / =================== / Neptune steps: / [
      NeptuneCountGlobalStep {
          JoinGroupNode {
              PatternNode[VL(?1, <~label>, ?2=<label>, <~>) . project ?1 .]
          }, {path=[Vertex(?1):GraphStep], maxVarId=3}
      }
  ]
  Predicates / ========== / # of predicates: 2

Emulator report for the same step:
  *******************************************************
  Neptune Gremlin Explain          <- flush left, not indented
  *******************************************************
  Query String / ============ / <query>
  Original Traversal / ================== / [GraphStep(vertex,[]), HasStep([~label.eq(<label>)]), CountGlobalStep]
  Optimized Traversal / =================== / Neptune steps: / [
      +- NeptuneGraphQueryStep(Vertex) {
          JoinGroupNode {
              PatternNode[(?1, <~label>, "<label>", <~>) . project ?1 .]
          }, annotations={path=[Vertex(?1):GraphNode], maxVarId=2}
      }
      +- NeptuneTraverserConverterStep
      +- CountGlobalStep
  ]
No Explain Time line, no Engine Version line, no Converted Traversal section, no "+ not converted into Neptune steps:" trailer, no Predicates section; "+- " bullets, annotations= prefix, quoted literal in the pattern, GraphNode instead of GraphStep in the path, and the terminal count left in the step list instead of pushed down to NeptuneCountGlobalStep.

(The recording's traversal text, its explain timestamp and its engine/TinkerPop versions are that run's values; the requirement is the sections, their headings and order, the step/pattern syntax and the pushdown — all derived from the submitted traversal and the serving cluster.)

## Steps to Reproduce
1. Replay fixture neptunedata-query-languages-live-cluster-round-trip against the emulator (replay_wire_corpus); it stands up the Neptune cluster and addresses the data plane at the cluster endpoint.
2. Step task#21 — POST /gremlin/explain (neptunedata ExecuteGremlinExplainQuery) with gremlin set to a traversal that ends in a terminal count, e.g. g.V().hasLabel('<some-label>').count().
3. Compare the 200 text payload against the recorded one: the indentation of the "Neptune Gremlin Explain" title, the presence of the "Explain Time:" and "Engine Version:" lines, the presence of the "Converted Traversal" section and its "+ not converted into Neptune steps:" trailer, the presence of the "Predicates"/"# of predicates:" trailer, the step-entry syntax inside each Neptune step list ("+- " bullets vs indented comma-terminated entries, annotations={...} vs a bare brace group), the pattern form (VL(...) with a ContainsFilter and "project distinct" vs a quoted literal with "project"), and whether the terminal count is reported as a NeptuneCountGlobalStep pushdown.
4. Also inspect the response's content-type header for this text payload.
5. Re-replay the fixture and confirm task#21 passes.
