// SPDX-License-Identifier: CC0-1.0

//! The NAME of the vended CloudWatch Logs group an app monitor advertises.
//!
//! The rule under test: the `DataStorage.CwLog.CwLogGroup` a read reports is the group
//! name the SERVICE vended for that monitor — the vended-logs prefix and the monitor's own
//! name followed by a part the service chose for it, which no caller can rebuild from the
//! request — minted once when the monitor is created, held in this service's state, stable
//! across reads and for the life of the monitor, distinct per monitor, and equal to the
//! name of the log group RUM actually provisions in CloudWatch Logs.
//!
//! `CwLog$CwLogGroup` is modelled as a plain `String` with no pattern and no length
//! (`smithy_query(query="shape", shape="CwLog")`, whose doc is only "The name of the log
//! group where the copies are stored"), so what pins the name is the group it has to name:
//! CloudWatch Logs' own `LogGroupName` is 1..512 characters matching
//! `^[\.\-_/#A-Za-z0-9]+$` (`smithy_query(query="shape", shape="LogGroupName",
//! model_path="services/service-com-amazonaws-cloudwatchlogs-logs-20140328/smithy-model/\
//! com-amazonaws-cloudwatchlogs-logs-20140328.json")`).
//!
//! The rows span what the rule generalises over: the monitor's name (one character, the
//! other characters `AppMonitorName` admits, and the longest name the model admits), which
//! call turns delivery on (the create or a later update), how many monitors share a name
//! prefix or a name across regions, and whether delivery has been toggled since.

use bytes::Bytes;
use serde_json::json;
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::support::{call_in, emulator, Reply, REGION};

/// The prefix a RUM-managed log group's name carries: RUM is a vended-logs service.
const CW_LOG_GROUP_PREFIX: &str = "/aws/vendedlogs/RUMService_";

/// The longest name `AppMonitorName` admits (length 1..255). Source:
/// `smithy_query(query="shape", shape="AppMonitorName")`.
const MAX_APP_MONITOR_NAME_LEN: usize = 255;

/// The bounds `LogGroupName` places on the name the group is created under. Source:
/// `smithy_query(query="shape", shape="LogGroupName")` of the CloudWatch Logs model.
const MAX_LOG_GROUP_NAME_LEN: usize = 512;

/// Whether a character may appear in a `LogGroupName`: its pattern is
/// `^[\.\-_/#A-Za-z0-9]+$`.
fn is_log_group_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_' | '/' | '#')
}

/// Which call turns CloudWatch Logs delivery on for a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TurnedOnBy {
    /// `CreateAppMonitor` already carries `CwLogEnabled: true`.
    TheCreate,
    /// `CreateAppMonitor` leaves it off and a later `UpdateAppMonitor` turns it on.
    AnUpdate,
}

