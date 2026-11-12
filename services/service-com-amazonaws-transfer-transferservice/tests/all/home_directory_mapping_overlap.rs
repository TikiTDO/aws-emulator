// SPDX-License-Identifier: CC0-1.0

//! The rule a LOGICAL home directory's mappings have to satisfy: no two `Entry` paths
//! may overlap.
//!
//! `HomeDirectoryMappings` is the set of logical directory mappings "where `Entry` shows
//! how the path is made visible and `Target` is the actual Amazon S3 or Amazon EFS path"
//! (`smithy_query(query="doc",shape="CreateUserRequest",member="HomeDirectoryMappings")`),
//! so the entries together are the directory tree the session sees. Two entries one of
//! which IS the other, or is an ancestor directory of the other — the chroot `Entry` `/`
//! the member documents subsumes every other path — would mount two targets at one path,
//! which resolves to nothing well defined, so such a write is refused with the modelled
//! `InvalidRequestException` (HTTP 400) and the service's own sentence naming the
//! `Entry` the offending mapping runs into, and NOTHING is written: a refused create
//! leaves no resource and a refused update leaves the stored mappings in force.
//!
//! The rows span what the rule generalises over: how the two paths relate (equal, one
//! inside the other, the root against a child, siblings, a shared text prefix that is
//! not a shared path, a path written with a trailing separator), where in the list the
//! overlapping pair sits, and the mapping COUNT (one, two and three entries). Each row
//! is driven through every operation that writes these mappings — `CreateUser`,
//! `UpdateUser` and `CreateAccess` — because one validator answers for all of them.

use serde_json::{json, Value};

use crate::harness::{member_of, Emulator, ROLE};

/// A `DirectoryId` satisfying the modelled `len 12..12`, pattern `^d-[0-9a-f]{10}$`: an
/// access is created on a directory-backed server, which is the only kind that has
/// accesses.
const DIRECTORY_ID: &str = "d-0123456789";

/// A group SID satisfying `ExternalId`'s modelled `len 1..256`, pattern `^S-1-[\d-]+$`.
const GROUP_SID: &str = "S-1-1-12-1234567890-123456789-1234567890-1234";

/// One row: the entries a write names, and the `Entry` the rule says the write collides
/// with — `None` when every entry names a path of its own and the write is served.
struct Row {
    label: &'static str,
    entries: &'static [&'static str],
    overlaps_with: Option<&'static str>,
}

/// The table every operation below is driven through.
const ROWS: &[Row] = &[
    Row {
        label: "the chroot mapping alone",
        entries: &["/"],
        overlaps_with: None,
    },
    Row {
        label: "two disjoint directories",
        entries: &["/inbound", "/outbound"],
        overlaps_with: None,
    },
    Row {
        label: "three disjoint directories",
        entries: &["/a", "/b", "/c"],
        overlaps_with: None,
    },
    Row {
        label: "siblings under one parent",
        entries: &["/share/a", "/share/b"],
        overlaps_with: None,
    },
    Row {
        // `/inbound` and `/inbox` share the text `/inbo` and name two directories.
        label: "a shared text prefix that is not a shared path",
        entries: &["/inbound", "/inbox"],
        overlaps_with: None,
    },
    Row {
        label: "the chroot mapping and a child of it",
        entries: &["/", "/inbound"],
        overlaps_with: Some("/inbound"),
    },
    Row {
        label: "a child and then the chroot mapping",
        entries: &["/inbound", "/"],
        overlaps_with: Some("/"),
    },
    Row {
        label: "the same entry twice",
        entries: &["/inbound", "/inbound"],
        overlaps_with: Some("/inbound"),
    },
    Row {
        label: "a directory and something inside it",
        entries: &["/share", "/share/reports"],
        overlaps_with: Some("/share/reports"),
    },
    Row {
        label: "the same path written with a trailing separator",
        entries: &["/share", "/share/"],
        overlaps_with: Some("/share/"),
    },
    Row {
        label: "an overlapping pair after a disjoint entry",
        entries: &["/x", "/y", "/y/z"],
        overlaps_with: Some("/y/z"),
    },
];

/// The mappings a row names, each with a target of its own, as the wire carries them.
fn mappings(entries: &[&str]) -> Value {
    Value::Array(
        entries
            .iter()
            .enumerate()
            .map(|(position, entry)| json!({"Entry": entry, "Target": format!("/bucket/{position}")}))
            .collect(),
    )
}

/// The refusal the service words for a mapping set whose entries overlap.
fn overlap_message(entry: &str) -> String {
    format!("[Contains overlapping mapping with: {entry}]")
}

