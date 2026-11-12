Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 9cdb13cb194e5f529c19ad9711760ae55b49f7ec

# ARNs are minted from the raw SigV4 credential-scope region, so ChangeSetArn / EntityArn / AssessmentArn can carry a value the modelled ARN pattern forbids

_Filed: 2026-11-12T19:41:34Z_
_Operation: StartChangeSet | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `9cdb13cb194e5f529c19ad9711760ae55b49f7ec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartChangeSet` is the only handler that reads the request's region, and it takes it verbatim out of the SigV4 credential scope and stores it on the `ChangeSetRecord`. Every ARN this service mints then interpolates that string: the change set's own ARN at start, each entity's ARN when the change set is applied, and each assessment's ARN when a delivery option is evaluated.

`ARN` — the shape every one of those output members targets (`StartChangeSetResponse$ChangeSetArn`, `CancelChangeSetResponse$ChangeSetArn`, `DescribeChangeSetResponse$ChangeSetArn`, `ChangeSetSummaryListItem$ChangeSetArn`, `DescribeEntityResponse$EntityArn`, `EntityDetail$EntityArn`, `EntitySummary$EntityArn`, `DescribeAssessmentResponse$AssessmentArn`, `AssessmentSummary$AssessmentArn`) — is `@pattern("^[a-zA-Z0-9:*/-]+$")`. The credential-scope region is not held to that character class anywhere: the parser that produces it only requires the field to be non-empty and to carry no `/`. So a request signed for a region spelled `us_east_1` (or `us.east.1`, or `us+east+1`) makes the handler store `arn:aws:aws-marketplace:us_east_1:000000000000:AWSMarketplace/ChangeSet/cs-000000000001`, which the `ARN` pattern does not admit — and that stored string is then served back by eight further operations, since every read reports the ARN the record holds.

This is the crate's own rule broken in the crate's own code. `assessment_arn`'s doc claims the value it builds is one "`ARN`'s `^[a-zA-Z0-9:*/-]+$` allows", which is only true for a region inside that class; and the crate states the general rule for every member whose boundary validation it replaces with a predicate of its own — "no value past it is accepted, stored or served back in a shape that forbids it". The region is the one component of these ARNs that passes through no predicate at all. (Note the asymmetry inside this service that makes the gap only partial, and so easy to miss: `ResourceARN`, the shape the tagging and resource-policy inputs target, permits `_` in its region field — `[\w+=/,.@-]*` — so `is_resource_arn` happily accepts exactly the ARN the `ARN`-targeted response members cannot carry.)

## Evidence

MODEL — the constraint every ARN-reporting member targets (smithy_query query="shape" shape="ARN"):
```
{"shape_id": "com.amazonaws.marketplacecatalog#ARN", "type": "string", "length": {"min": 1, "max": 2048}, "pattern": "^[a-zA-Z0-9:*/-]+$"}
```
(`^[a-zA-Z0-9:*/-]+$` admits no `_`, `.`, `+` or `=`.)

THE UNCHECKED REGION — services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/state.rs:3704-3709:
```
/// The region this request was signed for — what the ARNs it mints name.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}
```
WHERE IT IS READ — src/ops/start_change_set.rs:399 (the only caller in the crate; `grep request_region src/` returns this line and the definition):
```
        let region = crate::state::request_region(ctx.parts());
```
WHAT IT IS STORED AS — src/state.rs:971-974 (`ChangeSetRecord`):
```
    /// The region this change set was started in — what the ARNs of the
    /// entities it applies are built for.
    pub region: String,
```
THE THREE ARNs BUILT FROM IT:
- src/ops/start_change_set.rs:690-691: `arn: crate::state::change_set_arn(&region, &id),`
- src/state.rs:2117 (`apply_changes`): `arn: entity_arn(&region, &entity_type, &entity_id),`
- src/state.rs:2247 (`record_assessment`): `arn: assessment_arn(region, &id),`

HOW THE REGION LANDS IN THE STRING — src/state.rs:3357-3372:
```
pub fn change_set_arn(region: &str, change_set_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{AWS_MARKETPLACE_CATALOG}/ChangeSet/{change_set_id}"),
    )
}
```
and vendor/server/src/lib.rs:170-172:
```
pub fn emulator_arn(service: &str, region: &str, resource: &str) -> String {
    format!("arn:aws:{service}:{region}:{EMULATOR_ACCOUNT_ID}:{resource}")
}
```
NOTHING VALIDATES THE REGION'S CHARACTERS — vendor/server/src/lib.rs:1963-1975 (`sigv4_credential_scope`, the only producer):
```
    let (Some(access_key_id), Some(date), Some(region), Some(service), Some("aws4_request"), None) = (
        fields.next(), ... )
    else { return None; };
    if access_key_id.is_empty() || date.is_empty() || region.is_empty() || service.is_empty() {
        return None;
    }
```

