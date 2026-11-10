Issue-Source: trace-sweep-divergence
Issue-Trace: mq-activemq-broker-endpoint-and-user-discovery
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mq-activemq-broker-endpoint-and-user-discovery
Issue-Shape-Id: com.amazonaws.mq#mq
Issue-Key: mq|DescribeBroker|2xx->2xx|$.authenticationStrategy
Issue-Key: mq|DescribeBroker|2xx->2xx|$.brokerInstances
Issue-Key: mq|DescribeBroker|2xx->2xx|$.configurations
Issue-Key: mq|DescribeBroker|2xx->2xx|$.encryptionOptions
Issue-Key: mq|DescribeBroker|2xx->2xx|$.engineType
Issue-Key: mq|DescribeBroker|2xx->2xx|$.engineVersion
Issue-Key: mq|DescribeBroker|2xx->2xx|$.logGroups
Issue-Key: mq|DescribeBroker|2xx->2xx|$.logs.audit
Issue-Key: mq|DescribeBroker|2xx->2xx|$.logs.auditLogGroup
Issue-Key: mq|DescribeBroker|2xx->2xx|$.maintenanceWindowStartTime
Issue-Key: mq|DescribeBroker|2xx->2xx|$.storageType
Issue-Key: mq|DescribeUser|2xx->2xx|$.groups
Issue-Key: mq|DescribeUser|4xx->4xx|$.message
Issue-Commit: e38416fc0970ac2ae0c65df6bda0d7aff4be2337

# mq broker and user reads serve incomplete DescribeBroker bodies, mis-cased enums and non-AWS DescribeUser responses (3 root causes)

> _Note: this issue was identified against the code base at commit `e38416fc0970ac2ae0c65df6bda0d7aff4be2337`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeBroker

## Description
THREE net-new root causes in the mq broker/user read path, all in service mq and all grounded in failing wire-fixture steps of the recorded corpus simparity-wire-aws-v1. Model references are to services/service-com-amazonaws-mq-mq/smithy-model/com-amazonaws-mq-mq.json. None of the three is owned by an existing open issue: the replay reports owned_by=[] for all three. The nearest open issues are 20261104T044607Z-mq-broker-lifecycle-rebootbroker-and-promote-are-unimplemented.md (a BEHAVIORAL issue whose Cause 5 covers the absent brokerInstances member only, and whose Cause 3 covers the absent DescribeUser pending member - it says nothing about the other eight omitted DescribeBroker members, nothing about enum casing or engineVersion resolution, nothing about the empty groups list, and nothing about the DescribeUser not-found message), 20261103T045546Z-mq-refuses-invalid-requests-with-the-wrong-status.md (CreateBroker/ListBrokers/UpdateConfiguration/DescribeConfigurationRevision refusals), 20261103T045546Z-mq-response-bodies-carry-wrong-revision-state-and.md (configuration revisions and instance-option storageType) and 20261031T013311Z-mq-configuration-revision-reads-diverge-from-recorded-amazon.md (configuration revision reads). All THREE causes below must be closed before this issue is submitted.