/// `CreateAppMonitor` in `region`, asserting it succeeded.
async fn create_in(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    name: &str,
    cw_log_enabled: bool,
) {
    let reply = call_in(
        router,
        region,
        "POST",
        "/appmonitor",
        Some(json!({
            "Name": name,
            "Domain": "example.com",
            "CwLogEnabled": cw_log_enabled,
        })),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected CreateAppMonitor to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
}

/// `UpdateAppMonitor` in `region`, carrying `CwLogEnabled` alone and asserting it
/// succeeded.
async fn set_delivery_in(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    name: &str,
    enabled: bool,
) {
    let reply = call_in(
        router,
        region,
        "PATCH",
        &format!("/appmonitor/{name}"),
        Some(json!({"CwLogEnabled": enabled})),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected UpdateAppMonitor to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
}

/// The `CwLogGroup` `GetAppMonitor` reports in `region`, or `None` when it reports none.
async fn advertised_group_in(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    name: &str,
) -> Option<String> {
    let reply = call_in(router, region, "GET", &format!("/appmonitor/{name}"), None).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected GetAppMonitor to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    reply.json(case)["AppMonitor"]["DataStorage"]["CwLog"]["CwLogGroup"]
        .as_str()
        .map(str::to_string)
}

/// The same read, asserting the monitor advertises a group at all.
async fn vended_group_in(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    name: &str,
) -> String {
    advertised_group_in(router, case, region, name)
        .await
        .unwrap_or_else(|| {
            panic!("case {case:?}: a monitor with delivery on must advertise a CwLogGroup")
        })
}

/// One CloudWatch Logs `DescribeLogGroups` signed for `region` (awsJson1_1), answering the
/// `logGroupName`s it reports for `prefix`.
///
/// The emulated CloudWatch Logs service is mounted beside RUM by the shared harness, which
/// is how a test sees the group RUM provisioned in process.
async fn log_groups_in(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    prefix: &str,
) -> Vec<String> {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://logs.{region}.amazonaws.com/"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", "Logs_20140328.DescribeLogGroups")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/logs/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(&json!({"logGroupNamePrefix": prefix}))
                .expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let reply = Reply {
        status,
        body: response.into_body(),
        headers,
    };
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected DescribeLogGroups to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    reply.json(case)["logGroups"]
        .as_array()
        .map(|groups| {
            groups
                .iter()
                .filter_map(|group| group["logGroupName"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Assert `advertised` is a vended group name for the monitor named `name`: the documented
/// prefix, that name, and then a non-empty part the service chose — a name a client cannot
/// rebuild from the request, and one CloudWatch Logs admits.
fn assert_vended_shape(case: &str, name: &str, advertised: &str) {
    let derived_from_the_request = format!("{CW_LOG_GROUP_PREFIX}{name}");
    assert!(
        advertised.starts_with(&derived_from_the_request),
        "case {case:?}: expected the advertised group to start with \
         {derived_from_the_request:?}, got {advertised:?}"
    );
    let chosen_by_the_service = &advertised[derived_from_the_request.len()..];
    assert!(
        !chosen_by_the_service.is_empty(),
        "case {case:?}: the vended group name carries a part the service chose, so it cannot \
         be exactly {derived_from_the_request:?}, got {advertised:?}"
    );
    assert!(
        advertised.chars().all(is_log_group_character),
        "case {case:?}: every character of a log group's name must satisfy \
         LogGroupName's pattern, got {advertised:?}"
    );
    assert!(
        (1..=MAX_LOG_GROUP_NAME_LEN).contains(&advertised.chars().count()),
        "case {case:?}: expected a name inside LogGroupName's 1..{MAX_LOG_GROUP_NAME_LEN} \
         length, got {} characters ({advertised:?})",
        advertised.chars().count()
    );
}

#[tokio::test]
async fn the_advertised_group_is_the_vended_name_and_the_group_that_exists() {
    // Each row: the case, the monitor's name and which call turns delivery on. The names
    // span the `AppMonitorName` surface — one character, the punctuation its pattern admits
    // (`^(?!\.)[\.\-_#A-Za-z0-9]+$`, less the `#` a URI path would read as a fragment
    // delimiter), and the longest name it admits — because the monitor's name is one of the
    // two variable parts of the vended group's name.
    let longest = "l".repeat(MAX_APP_MONITOR_NAME_LEN);
    let rows: [(&str, &str, TurnedOnBy); 6] = [
        ("a one-character name, delivery on at create", "m", TurnedOnBy::TheCreate),
        ("a one-character name, delivery on by update", "u", TurnedOnBy::AnUpdate),
        (
            "the punctuation the name pattern admits, delivery on at create",
            "vended.group_name-2",
            TurnedOnBy::TheCreate,
        ),
        (
            "the punctuation the name pattern admits, delivery on by update",
            "vended.group_name-3",
            TurnedOnBy::AnUpdate,
        ),
        (
            "the longest name the model admits, delivery on at create",
            longest.as_str(),
            TurnedOnBy::TheCreate,
        ),
        (
            "a monitor that keeps no copies",
            "no-delivery",
            // Delivery is never turned on for this row; the variant only says which call
            // WOULD have turned it on, so the row below skips both.
            TurnedOnBy::AnUpdate,
        ),
    ];

    for (case, name, turned_on_by) in rows {
        let (router, _engine) = emulator();
        let keeps_copies = name != "no-delivery";
        create_in(
            &router,
            case,
            REGION,
            name,
            keeps_copies && turned_on_by == TurnedOnBy::TheCreate,
        )
        .await;
        if keeps_copies && turned_on_by == TurnedOnBy::AnUpdate {
            set_delivery_in(&router, case, REGION, name, true).await;
        }

        if !keeps_copies {
            // A monitor that stores no copies has no group: it advertises none and RUM
            // provisions none.
            assert_eq!(
                advertised_group_in(&router, case, REGION, name).await,
                None,
                "case {case:?}: a monitor that stores no copies advertises no CwLogGroup"
            );
            let groups = log_groups_in(&router, case, REGION, CW_LOG_GROUP_PREFIX).await;
            assert!(
                groups.is_empty(),
                "case {case:?}: no vended group is provisioned for a monitor that stores no \
                 copies, got {groups:?}"
            );
            continue;
        }

        let advertised = vended_group_in(&router, case, REGION, name).await;
        assert_vended_shape(case, name, &advertised);

        // The advertised name is the name of the group that EXISTS: CloudWatch Logs reports
        // exactly one group under it, and the client called no `logs:CreateLogGroup`.
        let groups = log_groups_in(&router, case, REGION, CW_LOG_GROUP_PREFIX).await;
        assert_eq!(
            groups,
            vec![advertised.clone()],
            "case {case:?}: expected exactly the advertised group {advertised:?} to exist, \
             got {groups:?}"
        );

        // And a client that takes the advertised name apart the way the vended-logs
        // convention lets it — the prefix plus the monitor's own name — addresses that very
        // group in CloudWatch Logs, which is what correlating "the group RUM vended" with
        // "the group RUM advertises" rests on.
        let named = format!("{CW_LOG_GROUP_PREFIX}{name}");
        let by_monitor = log_groups_in(&router, case, REGION, &named).await;
        assert_eq!(
            by_monitor,
            vec![advertised.clone()],
            "case {case:?}: expected the group RUM vended for this monitor to be the one \
             DescribeLogGroups finds under {named:?}, got {by_monitor:?}"
        );

        // And it is held in state rather than recomputed: a second read reports the same
        // name, and so does a read after an unrelated update.
        let again = vended_group_in(&router, case, REGION, name).await;
        assert_eq!(
            again, advertised,
            "case {case:?}: expected the advertised group to be stable across reads, got \
             {again:?} after {advertised:?}"
        );
        let patched = call_in(
            &router,
            REGION,
            "PATCH",
            &format!("/appmonitor/{name}"),
            Some(json!({"Domain": "changed.example.com"})),
        )
        .await;
        assert_eq!(
            patched.status, 200,
            "case {case:?}: expected UpdateAppMonitor to answer 200, got {} {:?}",
            patched.status,
            String::from_utf8_lossy(&patched.body)
        );
        let after_update = vended_group_in(&router, case, REGION, name).await;
        assert_eq!(
            after_update, advertised,
            "case {case:?}: an update that does not touch delivery must leave the vended \
             group name alone, got {after_update:?} after {advertised:?}"
        );
    }
}

#[tokio::test]
async fn no_two_monitors_are_handed_one_vended_group() {
    let (router, _engine) = emulator();
    // The monitors the groups are compared over: a name that is a PREFIX of another's, the
    // SAME name in two regions (a name is only unique within the region that owns it), and
    // the same name created again after a delete — so the only thing distinguishing the
    // rows is which monitor the service vended for, never anything in the request.
    let other_region = "eu-west-2";
    let mut vended: Vec<(String, String)> = Vec::new();

    for (case, region, name) in [
        ("a name", REGION, "app"),
        ("a name the first is a prefix of", REGION, "app2"),
        ("the same name in another region", other_region, "app"),
    ] {
        create_in(&router, case, region, name, true).await;
        let group = vended_group_in(&router, case, region, name).await;
        assert_vended_shape(case, name, &group);
        vended.push((case.to_string(), group));
    }

    // A monitor deleted and created again under the IDENTICAL request is a new resource, so
    // the group vended for it is a new one: the name is not a function of the request.
    let case = "the same name created again after a delete";
    let deleted = call_in(&router, REGION, "DELETE", "/appmonitor/app", None).await;
    assert_eq!(
        deleted.status, 200,
        "case {case:?}: expected DeleteAppMonitor to answer 200, got {} {:?}",
        deleted.status,
        String::from_utf8_lossy(&deleted.body)
    );
    create_in(&router, case, REGION, "app", true).await;
    let recreated = vended_group_in(&router, case, REGION, "app").await;
    assert_vended_shape(case, "app", &recreated);
    vended.push((case.to_string(), recreated));

    for (left, (left_case, left_group)) in vended.iter().enumerate() {
        for (right_case, right_group) in vended.iter().skip(left + 1) {
            assert_ne!(
                left_group, right_group,
                "case {left_case:?} vs {right_case:?}: two monitors must not be handed one \
                 vended group, got {left_group:?} twice"
            );
        }
    }

    // Every one of those names is a group that exists in the region that vended it.
    for (case, region, name) in [
        ("a name the first is a prefix of", REGION, "app2"),
        ("the same name in another region", other_region, "app"),
    ] {
        let group = vended_group_in(&router, case, region, name).await;
        let groups = log_groups_in(&router, case, region, &group).await;
        assert_eq!(
            groups,
            vec![group.clone()],
            "case {case:?}: expected the advertised group {group:?} to exist in {region}, got \
             {groups:?}"
        );
    }
}

#[tokio::test]
async fn toggling_delivery_readvertises_the_one_group_the_monitor_was_vended() {
    let (router, _engine) = emulator();
    let case = "delivery toggled off and on again";
    let name = "toggled";
    create_in(&router, case, REGION, name, true).await;
    let vended = vended_group_in(&router, case, REGION, name).await;
    assert_vended_shape(case, name, &vended);

    // Off: the monitor stores no copies, so it names no group — while the group itself is
    // left where it is, since the telemetry already in it belongs to the account.
    set_delivery_in(&router, case, REGION, name, false).await;
    assert_eq!(
        advertised_group_in(&router, case, REGION, name).await,
        None,
        "case {case:?}: a monitor with delivery off advertises no CwLogGroup"
    );
    let groups = log_groups_in(&router, case, REGION, CW_LOG_GROUP_PREFIX).await;
    assert_eq!(
        groups,
        vec![vended.clone()],
        "case {case:?}: turning delivery off leaves the vended group alone, got {groups:?}"
    );

    // On again: the SAME name, so a client that stored it still resolves the group RUM
    // writes to, and no second group was vended.
    set_delivery_in(&router, case, REGION, name, true).await;
    let readvertised = vended_group_in(&router, case, REGION, name).await;
    assert_eq!(
        readvertised, vended,
        "case {case:?}: expected the one group vended for this monitor, got {readvertised:?} \
         after {vended:?}"
    );
    let groups = log_groups_in(&router, case, REGION, CW_LOG_GROUP_PREFIX).await;
    assert_eq!(
        groups,
        vec![vended.clone()],
        "case {case:?}: exactly one group may back the advertised name, got {groups:?}"
    );
}
