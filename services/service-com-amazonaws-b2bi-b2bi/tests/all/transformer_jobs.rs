// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the transformer-job slice — `UpdateTransformer`,
//! `StartTransformerJob` and `GetTransformerJob` — driven over the wire (awsJson1_0,
//! `x-amz-target: B2BI.<Operation>`) through the same router the emulator boots, with the
//! emulator's own Amazon S3 service mounted beside b2bi: a job reads its input file from
//! S3 and writes its output file back.
//!
//! The rules under test:
//!
//! * a job runs only on an ACTIVE transformer: one in any other status is refused with
//!   `ValidationException` (HTTP 400) and nothing is written under the requested output
//!   prefix;
//! * a job on an active transformer performs the transformation the transformer is
//!   configured for — the X12 input conversion, then its JSONATA mapping evaluated by a
//!   real JSONata interpreter — writes the result under `outputLocation` and reports it:
//!   `running` until its outcome is due, then `succeeded` with the output file it wrote;
//! * a transformation this emulator cannot perform ends in the job's own modelled `failed`
//!   state with a message that says why, never in a success that wrote nothing;
//! * `GetTransformerJob` resolves a job only under the transformer it ran.

use serde_json::{json, Value};
use service_com_amazonaws_b2bi_b2bi::scaffolded::NOT_FOUND_STATUS;

use crate::edi_parsing::{given_object, BUCKET, X12_850};
use crate::onboarding_loop::{b2bi_router, call, error_code, s3};

/// The prefix a job's output file is written under in these tests.
const OUTPUT_PREFIX: &str = "out/";

/// Create a transformer with `configuration` merged into the request, and answer its id.
async fn given_transformer_with(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
    configuration: Value,
) -> String {
    let mut body = json!({ "name": name });
    for (member, value) in configuration.as_object().expect("an object of members") {
        body[member] = value.clone();
    }
    let (status, created) = call(router, "CreateTransformer", body).await;
    assert_eq!(
        status, 200,
        "creating the transformer {name:?} must succeed, got {status} {created}"
    );
    created["transformerId"]
        .as_str()
        .expect("a transformerId")
        .to_string()
}

/// Move `transformer_id` to `status`, answering the reported status.
pub(crate) async fn set_status(
    router: &smithy_server_runtime::MultiServiceRouter,
    transformer_id: &str,
    status: &str,
) -> Value {
    let (code, updated) = call(
        router,
        "UpdateTransformer",
        json!({ "transformerId": transformer_id, "status": status }),
    )
    .await;
    assert_eq!(
        code, 200,
        "moving the transformer to {status:?} must succeed, got {code} {updated}"
    );
    updated
}

/// The `StartTransformerJob` request for `transformer_id` over the object at `key`.
fn start_request(transformer_id: &str, key: &str) -> Value {
    json!({
        "transformerId": transformer_id,
        "inputFile": { "bucketName": BUCKET, "key": key },
        "outputLocation": { "bucketName": BUCKET, "key": OUTPUT_PREFIX }
    })
}

/// The current-style inbound X12 configuration, with the mapping `mapping` (absent when
/// `None`).
fn inbound(mapping: Option<Value>) -> Value {
    let mut configuration = json!({
        "inputConversion": {
            "fromFormat": "X12",
            "formatOptions": {
                "x12": { "transactionSet": "X12_850", "version": "VERSION_4010" }
            }
        }
    });
    if let Some(mapping) = mapping {
        configuration["mapping"] = mapping;
    }
    configuration
}

