// SPDX-License-Identifier: CC0-1.0

//! The DIGEST of a revision that carries its content inline, computed where that
//! revision is recorded.
//!
//! Both inline revision forms carry the digest beside the content: `AppSpecContent`
//! has `content` and `sha256`, "the SHA256 hash value of the revision content"
//! (`smithy_query(query="shape",shape="AppSpecContent")`), and the deprecated
//! `RawString` the `string` form uses has the same pair
//! (`smithy_query(query="shape",shape="RawString")`). The digest is a function of
//! the content, so it is not something a client has to send and not something a
//! reader can leave out: CodeDeploy computes it when the revision is recorded and
//! every read of that revision reports it, which is what lets a client tell two
//! revisions apart — or recognise one it already has — without parsing the AppSpec
//! again.
//!
//! So this module is where an inline revision is NORMALISED: every write of a
//! revision into this store goes through [`with_content_digest`]
//! ([`super::ComAmazonawsCodedeployCodedeploy20141006State::record_deployment`] for
//! a deployment's own revision, [`super::Application`]'s registry for an
//! application's), and every reader — `GetDeployment`, `BatchGetDeployments`,
//! `GetApplicationRevision`, `BatchGetApplicationRevisions`,
//! `ListApplicationRevisions` and the `targetRevision` of a deployment group —
//! answers from those records, so one rule fills the member everywhere.
//!
//! The digest is over the content and nothing else, which is also what identifies
//! an inline revision in this store ([`super::revision_key`]), so two revisions
//! carrying the same AppSpec report the same digest however they were sent and two
//! carrying different AppSpecs never do.

use crate::ops::types::RevisionLocation;

/// `revision` with the digest of its inline content filled in.
///
/// The S3 and GitHub forms name an artifact this service does not hold, so they
/// have nothing to digest and are answered back unchanged; an inline form carrying
/// no content at all has nothing to digest either. A digest the CLIENT sent is kept
/// as it sent it — `sha256` is an input member of both inline forms, so a revision
/// that states its own digest round-trips exactly as it was supplied — and it is
/// the revision that states none, which is every revision an AWS SDK or the AWS CLI
/// sends, that is given the digest of the content it carries.
#[must_use]
pub fn with_content_digest(mut revision: RevisionLocation) -> RevisionLocation {
    if let Some(app_spec) = revision.app_spec_content.as_mut() {
        if stated(app_spec.sha256.as_deref()).is_none() {
            app_spec.sha256 = app_spec.content.as_deref().map(super::content_digest);
        }
    }
    if let Some(raw) = revision.string.as_mut() {
        if stated(raw.sha256.as_deref()).is_none() {
            raw.sha256 = raw.content.as_deref().map(super::content_digest);
        }
    }
    revision
}

/// `digest` when the request STATED one: a member that is absent, or carries only
/// whitespace, states nothing a reader could report.
fn stated(digest: Option<&str>) -> Option<&str> {
    digest.map(str::trim).filter(|digest| !digest.is_empty())
}

#[cfg(test)]
mod tests {
    //! What [`with_content_digest`] fills in, over the revision forms the model
    //! holds. The same rule is asserted end to end, through the operations a client
    //! calls, in `tests/all/revision_content_digest.rs`.

    use super::*;
    use crate::ops::types::{AppSpecContent, GitHubLocation, RawString, S3Location};

    /// The hex SHA-256 of `content`, computed here the way a client computes one:
    /// over the bytes of the content it sent.
    fn digest_of(content: &str) -> String {
        use ::sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        ::hex::encode(hasher.finalize())
    }

