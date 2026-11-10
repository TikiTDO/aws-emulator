Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mq.native-protocol-stomp-roundtrip-activemq mq.rabbitmq-dead-letter-exchange-takes-unrequeued-and-expired-messages-with-their-death-reason mq.rabbitmq-durable-queue-holds-confirmed-persistent-messages-across-connections-in-order
Issue-Shape-Id: com.amazonaws.mq#mq
Issue-Key: mq.native-protocol-stomp-roundtrip-activemq|_stomp_connect#26|ConnectionRefusedError|raw = socket.create_connection((host, port), timeout=timeout_s)
Issue-Key: mq.rabbitmq-dead-letter-exchange-takes-unrequeued-and-expired-messages-with-their-death-reason|_amqp_session#3|AssertionError|raise AssertionError(
Issue-Key: mq.rabbitmq-durable-queue-holds-confirmed-persistent-messages-across-connections-in-order|_amqp_session#4|TimeoutError|raise TimeoutError(f"connecting to {host}:{port}: the run budget ended first; not graded") from exc
Issue-Commit: 76d01eabdbc8750ace4af5212f225bb5f29fbdd8

# mq: the broker endpoints DescribeBroker now advertises have no listener — neither ActiveMQ STOMP+SSL (61614) nor RabbitMQ AMQPS (5671) accepts a connection, so Amazon MQ has no data plane (2 root causes)

> _Note: this issue was identified against the code base at commit `76d01eabdbc8750ace4af5212f225bb5f29fbdd8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeBroker

## Description
TWO net-new root causes, grounded in three failing behavioral checks whose replay keys are carried by NO open issue. These checks have moved PAST endpoint discovery: DescribeBroker now does serve brokerInstances with wire endpoints (the gap described by the open issue 20261109T143402Z-mq-describebroker-serves-no-brokerinstances-for-a-running.md and by Cause 5 of 20261104T044607Z-mq-broker-lifecycle-rebootbroker-and-promote-are-unimplemented.md, whose replay keys are the older `advertised no amqps:// endpoint: []` assertions). What fails now is one step later and is not described by either issue: the address the emulator publishes is not listened on, so the broker's native message-broker protocol is entirely absent. Neither issue is marked fixed and neither covers these new keys, so this is filed separately and names them here.

Cause 1 of 2: an ACTIVEMQ broker in state RUNNING advertises a stomp+ssl:// endpoint in BrokerInstances[].Endpoints, but nothing accepts TCP on that endpoint's port (61614), so no STOMP session can be opened and no message can be sent or subscribed. Failing check: mq.native-protocol-stomp-roundtrip-activemq.
AWS: Smithy (services/service-com-amazonaws-mq-mq/smithy-model/com-amazonaws-mq-mq.json) declares BrokerInstance.Endpoints (jsonName "endpoints") as "The broker's wire-level protocol endpoints". Amazon MQ for ActiveMQ serves those endpoints: the stomp+ssl:// URI is a live STOMP-over-TLS listener on port 61614 that answers a CONNECT frame with a CONNECTED frame, accepts SEND to /queue/<name> and delivers the message body unchanged to a SUBSCRIBE on the same destination. Recorded behavior only for the frame exchange (the Smithy model covers the control plane; it does not model the wire protocol) — the recording shows CONNECT -> CONNECTED, then SEND + SUBSCRIBE returning a MESSAGE frame whose body equals the sent payload. Host, port-reachability path, queue name and payload are per-run values; the requirement is that the advertised endpoint is an actual STOMP+SSL listener that round-trips a message.
Emulator: the TCP connect to the advertised host:port is refused outright — `ConnectionRefusedError: [Errno 111] Connection refused` from socket.create_connection, before any TLS handshake or STOMP frame. The control plane returns 200 for CreateBroker, reports BrokerState RUNNING and publishes the stomp+ssl:// endpoint, but no process is bound to it.
Why: a client built against the emulator can discover the broker's endpoint and then cannot use it. Every ActiveMQ client (STOMP, OpenWire, AMQP, MQTT libraries) connects to exactly that published URI; a refused connection is indistinguishable from a broker outage, so any application messaging flow — produce, consume, request/reply — cannot be exercised at all, while the control plane claims a healthy RUNNING broker. The emulator announces an address other than one it is reached on.
Fix: the mq broker data plane — the component that must bind the broker's wire listeners when a broker reaches RUNNING and tear them down on DeleteBroker, paired with the endpoint publication in the DescribeBroker/CreateBroker response construction so the published URI and the bound listener are the same address. It needs a STOMP 1.1 frame server over TLS (CONNECT/CONNECTED with login+passcode checked against the broker's users, SEND, SUBSCRIBE with ack:auto, MESSAGE, DISCONNECT) and per-broker queue state.
Traces: mq.native-protocol-stomp-roundtrip-activemq.

