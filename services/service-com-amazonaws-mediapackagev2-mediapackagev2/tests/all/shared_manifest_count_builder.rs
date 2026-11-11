// SPDX-License-Identifier: CC0-1.0

//! Cover for the ONE rule both origin-endpoint writers implement: a request's manifest
//! set is counted PER FAMILY, each count placed at its own family's position, and that
//! per-family count is what decides the refusal — the count range across every family
//! together, and the discriminator for the family the container type cannot serve.
//!
//! `CreateOriginEndpoint` and `UpdateOriginEndpoint` both carry the whole manifest set, so
//! both count it the same way and both answer the same class for the same set. Every row
//! below is therefore driven through BOTH operations and their answers compared, and the
//! builder's family positions are asserted against the order the family enum declares, so
//! a count can never be read as another family's.
//!
//! The family members (`HlsManifests`, `LowLatencyHlsManifests`, `DashManifests`,
//! `MssManifests`), the container types (`CMAF | ISM | TS`) and the
//! `ValidationExceptionType` values each row expects are the model's own, from
//! `smithy_query(query="shape",shape="CreateOriginEndpointRequest,ContainerType,ValidationExceptionType")`.

use serde_json::{json, Value};
use service_com_amazonaws_mediapackagev2_mediapackagev2::state::{
    manifest_counts, ManifestFamily, MAX_MANIFESTS_PER_ORIGIN_ENDPOINT,
};

use crate::channel_lifecycle::{error_type, Emulator};

/// What the shared rule says about one manifest set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// The set is acceptable: the write is accepted.
    Accepted,
    /// The set is refused: 400 `ValidationException` with this discriminator, and — for a
    /// refusal that is about one family rather than the whole request — the member path
    /// that family is carried in.
    Refused {
        exception_type: &'static str,
        member: Option<&'static str>,
    },
}

/// One row: a container type, how many manifests of each family the request carries, and
/// what the rule answers for that set.
#[derive(Debug, Clone, Copy)]
struct Case {
    label: &'static str,
    container: &'static str,
    /// `(member, count)` pairs; a member not named here is not sent at all.
    families: &'static [(&'static str, usize)],
    expected: Expected,
}

/// A refusal about the request as a whole: the manifest-count range, and the
/// compatibility between the request's `ContainerType` and the manifest list it carries —
/// a rule about the RELATIONSHIP between two of the request's own members, which this
/// service states at the request rather than at either of them.
const fn set_refused(exception_type: &'static str) -> Expected {
    Expected::Refused {
        exception_type,
        member: None,
    }
}

/// The rows: one per family on each container type, so a count placed at the wrong
/// family's position shows up as the wrong answer, plus sets that spread the count over
/// several families and sets that leave the permitted range.
fn rows() -> Vec<Case> {
    const MAX: usize = MAX_MANIFESTS_PER_ORIGIN_ENDPOINT;
    vec![
        // A TS container serves the two HLS families only.
        Case {
            label: "TS with HLS alone",
            container: "TS",
            families: &[("HlsManifests", 1)],
            expected: Expected::Accepted,
        },
        Case {
            label: "TS with low-latency HLS alone",
            container: "TS",
            families: &[("LowLatencyHlsManifests", 2)],
            expected: Expected::Accepted,
        },
        Case {
            label: "TS with DASH alone",
            container: "TS",
            families: &[("DashManifests", 1)],
            expected: set_refused("TS_CONTAINER_TYPE_WITH_DASH_MANIFEST"),
        },
        Case {
            label: "TS with MSS alone",
            container: "TS",
            families: &[("MssManifests", 3)],
            expected: set_refused("TS_CONTAINER_TYPE_WITH_MSS_MANIFEST"),
        },
        // CMAF adds DASH.
        Case {
            label: "CMAF with DASH alone",
            container: "CMAF",
            families: &[("DashManifests", 1)],
            expected: Expected::Accepted,
        },
        Case {
            label: "CMAF with MSS alone",
            container: "CMAF",
            families: &[("MssManifests", 1)],
            expected: set_refused("CMAF_CONTAINER_TYPE_WITH_MSS_MANIFEST"),
        },
        Case {
            label: "CMAF with every family it serves, unevenly spread",
            container: "CMAF",
            families: &[
                ("HlsManifests", 1),
                ("LowLatencyHlsManifests", 4),
                ("DashManifests", 2),
            ],
            expected: Expected::Accepted,
        },
        // ISM serves MSS alone.
        Case {
            label: "ISM with MSS alone",
            container: "ISM",
            families: &[("MssManifests", 2)],
            expected: Expected::Accepted,
        },
        Case {
            label: "ISM with HLS alone",
            container: "ISM",
            families: &[("HlsManifests", 1)],
            expected: set_refused("ISM_CONTAINER_TYPE_WITH_HLS_MANIFEST"),
        },
        Case {
            label: "ISM with low-latency HLS alone",
            container: "ISM",
            families: &[("LowLatencyHlsManifests", 1)],
            expected: set_refused("ISM_CONTAINER_TYPE_WITH_LL_HLS_MANIFEST"),
        },
        Case {
            label: "ISM with DASH alone",
            container: "ISM",
            families: &[("DashManifests", 2)],
            expected: set_refused("ISM_CONTAINER_TYPE_WITH_DASH_MANIFEST"),
        },
        // The count range is about the total across every family, so an empty set and an
        // over-sized one are refused whichever families carry them.
        Case {
            label: "no manifest at all",
            container: "TS",
            families: &[],
            expected: set_refused("NUM_MANIFESTS_LOW"),
        },
        Case {
            label: "every served family present but empty",
            container: "CMAF",
            families: &[
                ("HlsManifests", 0),
                ("LowLatencyHlsManifests", 0),
                ("DashManifests", 0),
            ],
            expected: set_refused("NUM_MANIFESTS_LOW"),
        },
        Case {
            label: "past the ceiling, the excess carried by a second family",
            container: "CMAF",
            families: &[("LowLatencyHlsManifests", MAX), ("DashManifests", 1)],
            expected: set_refused("NUM_MANIFESTS_HIGH"),
        },
        Case {
            label: "past the ceiling in the MSS family of an ISM container",
            container: "ISM",
            families: &[("MssManifests", MAX + 1)],
            expected: set_refused("NUM_MANIFESTS_HIGH"),
        },
    ]
}

