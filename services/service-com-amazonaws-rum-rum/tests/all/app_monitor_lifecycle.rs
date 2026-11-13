// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the CloudWatch RUM app-monitor lifecycle and the
//! `PutRumEvents` data-plane call.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so routing, deserialization, the modelled constraints, the
//! handler and the response envelope are all exercised, not just the handler body.
//!
//! The assertions pin RULES over a SET of inputs rather than one request: the
//! create/read-back table spans the whole `AppMonitorPlatform` catalog, both
//! `CustomEventsStatus` values and both ways of naming a domain; the defaults table
//! walks the members the model documents a default for; the pagination test is an
//! invariant check over a grid of store sizes and page sizes; and the update table
//! changes one member per row and asserts every other member held still.

use std::collections::{BTreeSet, HashSet};

use serde_json::{json, Value};
use service_com_amazonaws_rum_rum::state::CREATED_STATE;
use service_com_amazonaws_rum_rum::ComAmazonawsRumRumState;
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

// The ONE wire harness, shared with this crate's other suites: the signed-request
// helpers, the raw `Reply`, and the two calls every suite sets a resource up with.
use crate::support::{
    call, call_in, create, emulator, get_app_monitor, is_modelled_app_monitor_id,
    rum_events_body, tag_map, ACCOUNT, REGION,
};

/// Every value `AppMonitorPlatform` admits.
/// Source: `smithy_query(query="shape", shape="AppMonitorPlatform")`.
const PLATFORMS: [&str; 3] = ["Web", "Android", "iOS"];

/// Every value `CustomEventsStatus` admits.
/// Source: `smithy_query(query="shape", shape="CustomEventsStatus")`.
const CUSTOM_EVENTS_STATUSES: [&str; 2] = ["DISABLED", "ENABLED"];

/// Every value `DeobfuscationStatus` admits.
/// Source: `smithy_query(query="shape", shape="DeobfuscationStatus")`.
const DEOBFUSCATION_STATUSES: [&str; 2] = ["DISABLED", "ENABLED"];

/// Every value `Telemetry` admits.
/// Source: `smithy_query(query="shape", shape="Telemetry")`.
const TELEMETRIES: [&str; 3] = ["errors", "http", "performance"];

/// The prefix a RUM app monitor's CloudWatch Logs group name carries.
const CW_LOG_GROUP_PREFIX: &str = "/aws/vendedlogs/RUMService_";

/// `ISOTimestampString` is `@length(min: 19, max: 19)`: `YYYY-MM-DDTHH:MM:SS`.
/// Source: `smithy_query(query="shape", shape="ISOTimestampString")`.
fn is_modelled_iso_timestamp(text: &str) -> bool {
    text.len() == 19
        && text.chars().enumerate().all(|(index, c)| match index {
            4 | 7 => c == '-',
            10 => c == 'T',
            13 | 16 => c == ':',
            _ => c.is_ascii_digit(),
        })
}

/// A create request body spanning every member `CreateAppMonitor` accepts, so the
/// read-back table can vary one dimension at a time.
struct CreateCase {
    case: String,
    name: String,
    body: Value,
}

/// The create/read-back table: each row differs from the others along what the rule
/// generalises over — which platform, which custom-events status, whether the domain is
/// given as `Domain` or as a `DomainList` (and how many entries it has), whether
/// CloudWatch Logs copies are on, how many tags there are, and whether a deobfuscation
/// configuration is present.
fn create_cases() -> Vec<CreateCase> {
    let mut cases = Vec::new();
    for (index, platform) in PLATFORMS.iter().enumerate() {
        let custom_events = CUSTOM_EVENTS_STATUSES[index % CUSTOM_EVENTS_STATUSES.len()];
        let deobfuscation = DEOBFUSCATION_STATUSES[index % DEOBFUSCATION_STATUSES.len()];
        let name = format!("monitor-{platform}");
        let mut body = json!({
            "Name": name,
            "Domain": format!("{}.example.com", platform.to_lowercase()),
            "CwLogEnabled": index % 2 == 0,
            "Platform": platform,
            "CustomEvents": {"Status": custom_events},
            "DeobfuscationConfiguration": {
                "JavaScriptSourceMaps": {"Status": deobfuscation, "S3Uri": "s3://maps-bucket/app"}
            },
            "AppMonitorConfiguration": {
                "SessionSampleRate": 0.25,
                "AllowCookies": index % 2 == 1,
                "EnableXRay": index % 2 == 0,
                "Telemetries": TELEMETRIES[..=index.min(TELEMETRIES.len() - 1)].to_vec(),
                "FavoritePages": ["https://www.example.com/home"],
            },
            "Tags": {"Team": "web", "Env": format!("env-{index}")},
        });
        if index == 1 {
            // The other admitted form: a DomainList rather than a single Domain, and no
            // tags at all.
            let object = body.as_object_mut().expect("a JSON object");
            object.remove("Domain");
            object.insert(
                "DomainList".to_string(),
                json!(["a.example.com", "b.example.com", "c.example.com"]),
            );
            object.insert("Tags".to_string(), json!({}));
        }
        cases.push(CreateCase {
            case: format!("platform={platform}"),
            name,
            body,
        });
    }
    cases
}

