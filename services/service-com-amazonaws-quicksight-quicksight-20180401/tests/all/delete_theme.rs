// SPDX-License-Identifier: CC0-1.0

//! `DeleteTheme`, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so what is asserted is what a QuickSight SDK client would see:
//! the route, the status, and a body the handler computed from the shared store.
//!
//! The rule under test: a delete addressing `(AwsAccountId, ThemeId)` removes what the
//! request names from the stored theme — every version of it, which is the theme, when
//! the request names no `VersionNumber`, and only that one version when it names one —
//! reports the addressed theme's ARN and id, and leaves nothing for a later read; a
//! delete addressing a theme, or a version, the account does not hold is refused with
//! the modelled not-found error. So the rows below vary what the rule generalises over:
//! the id's form, the account, how many versions the theme holds, which of them the
//! request names, and the names (aliases) pointing at them.

use serde_json::{json, Value};

use service_com_amazonaws_quicksight_quicksight_20180401::ops::types::ThemeConfiguration;
use service_com_amazonaws_quicksight_quicksight_20180401::state as qs_state;

use super::data_source_data_set_lifecycle::Harness;

/// The account most rows use.
const ACCOUNT: &str = "000000000000";
/// A second, unrelated 12-digit account id, for the rows about account keying.
const OTHER_ACCOUNT: &str = "210987654321";
/// The region the test client signs for; it is the region in every ARN below.
const REGION: &str = "us-west-2";

fn theme_arn(account: &str, theme_id: &str) -> String {
    format!("arn:aws:quicksight:{REGION}:{account}:theme/{theme_id}")
}

async fn create_theme(harness: &Harness, account: &str, theme_id: &str, name: &str) -> (u16, Value) {
    harness
        .call(
            "POST",
            &format!("/accounts/{account}/themes/{theme_id}"),
            Some(json!({
                "Name": name,
                "BaseThemeId": "MIDNIGHT",
                "Configuration": { "DataColorPalette": { "Colors": ["#37BFF5"] } },
            })),
        )
        .await
}

async fn delete_theme(
    harness: &Harness,
    account: &str,
    theme_id: &str,
    version_number: Option<i64>,
) -> (u16, Value) {
    let query = match version_number {
        Some(version_number) => format!("?version-number={version_number}"),
        None => String::new(),
    };
    harness
        .call(
            "DELETE",
            &format!("/accounts/{account}/themes/{theme_id}{query}"),
            None,
        )
        .await
}

async fn describe_theme(
    harness: &Harness,
    account: &str,
    theme_id: &str,
    version_number: Option<i64>,
) -> (u16, Value) {
    let query = match version_number {
        Some(version_number) => format!("?version-number={version_number}"),
        None => String::new(),
    };
    harness
        .call(
            "GET",
            &format!("/accounts/{account}/themes/{theme_id}{query}"),
            None,
        )
        .await
}

/// Give the stored theme the versions `2..=version_count`, as the version history a
/// theme updated that many times holds. `UpdateTheme` is not implemented yet, so the
/// history is written straight into the store the delete reads.
fn extend_version_history(harness: &Harness, account: &str, theme_id: &str, version_count: i64) {
    let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
    let mut store = state.lock();
    let record = store
        .themes
        .get_mut(&(account.to_string(), theme_id.to_string()))
        .unwrap_or_else(|| panic!("case {theme_id:?}: the created theme is not stored"));
    let now = std::time::SystemTime::now();
    for version_number in (qs_state::THEME_INITIAL_VERSION_NUMBER + 1)..=version_count {
        record.versions.push(qs_state::ThemeVersionRecord {
            version_number,
            arn: qs_state::theme_version_arn(REGION, account, theme_id, version_number),
            description: None,
            base_theme_id: "MIDNIGHT".to_string(),
            created_time: qs_state::now_timestamp(now),
            configuration: ThemeConfiguration::default(),
            creation_settles_at: now,
        });
    }
}

/// How many versions the account's theme holds, or `None` when it holds no such theme.
fn stored_version_numbers(harness: &Harness, account: &str, theme_id: &str) -> Option<Vec<i64>> {
    let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
    let store = state.lock();
    store
        .themes
        .get(&(account.to_string(), theme_id.to_string()))
        .map(|record| {
            record
                .versions
                .iter()
                .map(|version| version.version_number)
                .collect()
        })
}

