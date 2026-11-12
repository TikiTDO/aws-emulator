// SPDX-License-Identifier: CC0-1.0

//! `BatchStart`, end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are tables: a single stopped channel is ONE row of them.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::MAX_BATCH_START_IDS;
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::{describe, wait_for_the_clock};
use crate::multiplex_lifecycle::{describe_multiplex, settings};
use crate::multiplex_start::{multiplex_with_settings, start_multiplex};
use crate::wire::{call, error_code, medialive};

/// The URI `BatchStart` is bound to (`smithy_query(query="operation",
/// operation="BatchStart")`: POST /prod/batch/start).
const BATCH_START: &str = "/prod/batch/start";

/// An id no resource of any kind was minted with: the mint starts every id
/// family above a million (`state::FIRST_CHANNEL_ID` and its siblings).
const UNKNOWN_ID: &str = "0000001";

/// One row of the refusal table: the case, the request member the id travels
/// in, the setup that builds the resource, the error code the entry must carry,
/// the state the refused resource keeps (`None` when no resource has that id)
/// and the text its `Message` must contain.
type RefusalRow = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    Option<&'static str>,
    &'static str,
);

/// `BatchStart` with `body`, asserting the modelled 200 and returning the two
/// result lists as `(successful, failed)`.
async fn batch_start(
    router: &MultiServiceRouter,
    case: &str,
    body: Value,
) -> (Vec<Value>, Vec<Value>) {
    let (status, response) = call(router, "POST", BATCH_START, Some(body)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: BatchStart: expected 200, got {status}: {response}"
    );
    let successful = response["successful"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a successful list, got {response}"));
    let failed = response["failed"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a failed list, got {response}"));
    (successful, failed)
}

/// A channel created from `body`, still in `CREATING`.
async fn creating_channel(router: &MultiServiceRouter, case: &str, body: Value) -> String {
    let body = crate::support::resolved_channel_body(router, body).await;
    let (status, response) = call(router, "POST", "/prod/channels", Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateChannel: expected 201, got {status}: {response}"
    );
    response["channel"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateChannel reported no id: {response}"))
        .to_string()
}

/// `count` channels, each created and settled into `IDLE` — the one state a
/// start acts from. The clock is waited on once rather than once per channel, so
/// a row naming several channels costs no more waiting than a row naming one.
async fn idle_channels(router: &MultiServiceRouter, case: &str, count: usize) -> Vec<String> {
    let mut ids = Vec::new();
    for index in 0..count {
        ids.push(
            creating_channel(router, case, json!({ "name": format!("batch-start-{index}") })).await,
        );
    }
    if ids.is_empty() {
        return ids;
    }
    wait_for_the_clock();
    for id in &ids {
        let (status, described) = describe(router, id).await;
        assert_eq!(
            described["state"],
            json!("IDLE"),
            "case {case:?}: channel {id} did not reach IDLE ({status}): {described}"
        );
    }
    ids
}

/// `count` multiplexes, each created and settled into `IDLE`.
async fn idle_multiplexes(router: &MultiServiceRouter, case: &str, count: usize) -> Vec<String> {
    let mut ids = Vec::new();
    for index in 0..count {
        ids.push(
            multiplex_with_settings(
                router,
                case,
                &format!("batch-start-mux-{index}"),
                settings(1_000_000 + i64::try_from(index).unwrap_or_default(), 1),
            )
            .await,
        );
    }
    if ids.is_empty() {
        return ids;
    }
    wait_for_the_clock();
    for id in &ids {
        let (status, described) = describe_multiplex(router, id).await;
        assert_eq!(
            described["state"],
            json!("IDLE"),
            "case {case:?}: multiplex {id} did not reach IDLE ({status}): {described}"
        );
    }
    ids
}

/// The reader URI of one resource, per resource kind.
fn reader(member: &str, id: &str) -> String {
    match member {
        "channelIds" => format!("/prod/channels/{id}"),
        "multiplexIds" => format!("/prod/multiplexes/{id}"),
        other => panic!("no implemented reader for {other}"),
    }
}

/// Rule: `BatchStart` starts each stopped resource the request names and reports
/// it in `Successful` with its id, its ARN and the state the start left it in,
/// read back out of the store — the `STARTING` state the resource's own state
/// enum models, which the clock then settles into `RUNNING`.
///
/// The rows differ along the resource kinds the request's members name
/// (`smithy_query(query="operation", operation="BatchStart")`: ChannelIds,
/// MultiplexIds) and along how many of each one call names: a single stopped
/// channel is one row. The states come from the model's own enums
/// (`smithy_query(query="shape", shape="ChannelState")`, `shape="MultiplexState"`).
#[tokio::test]
async fn every_stopped_resource_is_started_and_reported_in_the_state_it_reached() {
    // (case, stopped channels to name, stopped multiplexes to name)
    let table: [(&str, usize, usize); 6] = [
        ("one channel", 1, 0),
        ("two channels", 2, 0),
        ("one multiplex", 0, 1),
        ("two multiplexes", 0, 2),
        ("one of each", 1, 1),
        ("several of both kinds", 2, 3),
    ];

    for (case, channels, multiplexes) in table {
        let router = medialive();
        let channel_ids = idle_channels(&router, case, channels).await;
        let multiplex_ids = idle_multiplexes(&router, case, multiplexes).await;

        // The ARN each resource reports before the start is the one its result
        // entry must carry.
        let mut named: Vec<(&str, String, Value)> = Vec::new();
        for id in &channel_ids {
            let (status, before) = call(&router, "GET", &reader("channelIds", id), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: reading channel {id}: got {status}: {before}"
            );
            named.push(("channelIds", id.clone(), before["arn"].clone()));
        }
        for id in &multiplex_ids {
            let (status, before) = call(&router, "GET", &reader("multiplexIds", id), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: reading multiplex {id}: got {status}: {before}"
            );
            named.push(("multiplexIds", id.clone(), before["arn"].clone()));
        }

        let (successful, failed) = batch_start(
            &router,
            case,
            json!({ "channelIds": channel_ids, "multiplexIds": multiplex_ids }),
        )
        .await;
        assert!(
            failed.is_empty(),
            "case {case:?}: expected no failures, got {failed:?}"
        );
        let expected: Vec<Value> = named
            .iter()
            .map(|(_, id, arn)| json!({ "id": id, "arn": arn, "state": "STARTING" }))
            .collect();
        assert_eq!(
            successful, expected,
            "case {case:?}: expected every named resource started once, got {successful:?}"
        );

        // The store, not the response, is what the start moved: every reader
        // reports STARTING and then the RUNNING the clock owes.
        for (member, id, arn) in &named {
            let (status, after) = call(&router, "GET", &reader(member, id), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected {id} to still be readable, got {status}: {after}"
            );
            assert_eq!(
                after["state"],
                json!("STARTING"),
                "case {case:?}: expected the reader of {id} to report STARTING, got {:?}",
                after["state"]
            );
            assert_eq!(
                &after["arn"], arn,
                "case {case:?}: expected the ARN of {id} to survive the start, got {:?}",
                after["arn"]
            );
        }

        wait_for_the_clock();
        for (member, id, _) in &named {
            let (status, settled) = call(&router, "GET", &reader(member, id), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected {id} to still be readable, got {status}: {settled}"
            );
            assert_eq!(
                settled["state"],
                json!("RUNNING"),
                "case {case:?}: expected the start of {id} to settle into RUNNING, got {:?}",
                settled["state"]
            );
        }
    }
}

/// Rule: a resource `BatchStart` cannot start is reported in `Failed` with the
/// error code the single-resource start of that resource answers with — an id
/// the account does not hold is `NotFoundException`, a channel whose delete has
/// been accepted is `NotFoundException` too, because that family's delete takes
/// the record out of resolution, and a resource that still exists whose state is
/// not the stopped one a start acts from is `ConflictException` — and the
/// resource is left exactly as it was. A multiplex being torn down keeps a
/// resolvable record, so it falls in that last class on its own recorded
/// evidence; those rows are
/// `crate::multiplex_start_after_delete`'s.
///
/// The rows span the states this service can put a channel and a multiplex in
/// other than the stopped `IDLE`, both request members, and the two shapes an
/// entry takes: with the resource's ARN when the resource exists and without one
/// when no resource has that id. Unlike a stop, a start of a resource that is
/// already on its way up is a conflict for BOTH kinds, never a no-op.
#[tokio::test]
async fn every_resource_a_start_cannot_touch_is_reported_as_its_own_failure() {
    // (case, request member, setup, expected code, state the resource keeps,
    //  the text the message must contain)
    let table: [RefusalRow; 8] = [
        (
            "unknown channel",
            "channelIds",
            "none",
            "NotFoundException",
            None,
            "Channel {id} not found",
        ),
        (
            "creating channel",
            "channelIds",
            "creating channel",
            "ConflictException",
            Some("CREATING"),
            "Channel {id} is in state CREATING and can only be started from IDLE",
        ),
        (
            "channel already starting",
            "channelIds",
            "starting channel",
            "ConflictException",
            Some("STARTING"),
            "Channel {id} is in state STARTING and can only be started from IDLE",
        ),
        (
            "running channel",
            "channelIds",
            "running channel",
            "ConflictException",
            Some("RUNNING"),
            "Channel {id} is in state RUNNING and can only be started from IDLE",
        ),
        (
            "deleting channel",
            "channelIds",
            "deleting channel",
            "NotFoundException",
            Some("DELETING"),
            "Channel {id} already deleted",
        ),
        (
            "unknown multiplex",
            "multiplexIds",
            "none",
            "NotFoundException",
            None,
            "Multiplex {id} not found",
        ),
        (
            "creating multiplex",
            "multiplexIds",
            "creating multiplex",
            "ConflictException",
            Some("CREATING"),
            "Multiplex {id} cannot be started while in CREATING state",
        ),
        (
            "running multiplex",
            "multiplexIds",
            "running multiplex",
            "ConflictException",
            Some("RUNNING"),
            "Multiplex {id} cannot be started while in RUNNING state",
        ),
    ];

    for (case, member, setup, expected_code, kept_state, expected_message) in table {
        let router = medialive();
        let id = match setup {
            "none" => UNKNOWN_ID.to_string(),
            "creating channel" => {
                creating_channel(&router, case, json!({ "name": "still-creating" })).await
            }
            "deleting channel" => {
                let id = idle_channels(&router, case, 1)
                    .await
                    .pop()
                    .unwrap_or_else(|| panic!("case {case:?}: no channel was created"));
                let (status, deleted) =
                    call(&router, "DELETE", &format!("/prod/channels/{id}"), None).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: DeleteChannel: got {status}: {deleted}"
                );
                id
            }
            "creating multiplex" => {
                multiplex_with_settings(&router, case, "still-creating", settings(2_000_000, 1))
                    .await
            }
            // The effective start is the FIRST one; the one under test is the
            // second, sent while the resource is on its way up (`STARTING`) or
            // already up (`RUNNING`).
            "starting channel" | "running channel" => {
                let id = idle_channels(&router, case, 1)
                    .await
                    .pop()
                    .unwrap_or_else(|| panic!("case {case:?}: no channel was created"));
                let (successful, failed) =
                    batch_start(&router, case, json!({ "channelIds": [id.clone()] })).await;
                assert!(
                    failed.is_empty(),
                    "case {case:?}: the first start was refused: {failed:?}"
                );
                assert_eq!(
                    successful[0]["state"],
                    json!("STARTING"),
                    "case {case:?}: the test did not reach STARTING, it reached {:?}",
                    successful[0]
                );
                if setup == "running channel" {
                    wait_for_the_clock();
                }
                id
            }
            _ => {
                let id = idle_multiplexes(&router, case, 1)
                    .await
                    .pop()
                    .unwrap_or_else(|| panic!("case {case:?}: no multiplex was created"));
                let (status, started) = start_multiplex(&router, &id).await;
                assert_eq!(
                    status, 202,
                    "case {case:?}: StartMultiplex: expected 202, got {status}: {started}"
                );
                wait_for_the_clock();
                id
            }
        };

        let (successful, failed) = batch_start(&router, case, json!({ member: [id.clone()] })).await;
        assert!(
            successful.is_empty(),
            "case {case:?}: expected no accepted start, got {successful:?}"
        );
        assert_eq!(
            failed.len(),
            1,
            "case {case:?}: expected exactly one failed entry, got {failed:?}"
        );
        let entry = &failed[0];
        assert_eq!(
            entry["id"],
            json!(id),
            "case {case:?}: expected the failure to name the requested id, got {entry:?}"
        );
        assert_eq!(
            entry["code"],
            json!(expected_code),
            "case {case:?}: expected code {expected_code}, got {entry:?}"
        );
        let expected_message = expected_message.replace("{id}", &id);
        assert!(
            entry["message"]
                .as_str()
                .is_some_and(|text| text.contains(&expected_message)),
            "case {case:?}: expected a message containing {expected_message:?}, got {entry:?}"
        );

        match kept_state {
            // A resource that exists carries its ARN into the failure, and the
            // refusal left it in the state it was already in.
            Some(state) => {
                let (status, after) = call(&router, "GET", &reader(member, &id), None).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the refused resource to still be readable, got {status}: {after}"
                );
                assert_eq!(
                    after["state"],
                    json!(state),
                    "case {case:?}: expected the refused resource to keep state {state}, got {:?}",
                    after["state"]
                );
                assert_eq!(
                    entry["arn"], after["arn"],
                    "case {case:?}: expected the failure to carry the resource's ARN, got {entry:?}"
                );
            }
            // No resource has that id, so there is no ARN to report.
            None => assert!(
                entry["arn"].is_null(),
                "case {case:?}: expected no ARN for an id nothing holds, got {entry:?}"
            ),
        }
    }
}

/// Rule: the resources named in one `BatchStart` are attempted independently, so
/// the call answers 200 and reports every accepted start in `Successful` and
/// every refused one in `Failed`, however the request mixes them.
///
/// The rows differ along how many resources are named, how many of them are
/// stopped and how many ids name nothing at all — the row naming nothing follows
/// from the model, whose `BatchStartRequest` has no required member.
#[tokio::test]
async fn one_call_reports_the_accepted_and_the_refused_side_by_side() {
    // (case, stopped channels, channels left creating, unknown channel ids)
    let table: [(&str, usize, usize, usize); 6] = [
        ("nothing named", 0, 0, 0),
        ("one stopped channel and one unknown id", 1, 0, 1),
        ("two stopped channels and one unknown id", 2, 0, 1),
        ("one stopped channel and two unknown ids", 1, 0, 2),
        ("stopped channels only", 3, 0, 0),
        ("stopped and creating channels and an unknown id", 2, 1, 1),
    ];

    for (case, stopped, creating, unknown) in table {
        let router = medialive();
        let stopped_ids = idle_channels(&router, case, stopped).await;
        // Created AFTER the clock was waited on, so these are still CREATING
        // when the batch is sent — the state a start refuses.
        let mut creating_ids = Vec::new();
        for index in 0..creating {
            creating_ids.push(
                creating_channel(&router, case, json!({ "name": format!("creating-{index}") }))
                    .await,
            );
        }

        // The ids nothing holds come after the real ones, so the result entries
        // cannot be an artefact of the request's order.
        let unknown_ids: Vec<String> = (0..unknown).map(|index| format!("000000{index}")).collect();
        let mut requested = stopped_ids.clone();
        requested.extend(creating_ids.clone());
        requested.extend(unknown_ids.clone());

        let (successful, failed) =
            batch_start(&router, case, json!({ "channelIds": requested })).await;

        let accepted: Vec<&str> = successful
            .iter()
            .map(|entry| entry["id"].as_str().unwrap_or_default())
            .collect();
        let expected_accepted: Vec<&str> = stopped_ids.iter().map(String::as_str).collect();
        assert_eq!(
            accepted, expected_accepted,
            "case {case:?}: expected every stopped channel accepted once, got {successful:?}"
        );
        for entry in &successful {
            assert_eq!(
                entry["state"],
                json!("STARTING"),
                "case {case:?}: expected an accepted start to report STARTING, got {entry:?}"
            );
        }

        let refused: Vec<&str> = failed
            .iter()
            .map(|entry| entry["id"].as_str().unwrap_or_default())
            .collect();
        let mut expected_refused: Vec<&str> = creating_ids.iter().map(String::as_str).collect();
        expected_refused.extend(unknown_ids.iter().map(String::as_str));
        assert_eq!(
            refused, expected_refused,
            "case {case:?}: expected every channel a start cannot touch refused once, got {failed:?}"
        );
        for entry in &failed {
            let code = if creating_ids.iter().any(|id| entry["id"] == json!(id)) {
                "ConflictException"
            } else {
                "NotFoundException"
            };
            assert_eq!(
                entry["code"],
                json!(code),
                "case {case:?}: expected code {code}, got {entry:?}"
            );
        }

        // Nothing but the stopped channels moved: each of them is STARTING and
        // every refused one is still exactly where it was.
        for id in &stopped_ids {
            let (status, described) = describe(&router, id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected channel {id} to still be readable, got {status}: {described}"
            );
            assert_eq!(
                described["state"],
                json!("STARTING"),
                "case {case:?}: expected channel {id} to be STARTING, got {:?}",
                described["state"]
            );
        }
        for id in &creating_ids {
            let (status, described) = describe(&router, id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected channel {id} to still be readable, got {status}: {described}"
            );
            assert_eq!(
                described["state"],
                json!("CREATING"),
                "case {case:?}: expected the refused channel {id} to stay CREATING, got {:?}",
                described["state"]
            );
        }
    }
}

/// Rule: one call may name at most
/// [`MAX_BATCH_START_IDS`](service_com_amazonaws_medialive_medialive::state::MAX_BATCH_START_IDS)
/// resource ids across its two id lists — the model constrains neither of their
/// lengths, and each id costs a lookup and a projection plus an entry in the
/// response — and a batch above that bound is refused whole with the operation's
/// modelled `BadRequestException` rather than walked.
///
/// The rows span the bound itself and one id past it, each named in one member
/// and spread over both, because the cap is on their sum.
#[tokio::test]
async fn a_batch_above_the_bound_is_refused_whole() {
    let cap = MAX_BATCH_START_IDS;
    let half = cap / 2;
    // (case, ids per member in model order, whether the batch is refused)
    let table: [(&str, [usize; 2], bool); 5] = [
        ("at the bound, one member", [cap, 0], false),
        ("one past the bound, one member", [cap + 1, 0], true),
        ("at the bound, spread over both members", [half, cap - half], false),
        (
            "one past the bound, spread over both members",
            [half, cap + 1 - half],
            true,
        ),
        ("far past the bound", [cap + 1, cap + 1], true),
    ];
    let members = ["channelIds", "multiplexIds"];

    for (case, counts, is_refused) in table {
        let router = medialive();
        let mut body = json!({});
        let mut minted = 0;
        for (member, count) in members.iter().zip(counts) {
            // Ids nothing holds, so the row needs no resources: what is under
            // test is the bound, not the per-resource outcome.
            let ids: Vec<String> = (0..count)
                .map(|_| {
                    minted += 1;
                    format!("9{minted:06}")
                })
                .collect();
            body[*member] = json!(ids);
        }

        let (status, response) = call(&router, "POST", BATCH_START, Some(body)).await;
        if is_refused {
            assert_eq!(
                status, 400,
                "case {case:?}: expected a batch of {} ids to be refused, got {status}: {response}",
                counts.iter().sum::<usize>()
            );
            assert_eq!(
                error_code(status, &response),
                "BadRequestException",
                "case {case:?}: expected BadRequestException, got {response}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: expected a batch of {} ids to be answered, got {status}: {response}",
                counts.iter().sum::<usize>()
            );
            assert_eq!(
                response["failed"].as_array().map(Vec::len),
                Some(cap),
                "case {case:?}: expected one failure per named id, got {:?}",
                response["failed"]
            );
            assert_eq!(
                response["successful"],
                json!([]),
                "case {case:?}: expected no accepted start, got {:?}",
                response["successful"]
            );
        }
    }
}