#[tokio::test]
async fn get_app_monitor_rebuilds_every_member_from_the_created_state() {
    let (router, engine) = emulator();

    for CreateCase { case, name, body } in create_cases() {
        let case = case.as_str();
        let request = body.clone();
        let id = create(&router, case, body).await;
        let monitor = get_app_monitor(&router, case, &name).await;

        assert_eq!(
            monitor["Id"].as_str(),
            Some(id.as_str()),
            "case {case:?}: GetAppMonitor must report the Id CreateAppMonitor minted, got {:?}",
            monitor["Id"]
        );
        assert_eq!(
            monitor["Name"].as_str(),
            Some(name.as_str()),
            "case {case:?}: expected Name {name:?}, got {:?}",
            monitor["Name"]
        );
        assert_eq!(
            monitor["Domain"], request["Domain"],
            "case {case:?}: expected Domain {:?}, got {:?}",
            request["Domain"], monitor["Domain"]
        );
        assert_eq!(
            monitor["DomainList"], request["DomainList"],
            "case {case:?}: expected DomainList {:?}, got {:?}",
            request["DomainList"], monitor["DomainList"]
        );
        assert_eq!(
            monitor["Platform"], request["Platform"],
            "case {case:?}: expected Platform {:?}, got {:?}",
            request["Platform"], monitor["Platform"]
        );
        assert_eq!(
            monitor["State"].as_str(),
            Some(CREATED_STATE),
            "case {case:?}: expected State {CREATED_STATE}, got {:?}",
            monitor["State"]
        );
        assert_eq!(
            monitor["CustomEvents"], request["CustomEvents"],
            "case {case:?}: expected CustomEvents {:?}, got {:?}",
            request["CustomEvents"], monitor["CustomEvents"]
        );
        assert_eq!(
            monitor["DeobfuscationConfiguration"], request["DeobfuscationConfiguration"],
            "case {case:?}: expected DeobfuscationConfiguration {:?}, got {:?}",
            request["DeobfuscationConfiguration"], monitor["DeobfuscationConfiguration"]
        );
        assert_eq!(
            monitor["AppMonitorConfiguration"], request["AppMonitorConfiguration"],
            "case {case:?}: expected AppMonitorConfiguration {:?}, got {:?}",
            request["AppMonitorConfiguration"], monitor["AppMonitorConfiguration"]
        );
        assert_eq!(
            tag_map(&monitor),
            request["Tags"],
            "case {case:?}: expected Tags {:?}, got {:?}",
            request["Tags"],
            monitor["Tags"]
        );

        // `DataStorage.CwLog` is DERIVED from the stored record: a monitor that keeps
        // copies names the group RUM VENDED for it, read off the record, and one that does
        // not names none.
        let cw_log_enabled = request["CwLogEnabled"].as_bool().expect("a bool in the row");
        assert_eq!(
            monitor["DataStorage"]["CwLog"]["CwLogEnabled"].as_bool(),
            Some(cw_log_enabled),
            "case {case:?}: expected DataStorage.CwLog.CwLogEnabled {cw_log_enabled}, got {:?}",
            monitor["DataStorage"]["CwLog"]["CwLogEnabled"]
        );
        let vended = engine
            .state::<ComAmazonawsRumRumState>()
            .lock()
            .app_monitor(REGION, &name)
            .map(|record| record.vended_log_group.clone())
            .expect("the created monitor is stored");
        let expected_group = cw_log_enabled.then(|| vended.clone());
        assert_eq!(
            monitor["DataStorage"]["CwLog"]["CwLogGroup"].as_str(),
            expected_group.as_deref(),
            "case {case:?}: expected DataStorage.CwLog.CwLogGroup {expected_group:?}, got {:?}",
            monitor["DataStorage"]["CwLog"]["CwLogGroup"]
        );
        // And that vended name is not reconstructible from the request: it carries the
        // documented prefix and the monitor's name, then a suffix the service chose.
        let named = format!("{CW_LOG_GROUP_PREFIX}{name}");
        assert!(
            vended.starts_with(&named) && vended.len() > named.len(),
            "case {case:?}: expected the vended log group to extend {named:?} with a \
             service-chosen suffix, got {vended:?}"
        );

        let created = monitor["Created"].as_str().unwrap_or_default();
        assert!(
            is_modelled_iso_timestamp(created),
            "case {case:?}: expected Created to be a 19-character ISOTimestampString, got {created:?}"
        );
        // `LastModified` is "the date and time of the most recent changes to this app
        // monitor's configuration", and nothing has changed this one since the create, so
        // there is no such instant to report and the member must be ABSENT.
        assert_eq!(
            monitor["LastModified"],
            Value::Null,
            "case {case:?}: nothing changed the monitor since it was created, so it must \
             report no LastModified, got {:?}",
            monitor["LastModified"]
        );

        // The ARN no implemented operation reports yet is stored, derived from the
        // signed region and the monitor's name.
        let stored_arn = engine
            .state::<ComAmazonawsRumRumState>()
            .lock()
            .app_monitor(REGION, &name)
            .map(|record| record.arn.clone());
        assert_eq!(
            stored_arn.as_deref(),
            Some(format!("arn:aws:rum:{REGION}:{ACCOUNT}:appmonitor/{name}").as_str()),
            "case {case:?}: expected the stored ARN to name the signed region and the monitor, \
             got {stored_arn:?}"
        );
    }
}