/// One row of the delete-the-whole-theme table.
struct WholeThemeRow {
    case: &'static str,
    account: &'static str,
    theme_id: &'static str,
    /// How many versions the theme holds when the delete arrives.
    version_count: i64,
}

#[tokio::test]
async fn delete_theme_without_a_version_removes_every_version() {
    let harness = Harness::new();

    // The rows vary the id's form (the model admits `^[\w\-]+$`, 1..512 characters),
    // the owning account, and the size of the version history the delete removes.
    // Source: smithy_query(query="operation", operation="DeleteTheme").
    for row in [
        WholeThemeRow {
            case: "single-version",
            account: ACCOUNT,
            theme_id: "sales-theme",
            version_count: 1,
        },
        WholeThemeRow {
            case: "three-versions-underscored-id",
            account: ACCOUNT,
            theme_id: "theme_with_underscores",
            version_count: 3,
        },
        WholeThemeRow {
            case: "shortest-id-other-account",
            account: OTHER_ACCOUNT,
            theme_id: "t",
            version_count: 2,
        },
    ] {
        let case = row.case;
        let (status, body) = create_theme(&harness, row.account, row.theme_id, "Deletable").await;
        assert_eq!(
            status, 200,
            "case {case:?}: the fixture create expected 200, got {status} with {body}"
        );
        extend_version_history(&harness, row.account, row.theme_id, row.version_count);
        assert_eq!(
            stored_version_numbers(&harness, row.account, row.theme_id),
            Some((1..=row.version_count).collect::<Vec<i64>>()),
            "case {case:?}: the fixture must hold {} versions",
            row.version_count
        );

        let (status, body) = delete_theme(&harness, row.account, row.theme_id, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: delete expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["Arn"],
            json!(theme_arn(row.account, row.theme_id)),
            "case {case:?}: delete reported the wrong Arn: {body}"
        );
        assert_eq!(
            body["ThemeId"],
            json!(row.theme_id),
            "case {case:?}: delete reported the wrong ThemeId: {body}"
        );
        assert!(
            body["RequestId"]
                .as_str()
                .is_some_and(|request_id| !request_id.is_empty()),
            "case {case:?}: delete reported no RequestId: {body}"
        );

        // Nothing of the theme is left: not in the store, not for a read, and not for a
        // second delete.
        assert_eq!(
            stored_version_numbers(&harness, row.account, row.theme_id),
            None,
            "case {case:?}: the deleted theme is still stored"
        );
        let (status, body) = describe_theme(&harness, row.account, row.theme_id, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: describe after the delete expected 404, got {status} with {body}"
        );
        let (status, body) = delete_theme(&harness, row.account, row.theme_id, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a second delete expected 404, got {status} with {body}"
        );
        assert_eq!(
            body["Message"],
            json!(format!("Theme with ID {} not found.", row.theme_id)),
            "case {case:?}: the refusal must name the theme: {body}"
        );
    }
}

/// One row of the delete-one-version table.
struct VersionRow {
    case: &'static str,
    /// How many versions the theme holds when the delete arrives.
    version_count: i64,
    /// The `VersionNumber` the delete names.
    delete_version: i64,
    /// The versions the theme holds afterwards; empty means the theme is gone.
    remaining: &'static [i64],
}

