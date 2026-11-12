Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: c441cdb97fe099c25286b36cfc473da57b1ed5b6

# TagResource answers a reserved/spaced tag pair with 404 when the ARN is unknown, where every sibling tag path in this crate answers 400 ValidationException

_Filed: 2026-10-29T00:02:29Z_
_Operation: TagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `c441cdb97fe099c25286b36cfc473da57b1ed5b6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate enforces the two prose-only tag rules (`Tag$Key` "can't start with `aws:`", `Tag` "Spaces are not allowed in tag values") in four places, and three of them answer the rule as a fact of the REQUEST — they refuse before the store is consulted at all:

* `CreateGateway` calls `crate::state::tag_map(..)` before it takes the store guard;
* `ImportHypervisorConfiguration` does the same, before the guard and before its ceiling check;
* `PutHypervisorPropertyMappings` calls `crate::state::validate_tag_mappings(..)` before `state.lock()`, with the stated rationale "It is a fact of the REQUEST, so it is answered before the store is consulted at all and nothing is written."

`TagResource` alone inverts that order. It calls `state.merge_resource_tags(..)`, whose first act is `self.taggable_mut(arn, now)?` — so the ARN is resolved FIRST and a `None` is answered with `ResourceNotFoundException` (404, or 400 `HypervisorNotFound` for a hypervisor ARN) before `merged_tags` ever looks at the pairs. The tag-rule refusal is reached only on the `Some(..)` branch.

Consequence: one request carrying a reserved `aws:` key (or a value with a space) gets two different answers from the same crate depending on something unrelated to the pair. Sent to an existing resource it is the modelled `ValidationException` (HTTP 400). Sent to a well-formed ARN the store holds no record under it is `ResourceNotFoundException` instead, so a caller that mistyped both the ARN and the tag key is told only about the ARN, and a caller fixing the ARN then discovers the tag refusal on the retry. The crate's own suite states the intended rule as ONE answer for every member that carries an AWS tag pair, which this path does not give.

The defect is the ordering of two checks the crate already has — `merged_tags`/`validate_tag_pair` and `taggable_mut` — not a new feature: the helpers exist and three sibling handlers call them in the opposite order.

## Evidence

Side that validates the REQUEST first, with the rule stated — `src/ops/put_hypervisor_property_mappings.rs:205-211` and `:226`:

        // model does NOT express is the tag rules its docs state for the AWS half of a
        // mapping — `AwsTagKey` and `AwsTagValue` target the very `TagKey` and `TagValue`
        // shapes a resource tag's key and value do — which
        // `crate::state::validate_tag_mappings` states and refuses, as `tag_map` does for
        // a request's `Tags`. It is a fact of the REQUEST, so it is answered before the
        // store is consulted at all and nothing is written.
        crate::state::validate_tag_mappings(&input.vmware_to_aws_tag_mappings).map_err(
    ...
        let mut store = state.lock();

Same order in `src/ops/create_gateway.rs:167` (validation) vs `:187` (store guard):

        let tags = crate::state::tag_map(input.tags.as_deref().unwrap_or(&[])).map_err(
    ...
        let mut store = state.lock();

and `src/ops/import_hypervisor_configuration.rs:167` vs `:186`:

        let tags = crate::state::tag_map(input.tags.as_deref().unwrap_or(&[])).map_err(
    ...
        let mut store = state.lock();

Side that resolves the STORE first — `src/ops/tag_resource.rs:169-193`:

        let Some(merged) = state.merge_resource_tags(
            &input.resource_arn,
            &input.tags,
            ::std::time::SystemTime::now(),
        ) else {
            let missing = crate::state::MissingResource::for_arn(&input.resource_arn);
            let error = TagResourceErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException { ... });
            return Err(match missing.status_override() { ... });
        };
        // The tag rules the model's own traits cannot express; nothing is written when
        // they refuse.
        let resource_arn = merged.map_err(|message| {
            TagResourceErrorModeled::ValidationException(ValidationException {

and the helper it calls — `src/state.rs:2195-2199`:

        let (resource_arn, tags) = self.taggable_mut(arn, now)?;
        Some(merged_tags(tags, requested).map(|merged| {
            *tags = merged;
            resource_arn
        }))

where `merged_tags` is the only place the pair rules are checked for `Tags` — `src/state.rs:2498-2499`:

        for tag in requested {
            validate_tag_pair(&tag.key, &tag.value, "Tags")?;

The rule the crate states for itself across every tag-carrying member — `tests/all/aws_tag_rules_span_every_tag_pair.rs:12-21` and `:30-31`:

    //! The rule under test: a pair whose KEY starts with the reserved `aws:` prefix, or whose
    //! VALUE carries a space, is refused with the modelled `ValidationException` at HTTP 400
    //! and nothing is written — whichever member carried it ...
    //! `tag_value_spaces` covers the `Tags` member on its own; this suite is what pins the two
    //! members to ONE answer.

Test coverage: `tests/all/tag_value_spaces.rs` and `tests/all/aws_tag_rules_span_every_tag_pair.rs` pin the 400 answer only for pairs sent to a resource that EXISTS; a grep of `tests/all` for a case combining `never_created_arn` with an invalid `Tags` pair returns no match, so the combined case is untested and either order leaves every existing assertion unchanged.

## Steps to Reproduce

1. Start the emulator with the backup-gateway service mounted.
2. Perform the activation handshake (GET /?activationRegion=us-east-1&no_redirect) and call CreateGateway with the issued key; note the returned GatewayArn.
3. Call TagResource with that GatewayArn and Tags [{"Key":"aws:billing","Value":"x"}] -> HTTP 400 ValidationException ('a tag key must not start with the reserved prefix aws:'), nothing written.
4. Call TagResource with the SAME Tags but a well-formed GatewayArn the store holds no record under (e.g. the suite's never_created_arn) -> HTTP 404 ResourceNotFoundException: the reserved-prefix violation in the very same request is never reported.
5. Repeat step 4 against ImportHypervisorConfiguration (Tags [{"Key":"aws:billing",...}] plus a store already at its hypervisor ceiling) or PutHypervisorPropertyMappings (AwsTagKey 'aws:billing' plus a HypervisorArn no gateway serves) -> both answer HTTP 400 ValidationException for the tag pair, because those handlers check the request before consulting the store.