BOTH SIDES OF THE IN-CRATE CONTRADICTION:
- src/state.rs:3375-3380 (`assessment_arn`'s own doc) ASSERTS the result is inside the pattern: "The ARN of an assessment, in the form `DescribeAssessmentRequest$AssessmentIdentifier` documents (\"arn:aws:aws-marketplace:us-east-1::AWSMarketplace/Assessment/assessment-12345\") and `ARN`'s `^[a-zA-Z0-9:*/-]+$` allows."
- src/state.rs:4182-4185 states the general rule the region escapes: "A member whose boundary validation this service replaces with a predicate of its own is still bounded by it: the predicate stands in for the whole constraint, length included, so no value past it is accepted, stored or served back in a shape that forbids it."
- src/state.rs:4211-4214 repeats it for `Entity$Type`: "a reference no `EntityType` can hold, so it is refused rather than stored and reported back out of contract."
- The looser sibling shape that hides the gap — src/state.rs:3571-3572 (`RESOURCE_ARN_PATTERN`): `^arn:[\w+=/,.@-]+:aws-marketplace:[\w+=/,.@-]*:[0-9]+:[\w+=,.@-]+(/[\w+=,.@-]+)*$`, whose region field `[\w+=/,.@-]*` DOES admit `_`, so `is_resource_arn` (src/state.rs:3587-3628) accepts the very ARN the `ARN`-targeted response members cannot carry.

TEST COVERAGE: every test in tests/all/ signs with a hyphen-only region — `tests/all/change_set_lifecycle.rs:29-30` and 35 sibling files use `Credential=AKIDEXAMPLE/20260101/eu-west-1/aws-marketplace/aws4_request`, `tests/all/resource_policy_lifecycle.rs:39-40` uses `us-east-1` — so no test exercises a region outside `[a-zA-Z0-9-]` and nothing pins the current behaviour. tests/all/modelled_answer_bounds.rs:6-9 states the crate's rule for answered members ("a message the emulator reports satisfies the modelled `@length` of the member that carries it") but covers only message lengths, not the minted ARNs.

## Steps to Reproduce

1. POST /StartChangeSet with the Authorization header `AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us_east_1/aws-marketplace/aws4_request, SignedHeaders=host, Signature=x` (a region spelled with an underscore; `sigv4_credential_scope` accepts it unchanged) and body {"Catalog":"AWSMarketplace","ChangeSet":[{"ChangeType":"CreateProduct","Entity":{"Type":"AmiProduct@1.0"},"Details":"{\"ProductTitle\":\"a product\"}"}]}.
2. Read `/ChangeSetArn` from the 200 answer: it is `arn:aws:aws-marketplace:us_east_1:000000000000:AWSMarketplace/ChangeSet/cs-000000000001`, which `StartChangeSetResponse$ChangeSetArn`'s target shape `ARN` forbids (`^[a-zA-Z0-9:*/-]+$` admits no `_`).
3. Wait out CHANGE_SET_PREPARING + CHANGE_SET_APPLYING, then GET /DescribeChangeSet?catalog=AWSMarketplace&changeSetId=<id> — the same out-of-pattern ARN is served back from the stored record, and POST /ListChangeSets reports it in `ChangeSetSummaryListItem$ChangeSetArn`.
4. Read the created entity id from `EntityIdList`, then GET /DescribeEntity?catalog=AWSMarketplace&entityId=<id> and POST /BatchDescribeEntities for the same id: `EntityArn` / `EntityDetails.<id>.EntityArn` carry `arn:aws:aws-marketplace:us_east_1:000000000000:AWSMarketplace/AmiProduct/<id>`, again outside `ARN`.
5. Repeat step 1 with a change that submits an AMI `DeliveryOptions` entry, then POST /ListAssessments: `AssessmentSummary$AssessmentArn` carries the same region, contradicting `assessment_arn`'s own doc comment at state.rs:3375-3378.
6. Contrast: POST /ListTagsForResource with that same ARN is accepted (`is_resource_arn` admits `_` in the region field), showing the two ARN shapes of this one service disagree about the value the handler minted.