#[tokio::test]
async fn an_arn_is_derived_from_the_region_the_request_was_signed_for() {
    // The rule generalises over the region: the ARN is built from the credential scope
    // of the request that created the monitor, not from a fixed one.
    for region in ["us-east-1", "eu-west-2", "ap-southeast-1"] {
        let (router, engine) = emulator();
        let name = format!("regional-{region}");
        let reply = call_in(
            &router,
            region,
            "POST",
            "/appmonitor",
            Some(json!({"Name": name, "Domain": "example.com"})),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {region:?}: expected CreateAppMonitor to answer 200, got {}",
            reply.status
        );
        let stored_arn = engine
            .state::<ComAmazonawsRumRumState>()
            .lock()
            .app_monitor(region, &name)
            .map(|record| record.arn.clone());
        assert_eq!(
            stored_arn.as_deref(),
            Some(format!("arn:aws:rum:{region}:{ACCOUNT}:appmonitor/{name}").as_str()),
            "case {region:?}: expected the ARN to carry the signed region, got {stored_arn:?}"
        );
    }
}

#[tokio::test]
async fn omitted_members_take_the_defaults_the_model_documents() {
    let (router, _engine) = emulator();
    // Only the two members a create needs: a name, and one of Domain/DomainList.
    let name = "defaults-monitor";
    create(
        &router,
        "defaults",
        json!({"Name": name, "Domain": "example.com"}),
    )
    .await;
    let monitor = get_app_monitor(&router, "defaults", name).await;

    // Each row: the member, the value the model documents for an omitted request, and
    // where that default is written down.
    // Source: `smithy_query(query="operation", operation="CreateAppMonitor")` member docs.
    let expected: [(&str, Value); 4] = [
        // "If you omit this parameter, the default is Web."
        ("Platform", json!("Web")),
        // "If you omit this parameter, the default is false."
        ("DataStorage.CwLog.CwLogEnabled", json!(false)),
        // "If you omit this parameter, custom events are DISABLED."
        ("CustomEvents.Status", json!("DISABLED")),
        // "If you omit this argument, the sample rate used for RUM is set to 10% of the
        // user sessions."
        ("AppMonitorConfiguration.SessionSampleRate", json!(0.1)),
    ];
    for (path, want) in expected {
        let mut actual = &monitor;
        for segment in path.split('.') {
            actual = &actual[segment];
        }
        assert_eq!(
            *actual, want,
            "case {path:?}: expected the documented default {want}, got {actual}"
        );
    }
    assert_eq!(
        monitor["DataStorage"]["CwLog"]["CwLogGroup"],
        Value::Null,
        "case \"CwLogGroup\": a monitor that keeps no CloudWatch Logs copy names no log \
         group, got {:?}",
        monitor["DataStorage"]["CwLog"]["CwLogGroup"]
    );
    assert_eq!(
        tag_map(&monitor),
        json!({}),
        "case \"Tags\": a monitor created with no tags reports an empty tag map, got {:?}",
        monitor["Tags"]
    );
}

#[tokio::test]
async fn a_second_monitor_cannot_take_a_name_already_in_use() {
    let (router, _engine) = emulator();
    // The rule holds whatever the rest of the request says, so the second create
    // differs from the first in every other member.
    let first = json!({"Name": "shared-name", "Domain": "first.example.com", "Platform": "Web"});
    let second = json!({
        "Name": "shared-name",
        "DomainList": ["second.example.com"],
        "Platform": "iOS",
        "CwLogEnabled": true,
    });
    let id = create(&router, "first create", first).await;

    let reply = call(&router, "POST", "/appmonitor", Some(second)).await;
    assert_eq!(
        reply.status, 409,
        "case \"duplicate name\": expected HTTP 409, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    assert_eq!(
        reply.error_code("duplicate name"),
        "ConflictException",
        "case \"duplicate name\": expected ConflictException, got {:?}",
        String::from_utf8_lossy(&reply.body)
    );

    // The refused create changed nothing: the stored monitor is still the first one.
    let monitor = get_app_monitor(&router, "after conflict", "shared-name").await;
    assert_eq!(
        monitor["Id"].as_str(),
        Some(id.as_str()),
        "case \"after conflict\": the first monitor must be untouched, got {:?}",
        monitor["Id"]
    );
    assert_eq!(
        monitor["Domain"].as_str(),
        Some("first.example.com"),
        "case \"after conflict\": the first monitor's Domain must be untouched, got {:?}",
        monitor["Domain"]
    );
}

