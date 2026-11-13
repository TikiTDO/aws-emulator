// SPDX-License-Identifier: CC0-1.0

//! `BatchGetRumMetricDefinitions`, end to end: what it REPORTS and what it REFUSES.
//!
//! The rule under test: the read answers from this service's persisted state for the ONE
//! `(AppMonitorName, Destination, DestinationArn)` target the request names — "Retrieves
//! the list of metrics and dimensions that a RUM app monitor is sending to a single
//! destination" — so a target the store does not hold is not found (an app monitor by
//! name, or a destination of one), a `Destination`/`DestinationArn` pair the model does
//! not pair is invalid ("This parameter is required if Destination is Evidently. If
//! Destination is CloudWatch, do not use this parameter"), a `NextToken` this listing
//! never issued for that target is invalid, and a destination that exists while holding no
//! definition reports an empty list rather than a refusal. It is `@readonly`, so reading
//! it — one page, every page, or a page abandoned half way — changes nothing a later
//! caller sees.
//!
//! The other suites of this crate cover what the read reports for definitions a create or
//! an update stored, its pagination over a grid of store and page sizes, and its
//! `MaxResults` bounds; these cases cover the refusals of the read itself and the
//! invariant that reading mutates nothing.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so routing, the `@httpLabel` app-monitor name, the `@httpQuery`
//! bindings (`destination`, `destinationArn`, `maxResults`, `nextToken`), the handler and
//! the response envelope are exercised rather than just a handler body.
//!
//! The tables span what the rules generalise over: each half of the target that can be
//! missing, both `MetricDestination` values, both forbidden pairings, a `Destination`
//! outside the enum, an absent versus an empty `DestinationArn`, several classes of
//! app-monitor name the pattern admits, and one token per way a value can fail to be a
//! token this listing issued.
//!
//! The target helpers (the query string, `PutRumMetricsDestination`, and the paginated
//! read followed to the end) are the shared ones in `tests/all/support.rs`, so the metrics
//! suites address a destination one way.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::support::{
    all_metric_definitions, call, create, emulator, experiment_arn, get_app_monitor,
    message, metric_definition_id, metric_definitions_page, metrics_destination_query,
    percent_encoded, put_metrics_destination, reported_metric_definitions,
};

/// Every value `MetricDestination` admits.
/// Source: `smithy_query(query="shape", shape="MetricDestination")`.
const METRIC_DESTINATIONS: [&str; 2] = ["CloudWatch", "Evidently"];

/// The wire error codes this read refuses with, and the status each carries.
/// Source: `smithy_query(query="operation", operation="BatchGetRumMetricDefinitions")` —
/// `ValidationException` is `@httpError(400)` and `ResourceNotFoundException` is
/// `@httpError(404)`.
const VALIDATION_CODE: &str = "ValidationException";
const VALIDATION_STATUS: u16 = 400;
const NOT_FOUND_CODE: &str = "ResourceNotFoundException";
const NOT_FOUND_STATUS: u16 = 404;

/// The literal every app-monitor not-found message of this service starts with, ahead of
/// the name the request asked about.
const NOT_FOUND_PREFIX: &str = "Resource not found ";

/// `AppMonitorName`'s modelled length bounds: 1..255.
/// Source: `smithy_query(query="shape", shape="AppMonitorName")`.
const APP_MONITOR_NAME_LENGTH: std::ops::RangeInclusive<usize> = 1..=255;

/// One custom metric definition: a name the extended catalog does not list, published to a
/// namespace of its own, which is what the model requires of a custom metric ("The
/// namespace that you use can't start with AWS/").
fn definition(index: usize) -> Value {
    json!({"Name": format!("CheckoutsCompleted{index}"), "Namespace": "batch-get-reads"})
}

/// Create one app monitor to hang destinations and definitions off.
async fn create_monitor(router: &MultiServiceRouter, case: &str, name: &str) {
    create(router, case, json!({"Name": name, "Domain": "example.com"})).await;
}

