Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 5bec88b641fb55fa8c22f4e210041b873076e418

# arn_resource_type never checks the ARN's service, so a foreign ARN is reported with an AppFabric resourceType its own doc says it must not

_Filed: 2026-10-29T05:21:07Z_
_Operation: ListTagsForResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `5bec88b641fb55fa8c22f4e210041b873076e418`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`tagged_resource_not_found` builds the modelled `ResourceNotFoundException` for the three tagging operations (`ListTagsForResource`, `TagResource`, `UntagResource`) and reads the shape's required `resourceType` member out of `arn_resource_type(arn)`. That helper's own documentation states the rule it implements: the segment names it matches are "the ones this service's own ARNs are built from", and "any other ARN — another service's, or a malformed one — is reported as the generic `Resource`".

The code under that sentence breaks it. It splits the WHOLE ARN on `/` and matches the four type-name tokens against every segment (taking each segment's last `:`-separated piece), with no check at all that the ARN's service field is `appfabric`. So any ARN of any service that happens to contain `appbundle`, `ingestion`, `ingestiondestination` or `appauthorization` as a `/`-segment (or as the tail of its head segment) is reported with that AppFabric `resourceType` instead of the generic `Resource` the doc promises:

* `arn:aws:s3:::appbundle` -> the single segment's last `:` piece is `appbundle` -> `resourceType: "AppBundle"`;
* `arn:aws:s3:::my-bucket/ingestion/2024-01.json` -> the `ingestion` segment matches -> `resourceType: "Ingestion"`.

Both ARNs name no AppFabric resource, so the refusal tells a client it was asking after a non-existent *app bundle* / *ingestion* of this service rather than naming a resource this service does not own. The one `resourceType` the caller can read is therefore decided by incidental text in a foreign ARN.

## Evidence

The rule, stated by the helper's own doc comment — services/service-com-amazonaws-appfabric-fabricfrontendservice/src/state.rs:1903-1908:

```
/// The kind of resource an ARN names, from the LAST type segment of its
/// resource part: `…:appbundle/<id>/ingestion/<id>` names an ingestion.
///
/// The segment names are the ones this service's own ARNs are built from (see
/// `CreateAppBundle` and `CreateIngestion`); any other ARN — another service's,
/// or a malformed one — is reported as the generic `Resource`.
```

The code directly under it, which matches the type tokens in ANY ARN with no service check — src/state.rs:1909-1923:

```
pub fn arn_resource_type(arn: &str) -> &'static str {
    let mut kind = "Resource";
    for segment in arn.split('/') {
        // The first segment carries the ARN head, whose last `:`-separated
        // piece is the resource type (`…:000000000000:appbundle`).
        match segment.split(':').next_back().unwrap_or(segment) {
            "appbundle" => kind = "AppBundle",
            "ingestion" => kind = "Ingestion",
            "ingestiondestination" => kind = "IngestionDestination",
            "appauthorization" => kind = "AppAuthorization",
            _ => {}
        }
    }
    kind
}
```

It is this value that reaches the wire — src/state.rs:1895-1900:

```
pub fn tagged_resource_not_found(arn: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: "The specified resource does not exist.".to_string(),
        resource_id: arn.to_string(),
        resource_type: arn_resource_type(arn).to_string(),
    }
}
```

and it is reached from all three tagging handlers: src/ops/list_tags_for_resource.rs:139-143, src/ops/tag_resource.rs:138-143, src/ops/untag_resource.rs:135-140 (`crate::state::tagged_resource_not_found(&input.resource_arn)`).

Test coverage: tests/all/resource_tags.rs:489-492 already drives the exact case —

```
        (
            "another service's ARN",
            "arn:aws:s3:::some-bucket",
        ),
```

— but the loop at tests/all/resource_tags.rs:499-516 asserts only `answer.status, 404` and the error code, never `resourceType`, and the chosen bucket name happens to contain none of the four tokens. So the gap is unpinned.

## Steps to Reproduce

1. Boot the emulator and address the AppFabric endpoint.
2. Call `GET /tags/arn%3Aaws%3As3%3A%3A%3Amy-bucket%2Fingestion%2F2024-01.json` (ListTagsForResource with a plain S3 object ARN).
3. Observe HTTP 404 `ResourceNotFoundException` whose `resourceType` is `"Ingestion"`.
4. Expected per `arn_resource_type`'s own documented rule ("any other ARN — another service's … — is reported as the generic `Resource`"): `resourceType: "Resource"`, because the ARN's service field is `s3`, not `appfabric`.
5. Repeat with `GET /tags/arn%3Aaws%3As3%3A%3A%3Aappbundle` and observe `resourceType: "AppBundle"`; the same two ARNs reproduce it through `POST /tags/{resourceArn}` (TagResource) and `DELETE /tags/{resourceArn}?tagKeys=env` (UntagResource), which share the same builder.
