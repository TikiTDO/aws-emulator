// SPDX-License-Identifier: CC0-1.0

//! `DescribeUsers` along the axes the `rbac_users` suite does not walk: the
//! letter case of the id looked up, an `Engine` that narrows the answer to
//! nothing (including the empty one the member's pattern admits), the values a
//! page request cannot be served for, and the invariant that a describe is a
//! read.
//!
//! Every assertion goes over the wire, through the same router the production
//! emulator mounts, and every user is put in the store by `CreateUser` over
//! that same wire — nothing reaches into the service's state struct, so a
//! response that is not computed from persisted state cannot pass. The request
//! builder and the XML reader are the ones the `cache_cluster_lifecycle` suite
//! marks `pub(crate)`, so this target keeps one harness rather than a second
//! copy that could drift.
//!
//! The tests are tables: each row is one input along the axis the rule under
//! test generalises over (the spelling of the id, the engine named, the page
//! size, the marker), so no single recorded request is what makes them pass.
//!
//! The pagination half of the same rule is the `describe_users` suite beside
//! this one, which walks the pages of a NARROWED describe through the helpers
//! below.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::harness::{params, Emulator, Reply};

/// Create one user over the wire, failing the test if it is not created.
///
/// A local `CreateUser` call rather than a reach into the `rbac_users` suite:
/// that file is edited by every branch landing a user operation, and a change
/// to its module surface from here collides with those. The five form members
/// below are `CreateUser`'s required ones plus `NoPasswordRequired`, so the
/// user is created without a password.
async fn create_user(emulator: &Emulator, id: &str, engine: &str) {
    let reply = emulator
        .call(
            "CreateUser",
            &params(&[
                ("UserId", id),
                ("UserName", id),
                ("Engine", engine),
                ("AccessString", "on ~* +@all"),
                ("NoPasswordRequired", "true"),
            ]),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case {id}: CreateUser must answer 200, got {} and body {}",
        reply.status, reply.body
    );
}

/// One user to put in the store: its id (which doubles as its name) and the
/// engine it runs. The rows span both engines RBAC supports and ids that sort
/// before and after the built-in `default` user.
pub(crate) const SEEDS: [(&str, &str); 5] = [
    ("alpha-reader", "redis"),
    ("zulu-writer", "valkey"),
    ("mid-user", "valkey"),
    ("iam-less", "redis"),
    // An id made only of hex letters: one more shape of id the tables below
    // walk over.
    ("beefcafe", "redis"),
];

/// An emulator holding [`SEEDS`] beside the built-in `default` user.
pub(crate) async fn emulator_with_users() -> Emulator {
    let emulator = Emulator::new();
    for (id, engine) in SEEDS {
        create_user(&emulator, id, engine).await;
    }
    emulator
}

/// One `DescribeUsers` call that must succeed, with the ids and engines it
/// reported, zipped — reading both columns is also the assertion that every
/// reported user carries both members.
pub(crate) async fn describe(
    emulator: &Emulator,
    case: &str,
    pairs: &[(String, String)],
) -> (Reply, Vec<(String, String)>) {
    let reply = emulator.call("DescribeUsers", pairs).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: DescribeUsers must answer 200, got {} and body {}",
        reply.status, reply.body
    );
    let ids = reply.all("UserId");
    let engines = reply.all("Engine");
    assert_eq!(
        engines.len(),
        ids.len(),
        "case {case}: Engine must be reported once per user ({}), got {}: {}",
        ids.len(),
        engines.len(),
        reply.body
    );
    let users = ids
        .iter()
        .zip(&engines)
        .map(|(id, engine)| ((*id).to_string(), (*engine).to_string()))
        .collect();
    (reply, users)
}

/// Every stored user, in the order the service reports it.
pub(crate) async fn all_users(emulator: &Emulator, case: &str) -> Vec<(String, String)> {
    describe(emulator, case, &[]).await.1
}

#[tokio::test]
async fn a_user_is_read_back_by_its_id_in_any_letter_case() {
    let emulator = emulator_with_users().await;
    let stored = all_users(&emulator, "unfiltered").await;
    assert!(
        stored.iter().any(|(id, _)| id == state::DEFAULT_USER_ID),
        "the built-in user must be among the stored users, got {stored:?}"
    );

    // The axis: how the request spells the id. `CreateUser$UserId` "is stored
    // as a lowercase string", so a lookup resolves against that form however it
    // is spelled — the built-in user included.
    for (id, engine) in &stored {
        let mut chars = id.chars();
        let capitalised = match chars.next() {
            Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            None => String::new(),
        };
        for spelling in [id.clone(), id.to_uppercase(), capitalised] {
            let case = format!("lookup {spelling}");
            let (reply, reported) =
                describe(&emulator, &case, &params(&[("UserId", &spelling)])).await;
            assert_eq!(
                reported,
                vec![(id.clone(), engine.clone())],
                "case {case}: a single-record lookup must report exactly that stored user, got \
                 {reported:?}"
            );
            assert!(
                reply.marker().is_none(),
                "case {case}: a single-record lookup paginates nothing, got marker {:?}",
                reply.marker()
            );
        }
    }
}