/// The request body for a row: its container type plus each named manifest member, every
/// manifest under a distinct name so no row is refused on a name collision instead of the
/// rule under test.
fn request_body(case: &Case) -> Value {
    let mut body = json!({"ContainerType": case.container});
    for (member, count) in case.families {
        let manifests: Value = (0..*count)
            .map(|index| json!({"ManifestName": format!("{}-{index}", member.to_lowercase())}))
            .collect();
        body[*member] = manifests;
    }
    body
}

/// Assert one answer against the rule, and report what the row asked for and what came
/// back when it does not hold.
fn assert_answer(label: &str, operation: &str, expected: Expected, status: u16, body: &Value) {
    match expected {
        Expected::Accepted => assert_eq!(
            status, 200,
            "case {label:?} on {operation}: an acceptable manifest set expected 200, \
             got {status} {body}"
        ),
        Expected::Refused {
            exception_type,
            member,
        } => {
            assert_eq!(
                status, 400,
                "case {label:?} on {operation}: a refused manifest set expected 400, \
                 got {status} {body}"
            );
            assert_eq!(
                error_type(body),
                "ValidationException",
                "case {label:?} on {operation}: expected ValidationException, got {body}"
            );
            assert_eq!(
                body.get("ValidationExceptionType").and_then(Value::as_str),
                Some(exception_type),
                "case {label:?} on {operation}: expected ValidationExceptionType \
                 {exception_type}, got {body}"
            );
            let message = body.get("Message").and_then(Value::as_str).unwrap_or("");
            match member {
                Some(member) => assert!(
                    message.contains(&format!("request.{member}:")),
                    "case {label:?} on {operation}: expected the message to name member \
                     {member}, got {message:?}"
                ),
                None => assert!(
                    message.contains("at request:"),
                    "case {label:?} on {operation}: expected a message about the request \
                     as a whole, got {message:?}"
                ),
            }
        }
    }
}

/// The discriminator an answer carries, or `None` for an accepted one — what the two
/// writers are compared on.
fn discriminator(status: u16, body: &Value) -> Option<String> {
    if status == 200 {
        return None;
    }
    Some(format!(
        "{status} {} {}",
        error_type(body),
        body.get("ValidationExceptionType")
            .and_then(Value::as_str)
            .unwrap_or("")
    ))
}

/// A channel group and a channel for the endpoints each test writes.
async fn hierarchy() -> Emulator {
    let emulator = Emulator::new();
    let (status, body) = emulator.create_group("grp", json!({})).await;
    assert_eq!(status, 200, "setup: CreateChannelGroup got {status} {body}");
    let (status, body) = emulator.create_channel("grp", "chan", json!({})).await;
    assert_eq!(status, 200, "setup: CreateChannel got {status} {body}");
    emulator
}