/// Store `count` definitions under one destination with `BatchCreateRumMetricDefinitions`,
/// returning the ids it minted.
async fn seed_definitions(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    destination: &str,
    destination_arn: Option<&str>,
    count: usize,
) -> Vec<String> {
    if count == 0 {
        return Vec::new();
    }
    let mut body = json!({"Destination": destination});
    if let Some(arn) = destination_arn {
        body["DestinationArn"] = json!(arn);
    }
    body["MetricDefinitions"] = Value::Array((0..count).map(definition).collect());
    let reply = call(
        router,
        "POST",
        &format!("/rummetrics/{monitor}/metrics"),
        Some(body),
    )
    .await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected BatchCreateRumMetricDefinitions to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let answered = reply.json(case);
    assert_eq!(
        answered["Errors"],
        json!([]),
        "case {case:?}: the seed batch was refused in part: {answered}"
    );
    assert_eq!(
        answered["MetricDefinitions"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        0,
        "case {case:?}: expected the create to report none of what it stored, got {answered}"
    );
    // The ids the seed minted are read back through the operation under test, matched to the
    // definitions the seed named by the unique `Name` each carries.
    let held = all_metric_definitions(router, case, monitor, destination, destination_arn, None)
        .await
        .into_values()
        .collect::<Vec<Value>>();
    (0..count)
        .map(|index| {
            let wanted = definition(index);
            held.iter()
                .find(|reported| reported["Name"] == wanted["Name"])
                .map(|reported| metric_definition_id(case, reported))
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no definition was stored for {wanted}: {held:?}")
                })
        })
        .collect()
}

/// The continuation token a first page of one definition carries, which there is one of
/// whenever the destination holds more than one definition.
async fn issued_token(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    destination: &str,
    destination_arn: Option<&str>,
) -> String {
    let query = format!(
        "{}&maxResults=1",
        metrics_destination_query(destination, destination_arn)
    );
    let reply = metric_definitions_page(router, monitor, &query).await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected the first page to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let page = reply.json(case);
    page["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a token while definitions are left, got {page}"))
        .to_string()
}

/// What one row of the pairing table expects of the answer.
#[derive(Debug)]
enum Expect {
    /// The pair is one the model pairs, so the read answers a page.
    Page,
    /// The pair is one the model forbids, so the read refuses it with this whole message —
    /// the prose rule's own wording, which the handler owns.
    Refused(String),
    /// The value is outside the member's own modelled constraint, so it is refused ahead of
    /// the handler: the row pins the status, the wire code and that the message names the
    /// member, whose wording the boundary owns.
    RefusedByConstraint,
}