#[tokio::test]
async fn delete_theme_with_a_version_removes_only_that_version() {
    let harness = Harness::new();

    // The rows vary which version of the history the request names — the first, one in
    // the middle, the latest, and the only one — because that is what the rule
    // generalises over.
    for (index, row) in [
        VersionRow {
            case: "first-of-three",
            version_count: 3,
            delete_version: 1,
            remaining: &[2, 3],
        },
        VersionRow {
            case: "middle-of-three",
            version_count: 3,
            delete_version: 2,
            remaining: &[1, 3],
        },
        VersionRow {
            case: "latest-of-three",
            version_count: 3,
            delete_version: 3,
            remaining: &[1, 2],
        },
        VersionRow {
            case: "the-only-version",
            version_count: 1,
            delete_version: 1,
            remaining: &[],
        },
    ]
    .into_iter()
    .enumerate()
    {
        let case = row.case;
        // One theme per row, so each row deletes from a history of its own.
        let theme_id = format!("versioned-theme-{index}");
        let (status, body) = create_theme(&harness, ACCOUNT, &theme_id, "Versioned").await;
        assert_eq!(
            status, 200,
            "case {case:?}: the fixture create expected 200, got {status} with {body}"
        );
        extend_version_history(&harness, ACCOUNT, &theme_id, row.version_count);

        let (status, body) = delete_theme(&harness, ACCOUNT, &theme_id, Some(row.delete_version)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: delete expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["Arn"],
            json!(theme_arn(ACCOUNT, &theme_id)),
            "case {case:?}: delete reported the wrong Arn: {body}"
        );
        assert_eq!(
            body["ThemeId"],
            json!(theme_id),
            "case {case:?}: delete reported the wrong ThemeId: {body}"
        );

        let stored = stored_version_numbers(&harness, ACCOUNT, &theme_id);
        let expected = (!row.remaining.is_empty()).then(|| row.remaining.to_vec());
        assert_eq!(
            stored, expected,
            "case {case:?}: expected the versions {expected:?} to be left, got {stored:?}"
        );

        // The version the delete named is gone from every read; the ones it did not
        // name still answer, and the latest of them is what an unversioned read reports.
        let (status, body) = describe_theme(&harness, ACCOUNT, &theme_id, Some(row.delete_version)).await;
        assert_eq!(
            status, 404,
            "case {case:?}: describing the deleted version expected 404, got {status} \
             with {body}"
        );
        for &version_number in row.remaining {
            let (status, body) = describe_theme(&harness, ACCOUNT, &theme_id, Some(version_number)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: version {version_number} must survive, got {status} with \
                 {body}"
            );
            assert_eq!(
                body["Theme"]["Version"]["VersionNumber"],
                json!(version_number),
                "case {case:?}: the read reported the wrong version: {body}"
            );
        }
        let (status, body) = describe_theme(&harness, ACCOUNT, &theme_id, None).await;
        match row.remaining.last() {
            Some(&latest) => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: the theme must survive, got {status} with {body}"
                );
                assert_eq!(
                    body["Theme"]["Version"]["VersionNumber"],
                    json!(latest),
                    "case {case:?}: the latest remaining version is {latest}: {body}"
                );
            }
            // A theme is its version history: with the last version deleted there is no
            // theme left to read.
            None => assert_eq!(
                status, 404,
                "case {case:?}: with no version left the theme is gone, got {status} with \
                 {body}"
            ),
        }
    }
}

#[tokio::test]
async fn delete_theme_refuses_a_version_the_theme_does_not_hold() {
    let harness = Harness::new();
    let theme_id = "two-version-theme";
    let (status, body) = create_theme(&harness, ACCOUNT, theme_id, "Versioned").await;
    assert_eq!(
        status, 200,
        "the fixture create expected 200, got {status} with {body}"
    );
    extend_version_history(&harness, ACCOUNT, theme_id, 2);

    // `VersionNumber` is a `range 1..` member, so 1 is the lowest version a request can
    // name; 2 is this theme's latest and every number past it names none.
    // Source: smithy_query(query="operation", operation="DeleteTheme").
    for (case, version_number) in [("one-past-the-history", 3i64), ("far-past-the-history", 99)] {
        let (status, body) = delete_theme(&harness, ACCOUNT, theme_id, Some(version_number)).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} with {body}"
        );
        assert_eq!(
            body["Message"],
            json!(format!(
                "Theme with ID {theme_id} has no version {version_number}."
            )),
            "case {case:?}: the refusal must name the theme and the version: {body}"
        );
    }

    // A refused delete removed nothing.
    assert_eq!(
        stored_version_numbers(&harness, ACCOUNT, theme_id),
        Some(vec![1, 2]),
        "a refused delete must leave the version history as it was"
    );
}

