Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 79ab93e4660c9d6243812edbc2097457ecd9acab

# Hand-applied TagKey check admits keys the modelled pattern's character class refuses, contradicting its own "whole modelled constraint is enforced" claim

_Filed: 2026-10-26T03:09:41Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `79ab93e4660c9d6243812edbc2097457ecd9acab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate takes over enforcement of the modelled `TagKey` pattern by hand, because the framework's boundary walk cannot compile the pattern's leading `(?!aws:)` lookahead and therefore drops the whole pattern (including its character class). `crate::state::tag_key_is_well_formed` is documented as answering "Whether `key` satisfies the modelled [`TAG_KEY_PATTERN`]" and as applying that pattern "verbatim", concluding that "the whole modelled constraint is enforced". It does not: the class half is approximated by `is_tag_key_char`, whose `\p{L}` leg is `char::is_alphabetic`, i.e. the Unicode `Alphabetic` property, which is strictly wider than `\p{L}`. The same file admits the consequence two doc-comments above the code: "A key carrying a combining mark is therefore accepted where AWS's class would refuse it". So a tag key containing any `Mn`/`Mc` combining mark that carries `Other_Alphabetic` — e.g. the ordinary Devanagari key "नि" (U+0928 U+093F, where U+093F is category Mc) — is accepted, stored and later reported, while the modelled pattern `^(?!aws:)[\p{L}\p{Z}\p{N}_.:/=+\-@]*$` (whose class names only L, Z, N plus eight literal punctuation characters) does not admit it.

Because the one helper is the single home of the rule, the same hole is in all three operations that accept a tag key: `TagResource` (`tags`), `UntagResource` (`tagKeys`) and `InvokeDataAutomationAsync` (`tags`) — a submission or tag edit carrying such a key answers 200 and the key becomes part of the resource's stored tag map, which `ListTagsForResource` then reports. The crate's stated reason for the approximation is to avoid "a second regex engine in the dependency graph", but a lookahead-capable engine is already in this workspace's Cargo.lock (`fancy-regex`, Cargo.lock:2488, taken as a direct dependency by three sibling service crates), and the class half alone is compilable by the plain `regex` crate the boundary walk already uses — so the premise the divergence is justified by does not hold. No test pins the divergence: the 14-row tag-key table covers L/N/Z members and non-class symbols, control characters and ASCII punctuation, but no combining mark.

## Evidence

The authority, stated in the crate itself: `src/state.rs:54-57` — "/// The modelled `TagKey` pattern, verbatim (`smithy_query` shape `TagKey`)." … `pub const TAG_KEY_PATTERN: &str = r"^(?!aws:)[\p{L}\p{Z}\p{N}_.:/=+\-@]*$";` (same pattern as `smithy_query` shape `TagKey`: `"pattern": "^(?!aws:)[\\p{L}\\p{Z}\\p{N}_.:/=+\\-@]*$"`).

The claim: `src/state.rs:91` — "/// Whether `key` satisfies the modelled [`TAG_KEY_PATTERN`]."; `src/state.rs:97-99` — "So the pattern is applied here, verbatim, by the three operations that accept a tag key (`InvokeDataAutomationAsync` and `TagResource` through `tags`, `UntagResource` through `tagKeys`)."; `src/state.rs:105-107` — "What remains is the plain character class [`is_tag_key_char`] applies to every character, anchored at both ends by `*$`, so the whole modelled constraint is enforced without a second regex engine in the dependency graph."

The admission that it is NOT enforced, in the same file: `src/state.rs:76-79` — "`\p{L}` is `char::is_alphabetic` up to `Alphabetic`'s wider definition: that property is `\p{L}` plus `Nl` — already admitted through `\p{N}` — plus `Other_Alphabetic`, which is combining marks. A key carrying a combining mark is therefore accepted where AWS's class would refuse it". The code under it: `src/state.rs:84-89` — `fn is_tag_key_char(c: char) -> bool { c.is_alphabetic() || c.is_numeric() || (c.is_whitespace() && !c.is_control()) || TAG_KEY_PUNCTUATION.contains(&c) }` and `src/state.rs:108-110` — `pub fn tag_key_is_well_formed(key: &str) -> bool { !key.starts_with(RESERVED_TAG_KEY_PREFIX) && key.chars().all(is_tag_key_char) }`.

Callers that reach it: `src/ops/tag_resource.rs:161` — `if let Some(message) = crate::state::malformed_tag_refusal(&input.tags)`; `src/ops/untag_resource.rs:159` — `crate::state::malformed_tag_key_refusal(&input.tag_keys)`; `src/ops/invoke_data_automation_async.rs:214-215` — `crate::state::malformed_tag_refusal(input.tags.as_deref().unwrap_or_default())`.

Stated reason already obsolete: `Cargo.lock:2488` — `name = "fancy-regex"`, and `services/service-com-amazonaws-odb-odb/Cargo.toml:35` — `fancy-regex = "0.16"` (also vpclattice-mercurycontrolplane and servicediscovery-route53autonaming).

Test coverage: `tests/all/tag_key_refusal.rs:190-237` is the 14-row table (`("a CJK letter key (\\p{L})", "請求書", true)`, `("a number-category key (\\p{N}, No)", "share\u{bc}", true)`, `("a math symbol, outside the class", "budget\u{2264}cap", false)`, …) driven through all three operations; no row carries a combining mark, so nothing pins the current acceptance. `tests/all/data_automation_invocations.rs:1426-1430` restates the same rule for the wire test.

## Steps to Reproduce

1. Start the emulator with this service registered and submit a job: POST / with `X-Amz-Target: AmazonBedrockKeystoneRuntimeService.InvokeDataAutomationAsync` and body {"inputConfiguration":{"s3Uri":"s3://in-bucket/assets/doc.pdf"},"outputConfiguration":{"s3Uri":"s3://out-bucket/results/"},"dataAutomationProfileArn":"arn:aws:bedrock:us-west-2:aws:data-automation-profile/us.data-automation-v1","dataAutomationConfiguration":{"dataAutomationProjectArn":"arn:aws:bedrock:us-west-2:000000000000:data-automation-project/invoices"}}; keep the returned invocationArn.
2. Call `TagResource` with a key containing a combining mark that is not in `\p{L}`/`\p{Z}`/`\p{N}`: {"resourceARN":"<arn>","tags":[{"key":"\u0928\u093f","value":"v"}]} (U+093F DEVANAGARI VOWEL SIGN I is category Mc).
3. Observe HTTP 200: the key was admitted, even though the modelled `TagKey` pattern quoted verbatim at src/state.rs:57 admits only `\p{L}`, `\p{Z}`, `\p{N}` and the eight literal punctuation characters.
4. Call `ListTagsForResource` with the same `resourceARN`: the response reports the key, so the value outside the modelled class is now part of persisted state.
5. Repeat step 2 against `UntagResource` ({"resourceARN":"<arn>","tagKeys":["\u0928\u093f"]}) and against `InvokeDataAutomationAsync`'s `tags` member: both also answer 200, because all three reach the same `tag_key_is_well_formed` helper (src/state.rs:108).