Cause 2 of 2: a RABBITMQ broker in state RUNNING advertises an amqps:// endpoint (AMQP TLS port 5671), but nothing accepts TCP there, so no AMQP 0-9-1 session can be established and none of the RabbitMQ queue semantics can be exercised. Failing checks: mq.rabbitmq-dead-letter-exchange-takes-unrequeued-and-expired-messages-with-their-death-reason, mq.rabbitmq-durable-queue-holds-confirmed-persistent-messages-across-connections-in-order.
AWS: Smithy declares the same BrokerInstance.Endpoints member ("The broker's wire-level protocol endpoints"); for RabbitMQ it carries one amqps:// URI on 5671 and IpAddress "Does not apply to RabbitMQ brokers". Amazon MQ for RabbitMQ serves that endpoint as a real AMQP 0-9-1 broker. Recorded behavior only for the protocol semantics: the recordings show a TLS AMQP 0-9-1 connection authenticated with the broker user, exchange.declare / queue.declare / queue.bind succeeding, publisher confirms, durable queues retaining persistent messages across the producer's connection and redelivering them in publish order with their properties, a reject(requeue=false) and an expired per-message TTL both republished through the queue's x-dead-letter-exchange with the configured routing key, each carrying exactly one x-death entry whose reason is "rejected"/"expired" and whose queue is the source queue with count 1, plus x-first-death-reason, and the expiration property removed from the dead-lettered message.
Emulator: the connection to the advertised endpoint is refused — the check reports `DATAPLANE_UNIMPLEMENTED: the advertised endpoint <host>:5671 accepted no AMQP session from inside its VPC (ConnectionRefusedError(111, 'Connection refused')); an endpoint that cannot be connected to is not a data plane`. The companion durable-queue check fails on the same connect to the same port and only stops reporting a refusal because the retry loop ran out of run budget (`TimeoutError: connecting to <host>:5671: the run budget ended first`); it is the identical refusal, included here only as a second example of this cause.
Why: a RabbitMQ consumer — an AMQP client library, or a Lambda event source mapping polling the broker — has no way to attach. Anything relying on RabbitMQ delivery guarantees (durability, confirms, ordering, dead-letter routing with x-death provenance) is untestable and silently non-functional against the emulator even though DescribeBroker reports RUNNING with an endpoint.
Fix: the same mq broker data plane component as Cause 1, with the RabbitMQ engine's listener: AMQP 0-9-1 over TLS on the published port, user authentication against the broker's users, exchange/queue/binding declaration, publisher confirms, durable+persistent message retention independent of the publishing connection, FIFO delivery with properties and headers preserved, redelivery on requeue, and x-dead-letter-exchange / x-dead-letter-routing-key handling that stamps x-death (reason, queue, count) and x-first-death-reason and drops the expiration property.
Traces: mq.rabbitmq-dead-letter-exchange-takes-unrequeued-and-expired-messages-with-their-death-reason, mq.rabbitmq-durable-queue-holds-confirmed-persistent-messages-across-connections-in-order.

## Evidence
Behavioral suite simparity-behavioral-aws-v1: 12/28 passed (42.9%); service mq 12/23 (52%), 11 failed. Eight of those eleven are excluded here as already carried: mq.delete-user-last-admin-accepted is ALREADY FILED (RebootBroker stub), and mq.create-configuration-duplicate-name-conflict-rejected, mq.create-user-weak-password-rejected, mq.delete-broker-removes-it, mq.promote-non-replica-broker-rejected, mq.reboot-broker-applies-pending-user-change, mq.reboot-broker-wrong-state-rejected are marked OWNED by open issues. The three below are marked neither.

Cause 1 — mq.native-protocol-stomp-roundtrip-activemq [error]:
  File "socket.py", line 865, in create_connection
    raise exceptions[0]
  File "socket.py", line 850, in create_connection
    sock.connect(sa)
  ConnectionRefusedError: [Errno 111] Connection refused
