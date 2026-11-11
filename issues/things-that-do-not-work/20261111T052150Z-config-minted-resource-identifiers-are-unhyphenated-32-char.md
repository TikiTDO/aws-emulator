Issue-Source: trace-sweep-divergence
Issue-Trace: config-list-evaluations-by-context-identifier
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: config-list-evaluations-by-context-identifier config-proactive-evaluation-lifecycle config-proactive-resource-evaluation-lifecycle config-resource-evaluation-context-grouping config-resource-evaluation-grouped-by-context config-start-resource-evaluation-smoke config-stored-query-expression-unicode-round-trip config-stored-query-unicode-and-emoji-round-trip config-stored-query-unicode-expression-round-trip
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config|StartResourceEvaluation|2xx->2xx|$.ResourceEvaluationId
Issue-Key: config|GetStoredQuery|2xx->2xx|$.StoredQuery.QueryId
Issue-Commit: f3853c2ce3831f59f19a9d4fb097e3f9d5244398

# config: minted resource identifiers are unhyphenated 32-char hex instead of AWS's hyphenated UUID form (2 causes)

> _Note: this issue was identified against the code base at commit `f3853c2ce3831f59f19a9d4fb097e3f9d5244398`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartResourceEvaluation

## Description
AWS Config mints its opaque resource identifiers in the canonical hyphenated UUID form (8-4-4-4-12, 36 characters, e.g. the shape class "xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"). This emulator mints them as a 32-character unhyphenated lowercase hex string — the same UUID with its hyphens stripped. The corpus asserts the SHAPE of these identifiers (the recorded value is masked as '<shaped-not-asserted>'), and the shape assertion fails on every identifier the emulator mints, on two different planes of the service. Both causes below are one fault in how the service's identifier generator formats a UUID; fixing the generator to emit the canonical hyphenated 36-character form fixes both.

Cause 1 of 2: StartResourceEvaluation.ResourceEvaluationId is an unhyphenated 32-char hex string.
  AWS: Smithy (com.amazonaws.configservice) StartResourceEvaluationResponse.ResourceEvaluationId -> ResourceEvaluationId, type string, @length min 1 max 128, NO @pattern — the model constrains only the length, so the wire format is recorded behavior only: the recording shows AWS answering 200 with a ResourceEvaluationId in the canonical hyphenated UUID shape (36 characters, five hyphen-separated hex groups). The id is echoed back by GetResourceEvaluationSummary / ListResourceEvaluations, so its text is the client's handle on the evaluation.
  Emulator: answers 200 but serves ResourceEvaluationId as a 32-character unhyphenated hex string (the replay reports `$.ResourceEvaluationId: value_mismatch (expected '<shaped-not-asserted>', got '0b2b6c82939c47b49c33e79a0e55c83e')`). The identifier is the only diverging member; status and the rest of the body match. This cluster covers 9 steps across 7 fixtures (config-list-evaluations-by-context-identifier, config-proactive-evaluation-lifecycle, config-proactive-resource-evaluation-lifecycle, config-resource-evaluation-context-grouping, config-resource-evaluation-grouped-by-context, config-start-resource-evaluation-smoke), so every StartResourceEvaluation the corpus makes diverges.
  Why: a client that validates, parses or re-renders the evaluation id as a UUID (uuid.UUID(...) in botocore-adjacent glue, a UUID column in a datastore, a UUID route segment, or a regex on the 8-4-4-4-12 form) rejects or mangles the emulator's id, so a resource-evaluation workflow that works against AWS fails against the emulator at the first hand-off of the id — even though the follow-up reads would have accepted it.
  Fix: the StartResourceEvaluation handler's identifier minting (the service's shared id/UUID generator) must emit the canonical hyphenated 36-character UUID text rather than the hex digest with separators stripped, and the evaluation state must key and echo that same text.
  Traces: config-list-evaluations-by-context-identifier#task#1 (representative); also config-list-evaluations-by-context-identifier#task#2, config-proactive-evaluation-lifecycle#task#1, config-proactive-resource-evaluation-lifecycle#task#1, config-resource-evaluation-context-grouping#task#1, config-resource-evaluation-context-grouping#task#2, config-resource-evaluation-grouped-by-context#task#1, config-resource-evaluation-grouped-by-context#task#3, config-start-resource-evaluation-smoke#task#1.

