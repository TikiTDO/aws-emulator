// SPDX-License-Identifier: CC0-1.0

//! The spelling `GetInstrumentationConfigurationStatus` (POST
//! `/get-instrumentation-configuration-status`) states a status with.
//!
//! The rule under test: a response states an `InstrumentationConfigurationStatus` in this
//! service's WIRE spelling for that enum — the modelled name capitalised, `Active` for the
//! status whose modelled name is `ACTIVE` — while a request names the status by the modelled
//! name, which is what selects the chronology the read reports. The two are different
//! tokens, so a client comparing the member against the value the service sends matches and a
//! poller waiting for the active state observes it.
//!
//! The rule is asserted over a TABLE whose rows differ along what it generalises over: which
//! of the four modelled statuses the request names (plus the request that names none and
//! takes the default), the instrumentation type the configuration is stored under, and the
//! form of the `LocationIdentifier` the read resolves the configuration by.
//!
//! The harness is the one `instrumentation_configuration_status` drives this surface with, so
//! every suite speaks to one set of helpers.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::instrumentation_configuration_status::{
    create_configuration, create_request, events_of, get_status, nearest_minute, now_minute,
    report, report_entry, router, status_request, status_wire_token, Query, BREAKPOINT, PROBE,
    STATUSES,
};

/// The configuration every row of this suite reports for.
const SERVICE: &str = "checkout";
const ENVIRONMENT: &str = "eks:cluster-prod/checkout";
const FILE_PATH: &str = "src/checkout/PricingService.java";

/// Whether the read names the configuration by its pre-computed hash or by the code location
/// itself: `LocationIdentifier` is "either full code location or a pre-computed hash"
/// (`smithy_query(query="shape",shape="LocationIdentifier")`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum By {
    Hash,
    CodeLocation,
}

/// One row: the status a report carries and a read asks for, and how the read names the
/// configuration.
struct Row {
    label: &'static str,
    kind: &'static str,
    /// The modelled status name the report carries.
    reported: &'static str,
    /// The `InstrumentationErrorCause` the report carries, for the error status.
    cause: Option<&'static str>,
    /// The status the READ names, or `None` to leave the member out and take the default.
    queried: Option<&'static str>,
    /// The modelled status name the response is about, whether the read named it or defaulted.
    answered: &'static str,
    by: By,
}

/// The table: every modelled status named explicitly, the request that names none, both
/// instrumentation types and both identifier forms.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "the readiness status, by hash",
            kind: PROBE,
            reported: STATUSES[0],
            cause: None,
            queried: Some(STATUSES[0]),
            answered: STATUSES[0],
            by: By::Hash,
        },
        Row {
            label: "the error status with a cause, by hash",
            kind: PROBE,
            reported: STATUSES[1],
            // `InstrumentationStatusEvent.ErrorCause` is "the error cause when the status is
            // ERROR" (`smithy_query(query="shape",shape="InstrumentationStatusEvent")`).
            cause: Some("METHOD_NOT_FOUND"),
            queried: Some(STATUSES[1]),
            answered: STATUSES[1],
            by: By::Hash,
        },
        Row {
            label: "the active status, by hash",
            kind: PROBE,
            reported: STATUSES[2],
            cause: None,
            queried: Some(STATUSES[2]),
            answered: STATUSES[2],
            by: By::Hash,
        },
        Row {
            label: "the disabled status on a breakpoint configuration, by hash",
            kind: BREAKPOINT,
            reported: STATUSES[3],
            cause: None,
            queried: Some(STATUSES[3]),
            answered: STATUSES[3],
            by: By::Hash,
        },
        Row {
            label: "no status named, so the default one is answered for",
            kind: PROBE,
            reported: STATUSES[2],
            cause: None,
            queried: None,
            // `Status` is "the status that was queried. If not specified in the request, this
            // is ACTIVE".
            answered: STATUSES[2],
            by: By::Hash,
        },
        Row {
            label: "the active status, by the code location itself",
            kind: BREAKPOINT,
            reported: STATUSES[2],
            cause: None,
            queried: Some(STATUSES[2]),
            answered: STATUSES[2],
            by: By::CodeLocation,
        },
    ]
}

/// Report one status event for the configuration `hash` names, failing the case loudly if the
/// report was not processed.
async fn report_one(
    router: &MultiServiceRouter,
    label: &str,
    kind: &str,
    hash: &str,
    status: &str,
    cause: Option<&str>,
    instant: f64,
) {
    let (code, body) = report(
        router,
        &json!({
            "Service": SERVICE,
            "Environment": ENVIRONMENT,
            "Configurations": [report_entry(kind, hash, status, instant, cause)],
        }),
    )
    .await;
    assert_eq!(
        code, 200,
        "case {label:?}: expected HTTP 200 from the report, got {code} with {body}"
    );
    assert_eq!(
        body["UnprocessedStatusEvents"],
        json!([]),
        "case {label:?}: expected the report to be processed, got {body}"
    );
}

/// A read states the status it answers for in the wire spelling, whichever status it is and
/// however the request named the configuration — while the events it selected are the ones
/// reported under the MODELLED name the request carried.
#[tokio::test]
async fn a_status_read_states_the_status_in_its_wire_spelling() {
    for row in rows() {
        let router = router();
        let request = create_request(SERVICE, ENVIRONMENT, row.kind, FILE_PATH);
        let hash = create_configuration(&router, &request).await;

        let instant = now_minute();
        report_one(
            &router,
            row.label,
            row.kind,
            &hash,
            row.reported,
            row.cause,
            instant,
        )
        .await;

        let identifier = match row.by {
            By::Hash => json!({ "LocationHash": hash.clone() }),
            By::CodeLocation => json!({ "CodeLocation": request["Location"]["CodeLocation"] }),
        };
        let (code, body) = get_status(
            &router,
            &status_request(
                SERVICE,
                ENVIRONMENT,
                row.kind,
                identifier,
                &Query {
                    status: row.queried,
                    ..Query::default()
                },
            ),
        )
        .await;
        assert_eq!(
            code, 200,
            "case {:?}: expected HTTP 200 from the read, got {code} with {body}",
            row.label
        );

        let expected = status_wire_token(row.answered);
        assert_eq!(
            body["Status"].as_str(),
            Some(expected),
            "case {:?}: expected the status stated as {expected:?}, got {:?}",
            row.label,
            body["Status"]
        );
        // The wire token is NOT the modelled name the request spells the status with, which is
        // what makes the two tokens worth telling apart at all.
        assert_ne!(
            body["Status"].as_str(),
            Some(row.answered),
            "case {:?}: expected the wire spelling rather than the modelled name, got {:?}",
            row.label,
            body["Status"]
        );

        // The chronology the read reported is the one the request's own modelled name selects,
        // so the rendering above changed which token is STATED and nothing else.
        assert_eq!(
            events_of(&body),
            vec![(
                nearest_minute(instant),
                row.cause.map(ToString::to_string)
            )],
            "case {:?}: expected the event reported under {:?}, got {:?}",
            row.label,
            row.answered,
            body["Events"]
        );
    }
}