The check CreateBroker(EngineType=ACTIVEMQ, DeploymentMode=SINGLE_INSTANCE, HostInstanceType=mq.t3.micro, PubliclyAccessible=false, one user), polls DescribeBroker to RUNNING, asserts BrokerInstances is non-empty and selects the endpoint whose scheme is stomp+ssl:// — both of which now succeed — then opens a TLS socket to that host:61614 and sends a STOMP CONNECT. Expected: a CONNECTED frame, then SEND to /queue/<q> followed by SUBSCRIBE returning a MESSAGE frame whose body equals the sent payload. Actual: the TCP connect is refused; no listener exists on the advertised port.

Cause 2 — mq.rabbitmq-dead-letter-exchange-takes-unrequeued-and-expired-messages-with-their-death-reason [mismatch]:
  File "check_mq_rabbitmq_dead_letter_exchange_...py", line 44, in run
    amqp = _amqp_session(ctx, endpoint, username, password)
  File "...", line 620, in _amqp_session
    raise AssertionError(
  AssertionError: DATAPLANE_UNIMPLEMENTED: the advertised endpoint 127.0.0.1:5671 accepted no AMQP session from inside its VPC (ConnectionRefusedError(111, 'Connection refused')); an endpoint that cannot be connected to is not a data plane
(the host:port in that message is this run's environment value; the requirement is that whatever address DescribeBroker publishes accepts an AMQP 0-9-1 session.)

Cause 2, second example — mq.rabbitmq-durable-queue-holds-confirmed-persistent-messages-across-connections-in-order [error]:
  File "check_mq_rabbitmq_durable_queue_...py", line 32, in run
    producer = _amqp_session(ctx, endpoint, username, password)
  File "...", line 628, in _amqp_session
    raise TimeoutError(f"connecting to {host}:{port}: the run budget ended first; not graded") from exc
  TimeoutError: connecting to 127.0.0.1:5671: the run budget ended first; not graded
Same helper, same refused connect to the published amqps:// port; the retry loop merely exhausted the run budget before giving up.

Both RabbitMQ checks got past the endpoint-discovery assertion (`a RUNNING RabbitMQ broker advertised no amqps:// endpoint: []`) that the open issue 20261109T143402Z-mq-describebroker-serves-no-brokerinstances-for-a-running.md carries as its replay keys, so their current failure is a distinct, net-new cause; that issue's own prose flags the missing listener only as a Hypothesis.

## Steps to Reproduce
1. Clone the repo, build and boot the emulator from the checkout, and replay the behavioral suite (simparity-behavioral-aws-v1), service mq.
2. Cause 1 (ActiveMQ STOMP): CreateBroker with EngineType=ACTIVEMQ, DeploymentMode=SINGLE_INSTANCE, HostInstanceType=mq.t3.micro, PubliclyAccessible=false, one subnet, one security group and one user. Poll GET /v1/brokers/{BrokerId} until brokerState is RUNNING, read brokerInstances[0].endpoints and pick the stomp+ssl:// URI.
3. Open a TCP connection to that URI's host and port (61614) and wrap it in TLS. Expect the connection to be accepted and a STOMP CONNECT (accept-version:1.1, login/passcode = the created user) to be answered with a CONNECTED frame; then SEND a body to /queue/<name>, SUBSCRIBE to the same destination with ack:auto and expect a MESSAGE frame carrying that body. Observed: the TCP connect fails with ECONNREFUSED.
4. Cause 2 (RabbitMQ AMQP): CreateBroker with EngineType=RABBITMQ, EngineVersion=3.13, DeploymentMode=SINGLE_INSTANCE, HostInstanceType=mq.m5.large, PubliclyAccessible=false, one user. Poll until RUNNING and read the amqps:// endpoint (port 5671) from brokerInstances[0].endpoints.
5. Open a TLS connection to that host:port and start an AMQP 0-9-1 session authenticated as the created user. Observed: the TCP connect fails with ECONNREFUSED, so no session is established.
6. With a listener present, the checks continue on it: (a) declare a direct exchange and a durable classic queue, bind it, publish persistent messages with publisher confirms, close the producer connection, reconnect and consume them in publish order with content_type/delivery_mode/message_id/headers intact, verify a reject(requeue=true) comes back redelivered and acks leave the queue empty; (b) declare a dead-letter exchange and queue, declare a work queue with x-dead-letter-exchange/x-dead-letter-routing-key, reject one message without requeue and let another expire via a per-message TTL, and expect both on the dead-letter queue, republished through that exchange with routing key 'dead', each with one x-death entry (reason 'rejected'/'expired', queue = the work queue, count 1), x-first-death-reason set, the expiration property removed, and the work queue left empty.
