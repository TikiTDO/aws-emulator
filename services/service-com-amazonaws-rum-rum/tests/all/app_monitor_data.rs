// SPDX-License-Identifier: CC0-1.0

//! `GetAppMonitorData`: the events one app monitor collected, read back through the wire.
//!
//! The rule under test: the operation reports the events the data plane persisted on the
//! monitor the route names — each as the RUM event document the service enriched it into,
//! oldest first — narrowed to the events whose instant falls inside the requested time
//! range and whose attributes match one of the request's filters, and paged at the
//! request's `MaxResults`. Each table below differs along what that rule generalises over:
//! the window's bounds (including the published default for an omitted `Before`), the
//! filter key and its values, the number of values and of filters, and the page size.

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::support::{call, create, emulator, message};

/// The monitor every case queries.
const MONITOR: &str = "data-monitor";

/// `2024-01-01T00:00:00Z` in seconds since the epoch: the instant the first fixture event
/// carries, with the rest at one-minute steps after it.
const BASE: i64 = 1_704_067_200;

/// One fixture event: what `PutRumEvents` sends, and the attributes the filters select it
/// by.
struct Fixture {
    /// Which event this is, 1-based: also the last hex digit of its `id`.
    index: usize,
    /// Its instant, as seconds after [`BASE`] — the unit `RumEvent$timestamp` travels in.
    offset: i64,
    /// `RumEvent$type`.
    event_type: &'static str,
    /// `RumEvent$metadata`, the client JSON a RUM web client reports the session's
    /// identity in. `None` for the one event that carries none.
    metadata: Option<Value>,
    /// `RumEvent$details`.
    details: Value,
}

impl Fixture {
    /// This event's `id`: 36 characters matching `RumEvent$id`'s pattern
    /// `^[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}$`.
    fn id(&self) -> String {
        format!("1111111{}-1234-4234-8234-123456789012", self.index)
    }

    /// The instant the reported document carries for this event: milliseconds since the
    /// epoch, the unit a RUM event document's `event_timestamp` is in.
    fn instant_millis(&self) -> i64 {
        millis(BASE + self.offset)
    }

    /// This event as `PutRumEvents` carries it: `metadata` and `details` are
    /// `@mediaType("application/json")` strings, so the document travels serialized.
    fn wire(&self) -> Value {
        let mut event = json!({
            "id": self.id(),
            "timestamp": BASE + self.offset,
            "type": self.event_type,
            "details": self.details.to_string(),
        });
        if let Some(metadata) = &self.metadata {
            event["metadata"] = Value::String(metadata.to_string());
        }
        event
    }
}

/// The events every case in this suite queries: six events spanning the filterable
/// dimensions (browser, country, device, OS, page, event type), one minute apart.
///
/// The attribute names the dimensions are carried under are the ones a RUM web client
/// reports in `RumEvent$metadata` ("a JSON serialization of the identity of the user for
/// this session … from information such as the HTTP user-agent request header and
/// document interface"); the fifth event deliberately carries its page in `details`
/// instead, and the sixth carries a NUMERIC page, since the model constrains neither
/// document.
fn fixtures() -> Vec<Fixture> {
    vec![
        Fixture {
            index: 1,
            offset: 0,
            event_type: "com.amazon.rum.page_view_event",
            metadata: Some(json!({
                "browserName": "Chrome",
                "countryCode": "US",
                "deviceType": "desktop",
                "osName": "Windows",
                "pageId": "/home",
                "version": "1.0.0",
            })),
            details: json!({"pageTitle": "Home"}),
        },
        Fixture {
            index: 2,
            offset: 60,
            event_type: "com.amazon.rum.js_error_event",
            metadata: Some(json!({
                "browserName": "Firefox",
                "countryCode": "DE",
                "deviceType": "desktop",
                "osName": "macOS",
                "pageId": "/about",
            })),
            details: json!({"message": "boom"}),
        },
        Fixture {
            index: 3,
            offset: 120,
            event_type: "com.amazon.rum.http_event",
            metadata: Some(json!({
                "browserName": "Safari",
                "countryCode": "JP",
                "deviceType": "mobile",
                "osName": "iOS",
                "pageId": "/checkout",
            })),
            details: json!({"request": {"method": "GET"}}),
        },
        Fixture {
            index: 4,
            offset: 180,
            event_type: "com.amazon.rum.page_view_event",
            metadata: Some(json!({
                "browserName": "Chrome",
                "countryCode": "US",
                "deviceType": "mobile",
                "osName": "Android",
                "pageId": "/home",
            })),
            details: json!({"pageTitle": "Home"}),
        },
        Fixture {
            index: 5,
            offset: 240,
            event_type: "com.amazon.rum.performance_navigation_event",
            metadata: None,
            details: json!({"pageId": "/pricing", "duration": 12}),
        },
        Fixture {
            index: 6,
            offset: 300,
            event_type: "com.amazon.rum.session_start_event",
            metadata: Some(json!({
                "browserName": "Edge",
                "countryCode": "GB",
                "deviceType": "desktop",
                "osName": "Windows",
                "pageId": 42,
            })),
            details: json!({"sessionStart": true}),
        },
    ]
}