Cause 2 of 2: GetStoredQuery's StoredQuery.QueryId is an unhyphenated 32-char hex string.
  AWS: Smithy (com.amazonaws.configservice) StoredQuery.QueryId -> QueryId, type string, @length min 1 max 36, @pattern ^\S+$ — the maximum length of exactly 36 is the length of a canonical hyphenated UUID, and the recording confirms it: AWS answers 200 with QueryId in the 8-4-4-4-12 hyphenated UUID shape. The sibling StoredQuery.QueryArn carries @pattern ^arn:aws[a-z\-]*:config:[a-z\-\d]+:\d+:stored-query/[a-zA-Z0-9-_]+/query-[a-zA-Z\d-_/]+$, i.e. the same id appears as the ARN's final `query-<id>` segment, so the two must agree.
  Emulator: answers 200 but serves StoredQuery.QueryId as a 32-character unhyphenated hex string (the replay reports `$.StoredQuery.QueryId: value_mismatch (expected '<shaped-not-asserted>', got 'ad533f9f9db04d978dd8996d738318f9')`). Everything else in the StoredQuery body — QueryName, Description and the Unicode Expression round-trip — matches.
  Why: a client that treats QueryId as a UUID, or that reconstructs/validates the stored-query ARN from the id (or compares the id it parsed out of QueryArn with the QueryId field), disagrees with the emulator where it agrees with AWS; a stored-query identity that is 32 hex characters instead of the documented 36-character form also silently uses up the whole @length budget differently, so round-tripping the id through a UUID-typed field loses it.
  Fix: the PutStoredQuery/GetStoredQuery state's QueryId minting (same shared id generator) must store and serve the canonical hyphenated 36-character UUID, and the QueryArn it builds must use that identical text as its `query-<id>` segment.
  Traces: config-stored-query-expression-unicode-round-trip#task#2 (representative); also config-stored-query-unicode-and-emoji-round-trip#task#2, config-stored-query-unicode-expression-round-trip#task#2.

Note: no open issue covers either cause — the open config issues named by the replay (20261030T003924Z-config-conformance-pack-describe-responses-omit-type-and.md, 20261030T003924Z-config-conformance-pack-refusal-messages-diverge-from-aws.md, 20261030T003924Z-config-remediation-execution-and-exception-paths-never-refuse.md, 20261109T123842Z-config-listdiscoveredresources-mutual-exclusion-refusal-uses-emulator-prose.md) own the conformance-pack, remediation and ListDiscoveredResources clusters and say nothing about identifier formatting.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, 282 steps, 250 passed, 32 mismatched, score 88.7%). The two clusters below are the only divergences with an empty `owned_by` and an empty `stub_path`.

Cause 1 — service=config|StartResourceEvaluation|status=200->200|keys=$.ResourceEvaluationId
  fixture_id: config-list-evaluations-by-context-identifier, step_key: task#1, verdict: mismatch
  expected_status 200, actual_status 200
  detail: $.ResourceEvaluationId: value_mismatch (expected '<shaped-not-asserted>', got '0b2b6c82939c47b49c33e79a0e55c83e')
  affected_count: 9 — config-list-evaluations-by-context-identifier#task#1, #task#2, config-proactive-evaluation-lifecycle#task#1, config-proactive-resource-evaluation-lifecycle#task#1, config-resource-evaluation-context-grouping#task#1, #task#2, config-resource-evaluation-grouped-by-context#task#1, #task#3, config-start-resource-evaluation-smoke#task#1
  Expected shape: canonical hyphenated UUID, 36 chars, 8-4-4-4-12 hex groups. Actual shape: 32 hex chars, no hyphens. The quoted literal '0b2b6c82939c47b49c33e79a0e55c83e' is a per-run generated value and is NOT a requirement — only its SHAPE is.

Cause 2 — service=config|GetStoredQuery|status=200->200|keys=$.StoredQuery.QueryId
  fixture_id: config-stored-query-expression-unicode-round-trip, step_key: task#2, verdict: mismatch
  expected_status 200, actual_status 200
  detail: $.StoredQuery.QueryId: value_mismatch (expected '<shaped-not-asserted>', got 'ad533f9f9db04d978dd8996d738318f9')
  affected_count: 3 — config-stored-query-expression-unicode-round-trip#task#2, config-stored-query-unicode-and-emoji-round-trip#task#2, config-stored-query-unicode-expression-round-trip#task#2
  Model corroboration: QueryId @length max 36 (exactly the hyphenated-UUID length) and QueryArn @pattern requires the id as the `query-<id>` ARN segment. The quoted literal 'ad533f9f9db04d978dd8996d738318f9' is a per-run generated value; only its SHAPE is a requirement.

No status code, error code or error message diverges in either cause — both are 200/200 with a body member whose minted-identifier shape is wrong.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus against it (replay_wire_corpus).
3. Cause 1: replay fixture config-list-evaluations-by-context-identifier, step task#1 — POST the AWS Config StartResourceEvaluation call it records (EvaluationMode + ResourceDetails + EvaluationContext). Read $.ResourceEvaluationId from the 200 body: it is 32 hex characters with no hyphens, where AWS returns a 36-character hyphenated UUID. The same divergence reproduces on config-proactive-evaluation-lifecycle#task#1, config-proactive-resource-evaluation-lifecycle#task#1, config-resource-evaluation-context-grouping#task#1/#task#2, config-resource-evaluation-grouped-by-context#task#1/#task#3, config-start-resource-evaluation-smoke#task#1 and config-list-evaluations-by-context-identifier#task#2.
4. Cause 2: replay fixture config-stored-query-expression-unicode-round-trip — task#1 PutStoredQuery, then step task#2 GetStoredQuery for that QueryName. Read $.StoredQuery.QueryId from the 200 body: it is 32 hex characters with no hyphens, where AWS returns a 36-character hyphenated UUID; compare it with the `query-<id>` segment of $.StoredQuery.QueryArn. Reproduces identically on config-stored-query-unicode-and-emoji-round-trip#task#2 and config-stored-query-unicode-expression-round-trip#task#2.
5. After fixing the identifier generator, re-replay all four fixtures named in Issue-Trace/trace_ids to confirm both causes are closed.