#[tokio::test]
async fn a_request_naming_no_stored_monitor_is_not_found() {
    let (router, _engine) = emulator();
    create(
        &router,
        "seed",
        json!({"Name": "present", "Domain": "example.com"}),
    )
    .await;

    // The rule generalises over the operation: every CONTROL-PLANE call that resolves a
    // monitor by name answers a name no stored monitor holds the same way. The data
    // plane's `PutRumEvents` is not a row here — it is authorized against the monitor its
    // `Id` names, so an id resolving to nothing is an authorization failure rather than a
    // missing resource (`put_rum_events_unresolvable_id`).
    let rows: [(&str, &str, &str, Option<Value>); 3] = [
        ("GetAppMonitor", "GET", "/appmonitor/absent", None),
        (
            "UpdateAppMonitor",
            "PATCH",
            "/appmonitor/absent",
            Some(json!({"Domain": "other.example.com"})),
        ),
        ("DeleteAppMonitor", "DELETE", "/appmonitor/absent", None),
    ];
    for (case, method, path, body) in rows {
        let reply = call(&router, method, path, body).await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected HTTP 404, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.error_code(case),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            String::from_utf8_lossy(&reply.body)
        );
    }
}

#[tokio::test]
async fn put_rum_events_appends_to_the_monitor_its_id_names() {
    let (router, engine) = emulator();
    // Two monitors, so the test shows the batch lands on the one its id names rather
    // than on whichever monitor happens to be stored.
    let first = create(
        &router,
        "first",
        json!({"Name": "events-first", "Domain": "first.example.com"}),
    )
    .await;
    let second = create(
        &router,
        "second",
        json!({"Name": "events-second", "Domain": "second.example.com"}),
    )
    .await;

    // Row: how many batches are delivered to the first monitor. Each batch carries two
    // events, so the buffer grows by two per call — appending, never replacing.
    for batches in 1..=3_usize {
        let reply = call(
            &router,
            "POST",
            &format!("/appmonitors/{first}/"),
            Some(rum_events_body(&first)),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case \"batch {batches}\": expected PutRumEvents to answer 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert!(
            reply.body.is_empty()
                || serde_json::from_slice::<Value>(&reply.body).ok() == Some(json!({})),
            "case \"batch {batches}\": PutRumEventsResponse models no members, so the body \
             must carry none, got {:?}",
            String::from_utf8_lossy(&reply.body)
        );

        let state = engine.state::<ComAmazonawsRumRumState>();
        let store = state.lock();
        let stored = store
            .app_monitor(REGION, "events-first")
            .expect("the first monitor is stored");
        assert_eq!(
            stored.events.len(),
            batches * 2,
            "case \"batch {batches}\": expected {} stored events after {batches} batches, got {}",
            batches * 2,
            stored.events.len()
        );
        let newest = stored.events.back().expect("an appended event");
        let document: Value = serde_json::from_str(&newest.event_json)
            .unwrap_or_else(|e| panic!("case \"batch {batches}\": stored event is not JSON ({e})"));
        assert_eq!(
            document["batch_id"].as_str(),
            Some("11111111-2222-4333-8444-555555555555"),
            "case \"batch {batches}\": expected the stored event to keep its BatchId, got \
             {document}"
        );
        assert_eq!(
            document["application_version"].as_str(),
            Some("1.0.0"),
            "case \"batch {batches}\": expected the stored event to keep the client's \
             AppMonitorDetails version, got {document}"
        );
        assert_eq!(
            document["event_type"].as_str(),
            Some("com.amazon.rum.js_error_event"),
            "case \"batch {batches}\": expected the last event of the batch, got {document}"
        );
        assert_eq!(
            document["event_details"]["message"].as_str(),
            Some("boom"),
            "case \"batch {batches}\": expected the event's details to be stored, got {document}"
        );

        // The other monitor saw none of it.
        let untouched = store
            .app_monitor(REGION, "events-second")
            .expect("the second monitor is stored");
        assert!(
            untouched.events.is_empty(),
            "case \"batch {batches}\": events must land only on the monitor the id names, \
             the other holds {}",
            untouched.events.len()
        );
    }

    // The second monitor's own id delivers a batch too: every stored id resolves, so
    // the 404 the not-found table pins is about the id naming nothing, not about the
    // route or the body.
    let reply = call(
        &router,
        "POST",
        &format!("/appmonitors/{second}/"),
        Some(rum_events_body(&second)),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case \"second monitor\": expected PutRumEvents to answer 200, got {}",
        reply.status
    );
    {
        let state = engine.state::<ComAmazonawsRumRumState>();
        let store = state.lock();
        let stored = store
            .app_monitor(REGION, "events-second")
            .expect("the second monitor is stored");
        assert_eq!(
            stored.events.len(),
            2,
            "case \"second monitor\": expected the batch's two events on the second \
             monitor, got {}",
            stored.events.len()
        );
    }
}