/// Poll `GetTransformerJob` until it reports a terminal status, and answer the response.
///
/// A job reports the non-terminal `running` status for a span after the call that started
/// it, exactly as AWS answers that call with a job that has not finished, so a caller
/// reads it twice: this is what a client's own poll loop does.
pub(crate) async fn await_job(
    router: &smithy_server_runtime::MultiServiceRouter,
    transformer_id: &str,
    job_id: &str,
) -> Value {
    let request = json!({ "transformerId": transformer_id, "transformerJobId": job_id });
    let mut last = Value::Null;
    for _ in 0..40 {
        let (status, response) = call(router, "GetTransformerJob", request.clone()).await;
        assert_eq!(
            status, 200,
            "GetTransformerJob must succeed, got {status} {response}"
        );
        if response["status"] != json!("running") {
            return response;
        }
        last = response;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("the job never left the running status, last report {last}");
}

/// RULE: a transformer job runs only on an ACTIVE transformer. A transformer in any other
/// status is refused with `ValidationException` (400) naming the transformer, and the
/// refused call writes nothing under the requested output prefix.
#[tokio::test]
async fn a_job_on_an_inactive_transformer_is_refused() {
    let router = b2bi_router();
    given_object(&router, "inbound/job-inactive.edi", X12_850).await;

    // Each row: the case, the configuration, and the status the transformer is left in.
    // A freshly created transformer is `inactive`, and `UpdateTransformer`'s one
    // documented move (`inactive` -> `active`) is the only thing that leaves it.
    let cases: Vec<(&str, Value, Option<&str>)> = vec![
        ("freshly created (inactive)", inbound(Some(json!({ "templateLanguage": "JSONATA", "template": "$" }))), None),
        ("renamed while inactive", inbound(None), Some("renamed-while-inactive")),
        (
            "legacy configuration, inactive",
            json!({ "fileFormat": "JSON", "mappingTemplate": "$" }),
            None,
        ),
    ];

    for (index, (case, configuration, rename)) in cases.into_iter().enumerate() {
        let transformer_id =
            given_transformer_with(&router, &format!("inactive-{index}"), configuration).await;
        if let Some(name) = rename {
            // An update that leaves the status alone: the record stays inactive, so the
            // row really does reach `StartTransformerJob` with an updated, inactive
            // transformer.
            let (code, renamed) = call(
                &router,
                "UpdateTransformer",
                json!({ "transformerId": transformer_id.clone(), "name": name }),
            )
            .await;
            assert_eq!(
                code, 200,
                "case {case:?}: renaming an inactive transformer must succeed, got {code} \
                 {renamed}"
            );
        }

        let (code, response) = call(
            &router,
            "StartTransformerJob",
            start_request(&transformer_id, "inbound/job-inactive.edi"),
        )
        .await;
        assert_eq!(
            code, 400,
            "case {case:?}: expected HTTP 400, got {code} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {response}"
        );
        // The modelled `ValidationException`'s message member is `Message`
        // (`smithy_query(query="shape",shape="ValidationException")`).
        let message = response["Message"].as_str().unwrap_or_default();
        assert!(
            message.contains(&transformer_id) && message.contains("active"),
            "case {case:?}: the refusal must name the transformer and the status it needs, \
             got {message:?}"
        );

        // Nothing was written under the output prefix: the listing is empty.
        let (code, listed) = s3(
            &router,
            "GET",
            &format!("/{BUCKET}?list-type=2&prefix={OUTPUT_PREFIX}"),
            b"",
        )
        .await;
        let listing = String::from_utf8_lossy(&listed);
        assert_eq!(code, 200, "case {case:?}: the listing must succeed, got {code}");
        assert!(
            !listing.contains("<Key>"),
            "case {case:?}: a refused job must write nothing under {OUTPUT_PREFIX}, got {listing}"
        );
    }
}

/// RULE: a job on an active transformer parses the X12 input file, evaluates the
/// transformer's JSONATA mapping over the parsed document and writes the result under
/// `outputLocation`; `GetTransformerJob` reports `succeeded` with the full path to that
/// file, and the file holds what the mapping computed.
#[tokio::test]
async fn a_job_on_an_active_transformer_writes_what_the_mapping_computed() {
    let router = b2bi_router();
    given_object(&router, "inbound/job.edi", X12_850).await;

    // Each row: the case, the transformer's mapping, and what the output document must
    // hold — the JSON pointer-ish path into the written file and the value at it. The
    // path follows the document this service's X12 reader builds: a transaction set's own
    // body segments are entries of its `segments` list (see `ops::test_parsing`).
    let cases: Vec<(&str, Option<Value>, &str, Value)> = vec![
        (
            "no mapping at all: the output is the parsed document",
            None,
            "interchanges/0/functional_groups/0/transactions/0/segments/0/BEG_03",
            json!("PO12345"),
        ),
        (
            "the JSONata identity expression",
            Some(json!({ "templateLanguage": "JSONATA", "template": "$" })),
            "interchanges/0/functional_groups/0/transactions/0/segments/0/BEG_03",
            json!("PO12345"),
        ),
        (
            "a JSONata expression that projects one field",
            Some(json!({
                "templateLanguage": "JSONATA",
                "template": "{\"purchaseOrderNumber\": interchanges[0].functional_groups[0].transactions[0].segments[0].BEG_03}"
            })),
            "purchaseOrderNumber",
            json!("PO12345"),
        ),
        (
            "a JSONata expression that counts the transactions",
            Some(json!({
                "templateLanguage": "JSONATA",
                "template": "{\"transactions\": $count(interchanges[0].functional_groups[0].transactions)}"
            })),
            "transactions",
            json!(1),
        ),
    ];

    for (index, (case, mapping, path, expected)) in cases.into_iter().enumerate() {
        let transformer_id =
            given_transformer_with(&router, &format!("active-{index}"), inbound(mapping)).await;
        set_status(&router, &transformer_id, "active").await;

        let (code, started) = call(
            &router,
            "StartTransformerJob",
            start_request(&transformer_id, "inbound/job.edi"),
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: the job must start, got {code} {started}"
        );
        let job_id = started["transformerJobId"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            job_id.len(),
            25,
            "case {case:?}: the modelled TransformerJobId is 25 characters, got {job_id:?}"
        );
        assert!(
            job_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
            "case {case:?}: the job id must satisfy the modelled pattern, got {job_id:?}"
        );

        let report = await_job(&router, &transformer_id, &job_id).await;
        assert_eq!(
            report["status"],
            json!("succeeded"),
            "case {case:?}: the job must succeed, got {report}"
        );
        assert_eq!(
            report["message"],
            Value::Null,
            "case {case:?}: a succeeded job carries no error message, got {report}"
        );
        let output = report["outputFiles"][0].clone();
        assert_eq!(
            output["bucketName"],
            json!(BUCKET),
            "case {case:?}: the output file must be reported in the requested bucket, got {report}"
        );
        let key = output["key"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            key,
            format!("{OUTPUT_PREFIX}{job_id}.json"),
            "case {case:?}: the output key must be the generated file name under the \
             requested prefix, got {key:?}"
        );

        // The file is really there, and holds what the mapping computed.
        let (code, body) = s3(&router, "GET", &format!("/{BUCKET}/{key}"), b"").await;
        assert_eq!(
            code, 200,
            "case {case:?}: the output file must be readable, got {code}"
        );
        let document: Value = serde_json::from_slice(&body).unwrap_or_else(|error| {
            panic!(
                "case {case:?}: the output file must be JSON, got {error}: {:?}",
                String::from_utf8_lossy(&body)
            )
        });
        let mut found = &document;
        for step in path.split('/') {
            found = match step.parse::<usize>() {
                Ok(index) => &found[index],
                Err(_) => &found[step],
            };
        }
        assert_eq!(
            *found, expected,
            "case {case:?}: the output at {path:?} must be {expected}, got {found} in {document}"
        );
    }
}

/// RULE: a transformation this emulator cannot perform reaches the job's own modelled
/// `failed` status with a message that says why — it is never reported as a success, and
/// it writes no output file.
#[tokio::test]
async fn a_transformation_this_emulator_cannot_perform_fails_the_job() {
    let router = b2bi_router();
    given_object(&router, "inbound/fails.edi", X12_850).await;
    given_object(&router, "inbound/not-edi.txt", "not an interchange at all").await;

    // Each row: the case, the configuration, the input object, and a fragment the failed
    // job's message must carry.
    let cases: Vec<(&str, Value, &str, &str)> = vec![
        (
            "an XSLT mapping, which needs a processor this emulator has not got",
            inbound(Some(json!({
                "templateLanguage": "XSLT",
                "template": "<xsl:stylesheet version=\"1.0\"/>"
            }))),
            "inbound/fails.edi",
            "XSLT",
        ),
        (
            "the outbound direction, which generates EDI",
            json!({
                "outputConversion": {
                    "toFormat": "X12",
                    "formatOptions": {
                        "x12": { "transactionSet": "X12_850", "version": "VERSION_4010" }
                    }
                }
            }),
            "inbound/fails.edi",
            "does not generate EDI",
        ),
        (
            // Both directions at once: the input could be parsed, but an EDI file is what
            // the transformer is configured to produce, so a JSON document would be a
            // plausible wrong answer rather than the transformation asked for.
            "an inputConversion AND an outputConversion",
            json!({
                "inputConversion": {
                    "fromFormat": "X12",
                    "formatOptions": {
                        "x12": { "transactionSet": "X12_850", "version": "VERSION_4010" }
                    }
                },
                "outputConversion": {
                    "toFormat": "X12",
                    "formatOptions": {
                        "x12": { "transactionSet": "X12_850", "version": "VERSION_4010" }
                    }
                }
            }),
            "inbound/fails.edi",
            "does not generate EDI",
        ),
        (
            "an input file that is not an interchange",
            inbound(None),
            "inbound/not-edi.txt",
            "The EDI file could not be parsed.",
        ),
        (
            "an input file that is not there",
            inbound(None),
            "inbound/absent.edi",
            "could not be read",
        ),
        (
            "a JSONATA mapping that is not a JSONata expression",
            inbound(Some(json!({ "templateLanguage": "JSONATA", "template": "{{{" }))),
            "inbound/fails.edi",
            "could not be evaluated",
        ),
    ];

    for (index, (case, configuration, key, fragment)) in cases.into_iter().enumerate() {
        let transformer_id =
            given_transformer_with(&router, &format!("failing-{index}"), configuration).await;
        set_status(&router, &transformer_id, "active").await;

        let (code, started) = call(
            &router,
            "StartTransformerJob",
            start_request(&transformer_id, key),
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: the job must start, got {code} {started}"
        );
        let job_id = started["transformerJobId"].as_str().unwrap_or_default().to_string();
        let report = await_job(&router, &transformer_id, &job_id).await;
        assert_eq!(
            report["status"],
            json!("failed"),
            "case {case:?}: the job must reach the failed status, got {report}"
        );
        let message = report["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(fragment),
            "case {case:?}: expected {fragment:?} in the job's message, got {message:?}"
        );
        assert_eq!(
            report["outputFiles"],
            Value::Null,
            "case {case:?}: a failed job reports no output file, got {report}"
        );
    }
}

/// RULE: a caller-supplied mapping is evaluated under bounds that always fire: an
/// expression that does not terminate, one that recurses past the depth bound and one that
/// constructs a document larger than this emulator writes all end as the job's own
/// modelled `failed` state with a message, never as a success and never by holding the
/// request open.
#[tokio::test]
async fn a_mapping_that_runs_away_fails_its_job() {
    let router = b2bi_router();
    given_object(&router, "inbound/runaway.edi", X12_850).await;

    // Each row: the case and the JSONata program. Each one is a program a client could
    // really send, and each one must be stopped by a DIFFERENT bound: unbounded
    // self-recursion by the depth bound, an unbounded loop by the time budget, and a
    // document far past `MAX_OUTPUT_CHARS` by the output bound.
    let cases: Vec<(&str, &str)> = vec![
        (
            "a function that recurses without end",
            "($f := function($n) { $f($n + 1) }; $f(0))",
        ),
        (
            "a fold over an unbounded range",
            "$reduce([1..100000000], function($a, $b) { $a + $b }, 0)",
        ),
        (
            "a document larger than this emulator writes",
            "{\"rows\": [1..400000]}",
        ),
    ];

    for (index, (case, template)) in cases.into_iter().enumerate() {
        let transformer_id = given_transformer_with(
            &router,
            &format!("runaway-{index}"),
            inbound(Some(json!({ "templateLanguage": "JSONATA", "template": template }))),
        )
        .await;
        set_status(&router, &transformer_id, "active").await;

        let (code, started) = call(
            &router,
            "StartTransformerJob",
            start_request(&transformer_id, "inbound/runaway.edi"),
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: the job must start, got {code} {started}"
        );
        let job_id = started["transformerJobId"].as_str().unwrap_or_default().to_string();
        let report = await_job(&router, &transformer_id, &job_id).await;
        assert_eq!(
            report["status"],
            json!("failed"),
            "case {case:?}: a mapping stopped by a bound must fail its job, got {report}"
        );
        let message = report["message"].as_str().unwrap_or_default();
        assert!(
            !message.is_empty(),
            "case {case:?}: a failed job must say why, got {report}"
        );
        assert_eq!(
            report["outputFiles"],
            Value::Null,
            "case {case:?}: a job stopped by a bound writes no output file, got {report}"
        );
    }
}

/// RULE: a job is reported only under the transformer that ran it, and only while this
/// service holds it: every other pair of ids is `ResourceNotFoundException`.
#[tokio::test]
async fn a_job_resolves_only_under_its_own_transformer() {
    let router = b2bi_router();
    given_object(&router, "inbound/resolve.edi", X12_850).await;

    let transformer_id = given_transformer_with(&router, "resolving", inbound(None)).await;
    set_status(&router, &transformer_id, "active").await;
    let other_id = given_transformer_with(&router, "other", inbound(None)).await;
    let (_, started) = call(
        &router,
        "StartTransformerJob",
        start_request(&transformer_id, "inbound/resolve.edi"),
    )
    .await;
    let job_id = started["transformerJobId"].as_str().unwrap_or_default().to_string();

    // Each row: the case, the transformerId and transformerJobId to ask for, the status.
    let cases: Vec<(&str, String, String, u16)> = vec![
        ("the job under its own transformer", transformer_id.clone(), job_id.clone(), 200),
        ("the job under another transformer", other_id, job_id.clone(), NOT_FOUND_STATUS),
        (
            "an unknown job under a known transformer",
            transformer_id.clone(),
            "0123456789abcdef012345678".to_string(),
            NOT_FOUND_STATUS,
        ),
        (
            "a job under an unknown transformer",
            "t-00000000000000000".to_string(),
            job_id,
            NOT_FOUND_STATUS,
        ),
    ];

    for (case, transformer, job, expected) in cases {
        let (status, response) = call(
            &router,
            "GetTransformerJob",
            json!({ "transformerId": transformer, "transformerJobId": job }),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected}, got {status} {response}"
        );
        if expected == NOT_FOUND_STATUS {
            assert_eq!(
                error_code(&response),
                "ResourceNotFoundException",
                "case {case:?}: expected a ResourceNotFoundException, got {response}"
            );
        }
    }
}