/// Every row driven through BOTH writers: each answers the row's expected class, and the
/// two answer the same class as each other — the one shared count-and-check path.
#[tokio::test]
async fn both_origin_endpoint_writers_count_a_manifest_set_the_same_way() {
    let emulator = hierarchy().await;

    // An endpoint's container type is fixed once it exists
    // (`CONTAINER_TYPE_IMMUTABLE`), so the update rows run against a seeded endpoint per
    // container type, each seeded with a set that container serves.
    for (container, seed) in [
        ("TS", json!({"ContainerType": "TS", "HlsManifests": [{"ManifestName": "seed"}]})),
        ("CMAF", json!({"ContainerType": "CMAF", "DashManifests": [{"ManifestName": "seed"}]})),
        ("ISM", json!({"ContainerType": "ISM", "MssManifests": [{"ManifestName": "seed"}]})),
    ] {
        let (status, body) = emulator
            .create_endpoint("grp", "chan", &format!("seed-{container}"), seed)
            .await;
        assert_eq!(
            status, 200,
            "setup: seeding the {container} endpoint got {status} {body}"
        );
    }

    for (index, case) in rows().iter().enumerate() {
        let (create_status, created) = emulator
            .create_endpoint("grp", "chan", &format!("ep-{index}"), request_body(case))
            .await;
        assert_answer(
            case.label,
            "CreateOriginEndpoint",
            case.expected,
            create_status,
            &created,
        );

        let (update_status, updated) = emulator
            .update_endpoint(
                "grp",
                "chan",
                &format!("seed-{}", case.container),
                request_body(case),
                &[],
            )
            .await;
        assert_answer(
            case.label,
            "UpdateOriginEndpoint",
            case.expected,
            update_status,
            &updated,
        );

        assert_eq!(
            discriminator(create_status, &created),
            discriminator(update_status, &updated),
            "case {:?}: the two writers answered the same manifest set differently \
             ({create_status} {created} vs {update_status} {updated})",
            case.label
        );
    }
}

/// The builder's positions ARE the family enum's order: a count given for one family
/// lands at that family's index in `ManifestFamily::ALL` and nowhere else, whatever the
/// list's element type.
#[test]
fn the_shared_builder_places_each_count_at_its_own_family_position() {
    // A distinct count per family, so a count read at another family's position cannot
    // look right by coincidence.
    let per_family: Vec<(ManifestFamily, Vec<&str>)> = vec![
        (ManifestFamily::Hls, vec!["a"]),
        (ManifestFamily::LowLatencyHls, vec!["a", "b"]),
        (ManifestFamily::Dash, vec!["a", "b", "c"]),
        (ManifestFamily::Mss, vec!["a", "b", "c", "d"]),
    ];
    for (family, manifests) in &per_family {
        let list = Some(manifests.as_slice());
        let counts = match family {
            ManifestFamily::Hls => manifest_counts::<_, &str, &str, &str>(list, None, None, None),
            ManifestFamily::LowLatencyHls => {
                manifest_counts::<&str, _, &str, &str>(None, list, None, None)
            }
            ManifestFamily::Dash => manifest_counts::<&str, &str, _, &str>(None, None, list, None),
            ManifestFamily::Mss => manifest_counts::<&str, &str, &str, _>(None, None, None, list),
        };
        let position = ManifestFamily::ALL
            .iter()
            .position(|declared| declared == family)
            .expect("every family is declared in ManifestFamily::ALL");
        let expected: Vec<usize> = ManifestFamily::ALL
            .iter()
            .enumerate()
            .map(|(index, _)| {
                if index == position {
                    manifests.len()
                } else {
                    0
                }
            })
            .collect();
        assert_eq!(
            counts.to_vec(),
            expected,
            "case {family:?}: expected {} manifests at position {position} and none \
             elsewhere, got {counts:?}",
            manifests.len()
        );
    }

    // And an absent list counts as an empty one, for every family at once.
    let empty: [&str; 0] = [];
    assert_eq!(
        manifest_counts::<&str, &str, &str, &str>(None, None, None, None).to_vec(),
        manifest_counts(
            Some(&empty[..]),
            Some(&empty[..]),
            Some(&empty[..]),
            Some(&empty[..])
        )
        .to_vec(),
        "case \"absent vs empty\": an absent manifest member must count as an empty list"
    );
}