#[tokio::test]
async fn update_app_monitor_changes_only_the_members_the_request_carries() {
    let (router, engine) = emulator();
    let name = "update-monitor";
    let id = create(
        &router,
        "seed",
        json!({
            "Name": name,
            "Domain": "before.example.com",
            "CwLogEnabled": false,
            "Platform": "Android",
            "CustomEvents": {"Status": "DISABLED"},
            "AppMonitorConfiguration": {"SessionSampleRate": 0.5},
            "Tags": {"Team": "web"},
        }),
    )
    .await;
    let before = get_app_monitor(&router, "seed", name).await;
    let created = before["Created"].as_str().unwrap_or_default().to_string();

    // `ISOTimestampString` has one-second resolution, so a change made within the same
    // second as the create would carry the same `LastModified`. Wait past the second
    // boundary once, so "bumped" is observable at the resolution the model admits.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;

    // A PATCH carrying no updatable member changes nothing — and because that wait has
    // already crossed a second boundary, a timestamp wrongly bumped on a no-op would
    // show up here.
    let reply = call(&router, "PATCH", &format!("/appmonitor/{name}"), Some(json!({}))).await;
    assert_eq!(
        reply.status, 200,
        "case \"no-op update\": expected UpdateAppMonitor to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let unchanged = get_app_monitor(&router, "no-op update", name).await;
    assert_eq!(
        unchanged, before,
        "case \"no-op update\": a PATCH with no updatable member must leave the monitor \
         exactly as it was, expected {before}, got {unchanged}"
    );

    // The group RUM vended for this monitor when it was created: what the update that
    // turns delivery on has to advertise, read off the record rather than rebuilt from the
    // monitor's name, whose service-chosen suffix is state.
    let vended_group = engine
        .state::<ComAmazonawsRumRumState>()
        .lock()
        .app_monitor(REGION, name)
        .map(|record| record.vended_log_group.clone())
        .expect("the seeded monitor is stored");

    // Each row changes ONE updatable member and names what the stored monitor must then
    // report. Source: `smithy_query(query="operation", operation="UpdateAppMonitor")` —
    // Domain, DomainList, AppMonitorConfiguration, CwLogEnabled, CustomEvents,
    // DeobfuscationConfiguration.
    let rows: [(&str, Value, &str, Value); 6] = [
        (
            "Domain",
            json!({"Domain": "after.example.com"}),
            "Domain",
            json!("after.example.com"),
        ),
        (
            "DomainList",
            json!({"DomainList": ["x.example.com", "y.example.com"]}),
            "DomainList",
            json!(["x.example.com", "y.example.com"]),
        ),
        (
            "CwLogEnabled",
            json!({"CwLogEnabled": true}),
            "DataStorage",
            json!({"CwLog": {"CwLogEnabled": true, "CwLogGroup": vended_group}}),
        ),
        (
            "CustomEvents",
            json!({"CustomEvents": {"Status": "ENABLED"}}),
            "CustomEvents",
            json!({"Status": "ENABLED"}),
        ),
        (
            "AppMonitorConfiguration",
            json!({"AppMonitorConfiguration": {"SessionSampleRate": 0.75, "AllowCookies": true}}),
            "AppMonitorConfiguration",
            json!({"SessionSampleRate": 0.75, "AllowCookies": true}),
        ),
        (
            "DeobfuscationConfiguration",
            json!({"DeobfuscationConfiguration": {"JavaScriptSourceMaps": {"Status": "ENABLED", "S3Uri": "s3://maps-bucket/app"}}}),
            "DeobfuscationConfiguration",
            json!({"JavaScriptSourceMaps": {"Status": "ENABLED", "S3Uri": "s3://maps-bucket/app"}}),
        ),
    ];

    for (case, patch, member, want) in rows {
        let reply = call(
            &router,
            "PATCH",
            &format!("/appmonitor/{name}"),
            Some(patch),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected UpdateAppMonitor to answer 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );

        let after = get_app_monitor(&router, case, name).await;
        assert_eq!(
            after[member], want,
            "case {case:?}: expected {member} {want}, got {}",
            after[member]
        );
        // The members this request did not carry are untouched, including the two that
        // are not updatable at all.
        assert_eq!(
            after["Id"].as_str(),
            Some(id.as_str()),
            "case {case:?}: Id must not change, got {:?}",
            after["Id"]
        );
        assert_eq!(
            after["Created"].as_str(),
            Some(created.as_str()),
            "case {case:?}: Created must not change, got {:?}",
            after["Created"]
        );
        assert_eq!(
            after["Platform"].as_str(),
            Some("Android"),
            "case {case:?}: Platform is not an UpdateAppMonitor member, so it must not \
             change, got {:?}",
            after["Platform"]
        );
        assert_eq!(
            tag_map(&after),
            json!({"Team": "web"}),
            "case {case:?}: Tags are not an UpdateAppMonitor member, so they must not \
             change, got {}",
            after["Tags"]
        );
        assert_eq!(
            after["State"].as_str(),
            Some(CREATED_STATE),
            "case {case:?}: State must stay {CREATED_STATE}, got {:?}",
            after["State"]
        );
        // The update DID change the configuration, so now there IS a "most recent
        // change" to report: the member appears, past `Created`.
        let last_modified = after["LastModified"].as_str().unwrap_or_default();
        assert!(
            is_modelled_iso_timestamp(last_modified),
            "case {case:?}: expected LastModified to be a 19-character ISOTimestampString, \
             got {last_modified:?}"
        );
        assert!(
            last_modified > created.as_str(),
            "case {case:?}: expected LastModified {last_modified:?} to be bumped past \
             Created {created:?}"
        );
    }
}

#[tokio::test]
async fn deleting_a_monitor_removes_it_from_every_index() {
    let (router, engine) = emulator();
    let name = "doomed-monitor";
    let id = create(
        &router,
        "seed",
        json!({"Name": name, "Domain": "example.com"}),
    )
    .await;
    let kept = create(
        &router,
        "kept",
        json!({"Name": "kept-monitor", "Domain": "kept.example.com"}),
    )
    .await;
    // Give the monitor an event buffer, so the delete has something to drop.
    let reply = call(
        &router,
        "POST",
        &format!("/appmonitors/{id}/"),
        Some(rum_events_body(&id)),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case \"seed events\": expected PutRumEvents to answer 200, got {}",
        reply.status
    );

    let reply = call(&router, "DELETE", &format!("/appmonitor/{name}"), None).await;
    assert_eq!(
        reply.status, 200,
        "case \"delete\": expected DeleteAppMonitor to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );

    // Gone from the name index: every control-plane call naming it is a 404.
    let rows: [(&str, &str, String, Option<Value>); 2] = [
        ("GetAppMonitor", "GET", format!("/appmonitor/{name}"), None),
        (
            "DeleteAppMonitor again",
            "DELETE",
            format!("/appmonitor/{name}"),
            None,
        ),
    ];
    for (case, method, path, body) in rows {
        let reply = call(&router, method, &path, body).await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected HTTP 404, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.error_code(case),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            String::from_utf8_lossy(&reply.body)
        );
    }

    // Gone from the id index too, and with it the stored events: the data plane refuses
    // the retired id, as an authorization failure rather than as a missing resource.
    let retired = call(
        &router,
        "POST",
        &format!("/appmonitors/{id}/"),
        Some(rum_events_body(&id)),
    )
    .await;
    assert_eq!(
        retired.status, 403,
        "case \"PutRumEvents with the old id\": expected HTTP 403, got {} {:?}",
        retired.status,
        String::from_utf8_lossy(&retired.body)
    );
    assert_eq!(
        retired.error_code("PutRumEvents with the old id"),
        "AccessDeniedException",
        "case \"PutRumEvents with the old id\": expected AccessDeniedException, got {:?}",
        String::from_utf8_lossy(&retired.body)
    );

    {
        let state = engine.state::<ComAmazonawsRumRumState>();
        let store = state.lock();
        assert!(
            !store.monitor_ids.contains_key(&id),
            "case \"id index\": the deleted monitor's id must leave the index"
        );
        assert!(
            store.monitor_ids.contains_key(&kept),
            "case \"id index\": the monitor that was not deleted keeps its id"
        );
        assert!(
            !store.contains_app_monitor(REGION, name),
            "case \"name index\": the deleted monitor must leave the store"
        );
    }

    // The monitor that was not deleted is untouched, and its name is free to be reused.
    let monitor = get_app_monitor(&router, "kept", "kept-monitor").await;
    assert_eq!(
        monitor["Id"].as_str(),
        Some(kept.as_str()),
        "case \"kept\": the other monitor must be untouched, got {:?}",
        monitor["Id"]
    );
    let reborn = create(
        &router,
        "recreate",
        json!({"Name": name, "Domain": "example.com"}),
    )
    .await;
    assert_ne!(
        reborn, id,
        "case \"recreate\": a monitor created after the delete gets a fresh Id, got {reborn}"
    );
}

#[tokio::test]
async fn updating_one_domain_form_replaces_the_other() {
    // A monitor's domain authority is held in exactly ONE of Domain/DomainList, so
    // naming one on an update leaves the other unset. The rule is symmetric, so both
    // directions are a row, and each row starts from the form the other row ends in.
    let rows: [(&str, Value, Value, Value); 2] = [
        (
            "DomainList replaces Domain",
            json!({"Name": "swap-a", "Domain": "before.example.com"}),
            json!({"DomainList": ["x.example.com", "y.example.com"]}),
            json!(["x.example.com", "y.example.com"]),
        ),
        (
            "Domain replaces DomainList",
            json!({"Name": "swap-b", "DomainList": ["before.example.com"]}),
            json!({"Domain": "after.example.com"}),
            json!("after.example.com"),
        ),
    ];
    for (case, create_body, patch, want) in rows {
        let (router, _engine) = emulator();
        let name = create_body["Name"].as_str().expect("a name in the row").to_string();
        create(&router, case, create_body).await;

        let reply = call(&router, "PATCH", &format!("/appmonitor/{name}"), Some(patch)).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected UpdateAppMonitor to answer 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );

        let monitor = get_app_monitor(&router, case, &name).await;
        let (set, cleared) = if want.is_array() {
            ("DomainList", "Domain")
        } else {
            ("Domain", "DomainList")
        };
        assert_eq!(
            monitor[set], want,
            "case {case:?}: expected {set} {want}, got {}",
            monitor[set]
        );
        assert_eq!(
            monitor[cleared],
            Value::Null,
            "case {case:?}: naming {set} must leave {cleared} unset, got {}",
            monitor[cleared]
        );
    }
}