/// RULE: `StartTransformerJob` resolves `transformerId` against this service's store: an
/// id it does not hold is `ResourceNotFoundException`, and a location naming no
/// bucket is `ValidationException` (400).
#[tokio::test]
async fn starting_a_job_resolves_its_transformer_and_its_locations() {
    let router = b2bi_router();
    given_object(&router, "inbound/guard.edi", X12_850).await;
    let transformer_id = given_transformer_with(&router, "guarded", inbound(None)).await;
    set_status(&router, &transformer_id, "active").await;

    // Each row: the case, the request, the expected status and error code.
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "an unknown transformerId",
            start_request("t-00000000000000000", "inbound/guard.edi"),
            NOT_FOUND_STATUS,
            "ResourceNotFoundException",
        ),
        (
            "an inputFile with no bucket",
            json!({
                "transformerId": transformer_id.clone(),
                "inputFile": { "key": "inbound/guard.edi" },
                "outputLocation": { "bucketName": BUCKET, "key": OUTPUT_PREFIX }
            }),
            400,
            "ValidationException",
        ),
        (
            "an inputFile with no key",
            json!({
                "transformerId": transformer_id.clone(),
                "inputFile": { "bucketName": BUCKET },
                "outputLocation": { "bucketName": BUCKET, "key": OUTPUT_PREFIX }
            }),
            400,
            "ValidationException",
        ),
        (
            "an outputLocation with no bucket",
            json!({
                "transformerId": transformer_id.clone(),
                "inputFile": { "bucketName": BUCKET, "key": "inbound/guard.edi" },
                "outputLocation": { "key": OUTPUT_PREFIX }
            }),
            400,
            "ValidationException",
        ),
    ];

    for (case, body, expected_status, expected_code) in cases {
        let (status, response) = call(&router, "StartTransformerJob", body).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            expected_code,
            "case {case:?}: expected a {expected_code}, got {response}"
        );
    }
}