#[tokio::test]
async fn an_id_no_stored_user_carries_is_refused_as_not_found() {
    let emulator = emulator_with_users().await;

    // The axis: how the id misses — a name of no user, one letter off a stored
    // id either way, a stored name's prefix, and a single letter. Every row
    // satisfies the modelled `UserId` pattern `^[a-zA-Z][a-zA-Z0-9\-]*$`, so
    // each reaches the handler rather than the boundary validator.
    let rows = [
        "nosuchuser",
        "defaults",
        "efault",
        "alpha",
        "zulu-writer-2",
        "d",
    ];
    for id in rows {
        let case = format!("missing id {id}");
        let reply = emulator
            .call("DescribeUsers", &params(&[("UserId", id)]))
            .await;
        assert_eq!(
            reply.status, 404,
            "case {case}: must be refused with HTTP 404, got {} and body {}",
            reply.status, reply.body
        );
        // `UserNotFoundFault`'s awsQuery code (smithy_query(query="operation",
        // operation="DescribeUsers"): awsQueryError code `UserNotFound`,
        // httpResponseCode 404).
        assert_eq!(
            reply.error_code(),
            "UserNotFound",
            "case {case}: must be refused with UserNotFound, got {:?}: {}",
            reply.error_code(),
            reply.body
        );
    }
}

#[tokio::test]
async fn the_engine_reports_exactly_the_users_running_it() {
    let emulator = emulator_with_users().await;
    let stored = all_users(&emulator, "unfiltered").await;

    // The axis: the engine named — every engine this service knows
    // (`state::ENGINES`, including `memcached`, which has no RBAC users at
    // all), the same names in other letter cases (an engine is stored
    // lowercased), and the empty string, which `EngineType`'s pattern
    // `^[a-zA-Z]*$` admits and which names no engine. A value outside the
    // engine list is left out deliberately: nothing in the model or the docs
    // says whether AWS refuses it or narrows to nothing, so this suite pins
    // only what the model states.
    let mut rows: Vec<String> = state::ENGINES.iter().map(|e| (*e).to_string()).collect();
    rows.push("REDIS".to_string());
    rows.push("Valkey".to_string());
    rows.push(String::new());
    for engine_name in rows {
        let case = format!("engine {engine_name:?}");
        let wanted = engine_name.to_ascii_lowercase();
        let expected: Vec<(String, String)> = stored
            .iter()
            .filter(|(_, engine)| *engine == wanted)
            .cloned()
            .collect();
        let (_, reported) = describe(&emulator, &case, &params(&[("Engine", &engine_name)])).await;
        assert_eq!(
            reported, expected,
            "case {case}: must report exactly the {wanted:?} users, got {reported:?}"
        );

        // The engine and a single-record lookup compose: the user's own engine
        // keeps it, another engine leaves the page empty while the user still
        // exists — HTTP 200, not the not-found fault.
        for (id, engine) in &stored {
            let case = format!("{case} + UserId {id}");
            let (_, reported) = describe(
                &emulator,
                &case,
                &params(&[("UserId", id), ("Engine", &engine_name)]),
            )
            .await;
            let expected = if *engine == wanted {
                vec![(id.clone(), engine.clone())]
            } else {
                Vec::new()
            };
            assert_eq!(
                reported, expected,
                "case {case}: the stored engine is {engine:?}, got {reported:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_value_the_describe_cannot_serve_is_refused() {
    let emulator = emulator_with_users().await;

    // The axis: which parameter carries a value the service cannot serve — a
    // page size that is no page, and a `Marker` that is not a token this
    // service could have minted at all (a non-hex character, an odd number of
    // hex digits, nothing hex about it). Each is refused with the
    // `InvalidParameterValue` every paginated describe of this service answers
    // such a value with.
    let rows: [(&str, &[(&str, &str)]); 5] = [
        ("MaxRecords 0", &[("MaxRecords", "0")]),
        ("MaxRecords -5", &[("MaxRecords", "-5")]),
        (
            "marker with a non-hex character",
            &[("Marker", "not-a-marker")],
        ),
        (
            "marker of an odd number of hex digits",
            &[("Marker", "abc")],
        ),
        ("marker that is not hex at all", &[("Marker", "zzzz")]),
    ];
    for (case, pairs) in rows {
        let reply = emulator.call("DescribeUsers", &params(pairs)).await;
        assert_eq!(
            reply.status, 400,
            "case {case}: must be refused with HTTP 400, got {} and body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidParameterValue",
            "case {case}: must be refused as a bad parameter value, got {:?}: {}",
            reply.error_code(),
            reply.body
        );
    }
}

#[tokio::test]
async fn describing_the_users_twice_reports_the_same_users() {
    let emulator = emulator_with_users().await;
    // A describe is a read: the second call sees exactly what the first did,
    // and reading one user in between changes nothing for the next caller.
    let first = all_users(&emulator, "first read").await;
    for (id, _) in &first {
        let case = format!("read {id}");
        describe(&emulator, &case, &params(&[("UserId", id)])).await;
    }
    let second = all_users(&emulator, "second read").await;
    assert_eq!(
        first, second,
        "case repeated read: a describe must not change what a later caller sees"
    );
}