/// Assert `answer` is the refusal of a write whose mappings overlap `entry`.
fn assert_refused(case: &str, answer: &crate::harness::Answer, entry: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected the write to be refused with 400, got {} and body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "InvalidRequestException",
        "case {case:?}: expected InvalidRequestException, got {} with body {}",
        answer.error_code(),
        answer.body
    );
    assert_eq!(
        member_of(&answer.body, "Message").as_str().unwrap_or_default(),
        overlap_message(entry),
        "case {case:?}: expected the overlap sentence naming {entry:?}, got body {}",
        answer.body
    );
}

/// The `HomeDirectoryMappings` `DescribeUser` reports for one user, as a JSON value
/// (`Value::Null` when the user carries none).
async fn described_user_mappings(emulator: &Emulator, server_id: &str, user_name: &str) -> Value {
    let answer = emulator
        .call(
            "DescribeUser",
            json!({"ServerId": server_id, "UserName": user_name}),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "DescribeUser {user_name}: expected 200, got {} with body {}",
        answer.status, answer.body
    );
    member_of(answer.get("User"), "HomeDirectoryMappings").clone()
}

/// The user names `ListUsers` reports for one server.
async fn listed_users(emulator: &Emulator, server_id: &str) -> Vec<String> {
    let answer = emulator
        .call("ListUsers", json!({"ServerId": server_id}))
        .await;
    assert_eq!(
        answer.status, 200,
        "ListUsers: expected 200, got {} with body {}",
        answer.status, answer.body
    );
    answer
        .get("Users")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|user| user.get("UserName").and_then(Value::as_str).map(str::to_string))
        .collect()
}

/// RULE through `CreateUser`: a logical home directory whose entries overlap is refused
/// and creates NO user; one whose entries are disjoint is created and reports the
/// mappings back.
#[tokio::test]
async fn create_user_refuses_overlapping_mappings_and_creates_nothing() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;

    for (position, row) in ROWS.iter().enumerate() {
        let case = row.label;
        let user_name = format!("sftp-user-{position}");
        let sent = mappings(row.entries);
        let answer = emulator
            .call(
                "CreateUser",
                json!({
                    "ServerId": &server_id,
                    "UserName": &user_name,
                    "Role": ROLE,
                    "HomeDirectoryType": "LOGICAL",
                    "HomeDirectoryMappings": sent.clone(),
                }),
            )
            .await;

        match row.overlaps_with {
            Some(entry) => {
                assert_refused(case, &answer, entry);
                let users = listed_users(&emulator, &server_id).await;
                assert!(
                    !users.contains(&user_name),
                    "case {case:?}: a refused create must leave no user, got {users:?}"
                );
            }
            None => {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: expected the create to be served, got {} with body {}",
                    answer.status, answer.body
                );
                let stored = described_user_mappings(&emulator, &server_id, &user_name).await;
                assert_eq!(
                    stored, sent,
                    "case {case:?}: expected the created user to report the mappings it was \
                     given, got {stored:?}"
                );
            }
        }
    }
}

/// RULE through `UpdateUser`: the same refusal, and a refused edit leaves the user's
/// STORED mappings in force; an accepted one replaces them.
#[tokio::test]
async fn update_user_refuses_overlapping_mappings_and_keeps_the_stored_ones() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;
    let user_name = "sftp-user";

    // The user starts with a mapping set the rule admits, so every refused row below is
    // asserted against mappings that are in force.
    let initial = mappings(&["/inbound", "/outbound"]);
    let created = emulator
        .call(
            "CreateUser",
            json!({
                "ServerId": &server_id,
                "UserName": user_name,
                "Role": ROLE,
                "HomeDirectoryType": "LOGICAL",
                "HomeDirectoryMappings": initial.clone(),
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "CreateUser: expected 200, got {} with body {}",
        created.status, created.body
    );

    let mut in_force = initial;
    for row in ROWS {
        let case = row.label;
        let sent = mappings(row.entries);
        let answer = emulator
            .call(
                "UpdateUser",
                json!({
                    "ServerId": &server_id,
                    "UserName": user_name,
                    "HomeDirectoryType": "LOGICAL",
                    "HomeDirectoryMappings": sent.clone(),
                }),
            )
            .await;

        match row.overlaps_with {
            Some(entry) => assert_refused(case, &answer, entry),
            None => {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: expected the update to be served, got {} with body {}",
                    answer.status, answer.body
                );
                in_force = sent;
            }
        }

        let stored = described_user_mappings(&emulator, &server_id, user_name).await;
        assert_eq!(
            stored, in_force,
            "case {case:?}: expected the user to hold {in_force:?}, got {stored:?}"
        );
    }
}

