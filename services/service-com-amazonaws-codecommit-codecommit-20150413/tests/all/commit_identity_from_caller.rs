// SPDX-License-Identifier: CC0-1.0

//! WHOSE NAME a commit this service writes carries, over the wire: awsJson1_1
//! requests through the router that serves the real emulator (the wire harness and
//! the principal registration come from [`crate::repository_lifecycle`] and
//! [`crate::wire`] rather than a second copy of either).
//!
//! The rule under test: a commit-writing operation whose request omits an identity
//! member records, for both the author and the committer, the NAME of the calling
//! principal — the last segment of that principal's ARN, which is the IAM user name
//! of `.../user/<name>` and the session name of an assumed-role principal — and
//! never the whole ARN, because a git identity line carries a person's name. The
//! member the request DOES carry always wins, and the one operation whose model says
//! its own email "is left blank"
//! (`smithy_query(query="doc",shape="DeleteFileInput",member="email")`) keeps that
//! blank.
//!
//! The table spans what the rule generalises over rather than one recorded call: the
//! shape of the calling principal's ARN (an IAM user whose name is not the access key
//! id, a role session, the long-term principal this emulator publishes), the
//! operation that writes the commit (the three that take an identity under two
//! different member names), whether the request names an identity at all, and the
//! reader the commit is read back through (`GetCommit` and `ListFileCommitHistory`,
//! which embeds the same commit in every revision of a file).

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;

use crate::repository_lifecycle::{base64, emulator_with_engine, string_at};
use crate::wire::{hold_long_term_key_for, ok_as};

/// The branch every row writes on.
const BRANCH: &str = "main";

/// The file a row seeds, then writes again, then deletes.
const SEEDED_PATH: &str = "docs/sub/notes.txt";

/// The identity the SEED commit carries, sent explicitly so the setup of a row never
/// depends on the rule under test.
const SEED_NAME: &str = "Seed Writer";
const SEED_EMAIL: &str = "seed@example.com";

/// How long a seeded session lasts: longer than this suite takes to run, so no row's
/// credential expires while its calls are driven.
const SESSION_LIFETIME: Duration = Duration::from_secs(3600);

/// The account every principal below belongs to.
fn account() -> String {
    engine::emulator_account_id().to_string()
}

/// Which principal a row's calls speak for, and so whose name the commits it writes
/// have to carry.
#[derive(Debug, Clone, Copy)]
enum Principal {
    /// A long-term access key the account holds for an IAM user, as
    /// `iam:CreateAccessKey` records one. The user's name is not the key id, so a row
    /// using it tells the identity's name from the credential's text.
    IamUser { user: &'static str },
    /// The temporary credentials of an assumed role, live in the STS session
    /// registry: the principal is the SESSION, whose name is the last segment of its
    /// ARN rather than the role's.
    RoleSession {
        role: &'static str,
        session: &'static str,
    },
    /// A long-term key no store holds anything for, which speaks for the principal
    /// this emulator publishes its own credential for.
    Published,
}

impl Principal {
    /// The access key id this row signs with.
    fn access_key_id(self) -> &'static str {
        match self {
            Self::IamUser { .. } => "AKIDIAMUSERCOMMITS",
            Self::RoleSession { .. } => "AKIDROLESESSION",
            Self::Published => "AKIDPUBLISHED",
        }
    }

    /// The ARN of the principal the credential speaks for — what this service records
    /// for everything a caller creates, and what a commit must NOT be attributed to.
    fn arn(self) -> String {
        match self {
            Self::IamUser { user } => format!("arn:aws:iam::{}:user/{user}", account()),
            Self::RoleSession { role, session } => {
                format!("arn:aws:sts::{}:assumed-role/{role}/{session}", account())
            }
            Self::Published => sts::ambient_identity().arn,
        }
    }

    /// The name a commit written by this principal with no identity named carries,
    /// stated per row rather than derived, so the row asserts a value and not a
    /// second copy of the service's own derivation.
    fn expected_name(self) -> &'static str {
        match self {
            Self::IamUser { user } => user,
            Self::RoleSession { session, .. } => session,
            // The published principal is the IAM user
            // `::service_common::caller_principal::AMBIENT_USER_NAME`.
            Self::Published => "emulator",
        }
    }

    /// Record what the account holds for this credential, the way the operation that
    /// mints one records it: an IAM access-key record for a long-term key, a session
    /// in the STS registry for temporary credentials, and nothing at all for a key no
    /// store holds.
    fn register(self, engine: &engine::InMemoryEngine) {
        let access_key_id = self.access_key_id();
        match self {
            Self::Published => {}
            Self::IamUser { user } => {
                hold_long_term_key_for(engine, user, access_key_id);
            }
            Self::RoleSession { role, session } => {
                let role_arn = format!("arn:aws:iam::{}:role/{role}", account());
                let identity = sts::Identity {
                    user_id: format!("{}:{session}", sts::role_id(&role_arn)),
                    account: account(),
                    arn: self.arn(),
                };
                let vend = sts::NewSession {
                    role_arn: Some(role_arn),
                    ..sts::NewSession::new(
                        identity,
                        sts::SessionKind::AssumedRole,
                        SESSION_LIFETIME.as_secs() as i32,
                    )
                };
                let registry = sts::service_state(engine)
                    .expect("the in-memory engine carries the STS store");
                registry
                    .lock()
                    .insert(vend.into_session(access_key_id, SystemTime::now()))
                    .expect("the session registry has room for one session");
            }
        }
    }
}