/// Page through `ListAppMonitors` with `max_results`, returning every name in the order
/// the pages reported them. Asserts the per-page invariants on the way.
async fn page_through(
    router: &MultiServiceRouter,
    case: &str,
    max_results: Option<usize>,
    expected_total: usize,
) -> Vec<String> {
    let mut names = Vec::new();
    let mut tokens_seen = HashSet::new();
    let mut next_token: Option<String> = None;
    loop {
        let query = match (&next_token, max_results) {
            (Some(token), Some(max)) => format!("?maxResults={max}&nextToken={token}"),
            (Some(token), None) => format!("?nextToken={token}"),
            (None, Some(max)) => format!("?maxResults={max}"),
            (None, None) => String::new(),
        };
        let reply = call(router, "POST", &format!("/appmonitors{query}"), None).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected ListAppMonitors to answer 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let body = reply.json(case);
        let page = body["AppMonitorSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no AppMonitorSummaries in {body}"))
            .clone();
        if let Some(max) = max_results {
            assert!(
                page.len() <= max,
                "case {case:?}: a page must hold at most maxResults={max} summaries, got {}",
                page.len()
            );
        }
        for summary in &page {
            // Every summary member is read off the stored monitor.
            let name = summary["Name"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a summary with no Name: {summary}"))
                .to_string();
            assert!(
                is_modelled_app_monitor_id(summary["Id"].as_str().unwrap_or_default()),
                "case {case:?}: summary {name:?} must carry the stored AppMonitorId, got {:?}",
                summary["Id"]
            );
            assert!(
                is_modelled_iso_timestamp(summary["Created"].as_str().unwrap_or_default()),
                "case {case:?}: summary {name:?} must carry a 19-character Created, got {:?}",
                summary["Created"]
            );
            assert_eq!(
                summary["State"].as_str(),
                Some(CREATED_STATE),
                "case {case:?}: summary {name:?} must report the stored State, got {:?}",
                summary["State"]
            );
            // These monitors were created and never updated, so the summary reports no
            // LastModified either — the summary and the detail projection agree.
            assert_eq!(
                summary["LastModified"],
                Value::Null,
                "case {case:?}: summary {name:?} was never updated, so it must report no \
                 LastModified, got {:?}",
                summary["LastModified"]
            );
            assert_eq!(
                summary["Platform"].as_str(),
                Some("Web"),
                "case {case:?}: summary {name:?} must report the stored Platform, got {:?}",
                summary["Platform"]
            );
            names.push(name);
        }

        match body["NextToken"].as_str() {
            None => break,
            Some(token) => {
                assert!(
                    !page.is_empty(),
                    "case {case:?}: an empty page must not carry a NextToken"
                );
                assert!(
                    tokens_seen.insert(token.to_string()),
                    "case {case:?}: NextToken {token:?} was handed out twice, so paging does \
                     not advance"
                );
                assert!(
                    names.len() < expected_total,
                    "case {case:?}: a NextToken was returned after every one of the \
                     {expected_total} monitors had been reported"
                );
                next_token = Some(token.to_string());
            }
        }
    }
    names
}

