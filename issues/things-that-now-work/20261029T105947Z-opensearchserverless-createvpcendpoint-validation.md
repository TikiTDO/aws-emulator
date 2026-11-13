Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 2d45ab8ecba5c3bbeb0b6029f45083afacab7d74

# CreateVpcEndpoint stores a subnetIds/securityGroupIds list that repeats an id, breaking the SET invariant this crate documents for those stored lists

_Filed: 2026-10-29T10:59:47Z_
_Operation: CreateVpcEndpoint | Class: validation_

> _Note: this issue was identified against the code base at commit `2d45ab8ecba5c3bbeb0b6029f45083afacab7d74`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateVpcEndpoint` copies `subnetIds` and `securityGroupIds` into the stored record verbatim, with no check that an id is named only once. The crate's own update path documents the stored lists as SETS — `apply_id_change` will not hold an id twice — and the crate already owns the helper for refusing a repeated element of a list the model gives no `@uniqueItems` (`first_repeated_tag_key`, used by `CreateCollection`, `CreateCollectionGroup`, `TagResource` and `UntagResource`). The create is the one writer that skips it, so it can persist a list the update path can never produce.

Consequences that follow from the duplicate being stored:
- `BatchGetVpcEndpoint`/`CreateVpcEndpoint` report the same subnet twice in `subnetIds`.
- Both subnet bounds count the copies as distinct entries, so an endpoint created with the same subnet repeated six times sits at the stored-list bound with ONE real subnet and every later `addSubnetIds` is refused with "The endpoint must not have more than 6 subnets".
- A single `removeSubnetIds` entry drops every copy at once (`retain` removes all), so one removal can take the endpoint from "6 subnets" to zero and the `MIN_SUBNET_IDS` refusal fires on a request that named one id.

## Evidence

The create stores the caller's lists unchanged:
"        let mut record = crate::state::VpcEndpointRecord {\n            id: id.clone(),\n            name: input.name,\n            vpc_id: input.vpc_id,\n            subnet_ids: input.subnet_ids,\n            security_group_ids: input.security_group_ids.unwrap_or_default()," — src/ops/create_vpc_endpoint.rs:237-242.

The crate documents those same stored lists as sets: "Apply an update's add and remove lists to one of an endpoint's id lists.\n///\n/// The list is a SET of ids the endpoint carries: an id it already holds is not held twice when it is added again, and one it does not hold is unaffected by a removal, so the same update applied twice leaves the same list." — src/state.rs:2234-2241, and the code that keeps it so: `let mut held: HashSet<String> = current.iter().cloned().collect(); for id in add { if held.insert(id.clone()) { current.push(id.clone()); } }` — src/state.rs:2254-2259.

The helper this path skips already exists and is generic over any `&str` iterator: "pub fn first_repeated_tag_key<'keys>(keys: impl IntoIterator<Item = &'keys str>) -> Option<String>" under "Neither `Tags` nor `TagKeys` carries `@uniqueItems` ... so the boundary validator admits a repeat and the rule is enforced by the handler that takes the member." — src/state.rs:2094-2113. Siblings call it: src/ops/create_collection.rs:228-230, src/ops/create_collection_group.rs:228-230, src/ops/tag_resource.rs:155-157, src/ops/untag_resource.rs:150-152. The same operation family already refuses a contradictory id list on the update side: `crate::state::id_named_in_both(input.add_subnet_ids.as_deref(), input.remove_subnet_ids.as_deref())` — src/ops/update_vpc_endpoint.rs:210-224.

The duplicate is counted by both bounds: `if input.subnet_ids.len() > crate::state::MAX_SUBNET_IDS` — src/ops/create_vpc_endpoint.rs:169; `if subnet_ids.len() > crate::state::MAX_SUBNET_IDS` on the update's RESULT — src/ops/update_vpc_endpoint.rs:320-336.

Test coverage: `tests/all/vpc_endpoint.rs:144-146` pins "expected the requested subnetIds in request order", and none of its create cases names an id twice (tests/all/vpc_endpoint.rs:200-247), so refusing a repeat leaves every existing assertion unchanged.

## Steps to Reproduce

1. CreateVpcEndpoint(name="dup-endpoint", vpcId="vpc-1a2b3c4d", subnetIds=["subnet-11111111","subnet-11111111"], securityGroupIds=["sg-1","sg-1"]) -> 200 instead of a ValidationException.
2. BatchGetVpcEndpoint(ids=[<id>]) -> `subnetIds` reports ["subnet-11111111","subnet-11111111"] and `securityGroupIds` ["sg-1","sg-1"] — a list the update path can never produce.
3. Repeat the create with the same subnet six times; then UpdateVpcEndpoint(id=<id>, addSubnetIds=["subnet-22222222"]) -> refused "The endpoint must not have more than 6 subnets", although the endpoint really holds one subnet.
4. UpdateVpcEndpoint(id=<id>, removeSubnetIds=["subnet-11111111"]) on that endpoint -> refused for keeping fewer than 1 subnet, because the one named id removes all six stored copies.