#[tokio::test]
async fn delete_theme_is_scoped_to_the_account_that_holds_the_theme() {
    let harness = Harness::new();
    let shared_id = "shared-theme";

    for (case, account) in [("first-account", ACCOUNT), ("second-account", OTHER_ACCOUNT)] {
        let (status, body) = create_theme(&harness, account, shared_id, case).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the fixture create expected 200, got {status} with {body}"
        );
    }

    let (status, body) = delete_theme(&harness, ACCOUNT, shared_id, None).await;
    assert_eq!(
        status, 200,
        "deleting the first account's theme expected 200, got {status} with {body}"
    );

    // The other account's theme of the same id is untouched, and a third account that
    // created nothing has nothing to delete.
    let (status, body) = describe_theme(&harness, OTHER_ACCOUNT, shared_id, None).await;
    assert_eq!(
        status, 200,
        "the other account's theme must survive, got {status} with {body}"
    );
    assert_eq!(
        body["Theme"]["Name"],
        json!("second-account"),
        "the surviving theme must be the other account's own: {body}"
    );
    let (status, body) = delete_theme(&harness, "123456789012", shared_id, None).await;
    assert_eq!(
        status, 404,
        "an account that created no theme has none to delete, got {status} with {body}"
    );
}

async fn create_theme_alias(
    harness: &Harness,
    account: &str,
    theme_id: &str,
    alias_name: &str,
    version_number: i64,
) -> (u16, Value) {
    harness
        .call(
            "POST",
            &format!("/accounts/{account}/themes/{theme_id}/aliases/{alias_name}"),
            Some(json!({ "ThemeVersionNumber": version_number })),
        )
        .await
}

async fn describe_theme_alias(
    harness: &Harness,
    account: &str,
    theme_id: &str,
    alias_name: &str,
) -> (u16, Value) {
    harness
        .call(
            "GET",
            &format!("/accounts/{account}/themes/{theme_id}/aliases/{alias_name}"),
            None,
        )
        .await
}

/// RULE: an alias is a NAME for one of the theme's versions, so a version-scoped delete
/// takes the names that pointed at the version it removed with it, and leaves every name
/// for a surviving version resolving exactly as before.
#[tokio::test]
async fn deleting_a_version_takes_the_aliases_that_named_it() {
    let harness = Harness::new();
    let theme_id = "aliased-theme";
    let (status, body) = create_theme(&harness, ACCOUNT, theme_id, "Aliased").await;
    assert_eq!(
        status, 200,
        "the fixture create expected 200, got {status} with {body}"
    );
    extend_version_history(&harness, ACCOUNT, theme_id, 3);

    // Several names per version, including two naming the same version, so what is
    // removed is keyed by the version rather than by one name.
    let aliases = [
        ("first", 1i64),
        ("doomed", 2),
        ("doomed-too", 2),
        ("newest", 3),
    ];
    for (alias_name, version_number) in aliases {
        let (status, body) =
            create_theme_alias(&harness, ACCOUNT, theme_id, alias_name, version_number).await;
        assert_eq!(
            status, 200,
            "case {alias_name:?}: the fixture alias create expected 200, got {status} with \
             {body}"
        );
    }

    let (status, body) = delete_theme(&harness, ACCOUNT, theme_id, Some(2)).await;
    assert_eq!(
        status, 200,
        "deleting version 2 expected 200, got {status} with {body}"
    );

    // Every name for the deleted version is gone; the names for the versions the delete
    // left alone still resolve to them.
    for (alias_name, version_number) in aliases {
        let expected_status = if version_number == 2 { 404 } else { 200 };
        let (status, body) = describe_theme_alias(&harness, ACCOUNT, theme_id, alias_name).await;
        assert_eq!(
            status, expected_status,
            "case {alias_name:?}: a name for version {version_number} expected \
             {expected_status}, got {status} with {body}"
        );
        if status == 200 {
            assert_eq!(
                body["ThemeAlias"]["ThemeVersionNumber"],
                json!(version_number),
                "case {alias_name:?}: the surviving alias must still name its version: {body}"
            );
        }
    }
    let stored_aliases = {
        let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
        let store = state.lock();
        let record = store
            .themes
            .get(&(ACCOUNT.to_string(), theme_id.to_string()))
            .expect("the theme survives a version-scoped delete");
        record.aliases.keys().cloned().collect::<Vec<String>>()
    };
    assert_eq!(
        stored_aliases,
        vec!["first".to_string(), "newest".to_string()],
        "expected only the names for surviving versions to be stored, got {stored_aliases:?}"
    );
}
