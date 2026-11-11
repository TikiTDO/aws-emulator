Issue-Source: trace-sweep-divergence
Issue-Trace: events-endpoint-errors
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: events-endpoint-errors events-endpoint-create-validation-error
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events|DescribeEndpoint|4xx->2xx|$.EndpointUrl
Issue-Key: events|UpdateEndpoint|4xx->2xx|$.EndpointUrl
Issue-Key: events|ListEndpoints|2xx->2xx|$.Endpoints[*].EndpointUrl
Issue-Related: 20261029T235600Z-events-global-endpoints-skip-validation-and-existence-checks.md
Issue-Commit: b37a548368ed88ff8e9aed060c39391f1ed9f4cf

# events global endpoint reads answer 200 and announce a foreign EndpointUrl host

> _Note: this issue was identified against the code base at commit `b37a548368ed88ff8e9aed060c39391f1ed9f4cf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeEndpoint

## Description
The global-endpoint read/describe surface of the events emulator never fails: DescribeEndpoint and UpdateEndpoint answer 200 for an endpoint name that was never created, ListEndpoints lists an endpoint AWS refused to create, and every one of those 200 bodies carries an EndpointUrl on a host (`*.endpoints.event.amazonaws.com`) that the emulator does not own and is not reachable on. Three causes.

NOTE ON OVERLAP: the open issue `20261029T235600Z-events-global-endpoints-skip-validation-and-existence-checks.md` describes the same three operations, but its recorded replay keys do not cover these clusters (its ListEndpoints key is `$.Endpoints[*]`, and its Describe/UpdateEndpoint keys omit `$.EndpointUrl`), so the replay reports them as unowned and the fabricated-host half of the divergence is not carried by any filed issue. These causes must be closed on their own replay keys.

Cause 1 of 3: DescribeEndpoint on an endpoint that does not exist answers 200 and returns a fabricated EndpointUrl.
AWS: Smithy (com.amazonaws.eventbridge#AWSEvents): DescribeEndpoint binds ResourceNotFoundException (errorKind client, HTTP 400); its output member EndpointUrl targets shape EndpointUrl with pattern `^(https://)?[\.\-a-z0-9]+$`, length 1-256. Recorded response: HTTP 400, `__type` = `ResourceNotFoundException`, message `Endpoint <name> does not exist in region <region>.` — the fixed text is `Endpoint ` + ` does not exist in region ` + `.`; `<name>` is the Name member taken verbatim from the request and `<region>` is the region the call was made in. No response body fields (including EndpointUrl) are produced at all for a refused read.
Emulator: answers HTTP 200 with neither `__type` nor `message`, and the body carries an EndpointUrl pointing at an `*.endpoints.event.amazonaws.com` host — an address other than the one the emulator is reached on (reported by the replay as `host_not_own`).
Why: a client using DescribeEndpoint as the standard existence probe (catch ResourceNotFoundException to decide create-vs-update) concludes the endpoint exists and skips creating it; worse, it then publishes events to the returned EndpointUrl, which resolves to the real AWS global-endpoint domain or to nothing at all, so traffic leaves the emulator entirely instead of being served by it.
Fix: the DescribeEndpoint handler in the events crate — look the endpoint up in state and raise ResourceNotFoundException with the message above before serializing any output; the EndpointUrl serializer must derive the host from the endpoint the emulator actually serves, never a hard-coded amazonaws.com name.
Traces: events-endpoint-errors#task#2.

Cause 2 of 3: UpdateEndpoint on an endpoint that does not exist answers 200 and returns the same fabricated EndpointUrl.
AWS: Smithy: UpdateEndpoint binds ResourceNotFoundException (errorKind client, HTTP 400) alongside ConcurrentModificationException and InternalException; its output EndpointUrl is the same constrained shape. Recorded response: HTTP 400, `__type` = `ResourceNotFoundException`, message `Endpoint <name> does not exist in region <region>.` — same template as cause 1, with `<name>` from the request's Name and `<region>` from the call's region.
Emulator: answers HTTP 200 with no `__type`/`message`, and the body again carries an EndpointUrl on the foreign `*.endpoints.event.amazonaws.com` host.
Why: an update against a missing endpoint reports success, so a client believes it applied routing/replication changes that were applied to nothing, and it caches a publish URL the emulator does not serve.
Fix: the UpdateEndpoint handler's existence check, sharing the lookup/refusal path with DescribeEndpoint, plus the same EndpointUrl host derivation.
Traces: events-endpoint-errors#task#3.

Cause 3 of 3: ListEndpoints returns an endpoint record that was never validly created, including its fabricated EndpointUrl.
AWS: Smithy: ListEndpoints binds only InternalException and returns Endpoints (EndpointList) plus NextToken; recorded behavior: because the preceding CreateEndpoint (whose RoutingConfig health check does not exist) is refused with 400, ListEndpoints answers 200 with no Endpoints entry for that name.
Emulator: answers 200 with an extra `$.Endpoints[0]` describing the endpoint that should not exist (name, description, ARN, RoutingConfig carrying the nonexistent health check, ReplicationConfig, both event buses, EndpointId, State `CREATING`), and `$.Endpoints[0].EndpointUrl` is again a host the emulator does not own.
Why: the refusal must leave no resource behind — a caller that handles the expected 400 and retries instead collides with an existing endpoint, and the listing hands out an unreachable publish URL for it. It also shows State never settles away from `CREATING`.
Fix: CreateEndpoint must validate referenced resources before mutating state (the state-write path, not ListEndpoints, which reads the store faithfully), and the endpoint EndpointUrl must be generated from the emulator's own serving host.
Traces: events-endpoint-create-validation-error#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 93.7%, 34 mismatched steps):
- events|DescribeEndpoint|status=400->200 (events-endpoint-errors#task#2): status expected 400, got 200; $.__type expected 'ResourceNotFoundException', got None; $.message expected 'Endpoint <name> does not exist in region us-west-2.', got None; $.EndpointUrl: host_not_own (expected None, got an 'https://<id>.endpoints.event.amazonaws.com' URL).
- events|UpdateEndpoint|status=400->200 (events-endpoint-errors#task#3): identical shape — status 400->200, $.__type and $.message absent, $.EndpointUrl host_not_own.
- events|ListEndpoints|status=200->200 (events-endpoint-create-validation-error#task#3): $.Endpoints[0] expected absent, got the rejected endpoint record (RoutingConfig.FailoverConfig.Primary.HealthCheck = the nonexistent Route 53 health-check ARN, State 'CREATING'); $.Endpoints[0].EndpointUrl host_not_own.
Neither the fabricated-host finding nor these three replay keys are carried by any open issue (the similar open issue 20261029T235600Z-events-global-endpoints-skip-validation-and-existence-checks.md records different keys).

## Steps to Reproduce
1. git_clone, then replay_wire_corpus against the built emulator.
2. Fixture events-endpoint-errors, task#2: DescribeEndpoint with Name = an endpoint never created -> expect HTTP 400, __type 'ResourceNotFoundException', message 'Endpoint <name> does not exist in region <region>.'; emulator answers 200 with an EndpointUrl on an *.endpoints.event.amazonaws.com host.
3. Same fixture, task#3: UpdateEndpoint on that same absent name -> expect the same 400 ResourceNotFoundException; emulator answers 200 with the same foreign EndpointUrl.
4. Fixture events-endpoint-create-validation-error, task#3: ListEndpoints after a CreateEndpoint that AWS refuses (health check ARN does not exist) -> expect no Endpoints entry; emulator lists the endpoint, State 'CREATING', with a foreign EndpointUrl.