/// Every principal shape a row is driven as.
fn principals() -> Vec<Principal> {
    vec![
        Principal::IamUser { user: "Mary_Major" },
        Principal::RoleSession {
            role: "CodeCommitRelease",
            session: "release-42",
        },
        Principal::Published,
    ]
}

/// The operations that write a commit and take an identity for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Writer {
    PutFile,
    CreateCommit,
    DeleteFile,
}

impl Writer {
    fn operation(self) -> &'static str {
        match self {
            Self::PutFile => "PutFile",
            Self::CreateCommit => "CreateCommit",
            Self::DeleteFile => "DeleteFile",
        }
    }

    /// `PutFile` and `DeleteFile` take the author's name as `name`; `CreateCommit`
    /// takes it as `authorName`.
    fn name_member(self) -> &'static str {
        match self {
            Self::CreateCommit => "authorName",
            _ => "name",
        }
    }

    /// Does the commit this operation writes hold the path at all? A deletion
    /// removes it, so that commit is no REVISION of the file and the file's history
    /// does not list it.
    fn revises_the_path(self) -> bool {
        self != Self::DeleteFile
    }

    /// The email a commit this operation writes carries when the request sends none:
    /// the calling principal's name, except on the one operation whose model says its
    /// own email member is left blank.
    fn default_email(self, caller_name: &str) -> String {
        match self {
            Self::DeleteFile => String::new(),
            _ => caller_name.to_string(),
        }
    }

    /// The request body this writer sends, on the branch's tip `parent`, with no
    /// identity members in it.
    fn body(self, repository: &str, parent: &str) -> Value {
        match self {
            Self::PutFile => json!({
                "repositoryName": repository,
                "branchName": BRANCH,
                "filePath": SEEDED_PATH,
                "fileContent": base64(b"a revision written with no identity\n"),
                "parentCommitId": parent,
            }),
            Self::CreateCommit => json!({
                "repositoryName": repository,
                "branchName": BRANCH,
                "parentCommitId": parent,
                "putFiles": [
                    { "filePath": SEEDED_PATH, "fileContent": base64(b"a commit with no identity\n") },
                ],
            }),
            Self::DeleteFile => json!({
                "repositoryName": repository,
                "branchName": BRANCH,
                "filePath": SEEDED_PATH,
                "parentCommitId": parent,
            }),
        }
    }
}

fn writers() -> Vec<Writer> {
    vec![Writer::PutFile, Writer::CreateCommit, Writer::DeleteFile]
}

