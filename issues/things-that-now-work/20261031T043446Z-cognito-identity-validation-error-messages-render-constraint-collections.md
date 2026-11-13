Issue-Source: trace-sweep-divergence
Issue-Trace: cogid-creds-role-mapping-match-type-out-of-enum
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cogid-creds-role-mapping-match-type-out-of-enum cogid-creds-role-mapping-multi-rule-missing-ambiguous-resolution cogid-creds-role-mapping-rules-without-rules-configuration cognito-identity-principal-tag-attribute-mapping cognito-identity-principal-tag-attribute-mapping-configuration
Issue-Shape-Id: com.amazonaws.cognitoidentity#AWSCognitoIdentityService
Issue-Key: cognito-identity|SetIdentityPoolRoles|4xx->4xx|$.message
Issue-Key: cognito-identity|SetPrincipalTagAttributeMap|4xx->4xx|$.message
Issue-Commit: f4a3cd4ad22a3104f2bb05cafe64a36012ad514d

# cognito-identity: validation-error messages render constraint collections in sorted order instead of AWS's fixed order

> _Note: this issue was identified against the code base at commit `f4a3cd4ad22a3104f2bb05cafe64a36012ad514d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetIdentityPoolRoles

## Description
Two distinct root causes in the cognito-identity request-validation message renderer. Each is a separate work item; BOTH must be closed before submitting. They share one component: every collection the renderer embeds in a constraint-violation message is emitted in alphabetical order, while AWS emits it in its own fixed, non-alphabetical order. Status (HTTP 400), error code and all fixed text already match in both cases — only the ordering of the rendered collection diverges.

Cause 1 of 2 — SetIdentityPoolRoles renders the enum value set alphabetically.
AWS: a Rules role mapping whose MappingRule.MatchType is outside the modeled enum is refused HTTP 400 with the validation message "1 validation error detected: Value '<supplied-value>' at '<member-path>' failed to satisfy constraint: Member must satisfy enum value set: [NotEqual, StartsWith, Contains, Equals]". Fixed text: "1 validation error detected: Value '", "' at '", "' failed to satisfy constraint: Member must satisfy enum value set: [NotEqual, StartsWith, Contains, Equals]". Variable parts taken from the request: the rejected MatchType value and the member path (roleMappings.<providerName>.member.rulesConfiguration.rules.<index>.member.matchType). The value set itself is NOT variable — it is a constant of the API (Smithy: enum MappingRuleMatchType, declared values [Equals, Contains, StartsWith, NotEqual]; Smithy: SetIdentityPoolRoles.RoleMappings @length 0..10, error InvalidParameterException @httpError 400). The recording shows AWS renders that set in a fixed order that is neither alphabetical nor the declaration order (observed: exactly the reverse of the Smithy declaration order).
Emulator: identical status, code and fixed text, but the enum value set is rendered sorted alphabetically — [Contains, Equals, NotEqual, StartsWith].
Why: the message is the only machine-readable description of which input was rejected and with which permitted set; clients and conformance harnesses compare this refusal byte-for-byte, so an AWS-worded match on the permitted-value list never fires and the handler for a bad MatchType is never reached.
Fix: the enum-constraint branch of the cognito-identity request-validation message renderer — the permitted value list must be emitted in the API's fixed order, not sorted.
Traces: cogid-creds-role-mapping-match-type-out-of-enum#task#4 (also cogid-creds-role-mapping-multi-rule-missing-ambiguous-resolution#task#4, cogid-creds-role-mapping-rules-without-rules-configuration#task#4).

Cause 2 of 2 — SetPrincipalTagAttributeMap renders the rejected PrincipalTags map alphabetically.
AWS: a PrincipalTags map exceeding the modeled size limit is refused HTTP 400 with "1 validation error detected: Value '{<key>=<value>, <key>=<value>, ...}' at 'principalTags' failed to satisfy constraint: Member must have length less than or equal to 50" (Smithy: SetPrincipalTagAttributeMap.PrincipalTags @length min 0 max 50; error InvalidParameterException @httpError 400). Fixed text: "1 validation error detected: Value '", "' at 'principalTags' failed to satisfy constraint: Member must have length less than or equal to 50"; the limit 50 is the modeled bound, and the rendered entries are the request's own map. AWS renders those entries in its own stable order, which is neither lexicographic nor request order (the recording interleaves the two-digit keys ahead of the one-digit ones).
Emulator: identical status, code, path and fixed text, but the entries are rendered sorted lexicographically by key.
Why: same as Cause 1 — a client that pattern-matches or diffs the refusal text to learn which tags were rejected sees an entry sequence AWS never produces.
Fix: the map-rendering branch of the same validation message renderer.
Traces: cognito-identity-principal-tag-attribute-mapping#task#6 (also cognito-identity-principal-tag-attribute-mapping-configuration#task#7).

Related open issue that does NOT carry these replay keys: 20261029T173827Z-cognito-identity-8-net-new-validation-message-and.md (its Cause 7 describes the same renderer from the behavioral suite, with request order rather than sorted order as the emulator's observed behavior).

## Evidence
Cause 1 (fixture cogid-creds-role-mapping-match-type-out-of-enum, step task#4, SetIdentityPoolRoles, 400 -> 400, key $.message):
  expected: "1 validation error detected: Value 'Matches' at 'roleMappings.graph.facebook.com.member.rulesConfiguration.rules.1.member.matchType' failed to satisfy constraint: Member must satisfy enum value set: [NotEqual, StartsWith, Contains, Equals]"
  actual:   "1 validation error detected: Value 'Matches' at 'roleMappings.graph.facebook.com.member.rulesConfiguration.rules.1.member.matchType' failed to satisfy constraint: Member must satisfy enum value set: [Contains, Equals, NotEqual, StartsWith]"
  The only difference is the order of the four enum values. 'Matches' and the member path are request-derived values; the bracketed set is API-constant. 3 steps in the cluster.

Cause 2 (fixture cognito-identity-principal-tag-attribute-mapping, step task#6, SetPrincipalTagAttributeMap, 400 -> 400, key $.message):
  expected entry order: '{tag40=sub, tag41=sub, ... tag49=sub, tag39=sub, tag50=sub, tag10=sub, tag11=sub, ... tag0=sub, tag1=sub, tag30=sub, ...}'
  actual entry order:   '{tag0=sub, tag1=sub, tag10=sub, tag11=sub, tag12=sub, ... tag49=sub, tag5=sub, tag50=sub, tag6=sub, tag7=sub, tag8=sub, tag9=sub}' (lexicographic)
  Fixed text, path 'principalTags' and the bound 50 all match; only the rendered entry order differs. The tagN=sub entries are the request's own 51-entry map. 2 steps in the cluster.

## Steps to Reproduce
1. git_clone, then replay_wire_corpus against the emulator built from the checkout.
2. Cause 1: re-replay fixture cogid-creds-role-mapping-match-type-out-of-enum, step task#4 — SetIdentityPoolRoles with a Rules role mapping whose second MappingRule has MatchType 'Matches'. The 400 message's enum value set must read [NotEqual, StartsWith, Contains, Equals].
3. Cause 2: re-replay fixture cognito-identity-principal-tag-attribute-mapping, step task#6 — SetPrincipalTagAttributeMap with 51 principal tags. The 400 message must render the map in AWS's stable order, not sorted by key.
4. Both must also still match on status (400), $.__type and the x-amzn-errortype header.
