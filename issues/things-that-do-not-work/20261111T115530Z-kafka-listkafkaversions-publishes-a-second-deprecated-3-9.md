Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.topic-retention-ms-expires-old-messages kafka.topic-retention-ttl-expiry-hides-old-messages
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.topic-retention-ms-expires-old-messages|_pick_kafka_version#6|AssertionError|raise AssertionError(f"Kafka 3.9.x is not ACTIVE in ListKafkaVersions: {sorted(status)}")
Issue-Key: kafka.topic-retention-ttl-expiry-hides-old-messages|_pick_kafka_version#4|AssertionError|raise AssertionError(f"Kafka 3.9.x is not ACTIVE in ListKafkaVersions: {sorted(status)}")
Issue-Commit: f72868a03d9f80c65b4a6e1240a63621dec7938e

# kafka: ListKafkaVersions publishes a second, DEPRECATED 3.9.x row after the ACTIVE one, so a client folding the catalog reads the only offered standard-broker version as deprecated (blocks both topic-retention expiry checks)

> _Note: this issue was identified against the code base at commit `f72868a03d9f80c65b4a6e1240a63621dec7938e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListKafkaVersions

## Description
One root cause, grounded in the two failing behavioral checks below. Both are the MSK topic-retention expiry checks; neither's replay key is carried by any open issue, so neither was reported as owned by the suite.

Cause 1 of 1: ListKafkaVersions publishes the version 3.9.x TWICE in the same page — an ACTIVE row and, later in the same page, a DEPRECATED row — so the ordinary client read (fold the page into a version -> status map, last row per version wins) yields 3.9.x -> DEPRECATED. Failing checks: kafka.topic-retention-ms-expires-old-messages, kafka.topic-retention-ttl-expiry-hides-old-messages.

AWS: Smithy (services/service-com-amazonaws-kafka-kafka/smithy-model/com-amazonaws-kafka-kafka.json): ListKafkaVersions, GET /v1/kafka-versions, HTTP 200 -> ListKafkaVersionsResponse.KafkaVersions (jsonName kafkaVersions) of KafkaVersion{Version, Status}, Status an enum KafkaVersionStatus = ACTIVE | DEPRECATED; paginated by NextToken/MaxResults. The model enumerates no concrete versions, so WHICH version carries WHICH status is recorded behavior only. The recorded behavior these checks encode is that 3.9.x is a version MSK currently offers for standard (non-Express) provisioned brokers: a client reads the catalog, requires Status == ACTIVE for 3.9.x, and then calls CreateCluster/CreateClusterV2 with KafkaVersion=3.9.x, which AWS accepts. The requirement is the rule, not one run's literal rows: a version must appear in the catalog with the status the service actually honours for it, and must not be published under two conflicting statuses, because the response is read as a version -> status mapping.

Emulator: GET /v1/kafka-versions answers 200 with 37 rows in which '3.9.x' appears twice — ACTIVE and then DEPRECATED — alongside a second duplicate ('4.2.x.kraft' DEPRECATED then ACTIVE). Folded by version, 3.9.x therefore reads DEPRECATED. The service's own gates contradict the page it serves: the offered-version predicate is computed over the de-duplicated version SET (catalogued_kafka_versions()), so CreateClusterV2 with Provisioned.KafkaVersion='3.9.x' is accepted and mints a CREATING cluster even though the catalog the same service publishes reports that version as deprecated.

Why: a client that enumerates ListKafkaVersions to choose a version — the documented purpose of the operation, and what CDK/CloudFormation tooling and every version picker does — concludes that the only Apache Kafka version MSK still offers for standard brokers is deprecated, and either refuses to create a cluster or steers onto a version MSK would refuse. For these two checks it is fatal at setup: neither reaches the behaviour it measures (broker-side retention.ms expiry advancing a partition's earliest offset, and a fresh earliest-offset consumer no longer seeing an expired record), so nothing about MSK retention is exercised at all.

Fix: the shared published version catalog in the service's state module and the projection in the ListKafkaVersions handler — a version the service still offers must not be published with DEPRECATED as the last status a reader of the page sees for it, and the catalog's rows must agree with the create/update gates about whether 3.9.x is offered.

Related open issue, which does NOT cover these two checks (it carries neither check's replay key, and the suite did not mark these as owned): 20261111T015651Z-kafka-listkafkaversions-publishes-3-9-x-s-deprecated.md reports the same catalog defect from a different set of checks. These two failures are grounded in the same catalog rows, so a single correction should close all of them; they are filed here because they are unowned work items, not because they are a different defect.

Traces: kafka.topic-retention-ms-expires-old-messages (abort in _pick_kafka_version, check line 157, called from _create_msk_cluster line 173) and kafka.topic-retention-ttl-expiry-hides-old-messages (abort in _pick_kafka_version, check line 97, called from run line 34). Catalog rows: services/service-com-amazonaws-kafka-kafka/src/state.rs KAFKA_VERSION_CATALOG lines 895 ('3.9.x' ACTIVE) and 897 ('3.9.x' DEPRECATED); the de-duplicated set the gates use is catalogued_kafka_versions() at state.rs lines 916-927; the page is projected by src/ops/list_kafka_versions.rs.

## Evidence
Both checks abort identically before any MSK call that measures retention:

  kafka.topic-retention-ms-expires-old-messages
    File "check_kafka_topic_retention_ms_expires_old_messages.py", line 173, in _create_msk_cluster
      KafkaVersion=_pick_kafka_version(ctx),
    File "check_kafka_topic_retention_ms_expires_old_messages.py", line 157, in _pick_kafka_version
      raise AssertionError(f"Kafka 3.9.x is not ACTIVE in ListKafkaVersions: {sorted(status)}")
    AssertionError: Kafka 3.9.x is not ACTIVE in ListKafkaVersions: ['1.1.1', ..., '3.8.x.kraft', '3.9.x', '3.9.x.kraft', '4.0.x.kraft', '4.1.x.kraft', '4.2.x.kraft']

  kafka.topic-retention-ttl-expiry-hides-old-messages
    File "check_kafka_topic_retention_ttl_expiry_hides_old_messages.py", line 34, in run
      version = _pick_kafka_version(ctx)
    File "check_kafka_topic_retention_ttl_expiry_hides_old_messages.py", line 97, in _pick_kafka_version
      raise AssertionError(f"Kafka 3.9.x is not ACTIVE in ListKafkaVersions: {sorted(status)}")

The check's read is `{v['Version']: v['Status'] for v in resp['KafkaVersions']}` and then `status.get('3.9.x') != 'ACTIVE'`.

Expected (AWS): HTTP 200; folded version -> status yields '3.9.x' -> 'ACTIVE', and CreateCluster(KafkaVersion='3.9.x') is accepted.
Actual (emulator): HTTP 200 with '3.9.x' published twice — ACTIVE (state.rs:895) and then DEPRECATED (state.rs:897) — so the fold yields '3.9.x' -> 'DEPRECATED'; the same emulator nevertheless accepts CreateClusterV2 with KafkaVersion='3.9.x' because its gates read the de-duplicated version set.

No status code or error code diverges here: the divergence is entirely in the body's version/status rows. The concrete version strings are the recorded catalog contents, not per-run values; cluster ARNs, topic names and timestamps in these checks are per-run and are not part of the requirement.

## Steps to Reproduce
1. Boot the emulator from this checkout (the kafka service answers).
2. Call ListKafkaVersions: `aws kafka list-kafka-versions` (GET /v1/kafka-versions).
3. Fold the response as any client does: `{v['Version']: v['Status'] for v in resp['KafkaVersions']}`.
4. Observe '3.9.x' -> 'DEPRECATED': the page carries two '3.9.x' rows and the DEPRECATED one is published last. (The same duplication exists for '4.2.x.kraft', in the opposite order.)
5. Call CreateClusterV2 with Provisioned.KafkaVersion='3.9.x' — it is accepted and the cluster goes CREATING, contradicting step 4.
6. Re-replay kafka.topic-retention-ms-expires-old-messages and kafka.topic-retention-ttl-expiry-hides-old-messages; each currently stops in its `_pick_kafka_version` helper before creating a cluster.
