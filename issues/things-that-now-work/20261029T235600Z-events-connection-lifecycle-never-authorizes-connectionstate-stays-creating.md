Issue-Source: trace-sweep-divergence
Issue-Trace: events-connection-and-api-destination-lifecycle
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: events-connection-and-api-destination-lifecycle
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events|CreateConnection|2xx->2xx|$.ConnectionState
Issue-Key: events|DescribeConnection|2xx->2xx|$.ConnectionState
Issue-Key: events|DescribeConnection|2xx->2xx|$.LastAuthorizedTime
Issue-Key: events|ListConnections|2xx->2xx|$.Connections[Name="sim2real-probe-events-connection-dd290078"].ConnectionState
Issue-Key: events|ListConnections|2xx->2xx|$.Connections[Name="sim2real-probe-events-connection-dd290078"].LastAuthorizedTime
Issue-Key: events|UpdateConnection|2xx->2xx|$.ConnectionState
Issue-Key: events|UpdateConnection|2xx->2xx|$.LastAuthorizedTime
Issue-Key: events|DeauthorizeConnection|2xx->2xx|$.LastAuthorizedTime
Issue-Key: events|DescribeConnection|2xx->2xx|$.StateReason
Issue-Key: events|DeleteConnection|2xx->2xx|$.LastAuthorizedTime
Issue-Commit: 10534c2dd099d7fcc20a17236779012152da4f21

# events connection lifecycle never authorizes: ConnectionState stays CREATING/UPDATING and LastAuthorizedTime and the deauthorize StateReason are never emitted

> _Note: this issue was identified against the code base at commit `10534c2dd099d7fcc20a17236779012152da4f21`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConnection

## Description
An EventBridge connection created through the emulator never reaches its authorized state and never carries an authorization timestamp, and a deauthorized connection carries no StateReason. Seven causes, all rooted in the connection state machine (no transition out of the transient state, no LastAuthorizedTime ever recorded) and the connection serializers that read it.

Cause 1 of 7 — CreateConnection returns ConnectionState=CREATING.
AWS: Smithy: CreateConnection output ConnectionState targets ConnectionState, enum = CREATING | UPDATING | DELETING | AUTHORIZED | DEAUTHORIZED | AUTHORIZING | DEAUTHORIZING | ACTIVE | FAILED_CONNECTIVITY. Recorded behavior: CreateConnection with an API-key/basic AuthParameters answers 200 with ConnectionState 'AUTHORIZED' — the credentials are stored and the connection is immediately usable.
Emulator: answers 200 with $.ConnectionState = 'CREATING'.
Why: a client that waits for a terminal ConnectionState (AUTHORIZED / DEAUTHORIZED / FAILED_CONNECTIVITY) before attaching an api destination never terminates; the emulator has no transition out of CREATING.
Fix: the CreateConnection handler / connection state in the events crate — store the connection in its settled authorized state and stamp the authorization time.
Traces: events-connection-and-api-destination-lifecycle#task#1.

Cause 2 of 7 — DescribeConnection reports CREATING and omits LastAuthorizedTime.
AWS: Smithy: DescribeConnection output carries ConnectionState and LastAuthorizedTime (Timestamp). Recorded behavior: ConnectionState 'AUTHORIZED' and LastAuthorizedTime present (its value is a clock reading, not a requirement).
Emulator: answers 200 with ConnectionState 'CREATING' and LastAuthorizedTime absent from the body.
Why: the state never converges, and a consumer reading LastAuthorizedTime to decide whether credentials have ever been accepted sees nothing at all.
Fix: connection state in the events crate plus the DescribeConnection serializer (emit LastAuthorizedTime whenever the connection has been authorized).
Traces: events-connection-and-api-destination-lifecycle#task#2.

Cause 3 of 7 — ListConnections carries the same unsettled entry.
AWS: Recorded behavior: the listed connection entry carries ConnectionState 'AUTHORIZED' and a LastAuthorizedTime.
Emulator: the entry selected by Name carries ConnectionState 'CREATING' and no LastAuthorizedTime.
Why: inventory/reconciliation over ListConnections classifies a usable connection as still being created.
Fix: the ListConnections serializer reads the same connection record; fixing the state and the stored timestamp fixes this reader.
Traces: events-connection-and-api-destination-lifecycle#task#7.