/// RULE at SCALE: the rule is about the PATHS, not about how many there are — a set of
/// many entries, each naming a directory of its own, is served, and one entry inside any
/// of them refuses the whole set wherever in the list it sits. The entry the refusal
/// names is the later of the overlapping pair in the order the request lists them, which
/// is what this test derives rather than spells out.
#[tokio::test]
async fn the_rule_holds_over_a_large_mapping_set() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;

    /// How many disjoint directories the large set names. Far above the handful a row of
    /// the table above carries, and far below `HomeDirectoryMappings`' modelled maximum
    /// of 50000 entries.
    const WIDTH: usize = 200;

    let disjoint: Vec<String> = (0..WIDTH).map(|index| format!("/dir{index}")).collect();
    let borrowed: Vec<&str> = disjoint.iter().map(String::as_str).collect();
    let answer = emulator
        .call(
            "CreateUser",
            json!({
                "ServerId": &server_id,
                "UserName": "wide-user",
                "Role": ROLE,
                "HomeDirectoryType": "LOGICAL",
                "HomeDirectoryMappings": mappings(&borrowed),
            }),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "{WIDTH} disjoint directories must be served, got {} with body {}",
        answer.status, answer.body
    );

    // The overlapping entry sits inside one directory in the middle of the set, and is
    // placed before it, just before it and after the whole set in turn.
    let covered = format!("/dir{}", WIDTH / 2);
    let inside = format!("{covered}/inner");
    for (case, position) in [
        ("inside a directory named later", 0),
        ("inside the directory named next", WIDTH / 2),
        ("inside a directory named earlier", WIDTH),
    ] {
        let mut entries = disjoint.clone();
        entries.insert(position, inside.clone());
        let borrowed: Vec<&str> = entries.iter().map(String::as_str).collect();
        // The rule names the entry that runs into a mapping listed BEFORE it, which is
        // the later of the pair in this list.
        let later = [&covered, &inside]
            .into_iter()
            .max_by_key(|path| {
                borrowed
                    .iter()
                    .position(|entry| entry == path)
                    .expect("both paths of the pair are in the list")
            })
            .expect("the pair is not empty");

        let answer = emulator
            .call(
                "CreateUser",
                json!({
                    "ServerId": &server_id,
                    "UserName": format!("wide-user-{position}"),
                    "Role": ROLE,
                    "HomeDirectoryType": "LOGICAL",
                    "HomeDirectoryMappings": mappings(&borrowed),
                }),
            )
            .await;
        assert_refused(case, &answer, later);
    }
}

/// RULE through `CreateAccess`, the other write of these mappings: one validator answers
/// for every operation that configures a logical home directory, so the same rows are
/// refused and served there, and a refused create stores no access.
#[tokio::test]
async fn create_access_refuses_overlapping_mappings() {
    let emulator = Emulator::new();
    let server_id = emulator
        .create_server(json!({
            "IdentityProviderType": "AWS_DIRECTORY_SERVICE",
            "IdentityProviderDetails": {"DirectoryId": DIRECTORY_ID},
        }))
        .await;

    for (position, row) in ROWS.iter().enumerate() {
        let case = row.label;
        // One `ExternalId` per row, so a served row does not collide with the one before
        // it; the id is a group SID with the row's position appended.
        let external_id = format!("{GROUP_SID}-{position}");
        let sent = mappings(row.entries);
        let answer = emulator
            .call(
                "CreateAccess",
                json!({
                    "ServerId": &server_id,
                    "ExternalId": &external_id,
                    "Role": ROLE,
                    "HomeDirectoryType": "LOGICAL",
                    "HomeDirectoryMappings": sent.clone(),
                }),
            )
            .await;

        let described = emulator
            .call(
                "DescribeAccess",
                json!({"ServerId": &server_id, "ExternalId": &external_id}),
            )
            .await;
        match row.overlaps_with {
            Some(entry) => {
                assert_refused(case, &answer, entry);
                assert_eq!(
                    described.error_code(),
                    "ResourceNotFoundException",
                    "case {case:?}: a refused create must leave no access, got {} with body {}",
                    described.status, described.body
                );
            }
            None => {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: expected the create to be served, got {} with body {}",
                    answer.status, answer.body
                );
                let stored = member_of(described.get("Access"), "HomeDirectoryMappings").clone();
                assert_eq!(
                    stored, sent,
                    "case {case:?}: expected the created access to report the mappings it was \
                     given, got {stored:?}"
                );
            }
        }
    }
}
