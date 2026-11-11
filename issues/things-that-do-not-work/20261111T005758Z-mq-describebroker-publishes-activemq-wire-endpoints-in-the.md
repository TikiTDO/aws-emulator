Issue-Source: trace-sweep-divergence
Issue-Trace: mq-activemq-broker-endpoint-and-user-discovery
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mq-activemq-broker-endpoint-and-user-discovery
Issue-Shape-Id: com.amazonaws.mq#mq
Issue-Key: mq|DescribeBroker|2xx->2xx|$.brokerInstances[*].endpoints[*]
Issue-Key: mq|DescribeBroker|2xx->2xx|$.brokerInstances[*].ipAddress
Issue-Key: mq|DescribeBroker|2xx->2xx|$.logGroups
Issue-Commit: 19d1c69ab09d72ffddfacb1632b30705ba84d18f

# mq DescribeBroker publishes ActiveMQ wire endpoints in the wrong order and omits the broker's logGroups

> _Note: this issue was identified against the code base at commit `19d1c69ab09d72ffddfacb1632b30705ba84d18f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeBroker

## Description
TWO net-new root causes in the DescribeBroker 200 body for a RUNNING ActiveMQ broker. Both are grounded in the same replayed step (fixture mq-activemq-broker-endpoint-and-user-discovery, step task#7) and neither is carried by any open issue's replay keys. Both causes must be closed before this issue is submitted.

Cause 1 of 2: brokerInstances[0].endpoints lists an ActiveMQ broker's wire-protocol endpoints in alphabetical order of URI scheme instead of the order Amazon MQ publishes them in.
AWS: Smithy (services/service-com-amazonaws-mq-mq/smithy-model/com-amazonaws-mq-mq.json) gives DescribeBroker output BrokerInstances (jsonName "brokerInstances", list of BrokerInstance), and BrokerInstance$Endpoints (jsonName "endpoints", __listOf__string, documented "The broker's wire-level protocol endpoints"). The model does not state the member order, so for the order this is recorded behavior only: the recorded Amazon MQ 200 body for a SINGLE_INSTANCE ACTIVEMQ broker lists the endpoints by wire protocol — OpenWire first (scheme ssl, port 61617), then AMQP (amqp+ssl, 5671), then STOMP (stomp+ssl, 61614), then MQTT (mqtt+ssl, 8883). The ordering is the requirement; the host part of each URI (and the broker-id-derived hostname in the recording) is an environment value and is not.
Emulator: the same four protocols are published with the same schemes and ports, but ordered amqp+ssl:5671, mqtt+ssl:8883, ssl:61617, stomp+ssl:61614 — i.e. sorted by scheme string. Replay detail for task#7: $.brokerInstances[0].endpoints[0] expected 'ssl://<broker-host>:61617', got 'amqp+ssl://127.0.0.1:5671'; endpoints[1] expected 'amqp+ssl://<broker-host>:5671', got 'mqtt+ssl://127.0.0.1:8883'; endpoints[2] expected 'stomp+ssl://<broker-host>:61614', got 'ssl://127.0.0.1:61617'; endpoints[3] expected 'mqtt+ssl://<broker-host>:8883', got 'stomp+ssl://127.0.0.1:61614'. Source: services/service-com-amazonaws-mq-mq/src/state/broker_endpoints.rs, const ACTIVEMQ_WIRE_PROTOCOLS at lines 45-66, whose doc comment claims "the order Amazon MQ publishes them in: AMQP, MQTT, OpenWire, STOMP and WebSocket — by protocol name" but whose entries are in fact ordered by scheme spelling; consumed by Broker::allocated_instances (lines 143-157) and serialized in services/service-com-amazonaws-mq-mq/src/ops/describe_broker.rs lines 263-274.
Why: a client that does not parse every URI takes endpoints[0] as the broker's primary (OpenWire/JMS) endpoint — that is what the AWS console, the Developer Guide's connection snippets and ActiveMQ JMS wiring all do. Against the emulator endpoints[0] is the AMQP endpoint, so an OpenWire client dials the wrong port with the wrong protocol and the connection never completes; any code that indexes the list positionally (endpoints[0] for OpenWire, [2] for STOMP) binds to the wrong protocol everywhere.
Fix: the broker-endpoint state in services/service-com-amazonaws-mq-mq/src/state/broker_endpoints.rs — reorder the ActiveMQ protocol table to the published protocol order (OpenWire, AMQP, STOMP, MQTT, then WebSocket) and correct the comment that asserts the current order is AWS's.
Traces: mq-activemq-broker-endpoint-and-user-discovery#task#7.

Cause 2 of 2: the DescribeBroker 200 body carries no logGroups member.
AWS: recorded behavior only — the Smithy model's DescribeBrokerResponse declares Logs (jsonName "logs", LogsSummary with audit/auditLogGroup/general/generalLogGroup/pending) and no logGroups member, so this is not modeled. The recorded Amazon MQ response additionally carries a top-level logGroups array with one entry per log kind the broker's engine has, each entry carrying the log kind's name ("general", "audit"), the enabled flag for that kind on this broker, and the CloudWatch Logs log-group path Amazon MQ derives from the broker's own id, /aws/amazonmq/broker/<brokerId>/<name>. The fixed parts are the member and entry names, the two log kinds and their order (general then audit) and the log-group path template; the broker id in the path and the enabled values come from the request and the emulator's state (both false for a broker created with logging off).
Emulator: the member is absent entirely. Replay detail for task#7: $.logGroups: missing (expected a two-entry list of {name, enabled, logGroup} for general and audit, got None). The handler builds only the modeled logs/LogsSummary — services/service-com-amazonaws-mq-mq/src/ops/describe_broker.rs lines 287-301 — and nothing emits logGroups.
Why: a client that enumerates a broker's CloudWatch log groups from logGroups (to subscribe to them, to build console deep links, or to assert logging is off) finds the member missing and reads it as a broker with no log groups at all, which Amazon MQ never returns; the per-kind enabled flag that pairs each path with its state is unavailable too.
Fix: DescribeBroker's response construction/serializer in services/service-com-amazonaws-mq-mq/src/ops/describe_broker.rs, plus the log-group derivation already in state (Broker::log_group with LOG_KIND_GENERAL/LOG_KIND_AUDIT) — emit the logGroups array alongside logs, deriving each path from the broker id and each enabled flag from the broker's stored logging settings, including the audit entry only for engines that have audit logging.
Traces: mq-activemq-broker-endpoint-and-user-discovery#task#7.

Relation to existing issues: 20261109T143402Z-mq-describebroker-serves-no-brokerinstances-for-a-running.md and cause 5 of 20261104T044607Z-mq-broker-lifecycle-rebootbroker-and-promote-are-unimplemented.md cover brokerInstances being absent altogether (it is now served), and 20261110T231924Z-mq-the-broker-endpoints-describebroker-now-advertises-have.md covers the advertised endpoints having no listener. None of them carries this fixture step or mentions the endpoint ORDER or the logGroups member, so neither cause here is covered by them.

## Evidence
Wire replay of simparity-wire-aws-v1 against the emulator built from this checkout: 68 steps, 44 passed, score 64.7%; 16 divergence clusters, of which this is the only one with no owning issue and no blocking stub.

Cluster: service=mq | DescribeBroker | status=200->200, fixture mq-activemq-broker-endpoint-and-user-discovery step task#7, verdict mismatch, expected_status 200, actual_status 200 (the status and the rest of the body are correct; only these members diverge):

  $.brokerInstances[0].endpoints[0]: value_mismatch (expected 'ssl://b-<id>-1.mq.<region>.amazonaws.com:61617', got 'amqp+ssl://127.0.0.1:5671')
  $.brokerInstances[0].endpoints[1]: value_mismatch (expected 'amqp+ssl://b-<id>-1.mq.<region>.amazonaws.com:5671', got 'mqtt+ssl://127.0.0.1:8883')
  $.brokerInstances[0].endpoints[2]: value_mismatch (expected 'stomp+ssl://b-<id>-1.mq.<region>.amazonaws.com:61614', got 'ssl://127.0.0.1:61617')
  $.brokerInstances[0].endpoints[3]: value_mismatch (expected 'mqtt+ssl://b-<id>-1.mq.<region>.amazonaws.com:8883', got 'stomp+ssl://127.0.0.1:61614')
  $.logGroups: missing (expected [{'name': 'general', 'enabled': false, 'logGroup': '/aws/amazonmq/broker/<brokerId>/general'}, {'name': 'audit', 'enabled': false, 'logGroup': '/aws/amazonmq/broker/<brokerId>/audit'}], got None)

The same cluster also reports $.brokerInstances[0].ipAddress (expected a shaped value, got '127.0.0.1') and the broker-id-derived hostname inside each endpoint URI. Those are per-environment addresses, not requirements, and are deliberately NOT part of this issue: the emulator publishes the host it is reachable on. Only the ORDER of the endpoint list (cause 1) and the absent logGroups member (cause 2) are filed.

Expected vs actual, as behavior:
  Expected (recorded AWS): endpoints ordered by wire protocol — OpenWire ssl:61617, AMQP amqp+ssl:5671, STOMP stomp+ssl:61614, MQTT mqtt+ssl:8883 — and a logGroups array naming the general and audit CloudWatch log groups with their enabled flags.
  Actual (emulator): endpoints ordered by scheme string — amqp+ssl:5671, mqtt+ssl:8883, ssl:61617, stomp+ssl:61614 — and no logGroups member.

Source: services/service-com-amazonaws-mq-mq/src/state/broker_endpoints.rs lines 45-66 (ACTIVEMQ_WIRE_PROTOCOLS) and 143-157 (allocated_instances); services/service-com-amazonaws-mq-mq/src/ops/describe_broker.rs lines 263-274 (brokerInstances) and 287-301 (logs, with no logGroups beside it).

## Steps to Reproduce
1. Clone the repo, build and boot the emulator from the checkout.
2. Replay the wire corpus simparity-wire-aws-v1, fixture mq-activemq-broker-endpoint-and-user-discovery, step task#7 (or reproduce it by hand with steps 3-6).
3. CreateBroker with EngineType=ACTIVEMQ, DeploymentMode=SINGLE_INSTANCE, a supported HostInstanceType and EngineVersion, PubliclyAccessible=false, one subnet, one security group, one user, and logging left off.
4. Poll GET /v1/brokers/{BrokerId} until brokerState is RUNNING.
5. Read brokerInstances[0].endpoints from that 200 body. Expect the list ordered by wire protocol: ssl://<host>:61617 (OpenWire) first, then amqp+ssl://<host>:5671, then stomp+ssl://<host>:61614, then mqtt+ssl://<host>:8883. The emulator returns them sorted by scheme instead: amqp+ssl, mqtt+ssl, ssl, stomp+ssl.
6. Read the top-level logGroups member of the same body. Expect a two-entry array: {name: general, enabled: <general logging flag>, logGroup: /aws/amazonmq/broker/<brokerId>/general} then {name: audit, enabled: <audit logging flag>, logGroup: /aws/amazonmq/broker/<brokerId>/audit}. The emulator omits the member entirely (it serves only the modeled logs/LogsSummary object).