Cause 1 of 3: DescribeBroker's 200 body omits most of the broker's modelled state and mis-serializes the members it does emit. Fixture mq-activemq-broker-endpoint-and-user-discovery, step task#7.
AWS: Smithy models DescribeBroker as GET /v1/brokers/{BrokerId} (input BrokerId, httpLabel, required), success 200, output members including BrokerInstances (jsonName brokerInstances, list of BrokerInstance), MaintenanceWindowStartTime (jsonName maintenanceWindowStartTime, WeeklyStartTime with required DayOfWeek/TimeOfDay plus TimeZone), Configurations (jsonName configurations, with current ConfigurationId {id,revision} and history list), Logs (jsonName logs, LogsSummary with audit/auditLogGroup/general/generalLogGroup), EncryptionOptions (jsonName encryptionOptions, with required UseAwsOwnedKey), StorageType (jsonName storageType, enum BrokerStorageType), EngineType (jsonName engineType, enum EngineType), EngineVersion and AuthenticationStrategy (jsonName authenticationStrategy, enum AuthenticationStrategy). For a RUNNING SINGLE_INSTANCE ActiveMQ broker, AWS returns ALL of those: one brokerInstances entry carrying consoleURL, a non-empty endpoints list with one URI per served wire protocol (the ssl/amqp+ssl/stomp+ssl/mqtt+ssl/wss forms) and ipAddress; a maintenanceWindowStartTime with dayOfWeek/timeOfDay/timeZone; a configurations object whose current names the applied configuration id and revision and whose history is a list (empty when none); a logs object that reports audit as a boolean (false when audit logging is off, NOT omitted) together with its auditLogGroup under /aws/amazonmq/broker/<broker-id>/audit; a logGroups summary naming the general and audit groups with their enabled flags; encryptionOptions with useAwsOwnedKey; and storageType. The non-model wire detail is recorded behavior only, and the recording is authoritative for it: although the Smithy enums spell the values ACTIVEMQ / SIMPLE / EFS, Amazon MQ's DescribeBroker response serializes engineType as 'ActiveMQ', authenticationStrategy as 'simple' and storageType as 'efs'. Also recorded behavior only: AWS resolves the broker's requested minor engine version to the full patch version it actually runs, so engineVersion comes back as a three-part version (e.g. 5.18.8 for a broker created on the 5.18 line), not the two-part line the request named.
Emulator: the 200 response carries none of brokerInstances, maintenanceWindowStartTime, configurations, logs.audit, logs.auditLogGroup, logGroups, encryptionOptions or storageType (all read back as absent/None), returns engineType 'ACTIVEMQ' where AWS returns 'ActiveMQ' and authenticationStrategy 'SIMPLE' where AWS returns 'simple', and echoes engineVersion '5.18' instead of the resolved patch version.
Why: DescribeBroker is the only discovery surface for a broker, so a client relying on AWS's behavior gets nothing to connect to (no brokerInstances endpoints or consoleURL), cannot read its maintenance window, applied configuration or log-group destinations, cannot tell audit logging is off from the member being missing, and cannot see its encryption or storage settings. The enum casing is worse than cosmetic: code and infrastructure tooling that compares engineType to 'ActiveMQ', authenticationStrategy to 'simple' or storageType to 'efs' - the values AWS actually puts on the wire - takes the false branch against the emulator, and a client that pins or reports the running patch version sees a version string AWS never returns.
Fix: DescribeBroker's response construction and the broker state it reads (the DescribeBroker handler under services/service-com-amazonaws-mq-mq/src/ops/, plus the shared broker-to-output serializer that ListBrokers/CreateBroker also use) - populate the omitted members from stored broker state, derive the instance endpoints/console URL and log-group names when the broker reaches RUNNING, resolve the engine version to its patch level at CreateBroker time, and serialize engineType/authenticationStrategy/storageType in the casing AWS puts on the wire.
Traces: mq-activemq-broker-endpoint-and-user-discovery#task#7.

Cause 2 of 3: DescribeUser omits the groups member instead of serving an empty list. Fixture mq-activemq-broker-endpoint-and-user-discovery, step task#9.
AWS: Smithy models DescribeUser as GET /v1/brokers/{BrokerId}/users/{Username} (both httpLabel, required), success 200, output member Groups (jsonName groups, list of string), documented 'The list of groups (20 maximum) to which the ActiveMQ user belongs'. For a user created with no groups, AWS returns groups as a present, EMPTY list.
Emulator: the 200 response has no groups member at all (expected [], got None).
Why: a client relying on AWS's behavior reads response['groups'] directly and gets a key error or a null instead of an empty collection, so group-membership logic (and any diff of current vs desired groups before UpdateUser) breaks on a user that simply has no groups - a case AWS represents as [] and never as absent.
Fix: DescribeUser's response serializer (the DescribeUser handler under services/service-com-amazonaws-mq-mq/src/ops/) - always emit groups, defaulting to the empty list rather than leaving the member unset.
Traces: mq-activemq-broker-endpoint-and-user-discovery#task#9.

Cause 3 of 3: DescribeUser's not-found refusal carries the emulator's own wording instead of Amazon MQ's. Fixture mq-activemq-broker-endpoint-and-user-discovery, step task#10.
AWS: Smithy models NotFoundException (httpError 404, members message/errorAttribute) among DescribeUser's errors. Recorded behavior for a username that does not exist on an existing broker: 404 NotFoundException whose message is the fixed text 'Can’t find user [' + <the Username from the request path> + '] for broker [' + <the BrokerId from the request path> + ']. Make sure user exists.' - the only variable parts are the two bracketed values, which come from the request; the rest of the text, including the typographic apostrophe in 'Can’t', is fixed. Note this is a DIFFERENT fixed template from mq's broker-not-found message ('Cannot find broker ID [...]. Verify that your broker exists or check broker permissions.'), so the two cannot share one string.
Emulator: correct status 404, but message 'Cannot find user [<username>] of broker ID [<broker-id>]. Verify that your user exists or check broker permissions.' - it reuses the broker-not-found phrasing ('Cannot find', 'of broker ID', 'Verify that your ... or check broker permissions') rather than AWS's user template, with the bracketed request values being the only part that matches.
Why: any client, console or test that surfaces or matches the service message gets text no AWS SDK would ever produce; code that keys on AWS's wording to distinguish a missing user from a missing broker cannot make that distinction against the emulator, since both refusals read with the same phrasing.
Fix: the DescribeUser handler's not-found error construction (and the shared mq error-message helper it calls) under services/service-com-amazonaws-mq-mq/src/ops/ - emit the user-specific fixed text with the username and broker id interpolated.
Traces: mq-activemq-broker-endpoint-and-user-discovery#task#10.