#[tokio::test]
async fn list_app_monitors_pages_over_the_whole_store_without_repeating_or_dropping() {
    // The invariant: for any store size and any page size, paging to exhaustion reports
    // every stored monitor exactly once, in one stable order, and only the last page
    // omits the token.
    for store_size in [0_usize, 1, 3, 7] {
        let (router, _engine) = emulator();
        let mut expected = BTreeSet::new();
        for index in 0..store_size {
            // Names are deliberately not created in sorted order, so the ordering the
            // listing reports cannot be an accident of insertion order.
            let name = format!("monitor-{:02}", (store_size - index) * 7 % 100);
            create(
                &router,
                "seed",
                json!({"Name": name, "Domain": "example.com"}),
            )
            .await;
            expected.insert(name);
        }

        for max_results in [None, Some(1), Some(2), Some(3), Some(100)] {
            let case = format!("store={store_size} maxResults={max_results:?}");
            let names = page_through(&router, &case, max_results, expected.len()).await;
            assert_eq!(
                names.len(),
                expected.len(),
                "case {case:?}: expected {} summaries in total, got {}",
                expected.len(),
                names.len()
            );
            let unique: BTreeSet<String> = names.iter().cloned().collect();
            assert_eq!(
                unique,
                expected,
                "case {case:?}: expected the pages to report exactly the stored monitors \
                 {expected:?}, got {unique:?}"
            );
            let mut sorted = names.clone();
            sorted.sort();
            assert_eq!(
                names, sorted,
                "case {case:?}: expected a stable, ordered listing, got {names:?}"
            );
        }
    }
}

