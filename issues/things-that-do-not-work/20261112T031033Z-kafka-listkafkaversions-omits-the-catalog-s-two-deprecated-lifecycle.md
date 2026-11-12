Issue-Source: trace-sweep-divergence
Issue-Trace: kafka-catalog-list-kafka-versions
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kafka-catalog-list-kafka-versions
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka|ListKafkaVersions|2xx->2xx|$.kafkaVersions[*]
Issue-Commit: 4c92736e4d0a5b6c9b4225b90c132864da8692c0

# ListKafkaVersions omits the catalog's two DEPRECATED lifecycle rows, inverting the published status of 3.9.x and 4.2.x.kraft

> _Note: this issue was identified against the code base at commit `4c92736e4d0a5b6c9b4225b90c132864da8692c0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListKafkaVersions

## Description
One net-new root cause, grounded in the single wire divergence below. The replay reports it as unowned: no open issue carries this fixture step's replay key.

Cause 1 of 1: the Apache Kafka version catalog the emulator publishes on GET /v1/kafka-versions is missing two of the rows the service publishes, and both missing rows are the DEPRECATED lifecycle row of a version the page also publishes as ACTIVE. Fixture kafka-catalog-list-kafka-versions, step task#1.

AWS: Smithy (services/service-com-amazonaws-kafka-kafka/smithy-model/com-amazonaws-kafka-kafka.json): ListKafkaVersions, GET /v1/kafka-versions, HTTP 200 -> ListKafkaVersionsResponse.KafkaVersions (jsonName kafkaVersions), a list of KafkaVersion{Version (jsonName version), Status (jsonName status, enum KafkaVersionStatus = ACTIVE | DEPRECATED)}, paginated by NextToken/MaxResults (MaxResults range 1..100). The model enumerates no concrete versions and says nothing about how many rows a version may have, so WHICH versions the page carries and WHICH status each row carries is recorded behavior only. What the recording shows is that the page is a sequence of published ROWS, not a version->status map: the service publishes 36 rows, and two versions appear on two rows each — '3.9.x' on an ACTIVE row and, later in the same page, on a DEPRECATED row, and '4.2.x.kraft' on an ACTIVE row followed by a DEPRECATED row as the page's last entry. The requirement is the rule the recording states: every row the service publishes for a version — including a second row carrying the other lifecycle status — must appear in the page, in the published order, so that a walk of the pages reproduces the catalog row for row. Status 200 and the response shape do not diverge here; only the rows do.

Emulator: GET /v1/kafka-versions answers 200 with 35 rows — exactly one row per version. The two DEPRECATED rows are absent: at the recorded index of the '3.9.x' DEPRECATED row the page instead carries {version: '4.0.x.kraft', status: 'ACTIVE'} (every following row is shifted up by one), and the recorded trailing {version: '4.2.x.kraft', status: 'DEPRECATED'} row has no counterpart at all (the emulator's page ends one row earlier). This is by construction, not by accident: crate::state::KAFKA_VERSION_CATALOG documents 'the catalog publishes each version it knows about EXACTLY ONCE' and holds 35 single rows, with '3.9.x' and '4.2.x.kraft' carried ACTIVE only; the handler projects that table verbatim.

Why: ListKafkaVersions is the version picker — it is what a client, the console, CDK and CloudFormation tooling read to decide which Apache Kafka version to create or upgrade a cluster onto. Against AWS the catalog reports a DEPRECATED row for '3.9.x' and for '4.2.x.kraft'; against the emulator those two versions are published as unqualified ACTIVE, so a client reads the lifecycle signal for the two newest/most-used versions inverted and will select, or keep, a version the real service publishes as deprecated. A client that walks the pages and reconciles counts also sees 35 rows where AWS serves 36, so any paging/row-count assertion over the catalog is off by one, and GetCompatibleKafkaVersions — which the emulator derives from this same table — inherits the same missing lifecycle information.

Fix: the published row table and its projection: the service's state module (services/service-com-amazonaws-kafka-kafka/src/state.rs, KAFKA_VERSION_CATALOG and the kafka_version_page/decode_kafka_version_page_token paging it feeds) and the ListKafkaVersions handler's projection (services/service-com-amazonaws-kafka-kafka/src/ops/list_kafka_versions.rs). The table must carry the rows the service publishes — including a version's second, other-status row — in published order, while the question-answering helpers (catalogued_kafka_versions / kafka_version_status, which already de-duplicate) keep driving the create/update gates, so restoring the rows does not change which versions the emulator offers.

Related open issues, which do NOT cover this fixture step (the replay reports it unowned, and both are behavioral issues keyed to check ids, not to this fixture): 20261111T015651Z-kafka-listkafkaversions-publishes-3-9-x-s-deprecated.md and 20261111T115530Z-kafka-listkafkaversions-publishes-a-second-deprecated-3-9.md both argue the opposite direction — that a duplicate '3.9.x' row whose DEPRECATED entry comes last makes a client folding the page to a version->status map read 3.9.x as deprecated. Those issues and this one are about the same rows, so a fixer must satisfy both at once rather than deleting the rows again: the page must carry the recorded rows in the recorded order (this issue, which the corpus is the authority for) while the emulator's own version-aware answers keep reading the de-duplicated version set. Removing the rows to satisfy the folded-map reading is what reopened this wire divergence.

Traces: kafka-catalog-list-kafka-versions#task#1 (GET /v1/kafka-versions).

## Evidence
Fixture kafka-catalog-list-kafka-versions, step task#1, ListKafkaVersions — verdict mismatch, expected status 200, actual status 200; only the kafkaVersions rows diverge:

  $.kafkaVersions[32]: missing (expected {'version': '3.9.x', 'status': 'DEPRECATED'}, got {'version': '4.0.x.kraft', 'status': 'ACTIVE'})
  $.kafkaVersions[35]: missing (expected {'version': '4.2.x.kraft', 'status': 'DEPRECATED'}, got None)

Expected (AWS, recorded): 200 with 36 kafkaVersions rows; '3.9.x' is published twice (ACTIVE, then DEPRECATED at index 32) and '4.2.x.kraft' is published twice (ACTIVE, then DEPRECATED as the final row at index 35).
Actual (emulator): 200 with 35 kafkaVersions rows, one per version; neither DEPRECATED row is present, so index 32 holds the next version's row ('4.0.x.kraft' ACTIVE) and index 35 does not exist.

Source of the actual page: services/service-com-amazonaws-kafka-kafka/src/state.rs lines 882-918 (KAFKA_VERSION_CATALOG — 35 entries, '3.9.x' at line 913 ACTIVE only, '4.2.x.kraft' at line 917 ACTIVE only; the doc comment at lines 865-872 asserts one row per version) and services/service-com-amazonaws-kafka-kafka/src/ops/list_kafka_versions.rs lines 205-217 (the page is projected row for row from that table).

No HTTP status, error code or error message diverges on this step. The version strings and their statuses are the service's published catalog content, not per-run values; nothing in this response is account-, host- or clock-dependent.

## Steps to Reproduce
1. Boot the emulator from this checkout (the kafka service answers).
2. Call ListKafkaVersions with no MaxResults: `aws kafka list-kafka-versions` (GET /v1/kafka-versions).
3. Count the rows in kafkaVersions: the emulator serves 35, the recorded service serves 36.
4. Filter the rows for Version == '3.9.x': the emulator returns one row (ACTIVE); the recording carries two — ACTIVE and then DEPRECATED (recorded index 32).
5. Filter the rows for Version == '4.2.x.kraft': the emulator returns one row (ACTIVE); the recording carries two — ACTIVE and then DEPRECATED as the page's last row (recorded index 35).
6. Re-replay fixture kafka-catalog-list-kafka-versions, step task#1, to confirm the page matches the recorded rows index for index.
