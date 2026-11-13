Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 442a99274869dbb92025fb75ce375f34a4aa7eac

# UpdateBridge lowers IngressGatewayBridge.MaxOutputs below the outputs the bridge already holds, while the same handler re-checks the mirror source bound it moves

_Filed: 2026-10-29T12:14:02Z_
_Operation: UpdateBridge | Class: validation_

> _Note: this issue was identified against the code base at commit `442a99274869dbb92025fb75ce375f34a4aa7eac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

UpdateBridge can MOVE two bounds that the bridge's own record declares: the source bound (via `SourceFailoverConfig`, which decides whether a bridge may hold one or two sources) and the output bound (via `IngressGatewayBridge.MaxOutputs`, "the maximum number of outputs on the ingress bridge").

For the source bound the handler does exactly the right thing: because the request moves the bound rather than the source count, it re-evaluates the new bound against the sources the bridge ALREADY holds and refuses the update when the stored record would no longer satisfy it (src/ops/update_bridge.rs:309-325).

For the output bound it does nothing at all: `max_outputs: update.max_outputs.unwrap_or(stored.max_outputs)` is written straight onto the record (src/ops/update_bridge.rs:296), with no reference to `bridge.outputs.len()`, `crate::state::bridge_output_capacity` or `crate::state::request_bridge_output_rejection`. A ground-to-cloud bridge that currently holds 2 flow outputs (the `BridgeFlowOutput`s this service materialises when cloud flows take the bridge as their source) can therefore be updated to `maxOutputs: 1`, and DescribeBridge afterwards reports a bridge whose `outputs` list is longer than its own required `ingressGatewayBridge.maxOutputs` — a record the crate's own helper says cannot exist.

This is NOT the same gap as the open AddFlowSources issue (20261029T110130Z, "A ground-to-cloud bridge is given more flow outputs than its own IngressGatewayBridge.MaxOutputs declares"): that one is about the ADD side (`sync_bridge_flow_outputs` pushing outputs past the declared maximum). Even with the add side bounded, UpdateBridge would still be able to move the declared maximum down underneath the outputs already held, exactly as it would be able to move the failover bound down underneath the sources already held were the check at :309-325 missing. The fix is a second, independent check in this handler.

## Evidence

The crate's own rule for what the ingress maximum bounds — src/state.rs:606-609 (doc of `bridge_output_capacity`):
```
/// What a ground-to-cloud bridge declares bounds the outputs it can ever hold — the flow
/// outputs the service materialises for it — and not the ones a request configures, of
/// which it takes none at all (see [`request_bridge_output_rejection`], which refuses
/// those by the bridge's direction before it counts anything).
```
and the model member it reads, `IngressGatewayBridge$MaxOutputs` (required, smithy_query(query="shape",shape="IngressGatewayBridge")): "The maximum number of outputs on the ingress bridge."

The crate states that an operation which MOVES a bound must re-check it against what the record already holds — src/state.rs:678-681 (doc of `max_bridge_sources`):
```
/// Every operation that decides a bridge's source count reads the bound HERE —
/// CreateBridge's `Sources`, AddBridgeSources' append and UpdateBridge's
/// `SourceFailoverConfig` (which moves the bound under the sources the bridge already
/// holds) — so the add side, the create side and the failover side cannot drift apart.
```
and this handler does so for the source bound — src/ops/update_bridge.rs:316-325:
```
        let bound_rejection = input.source_failover_config.as_ref().and_then(|_| {
            crate::state::bridge_source_bound_rejection(
                source_failover_config.as_ref(),
                bridge.sources.len(),
                0,
            )
        });
        if let Some(rejection) = bound_rejection {
            return Err(bad_request(rejection));
        }
```
The output bound is written with no such check — src/ops/update_bridge.rs:293-297:
```
                Some(IngressGatewayBridge {
                    instance_id: stored.instance_id.clone(),
                    max_bitrate: update.max_bitrate.unwrap_or(stored.max_bitrate),
                    max_outputs: update.max_outputs.unwrap_or(stored.max_outputs),
                })
```
and the merged value is stored unconditionally — src/ops/update_bridge.rs:343-345:
```
        bridge.egress_gateway_bridge = egress_gateway_bridge;
        bridge.ingress_gateway_bridge = ingress_gateway_bridge;
        bridge.source_failover_config = source_failover_config;
```
`grep -n 'max_outputs' src/ops/update_bridge.rs` matches only line 296, so nothing in the file consults the outputs held.

Test coverage: tests/all/update_bridge.rs:153-160 is the only row that moves `MaxOutputs` —
```
        UpdateCase {
            case: "ingress bridge, MaxOutputs alone: MaxBitrate keeps its stored value",
            create: ingress("bridge-ingress-outputs", None),
            update: json!({ "ingressGatewayBridge": { "maxOutputs": 0 } }),
            egress: Value::Null,
            ingress: json!({ "maxBitrate": 20_000_000, "maxOutputs": 0 }),
```
That bridge holds NO outputs (an ingress bridge takes none from a request), so a check against the held count leaves this test passing; no test pins a declared maximum below the outputs a bridge holds.

## Steps to Reproduce

1. POST /v1/gateways with one network and one egress CIDR block; wait for the gateway to settle ACTIVE.
2. POST /v1/bridges with {"name":"ingress-bridge","placementArn":<gatewayArn>,"ingressGatewayBridge":{"maxBitrate":20000000,"maxOutputs":2},"sources":[{"networkSource":{...}}]}.
3. POST /v1/flows twice, each with a source whose `gatewayBridgeSource.bridgeArn` names that bridge (or use AddFlowSources). GET /v1/bridges/{BridgeArn} now reports 2 `flowOutput` entries — the declared maximum.
4. PUT /v1/bridges/{BridgeArn} with {"ingressGatewayBridge":{"maxOutputs":1}} — the call is accepted (200) and the response reports `ingressGatewayBridge.maxOutputs: 1`.
5. GET /v1/bridges/{BridgeArn}: the bridge reports 2 outputs while its own required `ingressGatewayBridge.maxOutputs` is 1. Compare PUT /v1/bridges/{BridgeArn} with {"sourceFailoverConfig":{"state":"DISABLED"}} on a bridge holding two sources, which this same handler refuses 400 for exactly the mirror reason.