Cause 4 of 7 — UpdateConnection returns UPDATING and no LastAuthorizedTime.
AWS: Smithy: UpdateConnection output ConnectionState + LastAuthorizedTime. Recorded behavior: an UpdateConnection that re-states AuthParameters answers 200 with ConnectionState 'AUTHORIZED' and a LastAuthorizedTime.
Emulator: answers 200 with ConnectionState 'UPDATING' and LastAuthorizedTime absent.
Why: same non-convergence on the update path; re-authorizing credentials is unobservable.
Fix: the UpdateConnection handler and the state/timestamp it writes.
Traces: events-connection-and-api-destination-lifecycle#task#9.

Cause 5 of 7 — DeauthorizeConnection omits LastAuthorizedTime.
AWS: Smithy: DeauthorizeConnection output members are ConnectionArn, ConnectionState, CreationTime, LastModifiedTime, LastAuthorizedTime. Recorded behavior: the response carries LastAuthorizedTime — the time the connection was last authorized is retained across deauthorization.
Emulator: answers 200 with LastAuthorizedTime absent.
Why: a client cannot tell a connection that was authorized and then revoked from one that was never authorized.
Fix: the DeauthorizeConnection handler must preserve, and its serializer must emit, the stored last-authorized timestamp.
Traces: events-connection-and-api-destination-lifecycle#task#15.

Cause 6 of 7 — DescribeConnection after deauthorization omits both LastAuthorizedTime and StateReason.
AWS: Smithy: DescribeConnection output StateReason targets ConnectionStateReason (length 0..512). Recorded behavior: after DeauthorizeConnection, DescribeConnection answers 200 with StateReason exactly 'The authorization parameters were removed from this connection by a DeauthorizeConnection operation.' — fixed text with no variable parts — plus the retained LastAuthorizedTime.
Emulator: both members are absent from the 200 body.
Why: the documented explanation of why a connection is deauthorized is lost, so operators and automation have no reason string to branch on.
Fix: the DeauthorizeConnection handler (set the state reason) and the DescribeConnection serializer (emit StateReason/LastAuthorizedTime).
Traces: events-connection-and-api-destination-lifecycle#task#16.

Cause 7 of 7 — DeleteConnection omits LastAuthorizedTime.
AWS: Recorded behavior: DeleteConnection answers 200 echoing the connection's final envelope including LastAuthorizedTime.
Emulator: answers 200 with LastAuthorizedTime absent.
Why: the delete response is the last observation of the connection; losing the timestamp breaks audit of when the connection last held credentials.
Fix: the DeleteConnection serializer, over the same stored timestamp.
Traces: events-connection-and-api-destination-lifecycle#task#17.

Related (does not cover this): open issue 'EventBridge: an OAUTH_CLIENT_CREDENTIALS connection never obtains a token, so its api destination never delivers the event' concerns token acquisition and delivery, not the ConnectionState/LastAuthorizedTime wire envelope reported here.

## Evidence
Replay of trace set simparity-wire-aws-v1 (fixture events-connection-and-api-destination-lifecycle):
- CreateConnection 200->200: $.ConnectionState expected 'AUTHORIZED', got 'CREATING'.
- DescribeConnection 200->200: $.ConnectionState expected 'AUTHORIZED', got 'CREATING'; $.LastAuthorizedTime expected present (shaped-not-asserted), got None.
- ListConnections 200->200: $.Connections[Name="..."].ConnectionState expected 'AUTHORIZED', got 'CREATING'; .LastAuthorizedTime missing.
- UpdateConnection 200->200: $.ConnectionState expected 'AUTHORIZED', got 'UPDATING'; $.LastAuthorizedTime missing.
- DeauthorizeConnection 200->200: $.LastAuthorizedTime missing.
- DescribeConnection (post-deauthorize) 200->200: $.LastAuthorizedTime missing; $.StateReason expected 'The authorization parameters were removed from this connection by a DeauthorizeConnection operation.', got None.
- DeleteConnection 200->200: $.LastAuthorizedTime missing.
All statuses match (200); the divergences are the state value and two never-emitted members.

## Steps to Reproduce
1. Replay fixture events-connection-and-api-destination-lifecycle (replay_wire_corpus).
2. task#1 CreateConnection -> $.ConnectionState should be AUTHORIZED.
3. task#2 DescribeConnection, task#7 ListConnections, task#9 UpdateConnection -> ConnectionState AUTHORIZED and LastAuthorizedTime present.
4. task#15 DeauthorizeConnection -> LastAuthorizedTime present.
5. task#16 DescribeConnection -> StateReason 'The authorization parameters were removed from this connection by a DeauthorizeConnection operation.' and LastAuthorizedTime present.
6. task#17 DeleteConnection -> LastAuthorizedTime present.