#[tokio::test]
async fn only_the_destination_pairs_the_model_pairs_are_read() {
    let (router, _engine) = emulator();
    let monitor = "pair-rules";
    let experiment = experiment_arn("pair-rules");
    create_monitor(&router, "pair rules", monitor).await;
    // Both destinations exist, so a refusal is about the PAIR rather than about a
    // destination that is not there.
    put_metrics_destination(&router, "pair rules", monitor, METRIC_DESTINATIONS[0], None).await;
    put_metrics_destination(
        &router,
        "pair rules",
        monitor,
        METRIC_DESTINATIONS[1],
        Some(&experiment),
    )
    .await;

    // (case, Destination, DestinationArn as the query carries it, what the answer must be)
    //
    // The refused rows pin the whole message, which is this service's ONE wording for the
    // documented pairing rule: every operation of it that accepts the pair refuses a
    // forbidden one with the same sentence, so a row that drifted from it would also be
    // asserting a different answer from the create, the update and the delete.
    let rows: Vec<(&str, &str, Option<&str>, Expect)> = vec![
        (
            "CloudWatch with no ARN",
            METRIC_DESTINATIONS[0],
            None,
            Expect::Page,
        ),
        (
            "CloudWatch with an experiment ARN",
            METRIC_DESTINATIONS[0],
            Some(experiment.as_str()),
            Expect::Refused(format!(
                "DestinationArn cannot be used when Destination is {}.",
                METRIC_DESTINATIONS[0]
            )),
        ),
        // An empty value is inside `DestinationArn`'s modelled `length 0..2048` and outside
        // its modelled `@pattern arn:[^:]*:[^:]*:[^:]*:[^:]*:.*`, so it is not an ARN at
        // all: the member's own constraint refuses it whichever destination names it.
        (
            "CloudWatch with an empty ARN",
            METRIC_DESTINATIONS[0],
            Some(""),
            Expect::RefusedByConstraint,
        ),
        (
            "Evidently with its experiment ARN",
            METRIC_DESTINATIONS[1],
            Some(experiment.as_str()),
            Expect::Page,
        ),
        (
            "Evidently with no ARN",
            METRIC_DESTINATIONS[1],
            None,
            Expect::Refused(format!(
                "DestinationArn is required when Destination is {}.",
                METRIC_DESTINATIONS[1]
            )),
        ),
        (
            "Evidently with an empty ARN",
            METRIC_DESTINATIONS[1],
            Some(""),
            Expect::RefusedByConstraint,
        ),
    ];

    for (case, destination, destination_arn, expected) in rows {
        let reply = metric_definitions_page(
            &router,
            monitor,
            &metrics_destination_query(destination, destination_arn),
        )
        .await;
        match expected {
            Expect::Page => {
                assert_eq!(
                    reply.status,
                    200,
                    "case {case:?}: expected a pair the model pairs to be read, got {} {:?}",
                    reply.status,
                    String::from_utf8_lossy(&reply.body)
                );
                let page = reply.json(case);
                assert!(
                    reported_metric_definitions(&page).is_empty(),
                    "case {case:?}: a destination holding no definition reported {page}"
                );
                assert_eq!(
                    page["NextToken"].as_str(),
                    None,
                    "case {case:?}: a listing with nothing left carried a token: {page}"
                );
            }
            Expect::Refused(expected) => {
                assert_eq!(
                    reply.status,
                    VALIDATION_STATUS,
                    "case {case:?}: expected HTTP {VALIDATION_STATUS}, got {} {:?}",
                    reply.status,
                    String::from_utf8_lossy(&reply.body)
                );
                let code = reply.error_code(case);
                assert_eq!(
                    code, VALIDATION_CODE,
                    "case {case:?}: expected {VALIDATION_CODE}, got {code:?}"
                );
                let document = reply.json(case);
                assert_eq!(
                    message(&document),
                    Some(expected.as_str()),
                    "case {case:?}: expected the message {expected:?}, got {document}"
                );
            }
            Expect::RefusedByConstraint => {
                assert_eq!(
                    reply.status,
                    VALIDATION_STATUS,
                    "case {case:?}: expected HTTP {VALIDATION_STATUS}, got {} {:?}",
                    reply.status,
                    String::from_utf8_lossy(&reply.body)
                );
                let code = reply.error_code(case);
                assert_eq!(
                    code, VALIDATION_CODE,
                    "case {case:?}: expected {VALIDATION_CODE}, got {code:?}"
                );
                let document = reply.json(case);
                let reported = message(&document).unwrap_or_else(|| {
                    panic!("case {case:?}: the refusal carries no message: {document}")
                });
                assert!(
                    reported.contains("destinationArn"),
                    "case {case:?}: expected the refusal to name the member it is about, got \
                     {reported:?}"
                );
            }
        }
    }

    // A `Destination` the enum does not list names no kind of destination at all, whatever
    // else the request carries: the casing of a listed value is not a listed value.
    for case in ["Cloudwatch", "EVIDENTLY", "Firehose", ""] {
        let reply = metric_definitions_page(&router, monitor, &metrics_destination_query(case, None)).await;
        assert_eq!(
            reply.status,
            VALIDATION_STATUS,
            "case {case:?}: expected a destination outside the enum to be refused, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let code = reply.error_code(case);
        assert_eq!(
            code, VALIDATION_CODE,
            "case {case:?}: expected {VALIDATION_CODE}, got {code:?}"
        );
    }
}

#[tokio::test]
async fn a_target_the_store_does_not_hold_is_not_found_and_an_empty_one_is_a_page() {
    let (router, _engine) = emulator();
    // One monitor with one destination holding definitions exists throughout, so a 404
    // means "that target is not stored" rather than "this emulator holds nothing".
    let present = "stored-target";
    create_monitor(&router, "stored target", present).await;
    put_metrics_destination(
        &router,
        "stored target",
        present,
        METRIC_DESTINATIONS[0],
        None,
    )
    .await;
    let seeded = seed_definitions(
        &router,
        "stored target",
        present,
        METRIC_DESTINATIONS[0],
        None,
        3,
    )
    .await;

    // Names no app monitor is stored under, spanning both published bounds of
    // `AppMonitorName`'s length and the character classes its pattern admits — including
    // `#`, which cannot travel unencoded in a URI path, so the message has to report the
    // DECODED name.
    let absent: Vec<(&str, String, String)> = vec![
        (
            "the shortest admissible name",
            "a".to_string(),
            "a".to_string(),
        ),
        (
            "the longest admissible name",
            "x".repeat(*APP_MONITOR_NAME_LENGTH.end()),
            "x".repeat(*APP_MONITOR_NAME_LENGTH.end()),
        ),
        (
            "dots, dashes, underscores and digits",
            "absent.app-monitor_0123".to_string(),
            "absent.app-monitor_0123".to_string(),
        ),
        (
            "a hash that travels percent-encoded",
            "absent#monitor".to_string(),
            "absent%23monitor".to_string(),
        ),
    ];
    for (case, name, encoded) in absent {
        assert!(
            APP_MONITOR_NAME_LENGTH.contains(&name.len()),
            "case {case:?}: the row's name is outside the modelled length: {} characters",
            name.len()
        );
        let reply = metric_definitions_page(
            &router,
            &encoded,
            &metrics_destination_query(METRIC_DESTINATIONS[0], None),
        )
        .await;
        assert_eq!(
            reply.status,
            NOT_FOUND_STATUS,
            "case {case:?}: expected HTTP {NOT_FOUND_STATUS}, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let code = reply.error_code(case);
        assert_eq!(
            code, NOT_FOUND_CODE,
            "case {case:?}: expected {NOT_FOUND_CODE}, got {code:?}"
        );
        let document = reply.json(case);
        let expected = format!("{NOT_FOUND_PREFIX}{name}");
        assert_eq!(
            message(&document),
            Some(expected.as_str()),
            "case {case:?}: expected the message {expected:?}, got {document}"
        );
    }

    // A monitor that exists while the destination the request names does not: the pair
    // addresses no resource, on a monitor that has another destination and on one that has
    // none at all.
    let destination_less = "no-destinations";
    create_monitor(&router, "no destinations", destination_less).await;
    let evidently = experiment_arn("unheld");
    let rows: Vec<(&str, &str, &str, Option<&str>)> = vec![
        (
            "a monitor holding another destination",
            present,
            METRIC_DESTINATIONS[1],
            Some(evidently.as_str()),
        ),
        (
            "a monitor holding no destination",
            destination_less,
            METRIC_DESTINATIONS[0],
            None,
        ),
        (
            "a monitor holding no destination, asked for an experiment",
            destination_less,
            METRIC_DESTINATIONS[1],
            Some(evidently.as_str()),
        ),
    ];
    for (case, monitor, destination, destination_arn) in rows {
        let reply = metric_definitions_page(
            &router,
            monitor,
            &metrics_destination_query(destination, destination_arn),
        )
        .await;
        assert_eq!(
            reply.status,
            NOT_FOUND_STATUS,
            "case {case:?}: expected HTTP {NOT_FOUND_STATUS}, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let code = reply.error_code(case);
        assert_eq!(
            code, NOT_FOUND_CODE,
            "case {case:?}: expected {NOT_FOUND_CODE}, got {code:?}"
        );
        // RECORDED: the refusal names the app monitor the request addressed and refers to
        // the `(Destination, DestinationArn)` pair as "the specified parameters", so neither
        // the destination kind nor the experiment ARN is echoed into it.
        let document = reply.json(case);
        let reported = message(&document)
            .unwrap_or_else(|| panic!("case {case:?}: the refusal carries no message: {document}"));
        let expected = format!(
            "No destination for Rum metric definitions exists on app monitor {monitor}, for \
             the specified parameters. Please try again."
        );
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the message {expected:?}, got {reported:?}"
        );
        assert!(
            !reported.contains("arn:"),
            "case {case:?}: the message echoed an ARN: {reported:?}"
        );
    }

    // The contrast: a destination that EXISTS and holds no definition is a page, not a
    // refusal — on both destination kinds.
    let empty = "empty-destinations";
    create_monitor(&router, "empty destinations", empty).await;
    let empty_experiment = experiment_arn("empty");
    put_metrics_destination(
        &router,
        "empty destinations",
        empty,
        METRIC_DESTINATIONS[0],
        None,
    )
    .await;
    put_metrics_destination(
        &router,
        "empty destinations",
        empty,
        METRIC_DESTINATIONS[1],
        Some(&empty_experiment),
    )
    .await;
    for (case, destination, destination_arn) in [
        ("an empty CloudWatch destination", METRIC_DESTINATIONS[0], None),
        (
            "an empty Evidently destination",
            METRIC_DESTINATIONS[1],
            Some(empty_experiment.as_str()),
        ),
    ] {
        let reply = metric_definitions_page(
            &router,
            empty,
            &metrics_destination_query(destination, destination_arn),
        )
        .await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected HTTP 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let page = reply.json(case);
        assert!(
            reported_metric_definitions(&page).is_empty(),
            "case {case:?}: expected no definition, got {page}"
        );
        assert_eq!(
            page["NextToken"].as_str(),
            None,
            "case {case:?}: an empty listing carried a token: {page}"
        );
    }

    // And the seeded target still reports exactly what was stored under it, after every
    // refusal above.
    let stored = all_metric_definitions(
        &router,
        "stored target",
        present,
        METRIC_DESTINATIONS[0],
        None,
        None,
    )
    .await;
    assert_eq!(
        stored.keys().cloned().collect::<Vec<String>>(),
        {
            let mut ids = seeded.clone();
            ids.sort();
            ids
        },
        "case {:?}: the refusals changed what the stored target reports",
        "stored target"
    );
}

#[tokio::test]
async fn a_next_token_this_listing_did_not_issue_is_refused() {
    let (router, _engine) = emulator();
    let monitor = "token-rules";
    let other = "token-rules-other";
    let case = "token rules";
    for name in [monitor, other] {
        create_monitor(&router, case, name).await;
        put_metrics_destination(&router, case, name, METRIC_DESTINATIONS[0], None).await;
        seed_definitions(&router, case, name, METRIC_DESTINATIONS[0], None, 3).await;
    }
    let seeded: Vec<String> =
        all_metric_definitions(&router, case, monitor, METRIC_DESTINATIONS[0], None, None)
            .await
            .into_keys()
            .collect();
    assert_eq!(
        seeded.len(),
        3,
        "case {case:?}: expected the seed to store 3 definitions, got {seeded:?}"
    );

    // One row per way a value can fail to be a token THIS listing issued for THIS target.
    // The forged ones are built with the `hex` codec the service's token codec uses, so
    // they are well-formed everywhere except in the part each row varies.
    let foreign = issued_token(&router, case, other, METRIC_DESTINATIONS[0], None).await;
    let forged: Vec<(&str, String)> = vec![
        ("not hexadecimal at all", "not-a-token".to_string()),
        (
            "hexadecimal naming no operation's marker",
            hex::encode("whatever/1/CheckoutsCompleted0"),
        ),
        (
            "another listing's marker",
            hex::encode("ListAppMonitors/1/token-rules"),
        ),
        (
            "this listing's marker with no resume key",
            hex::encode("BatchGetRumMetricDefinitions/1/"),
        ),
        ("a token issued for another app monitor", foreign),
    ];
    for (row, token) in forged {
        let query = format!(
            "{}&maxResults=1&nextToken={}",
            metrics_destination_query(METRIC_DESTINATIONS[0], None),
            percent_encoded(&token)
        );
        let reply = metric_definitions_page(&router, monitor, &query).await;
        assert_eq!(
            reply.status,
            VALIDATION_STATUS,
            "case {row:?}: expected HTTP {VALIDATION_STATUS}, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let code = reply.error_code(row);
        assert_eq!(
            code, VALIDATION_CODE,
            "case {row:?}: expected {VALIDATION_CODE}, got {code:?}"
        );
        // The refusal is built from the request: it names the value the caller sent and the
        // monitor it sent it to.
        let document = reply.json(row);
        let reported = message(&document)
            .unwrap_or_else(|| panic!("case {row:?}: the refusal carries no message: {document}"));
        assert!(
            reported.contains(&token) && reported.contains(monitor),
            "case {row:?}: expected the message to name {token:?} and {monitor:?}, got {reported:?}"
        );
    }

    // A token this listing DID issue resumes strictly after the definition the page that
    // issued it ended on, so following it covers the rest exactly once.
    let first_query = format!(
        "{}&maxResults=1",
        metrics_destination_query(METRIC_DESTINATIONS[0], None)
    );
    let first = metric_definitions_page(&router, monitor, &first_query).await;
    assert_eq!(
        first.status,
        200,
        "case {case:?}: expected the first page to answer 200, got {} {:?}",
        first.status,
        String::from_utf8_lossy(&first.body)
    );
    let first_page = first.json(case);
    let on_first: Vec<String> = reported_metric_definitions(&first_page)
        .iter()
        .map(|reported| metric_definition_id(case, reported))
        .collect();
    let token = first_page["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a token, got {first_page}"));

    let mut after: Vec<String> = Vec::new();
    let mut resume = Some(token.to_string());
    while let Some(issued) = resume {
        let query = format!(
            "{}&maxResults=1&nextToken={}",
            metrics_destination_query(METRIC_DESTINATIONS[0], None),
            percent_encoded(&issued)
        );
        let reply = metric_definitions_page(&router, monitor, &query).await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected a resumed page to answer 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let page = reply.json(case);
        after.extend(
            reported_metric_definitions(&page)
                .iter()
                .map(|reported| metric_definition_id(case, reported)),
        );
        resume = page["NextToken"].as_str().map(str::to_string);
    }

    let mut covered: Vec<String> = on_first.clone();
    covered.extend(after.iter().cloned());
    covered.sort();
    assert_eq!(
        covered, seeded,
        "case {case:?}: following the issued token covered {covered:?} of {seeded:?}"
    );
    assert!(
        after.iter().all(|id| !on_first.contains(id)),
        "case {case:?}: a resumed page repeated a definition the first page reported: \
         {after:?} after {on_first:?}"
    );
}

#[tokio::test]
async fn reading_and_paging_change_nothing_a_later_caller_sees() {
    let (router, _engine) = emulator();
    let monitor = "readonly";
    let case = "readonly";
    let experiment = experiment_arn("readonly");
    create_monitor(&router, case, monitor).await;
    put_metrics_destination(&router, case, monitor, METRIC_DESTINATIONS[0], None).await;
    put_metrics_destination(
        &router,
        case,
        monitor,
        METRIC_DESTINATIONS[1],
        Some(&experiment),
    )
    .await;
    seed_definitions(&router, case, monitor, METRIC_DESTINATIONS[0], None, 4).await;
    seed_definitions(
        &router,
        case,
        monitor,
        METRIC_DESTINATIONS[1],
        Some(&experiment),
        2,
    )
    .await;

    // The two targets, as (case, Destination, DestinationArn).
    let targets: Vec<(&str, &str, Option<&str>)> = vec![
        ("the CloudWatch destination", METRIC_DESTINATIONS[0], None),
        (
            "the Evidently destination",
            METRIC_DESTINATIONS[1],
            Some(experiment.as_str()),
        ),
    ];

    let monitor_before = get_app_monitor(&router, case, monitor).await;
    let mut before: Vec<BTreeMap<String, Value>> = Vec::new();
    for (row, destination, destination_arn) in &targets {
        before.push(
            all_metric_definitions(&router, row, monitor, destination, *destination_arn, None).await,
        );
    }

    // Each way of reading the same state: one whole page, every page of each size, and a
    // first page whose token is never followed (a client that stops half way).
    for (row, destination, destination_arn) in &targets {
        for page_size in [None, Some(1), Some(2), Some(100)] {
            let seen = all_metric_definitions(&router, row, monitor, destination, *destination_arn, page_size)
                .await;
            let expected = &before[targets
                .iter()
                .position(|(name, _, _)| name == row)
                .expect("each target is one of the rows")];
            assert_eq!(
                &seen, expected,
                "case {row:?}: paging at {page_size:?} reported {seen:?}, not the stored \
                 {expected:?}"
            );
        }
        let abandoned = format!(
            "{}&maxResults=1",
            metrics_destination_query(destination, *destination_arn)
        );
        let reply = metric_definitions_page(&router, monitor, &abandoned).await;
        assert_eq!(
            reply.status,
            200,
            "case {row:?}: expected an abandoned first page to answer 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
    }

    // Refused reads touch nothing either: a pair the model does not pair, a destination the
    // monitor does not hold, and a token this listing never issued.
    let refusals: Vec<(&str, String)> = vec![
        (
            "a forbidden pairing",
            metrics_destination_query(METRIC_DESTINATIONS[0], Some(experiment.as_str())),
        ),
        (
            "a destination the monitor does not hold",
            metrics_destination_query(METRIC_DESTINATIONS[1], Some(&experiment_arn("absent"))),
        ),
        (
            "a token this listing never issued",
            format!(
                "{}&nextToken={}",
                metrics_destination_query(METRIC_DESTINATIONS[0], None),
                percent_encoded(&hex::encode("ListAppMonitors/1/readonly"))
            ),
        ),
    ];
    for (row, query) in refusals {
        let reply = metric_definitions_page(&router, monitor, &query).await;
        assert!(
            (400..500).contains(&reply.status),
            "case {row:?}: expected a client refusal, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
    }

    for (index, (row, destination, destination_arn)) in targets.iter().enumerate() {
        let after =
            all_metric_definitions(&router, row, monitor, destination, *destination_arn, None).await;
        assert_eq!(
            after, before[index],
            "case {row:?}: reading changed what the destination reports: {after:?}, was {:?}",
            before[index]
        );
    }
    let monitor_after = get_app_monitor(&router, case, monitor).await;
    assert_eq!(
        monitor_after, monitor_before,
        "case {case:?}: reading the metric definitions changed the app monitor: \
         {monitor_after}, was {monitor_before}"
    );
}