/// Create `repository` and seed one commit on `BRANCH` with an explicit identity,
/// answering the tip it left.
async fn seeded(
    router: &MultiServiceRouter,
    access_key_id: &str,
    repository: &str,
) -> String {
    ok_as(
        router,
        access_key_id,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;
    let written = ok_as(
        router,
        access_key_id,
        "PutFile",
        &json!({
            "repositoryName": repository,
            "branchName": BRANCH,
            "filePath": SEEDED_PATH,
            "fileContent": base64(b"the seeded version\n"),
            "commitMessage": "seed",
            "name": SEED_NAME,
            "email": SEED_EMAIL,
        }),
    )
    .await;
    string_at(&written, "/commitId")
}

/// Assert the identity members of the commit `commit` describes, for both the author
/// and the committer.
fn assert_identity(commit: &Value, case: &str, reader: &str, name: &str, email: &str, arn: &str) {
    for role in ["author", "committer"] {
        let actual_name = string_at(commit, &format!("/{role}/name"));
        assert_eq!(
            actual_name, name,
            "case {case:?} through {reader}: expected {role} name {name:?}, got \
             {actual_name:?} in {commit}"
        );
        let actual_email = string_at(commit, &format!("/{role}/email"));
        assert_eq!(
            actual_email, email,
            "case {case:?} through {reader}: expected {role} email {email:?}, got \
             {actual_email:?} in {commit}"
        );
        // The same assertion stated as the property it protects: a git identity field
        // is a name, so neither of them is the principal's ARN.
        assert_ne!(
            actual_name, arn,
            "case {case:?} through {reader}: the {role} name must not be the caller's \
             ARN, got {actual_name:?}"
        );
        assert_ne!(
            actual_email, arn,
            "case {case:?} through {reader}: the {role} email must not be the caller's \
             ARN, got {actual_email:?}"
        );
    }
}

/// The commit `commit_id` as `GetCommit` reports it.
async fn fetched(
    router: &MultiServiceRouter,
    access_key_id: &str,
    repository: &str,
    commit_id: &str,
) -> Value {
    let payload = ok_as(
        router,
        access_key_id,
        "GetCommit",
        &json!({ "repositoryName": repository, "commitId": commit_id }),
    )
    .await;
    payload
        .get("commit")
        .cloned()
        .unwrap_or_else(|| panic!("expected a commit in {payload}"))
}

/// The commit `ListFileCommitHistory` embeds for the newest revision of
/// `SEEDED_PATH`, or `None` when the path has no revision there (a deletion removes
/// the file, so its history is read at the commit that deleted it).
async fn newest_revision(
    router: &MultiServiceRouter,
    access_key_id: &str,
    repository: &str,
    commit_specifier: &str,
) -> Option<Value> {
    let payload = ok_as(
        router,
        access_key_id,
        "ListFileCommitHistory",
        &json!({
            "repositoryName": repository,
            "filePath": SEEDED_PATH,
            "commitSpecifier": commit_specifier,
        }),
    )
    .await;
    payload
        .get("revisionDag")
        .and_then(Value::as_array)
        .and_then(|dag| dag.first())
        .and_then(|entry| entry.get("commit"))
        .cloned()
}

#[tokio::test]
async fn a_commit_written_with_no_identity_carries_the_callers_name() {
    let (router, engine) = emulator_with_engine();
    for principal in principals() {
        principal.register(&engine);
    }

    for (index, principal) in principals().into_iter().enumerate() {
        let access_key_id = principal.access_key_id();
        let arn = principal.arn();
        let expected_name = principal.expected_name();

        for (variant, writer) in writers().into_iter().enumerate() {
            let case = format!("{} as {arn}", writer.operation());
            let repository = format!("commit-identity-{index}-{variant}");
            let parent = seeded(&router, access_key_id, &repository).await;

            let written = ok_as(
                &router,
                access_key_id,
                writer.operation(),
                &writer.body(&repository, &parent),
            )
            .await;
            let commit_id = string_at(&written, "/commitId");
            let expected_email = writer.default_email(expected_name);

            let commit = fetched(&router, access_key_id, &repository, &commit_id).await;
            assert_identity(
                &commit,
                &case,
                "GetCommit",
                expected_name,
                &expected_email,
                &arn,
            );

            // The reader the history is read through reports the very same commit, so
            // the identity is the stored one rather than one a reader composes.
            if writer.revises_the_path() {
                let embedded = newest_revision(&router, access_key_id, &repository, &commit_id)
                    .await
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: expected a revision of {SEEDED_PATH} at {commit_id}")
                    });
                assert_eq!(
                    string_at(&embedded, "/commitId"),
                    commit_id,
                    "case {case:?}: the newest revision is the commit just written, got {embedded}"
                );
                assert_identity(
                    &embedded,
                    &case,
                    "ListFileCommitHistory",
                    expected_name,
                    &expected_email,
                    &arn,
                );
            }
        }
    }
}

#[tokio::test]
async fn an_identity_the_request_names_is_the_one_recorded() {
    let (router, engine) = emulator_with_engine();
    for principal in principals() {
        principal.register(&engine);
    }

    // The identity the request carries, which no caller's name may displace.
    let sent_name = "Ada Lovelace";
    let sent_email = "ada@example.com";

    for (index, principal) in principals().into_iter().enumerate() {
        let access_key_id = principal.access_key_id();
        let arn = principal.arn();

        for (variant, writer) in writers().into_iter().enumerate() {
            let case = format!("{} as {arn} naming an identity", writer.operation());
            let repository = format!("named-identity-{index}-{variant}");
            let parent = seeded(&router, access_key_id, &repository).await;

            let mut body = writer.body(&repository, &parent);
            body[writer.name_member()] = json!(sent_name);
            body["email"] = json!(sent_email);
            let written = ok_as(&router, access_key_id, writer.operation(), &body).await;
            let commit_id = string_at(&written, "/commitId");

            let commit = fetched(&router, access_key_id, &repository, &commit_id).await;
            assert_identity(&commit, &case, "GetCommit", sent_name, sent_email, &arn);
        }
    }
}