#[tokio::test]
async fn list_app_monitors_refuses_a_token_it_never_issued() {
    let (router, _engine) = emulator();
    for index in 0..3 {
        create(
            &router,
            "seed",
            json!({"Name": format!("token-monitor-{index}"), "Domain": "example.com"}),
        )
        .await;
    }
    // One real token, so the table shows the refusal is about the token's provenance
    // rather than about tokens in general.
    let first = call(&router, "POST", "/appmonitors?maxResults=1", None).await;
    let issued = first.json("issued")["NextToken"]
        .as_str()
        .expect("a NextToken while monitors remain")
        .to_string();
    let resumed = call(
        &router,
        "POST",
        &format!("/appmonitors?maxResults=1&nextToken={issued}"),
        None,
    )
    .await;
    assert_eq!(
        resumed.status, 200,
        "case \"issued token\": a token this service issued resumes the listing, got {}",
        resumed.status
    );

    // Rows: a token this service never issued, in each form a client might send — not
    // hex at all, hex of a string with no marker, hex of a truncated marker, and the
    // marker with no resume point behind it.
    for case in [
        "not-hex-at-all",
        "6e6f742d612d746f6b656e",
        "4c697374417070",
        "4c6973744170704d6f6e69746f72732f312f",
    ] {
        let reply = call(
            &router,
            "POST",
            &format!("/appmonitors?maxResults=1&nextToken={case}"),
            None,
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400 for a token this service never issued, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.error_code(case),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            String::from_utf8_lossy(&reply.body)
        );
    }
}

#[tokio::test]
async fn a_create_or_update_the_model_forbids_in_prose_is_refused() {
    let (router, _engine) = emulator();
    create(
        &router,
        "seed",
        json!({"Name": "prose-monitor", "Domain": "example.com"}),
    )
    .await;

    // Rows: each is a request every modelled constraint admits but the model's prose
    // forbids. Source: `CreateAppMonitorRequest$DomainList` ("The CreateAppMonitor
    // requires either the domain or the domain list"),
    // `UpdateAppMonitorRequest$DomainList` ("The UpdateAppMonitor allows either the
    // domain or the domain list") and `AppMonitorConfiguration$IncludedPages` /
    // `$ExcludedPages` ("You can't include both ExcludedPages and IncludedPages in the
    // same operation"). Which platforms the first row's refusal applies to is the
    // `platform_domain_authority` suite's table: these rows name no `Platform`, so they
    // are web-platform requests.
    let rows: [(&str, &str, &str, Value); 5] = [
        (
            "no Domain and no DomainList on the web platform",
            "POST",
            "/appmonitor",
            json!({"Name": "domainless-monitor"}),
        ),
        (
            "create with both Domain and DomainList",
            "POST",
            "/appmonitor",
            json!({
                "Name": "both-domains-monitor",
                "Domain": "example.com",
                "DomainList": ["other.example.com"],
            }),
        ),
        (
            "update with both Domain and DomainList",
            "PATCH",
            "/appmonitor/prose-monitor",
            json!({"Domain": "example.com", "DomainList": ["other.example.com"]}),
        ),
        (
            "create with both IncludedPages and ExcludedPages",
            "POST",
            "/appmonitor",
            json!({
                "Name": "paged-monitor",
                "Domain": "example.com",
                "AppMonitorConfiguration": {
                    "IncludedPages": ["https://www.example.com/in"],
                    "ExcludedPages": ["https://www.example.com/out"],
                },
            }),
        ),
        (
            "update with both IncludedPages and ExcludedPages",
            "PATCH",
            "/appmonitor/prose-monitor",
            json!({
                "AppMonitorConfiguration": {
                    "IncludedPages": ["https://www.example.com/in"],
                    "ExcludedPages": ["https://www.example.com/out"],
                },
            }),
        ),
    ];
    for (case, method, path, body) in rows {
        let reply = call(&router, method, path, Some(body)).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.error_code(case),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            String::from_utf8_lossy(&reply.body)
        );
    }

    // Nothing the refused requests named was stored, and the monitor they could have
    // changed is untouched.
    for absent in [
        "domainless-monitor",
        "both-domains-monitor",
        "paged-monitor",
    ] {
        let reply = call(&router, "GET", &format!("/appmonitor/{absent}"), None).await;
        assert_eq!(
            reply.status, 404,
            "case {absent:?}: a refused create must store nothing, got {}",
            reply.status
        );
    }
    let monitor = get_app_monitor(&router, "after refusal", "prose-monitor").await;
    assert_eq!(
        monitor["AppMonitorConfiguration"]["IncludedPages"],
        Value::Null,
        "case \"after refusal\": a refused update must change nothing, got {}",
        monitor["AppMonitorConfiguration"]
    );
}