/// RULE: `clientToken` is this operation's `@idempotencyToken`, so one token names ONE
/// request: a repeat of the SAME request under a settled token answers with the job that
/// token started (and starts no second job), while a DIFFERENT request under that token is
/// refused with `ConflictException` (409) — the same two arms `CreateTransformer` applies
/// to its own token. A request carrying no token at all starts a job of its own every time.
///
/// The rows differ along what the rule generalises over: which member of the request
/// changed, and whether a token is carried at all.
#[tokio::test]
async fn one_client_token_names_one_transformer_job() {
    let router = b2bi_router();
    given_object(&router, "inbound/token-a.edi", X12_850).await;
    given_object(&router, "inbound/token-b.edi", X12_850).await;
    let transformer_id = given_transformer_with(&router, "tokened", inbound(None)).await;
    set_status(&router, &transformer_id, "active").await;

    let token = "shared-job-token";
    let with_token = |body: Value| {
        let mut body = body;
        body["clientToken"] = json!(token);
        body
    };

    // The first call under the token starts the job every later row is compared with. Its
    // in-flight window is the processing of that call, so it is over as soon as the call has
    // answered: a repeat is a replay from then on, with nothing waited for.
    let first = with_token(start_request(&transformer_id, "inbound/token-a.edi"));
    let (status, started) = call(&router, "StartTransformerJob", first.clone()).await;
    assert_eq!(
        status, 200,
        "the first call under the token must start a job, got {status} {started}"
    );
    let job_id = started["transformerJobId"]
        .as_str()
        .expect("a transformerJobId")
        .to_string();
    await_job(&router, &transformer_id, &job_id).await;

    // Each row: the case, the request, and what it must be answered with — the job the
    // token already started, or a conflict.
    let cases: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "the identical request under the same token",
            first.clone(),
            Some(job_id.as_str()),
        ),
        (
            "a different inputFile under the same token",
            with_token(start_request(&transformer_id, "inbound/token-b.edi")),
            None,
        ),
        (
            "a different outputLocation under the same token",
            with_token(json!({
                "transformerId": transformer_id.clone(),
                "inputFile": { "bucketName": BUCKET, "key": "inbound/token-a.edi" },
                "outputLocation": { "bucketName": BUCKET, "key": "elsewhere/" }
            })),
            None,
        ),
    ];

    for (case, body, replayed) in cases {
        let (status, response) = call(&router, "StartTransformerJob", body).await;
        match replayed {
            Some(expected) => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a replay must answer with the original job, got \
                     {status} {response}"
                );
                assert_eq!(
                    response["transformerJobId"],
                    json!(expected),
                    "case {case:?}: the replay must answer with job {expected:?}, got \
                     {response}"
                );
            }
            None => {
                assert_eq!(
                    status, 409,
                    "case {case:?}: a different request under a used token must conflict, \
                     got {status} {response}"
                );
                assert_eq!(
                    error_code(&response),
                    "ConflictException",
                    "case {case:?}: expected a ConflictException, got {response}"
                );
            }
        }
    }

    // …and two calls carrying NO token each start their own job.
    let mut ids = Vec::new();
    for attempt in 0..2 {
        let (status, response) = call(
            &router,
            "StartTransformerJob",
            start_request(&transformer_id, "inbound/token-a.edi"),
        )
        .await;
        assert_eq!(
            status, 200,
            "attempt {attempt} without a token must start a job, got {status} {response}"
        );
        ids.push(
            response["transformerJobId"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }
    assert_ne!(
        ids[0], ids[1],
        "two calls carrying no clientToken must start two jobs, got {ids:?}"
    );
}

/// RULE: the status move `StartTransformerJob` needs is `UpdateTransformer`'s: a stored
/// transformer moves from `inactive` to `active`, the move is what a later
/// `GetTransformer` reports, and a job's precondition reads that stored status — which, as
/// the one-way move leaves it, is terminal.
///
/// `UpdateTransformer`'s own contract — partial update, the configuration styles, the
/// immutability of an active transformer — is covered by that operation's suite
/// (`tests/all/update_transformer.rs`); this asserts only the part the transformer-job
/// slice depends on, on each side of the move.
#[tokio::test]
async fn a_transformers_status_move_is_what_the_job_reads() {
    let router = b2bi_router();
    given_object(&router, "inbound/status.edi", X12_850).await;
    let transformer_id = given_transformer_with(&router, "movable", inbound(None)).await;

    // Each row: the status the record rests in, and whether a job may then run. The first
    // row is the status a create leaves it in, the second the one move a caller can drive.
    for (status, job_runs) in [("inactive", false), ("active", true)] {
        if status == "active" {
            let updated = set_status(&router, &transformer_id, status).await;
            assert_eq!(
                updated["status"],
                json!(status),
                "moving to {status:?} must be reported, got {updated}"
            );
        }
        let (code, read_back) = call(
            &router,
            "GetTransformer",
            json!({ "transformerId": transformer_id.clone() }),
        )
        .await;
        assert_eq!(code, 200, "the read-back must succeed, got {code} {read_back}");
        assert_eq!(
            read_back["status"],
            json!(status),
            "the stored transformer must report {status:?}, got {read_back}"
        );

        let (code, response) = call(
            &router,
            "StartTransformerJob",
            start_request(&transformer_id, "inbound/status.edi"),
        )
        .await;
        let expected = if job_runs { 200 } else { 400 };
        assert_eq!(
            code, expected,
            "with the transformer {status:?} a job must answer {expected}, got {code} \
             {response}"
        );
    }
}