/// The `id` of the fixture event at `index`.
fn id_of(index: usize) -> String {
    format!("1111111{index}-1234-4234-8234-123456789012")
}

/// One monitor with every fixture event delivered to it, ready to query.
async fn seeded() -> MultiServiceRouter {
    let (router, _engine) = emulator();
    let monitor_id = create(
        &router,
        "seed",
        json!({"Name": MONITOR, "Domain": "data.example.com"}),
    )
    .await;
    let reply = call(
        &router,
        "POST",
        &format!("/appmonitors/{monitor_id}/"),
        Some(json!({
            "BatchId": "11111111-2222-4333-8444-555555555555",
            "AppMonitorDetails": {"name": MONITOR, "id": monitor_id, "version": "1.0.0"},
            "UserDetails": {
                "userId": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
                "sessionId": "ffffffff-eeee-4ddd-8ccc-bbbbbbbbbbbb",
            },
            "RumEvents": fixtures().iter().map(Fixture::wire).collect::<Vec<_>>(),
        })),
    )
    .await;
    assert_eq!(
        reply.status,
        200,
        "case \"seed\": expected PutRumEvents to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    router
}

/// Query `MONITOR` with `body`.
async fn query(router: &MultiServiceRouter, body: Value) -> crate::support::Reply {
    call(
        router,
        "POST",
        &format!("/appmonitor/{MONITOR}/data"),
        Some(body),
    )
    .await
}

/// The `Events` of a successful query, each parsed from the JSON document the reported
/// string carries.
async fn events(router: &MultiServiceRouter, case: &str, body: Value) -> (Vec<Value>, Option<String>) {
    let reply = query(router, body).await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected GetAppMonitorData to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let reported = reply.json(case);
    let documents = reported["Events"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected an Events array, got {reported}"))
        .iter()
        .map(|entry| {
            let text = entry.as_str().unwrap_or_else(|| {
                panic!("case {case:?}: expected each event to be a JSON string, got {entry}")
            });
            serde_json::from_str::<Value>(text).unwrap_or_else(|error| {
                panic!("case {case:?}: expected each event to carry a JSON document, got {text:?} ({error})")
            })
        })
        .collect();
    let next_token = reported["NextToken"].as_str().map(str::to_string);
    (documents, next_token)
}

/// The `event_id` of each reported event, in the order they were reported.
fn reported_ids(documents: &[Value]) -> Vec<String> {
    documents
        .iter()
        .map(|document| document["event_id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// How many milliseconds one second is: the fixture's instants are the epoch SECONDS
/// restJson1 carries a `Timestamp` as, while `TimeRange`'s bounds — and a reported
/// document's `event_timestamp` — are epoch MILLISECONDS.
const MILLIS_PER_SECOND: i64 = 1_000;

/// The whole second `seconds` as the milliseconds a window bound is expressed in.
fn millis(seconds: i64) -> i64 {
    seconds * MILLIS_PER_SECOND
}

/// A query body for the window `[after, before]` — both named as the whole second a
/// fixture event sits at, converted to the milliseconds `TimeRange` carries — with
/// `filters` and `max_results`.
fn body(after: i64, before: Option<i64>, filters: Value, max_results: i64) -> Value {
    let mut time_range = json!({"After": millis(after)});
    if let Some(before) = before {
        time_range["Before"] = json!(millis(before));
    }
    json!({"TimeRange": time_range, "Filters": filters, "MaxResults": max_results})
}

#[tokio::test]
async fn reported_events_are_the_enriched_documents_of_what_the_data_plane_accepted() {
    let router = seeded().await;

    let (documents, next_token) = events(
        &router,
        "every event",
        body(0, None, json!([]), 0),
    )
    .await;

    assert_eq!(
        reported_ids(&documents),
        (1..=6).map(id_of).collect::<Vec<_>>(),
        "case \"every event\": expected every stored event, oldest first, got {documents:?}"
    );
    assert!(
        next_token.is_none(),
        "case \"every event\": a page holding everything carries no NextToken, got {next_token:?}"
    );

    for fixture in fixtures() {
        let case = format!("event {}", fixture.index);
        let document = &documents[fixture.index - 1];
        assert_eq!(
            document["event_type"].as_str(),
            Some(fixture.event_type),
            "case {case:?}: expected the type the batch carried, got {document}"
        );
        assert_eq!(
            document["event_timestamp"].as_i64(),
            Some(fixture.instant_millis()),
            "case {case:?}: expected the instant the batch carried, in epoch milliseconds, \
             got {document}"
        );
        assert_eq!(
            document["event_details"], fixture.details,
            "case {case:?}: expected the details the batch carried, got {document}"
        );
        match &fixture.metadata {
            Some(metadata) => assert_eq!(
                &document["metadata"], metadata,
                "case {case:?}: expected the metadata the batch carried, got {document}"
            ),
            // Metadata is part of every RUM event document; an event that carried none has
            // nothing in it rather than no field at all.
            None => assert_eq!(
                document["metadata"],
                json!({}),
                "case {case:?}: an event that carried no metadata reports an empty one, got \
                 {document}"
            ),
        }
    }
}

#[tokio::test]
async fn the_time_range_selects_the_events_inside_it() {
    let router = seeded().await;

    // Each row differs along the window's bounds: both published bounds of the fixture
    // span, each inclusive edge, a window of one instant, windows outside the span on
    // either side, an inverted window, and an omitted `Before` — which `TimeRange$Before`
    // documents as extending the range "to the time that this operation is performed".
    let table: Vec<(&str, i64, Option<i64>, Vec<usize>)> = vec![
        ("the whole epoch", 0, None, vec![1, 2, 3, 4, 5, 6]),
        ("from the first instant", BASE, None, vec![1, 2, 3, 4, 5, 6]),
        ("after the first event", BASE + 1, None, vec![2, 3, 4, 5, 6]),
        ("from the second event", BASE + 60, None, vec![2, 3, 4, 5, 6]),
        ("up to the third event", 0, Some(BASE + 120), vec![1, 2, 3]),
        ("up to one second before it", 0, Some(BASE + 119), vec![1, 2]),
        ("one instant", BASE + 120, Some(BASE + 120), vec![3]),
        ("the middle of the span", BASE + 60, Some(BASE + 180), vec![2, 3, 4]),
        ("the last event only", BASE + 300, None, vec![6]),
        ("entirely before the span", 0, Some(BASE - 1), vec![]),
        ("entirely after the span", BASE + 301, None, vec![]),
        ("end before start", BASE + 120, Some(BASE + 60), vec![]),
    ];

    for (case, after, before, expected) in table {
        let (documents, _) = events(&router, case, body(after, before, json!([]), 0)).await;
        assert_eq!(
            reported_ids(&documents),
            expected.iter().copied().map(id_of).collect::<Vec<_>>(),
            "case {case:?}: expected the events inside the window, got {:?}",
            reported_ids(&documents)
        );
    }
}

#[tokio::test]
async fn a_filter_selects_the_events_matching_its_key_and_values() {
    let router = seeded().await;

    // Each row differs along the filter key, the number of values, and where the matched
    // attribute lives. The keys are the ones the model enumerates (`QueryFilter$Name`:
    // "Valid values for Name are Browser | Device | Country | Page | OS | EventType |
    // Invert"), each exercised on an attribute the issue did not cite.
    let table: Vec<(&str, Value, Vec<usize>)> = vec![
        ("one browser", json!([{"Name": "Browser", "Values": ["Chrome"]}]), vec![1, 4]),
        (
            "two browsers",
            json!([{"Name": "Browser", "Values": ["Chrome", "Edge"]}]),
            vec![1, 4, 6],
        ),
        ("one country", json!([{"Name": "Country", "Values": ["DE"]}]), vec![2]),
        ("one device", json!([{"Name": "Device", "Values": ["mobile"]}]), vec![3, 4]),
        ("one os", json!([{"Name": "OS", "Values": ["Windows"]}]), vec![1, 6]),
        ("a page from metadata", json!([{"Name": "Page", "Values": ["/home"]}]), vec![1, 4]),
        (
            "a page from details",
            json!([{"Name": "Page", "Values": ["/pricing"]}]),
            vec![5],
        ),
        ("a numeric page", json!([{"Name": "Page", "Values": ["42"]}]), vec![6]),
        (
            "one event type",
            json!([{"Name": "EventType", "Values": ["com.amazon.rum.page_view_event"]}]),
            vec![1, 4],
        ),
        ("a value nothing carries", json!([{"Name": "Browser", "Values": ["Opera"]}]), vec![]),
        (
            "a value only another dimension carries",
            json!([{"Name": "Country", "Values": ["Chrome"]}]),
            vec![],
        ),
        (
            "an inverted browser",
            json!([{"Name": "Invert", "Values": ["Chrome"]}]),
            vec![2, 3, 5, 6],
        ),
        (
            "an inverted os and country",
            json!([{"Name": "Invert", "Values": ["Windows", "JP"]}]),
            vec![2, 4, 5],
        ),
        (
            "an inverted event type",
            json!([{"Name": "Invert", "Values": ["com.amazon.rum.page_view_event"]}]),
            vec![2, 3, 5, 6],
        ),
        ("no filters at all", json!([]), vec![1, 2, 3, 4, 5, 6]),
        (
            "a filter with no values",
            json!([{"Name": "Browser", "Values": []}]),
            vec![1, 2, 3, 4, 5, 6],
        ),
        (
            "a filter with no key",
            json!([{"Values": ["Chrome"]}]),
            vec![1, 2, 3, 4, 5, 6],
        ),
        // The filters narrow what the window already selected, rather than replacing it.
        (
            "a browser inside half the window",
            json!([{"Name": "Browser", "Values": ["Chrome"]}]),
            vec![1, 4],
        ),
    ];

    for (case, filters, expected) in table {
        let (documents, _) = events(&router, case, body(0, None, filters, 0)).await;
        assert_eq!(
            reported_ids(&documents),
            expected.iter().copied().map(id_of).collect::<Vec<_>>(),
            "case {case:?}: expected the events matching the filter, got {:?}",
            reported_ids(&documents)
        );
    }

    // Window and filters together: the Chrome events of the first half of the span.
    let (documents, _) = events(
        &router,
        "a browser inside the first half",
        body(
            0,
            Some(BASE + 120),
            json!([{"Name": "Browser", "Values": ["Chrome"]}]),
            0,
        ),
    )
    .await;
    assert_eq!(
        reported_ids(&documents),
        vec![id_of(1)],
        "case \"a browser inside the first half\": expected only the in-window match, got {:?}",
        reported_ids(&documents)
    );
}

#[tokio::test]
async fn an_event_matching_one_of_the_filters_is_reported() {
    let router = seeded().await;

    // The rule: an event is reported when it matches ONE of the request's `QueryFilter`
    // structures (`QueryFilter`: "The only performance events that are returned are those
    // that have values matching the ones that you specify in one of your QueryFilter
    // structures"; `GetAppMonitorDataRequest$Filters`: "filter the results to those that
    // match one or more sets of key-value pairs that you specify"). Each row differs along
    // what that generalises over: how many filters the request carries, whether their keys
    // are the same or different dimensions, whether their matches overlap, how many values
    // each carries, the order they arrive in, whether one of them is the negating `Invert`
    // key, and whether one of them selects on nothing at all.
    let table: Vec<(&str, Value, Vec<usize>)> = vec![
        (
            "two filters over disjoint dimensions",
            json!([
                {"Name": "Browser", "Values": ["Chrome"]},
                {"Name": "Country", "Values": ["DE"]},
            ]),
            vec![1, 2, 4],
        ),
        (
            "the same two filters in the other order",
            json!([
                {"Name": "Country", "Values": ["DE"]},
                {"Name": "Browser", "Values": ["Chrome"]},
            ]),
            vec![1, 2, 4],
        ),
        (
            "two filters over one dimension",
            json!([
                {"Name": "Browser", "Values": ["Chrome"]},
                {"Name": "Browser", "Values": ["Edge"]},
            ]),
            vec![1, 4, 6],
        ),
        (
            "two filters with one event in common",
            json!([
                {"Name": "Browser", "Values": ["Chrome"]},
                {"Name": "Device", "Values": ["mobile"]},
            ]),
            vec![1, 3, 4],
        ),
        (
            "two filters selecting the same events",
            json!([
                {"Name": "Browser", "Values": ["Chrome"]},
                {"Name": "Page", "Values": ["/home"]},
            ]),
            vec![1, 4],
        ),
        (
            "three filters over three dimensions",
            json!([
                {"Name": "Browser", "Values": ["Safari"]},
                {"Name": "Country", "Values": ["GB"]},
                {"Name": "Page", "Values": ["/pricing"]},
            ]),
            vec![3, 5, 6],
        ),
        (
            "four filters over four dimensions",
            json!([
                {"Name": "OS", "Values": ["macOS"]},
                {"Name": "Device", "Values": ["mobile"]},
                {"Name": "EventType", "Values": ["com.amazon.rum.session_start_event"]},
                {"Name": "Country", "Values": ["US"]},
            ]),
            vec![1, 2, 3, 4, 6],
        ),
        (
            "two filters between them covering every event carrying that dimension",
            json!([
                {"Name": "Device", "Values": ["desktop"]},
                {"Name": "Device", "Values": ["mobile"]},
            ]),
            vec![1, 2, 3, 4, 6],
        ),
        (
            "several values across several filters",
            json!([
                {"Name": "Browser", "Values": ["Chrome", "Firefox"]},
                {"Name": "EventType", "Values": ["com.amazon.rum.http_event"]},
            ]),
            vec![1, 2, 3, 4],
        ),
        (
            "one filter matching nothing beside one that matches",
            json!([
                {"Name": "Browser", "Values": ["Opera"]},
                {"Name": "Device", "Values": ["mobile"]},
            ]),
            vec![3, 4],
        ),
        (
            "every filter matching nothing",
            json!([
                {"Name": "Browser", "Values": ["Opera"]},
                {"Name": "Country", "Values": ["FR"]},
            ]),
            vec![],
        ),
        (
            "an inverted value beside a filter",
            json!([
                {"Name": "Device", "Values": ["desktop"]},
                {"Name": "Invert", "Values": ["Windows"]},
            ]),
            vec![1, 2, 3, 4, 5, 6],
        ),
        (
            "two inverted filters",
            json!([
                {"Name": "Invert", "Values": ["Chrome"]},
                {"Name": "Invert", "Values": ["Windows"]},
            ]),
            vec![2, 3, 4, 5, 6],
        ),
        // A filter that names no key, or no values, selects nothing out: it neither
        // excludes an event nor admits one of its own.
        (
            "a selective filter beside one naming no values",
            json!([
                {"Name": "Browser", "Values": ["Chrome"]},
                {"Name": "Country", "Values": []},
            ]),
            vec![1, 4],
        ),
        (
            "a selective filter beside one naming no key",
            json!([
                {"Values": ["DE"]},
                {"Name": "Browser", "Values": ["Chrome"]},
            ]),
            vec![1, 4],
        ),
    ];

    for (case, filters, expected) in table {
        let (documents, _) = events(&router, case, body(0, None, filters, 0)).await;
        assert_eq!(
            reported_ids(&documents),
            expected.iter().copied().map(id_of).collect::<Vec<_>>(),
            "case {case:?}: expected the events matching one of the filters, got {:?}",
            reported_ids(&documents)
        );
    }

    // The window still narrows what the filters between them selected.
    let (documents, _) = events(
        &router,
        "a union inside the first half of the span",
        body(
            0,
            Some(BASE + 120),
            json!([
                {"Name": "Browser", "Values": ["Chrome"]},
                {"Name": "Country", "Values": ["DE"]},
            ]),
            0,
        ),
    )
    .await;
    assert_eq!(
        reported_ids(&documents),
        vec![id_of(1), id_of(2)],
        "case \"a union inside the first half of the span\": expected only the in-window \
         matches, got {:?}",
        reported_ids(&documents)
    );

    // Paging walks what the filters between them selected, once each, in order.
    let mut collected = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = body(
            0,
            None,
            json!([
                {"Name": "Browser", "Values": ["Chrome"]},
                {"Name": "Country", "Values": ["DE"]},
            ]),
            2,
        );
        if let Some(token) = &token {
            request["NextToken"] = json!(token);
        }
        let (documents, next) = events(&router, "a union paged at two", request).await;
        collected.extend(reported_ids(&documents));
        match next {
            Some(next) => token = Some(next),
            None => break,
        }
    }
    assert_eq!(
        collected,
        vec![id_of(1), id_of(2), id_of(4)],
        "case \"a union paged at two\": expected paging to walk every matching event once, \
         got {collected:?}"
    );
}

#[tokio::test]
async fn max_results_pages_the_matching_events() {
    let router = seeded().await;

    // Each row differs along the requested page size: every size from one event to one
    // more than the whole set, the modelled default (`MaxQueryResults` is
    // `@range(min: 0, max: 100)` with `@default(0)`) and the modelled maximum.
    for page_size in [1_i64, 2, 3, 4, 5, 6, 7, 100] {
        let case = format!("pages of {page_size}");
        let mut collected = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = body(0, None, json!([]), page_size);
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let (documents, next) = events(&router, &case, request).await;
            assert!(
                documents.len() <= page_size as usize,
                "case {case:?}: expected at most {page_size} events on a page, got {}",
                documents.len()
            );
            collected.extend(reported_ids(&documents));
            pages += 1;
            assert!(
                pages <= 7,
                "case {case:?}: paging did not terminate after {pages} pages"
            );
            match next {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        assert_eq!(
            collected,
            (1..=6).map(id_of).collect::<Vec<_>>(),
            "case {case:?}: expected paging to walk every event once, in order, got {collected:?}"
        );
    }

    // The modelled default, 0, names no page size, so it pages at the modelled maximum:
    // one page holding everything and no token.
    let (documents, next_token) = events(&router, "the default page size", body(0, None, json!([]), 0)).await;
    assert_eq!(
        documents.len(),
        6,
        "case \"the default page size\": expected every event on one page, got {}",
        documents.len()
    );
    assert!(
        next_token.is_none(),
        "case \"the default page size\": expected no NextToken, got {next_token:?}"
    );

    // Paging applies to what the filters selected, not to the whole buffer.
    let mut collected = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = body(0, None, json!([{"Name": "Browser", "Values": ["Chrome"]}]), 1);
        if let Some(token) = &token {
            request["NextToken"] = json!(token);
        }
        let (documents, next) = events(&router, "filtered pages of 1", request).await;
        collected.extend(reported_ids(&documents));
        match next {
            Some(next) => token = Some(next),
            None => break,
        }
    }
    assert_eq!(
        collected,
        vec![id_of(1), id_of(4)],
        "case \"filtered pages of 1\": expected paging to walk the matching events once, \
         got {collected:?}"
    );
}

#[tokio::test]
async fn a_page_size_outside_the_modelled_range_is_refused() {
    let router = seeded().await;

    // `MaxQueryResults` is `@range(min: 0, max: 100)`: each published bound is accepted
    // and one past it is refused.
    for (case, max_results, expected_status) in [
        ("the minimum", 0_i64, 200_u16),
        ("the maximum", 100, 200),
        ("one past the maximum", 101, 400),
        ("one below the minimum", -1, 400),
    ] {
        let reply = query(&router, body(0, None, json!([]), max_results)).await;
        assert_eq!(
            reply.status, expected_status,
            "case {case:?}: expected MaxResults {max_results} to answer {expected_status}, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
    }
}

#[tokio::test]
async fn an_unreadable_query_is_refused_and_a_missing_monitor_is_not_found() {
    let router = seeded().await;

    // A monitor nothing created has no data to report.
    let reply = call(
        &router,
        "POST",
        "/appmonitor/no-such-monitor/data",
        Some(body(0, None, json!([]), 0)),
    )
    .await;
    assert_eq!(
        reply.status,
        404,
        "case \"unknown monitor\": expected 404, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    assert_eq!(
        reply.error_code("unknown monitor"),
        "ResourceNotFoundException",
        "case \"unknown monitor\": expected the modelled not-found error, got {:?}",
        String::from_utf8_lossy(&reply.body)
    );

    // A token this operation never issued names no resume point, and neither does one
    // issued for another monitor's query.
    let (_, token) = events(&router, "first page", body(0, None, json!([]), 1)).await;
    let issued = token.expect("a token while events remain");
    let other = create(
        &router,
        "other monitor",
        json!({"Name": "other-data-monitor", "Domain": "other.example.com"}),
    )
    .await;
    assert!(!other.is_empty(), "case \"other monitor\": expected an Id");
    let foreign = call(
        &router,
        "POST",
        "/appmonitor/other-data-monitor/data",
        Some({
            let mut request = body(0, None, json!([]), 1);
            request["NextToken"] = json!(issued);
            request
        }),
    )
    .await;
    assert_eq!(
        foreign.status,
        400,
        "case \"a token from another monitor\": expected 400, got {} {:?}",
        foreign.status,
        String::from_utf8_lossy(&foreign.body)
    );

    for (case, token) in [
        ("not a token", "this-is-not-a-token"),
        ("hex that is not a token", "6162636465"),
        ("empty", ""),
    ] {
        let mut request = body(0, None, json!([]), 1);
        request["NextToken"] = json!(token);
        let reply = query(&router, request).await;
        assert_eq!(
            reply.status,
            400,
            "case {case:?}: expected a token this service never issued to be refused, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.error_code(case),
            "ValidationException",
            "case {case:?}: expected the modelled validation error, got {:?}",
            String::from_utf8_lossy(&reply.body)
        );
    }

    // A filter key outside the ones the model enumerates is refused, and the refusal
    // names the key the request sent.
    for (case, key) in [
        ("a lowercase key", "browser"),
        ("an attribute name", "browserName"),
        ("a key of another API", "Url"),
    ] {
        let reply = query(
            &router,
            body(0, None, json!([{"Name": key, "Values": ["Chrome"]}]), 0),
        )
        .await;
        assert_eq!(
            reply.status,
            400,
            "case {case:?}: expected an unknown filter key to be refused, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.error_code(case),
            "ValidationException",
            "case {case:?}: expected the modelled validation error, got {:?}",
            String::from_utf8_lossy(&reply.body)
        );
        let reported = reply.json(case);
        let text = message(&reported).unwrap_or_default();
        assert!(
            text.contains(key),
            "case {case:?}: expected the refusal to name the key the request sent, got {text:?}"
        );
    }
}

#[tokio::test]
async fn reading_the_data_changes_nothing_a_later_caller_sees() {
    let router = seeded().await;

    let first = events(&router, "first read", body(0, None, json!([]), 0)).await;
    let paged = events(&router, "a paged read", body(0, None, json!([]), 2)).await;
    let filtered = events(
        &router,
        "a filtered read",
        body(0, None, json!([{"Name": "Device", "Values": ["mobile"]}]), 0),
    )
    .await;
    let second = events(&router, "second read", body(0, None, json!([]), 0)).await;

    assert_eq!(
        reported_ids(&second.0),
        reported_ids(&first.0),
        "case \"second read\": reading the data must not change what a later caller sees, \
         got {:?} after {:?} and {:?}",
        reported_ids(&second.0),
        reported_ids(&paged.0),
        reported_ids(&filtered.0)
    );
}