## Evidence
Wire replay of simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up=true, 9 fixtures replayed, 68 steps, 41 passed, 26 mismatched, 1 blocked, score 60.3%, 18 divergence clusters - all on service mq. 15 of the 18 are already owned by open issues and are excluded here; the three below are the net-new ones (owned_by=[], stub_path="").

Cause 1 - mq-activemq-broker-endpoint-and-user-discovery#task#7, DescribeBroker, expected 200 / actual 200, verdict mismatch:
  $.engineType: value_mismatch (expected 'ActiveMQ', got 'ACTIVEMQ')
  $.authenticationStrategy: value_mismatch (expected 'simple', got 'SIMPLE')
  $.engineVersion: value_mismatch (expected '5.18.8', got '5.18')
  $.brokerInstances: missing - expected one instance carrying consoleURL (https://<broker-host>:8162), endpoints for ssl://:61617, amqp+ssl://:5671, stomp+ssl://:61614, mqtt+ssl://:8883 and wss://:61619, and ipAddress; got None. (Hosts, ports-per-protocol aside, ids and addresses are environment values; the presence of the member with one endpoint per served protocol is the requirement.)
  $.maintenanceWindowStartTime: missing - expected {timeOfDay, dayOfWeek, timeZone}; got None
  $.configurations: missing - expected {current: {id, revision}, history: []}; got None
  $.logs.audit: missing (expected false, got None)
  $.logs.auditLogGroup: missing (expected /aws/amazonmq/broker/<broker-id>/audit, got None)
  $.logGroups: missing - expected the general and audit entries with their enabled flags and log-group paths; got None
  $.encryptionOptions: missing (expected {useAwsOwnedKey: true}, got None)
  $.storageType: missing (expected 'efs', got None)

Cause 2 - mq-activemq-broker-endpoint-and-user-discovery#task#9, DescribeUser, expected 200 / actual 200, verdict mismatch:
  $.groups: missing (expected [], got None)

Cause 3 - mq-activemq-broker-endpoint-and-user-discovery#task#10, DescribeUser, expected 404 / actual 404, verdict mismatch:
  $.message: value_mismatch
    expected: 'Can’t find user [<username>] for broker [<broker-id>]. Make sure user exists.'
    actual:   'Cannot find user [<username>] of broker ID [<broker-id>]. Verify that your user exists or check broker permissions.'
  Fixed text in the expected message: 'Can’t find user [' ... '] for broker [' ... ']. Make sure user exists.' Variable parts: the username and the broker id, both taken from the request path (the recording's own values were the probe username and the probe broker's generated id, which are per-run and not part of the requirement).

## Steps to Reproduce
1. Clone the repo, build and boot the emulator from that checkout, then replay the wire corpus simparity-wire-aws-v1 (fixture mq-activemq-broker-endpoint-and-user-discovery).
2. Cause 1: create an ActiveMQ SINGLE_INSTANCE broker (mq.t3.micro, simple authentication, EFS storage) and wait until it reports RUNNING, then GET /v1/brokers/{brokerId} (step task#7). Compare the 200 body against the recording: expect engineType 'ActiveMQ', authenticationStrategy 'simple', a resolved three-part engineVersion, storageType 'efs', and present brokerInstances / maintenanceWindowStartTime / configurations / logs.audit / logs.auditLogGroup / logGroups / encryptionOptions members. The emulator returns uppercase enum values, the unresolved engine-version line, and omits the eight members above.
3. Cause 2: on that broker, create a user with no groups, then GET /v1/brokers/{brokerId}/users/{username} (step task#9). Expect a present, empty groups list; the emulator omits groups entirely.
4. Cause 3: GET /v1/brokers/{brokerId}/users/{username} for a username that was never created on that existing broker (step task#10). Expect 404 NotFoundException with the message 'Can’t find user [<username>] for broker [<brokerId>]. Make sure user exists.'; the emulator answers 404 with the broker-not-found phrasing 'Cannot find user [<username>] of broker ID [<brokerId>]. Verify that your user exists or check broker permissions.'