    #[test]
    fn an_inline_revision_carries_the_digest_of_its_content() {
        // Rows differ along what the revision carries: each inline form, each with
        // a different content, one whose content is empty, one that carries a
        // digest of its own, and the forms that carry no content at all.
        let cases: &[(&str, RevisionLocation, Option<String>)] = &[
            (
                "an AppSpecContent revision",
                RevisionLocation {
                    app_spec_content: Some(AppSpecContent {
                        content: Some("version: 0.0\n".to_string()),
                        sha256: None,
                    }),
                    ..Default::default()
                },
                Some(digest_of("version: 0.0\n")),
            ),
            (
                "an AppSpecContent revision carrying other content",
                RevisionLocation {
                    app_spec_content: Some(AppSpecContent {
                        content: Some("version: 0.0\nResources: []\n".to_string()),
                        sha256: None,
                    }),
                    ..Default::default()
                },
                Some(digest_of("version: 0.0\nResources: []\n")),
            ),
            (
                "an AppSpecContent revision whose content is empty",
                RevisionLocation {
                    app_spec_content: Some(AppSpecContent {
                        content: Some(String::new()),
                        sha256: None,
                    }),
                    ..Default::default()
                },
                Some(digest_of("")),
            ),
            (
                "an AppSpecContent revision that sent a digest of its own",
                RevisionLocation {
                    app_spec_content: Some(AppSpecContent {
                        content: Some("version: 0.0\n".to_string()),
                        sha256: Some("a-digest-the-client-sent".to_string()),
                    }),
                    ..Default::default()
                },
                Some("a-digest-the-client-sent".to_string()),
            ),
            (
                "an AppSpecContent revision whose digest member is blank",
                RevisionLocation {
                    app_spec_content: Some(AppSpecContent {
                        content: Some("version: 0.0\n".to_string()),
                        sha256: Some("  ".to_string()),
                    }),
                    ..Default::default()
                },
                Some(digest_of("version: 0.0\n")),
            ),
            (
                "an AppSpecContent revision with no content",
                RevisionLocation {
                    app_spec_content: Some(AppSpecContent::default()),
                    ..Default::default()
                },
                None,
            ),
        ];

        for (case, revision, expected) in cases {
            let digested = with_content_digest(revision.clone());
            let answered = digested
                .app_spec_content
                .as_ref()
                .and_then(|content| content.sha256.clone());
            assert_eq!(
                &answered, expected,
                "case {case:?}: expected the sha256 {expected:?}, got {answered:?}"
            );
        }
    }

    #[test]
    fn the_string_form_carries_the_same_digest_as_the_appspec_form() {
        let content = "version: 0.0\nResources:\n  - web: {}\n";
        let raw = with_content_digest(RevisionLocation {
            string: Some(RawString {
                content: Some(content.to_string()),
                sha256: None,
            }),
            ..Default::default()
        });
        let app_spec = with_content_digest(RevisionLocation {
            app_spec_content: Some(AppSpecContent {
                content: Some(content.to_string()),
                sha256: None,
            }),
            ..Default::default()
        });
        let answered = raw.string.and_then(|raw| raw.sha256);
        let expected = app_spec.app_spec_content.and_then(|content| content.sha256);
        assert_eq!(
            answered, expected,
            "case the two inline forms: one content has one digest, expected {expected:?}, got \
             {answered:?}"
        );
        assert_eq!(
            answered,
            Some(digest_of(content)),
            "case the two inline forms: expected the digest of the content, got {answered:?}"
        );
    }

    #[test]
    fn a_revision_that_names_an_artifact_is_unchanged() {
        // Rows differ along the form that carries no inline content: the two
        // locations the model holds for an artifact elsewhere.
        let cases: &[(&str, RevisionLocation)] = &[
            (
                "an S3 revision",
                RevisionLocation {
                    revision_type: Some("S3".to_string()),
                    s3_location: Some(S3Location {
                        bucket: Some("artifacts".to_string()),
                        key: Some("app.zip".to_string()),
                        bundle_type: Some("zip".to_string()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            ),
            (
                "a GitHub revision",
                RevisionLocation {
                    revision_type: Some("GitHub".to_string()),
                    git_hub_location: Some(GitHubLocation {
                        repository: Some("acct/repo".to_string()),
                        commit_id: Some("0123456789abcdef".to_string()),
                    }),
                    ..Default::default()
                },
            ),
        ];

        for (case, revision) in cases {
            let digested = with_content_digest(revision.clone());
            assert!(
                digested.app_spec_content.is_none() && digested.string.is_none(),
                "case {case:?}: expected no inline content to appear, got {digested:?}"
            );
        }
    }
}
