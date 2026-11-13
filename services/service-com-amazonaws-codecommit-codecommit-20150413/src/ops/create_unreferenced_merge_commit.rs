// SPDX-License-Identifier: CC0-1.0

//! @op CreateUnreferencedMergeCommit
//!
//! Merges a source commit into a destination commit and writes the result as a
//! commit that NO reference names: the merge of the two trees is computed by
//! [`crate::state::merge`] — the one reading of a CodeCommit merge this crate has —
//! the blobs and the tree and commit objects it needs are written into the
//! repository's object store, and the commit's id and tree id are reported.
//!
//! Nothing else moves: no branch is created or advanced, so the commit "can only be
//! accessed using the GetCommit API or through git commands such as git fetch". It
//! is recorded as a retained unreferenced commit
//! ([`crate::state::Repository::retain_unreferenced`]) so the id this call hands
//! back stays readable instead of being released as unreachable by the next write's
//! garbage collection.
//!
//! The merge option decides the commit's PARENTS, not its tree: `THREE_WAY_MERGE`
//! records both tips (the destination first, the side being merged into), while
//! `SQUASH_MERGE` collapses the source history and records the destination tip
//! alone — so the two options over one pair of commits produce the same `treeId`
//! and different `commitId`s. `FAST_FORWARD_MERGE` is refused: "This API cannot be
//! used with the fast-forward merge strategy because that strategy does not create
//! a merge commit."
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateUnreferencedMergeCommit`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-codecommit-codecommit-20150413/smithy-model/com-amazonaws-codecommit-codecommit-20150413.json")
//! ```
//!
//! Creates an unreferenced commit that represents the result of merging two branches using
//! a specified merge strategy. This can help you determine the outcome of a potential
//! merge. This API cannot be used with the fast-forward merge strategy because that
//! strategy does not create a merge commit. This unreferenced merge commit can only be
//! accessed using the GetCommit API or through git commands such as git fetch. To retrieve
//! this commit, you must specify its commit ID or otherwise reference it.
//!
//! ## Input
//!
//! * `authorName` (Name) — The name of the author who created the unreferenced commit. This
//!   information is used as both the author and committer for the commit.
//! * `commitMessage` (Message) — The commit message for the unreferenced commit.
//! * `conflictDetailLevel` (ConflictDetailLevelTypeEnum, one of: FILE_LEVEL | LINE_LEVEL) —
//!   The level of conflict detail to use. If unspecified, the default FILE_LEVEL is used,
//!   which returns a not-mergeable result if the same file has differences in both branches.
//!   If LINE_LEVEL is specified, a conflict is considered not mergeable if the same file in
//!   both branches has differences on the same line.
//! * `conflictResolution` (ConflictResolution) — If AUTOMERGE is the conflict resolution
//!   strategy, a list of inputs to use when resolving conflicts during a merge.
//! * `conflictResolutionStrategy` (ConflictResolutionStrategyTypeEnum, one of:
//!   ACCEPT_DESTINATION | ACCEPT_SOURCE | AUTOMERGE | NONE) — Specifies which branch to use
//!   when resolving conflicts, or whether to attempt automatically merging two versions of a
//!   file. The default is NONE, which requires any conflicts to be resolved manually before
//!   the merge operation is successful.
//! * `destinationCommitSpecifier` (CommitName) **required** — The branch, tag, HEAD, or other
//!   fully qualified reference used to identify a commit (for example, a branch name or a
//!   full commit ID).
//! * `email` (Email) — The email address for the person who created the unreferenced commit.
//! * `keepEmptyFolders` (KeepEmptyFolders, default false) **required** — If the commit
//!   contains deletions, whether to keep a folder or folder structure if the changes leave
//!   the folders empty. If this is specified as true, a .gitkeep file is created for empty
//!   folders. The default is false.
//! * `mergeOption` (MergeOptionTypeEnum, one of: FAST_FORWARD_MERGE | SQUASH_MERGE |
//!   THREE_WAY_MERGE) **required** — The merge option or strategy you want to use to merge
//!   the code.
//! * `repositoryName` (RepositoryName, len 1..100, pattern ^[\w\.-]+$) **required** — The
//!   name of the repository where you want to create the unreferenced merge commit.
//! * `sourceCommitSpecifier` (CommitName) **required** — The branch, tag, HEAD, or other
//!   fully qualified reference used to identify a commit (for example, a branch name or a
//!   full commit ID).
//!
//! ## Output
//!
//! * `commitId` (ObjectId) — The full commit ID of the commit that contains your merge
//!   results.
//! * `treeId` (ObjectId) — The full SHA-1 pointer of the tree information for the commit that
//!   contains the merge results.
//!
//! ## Errors
//!
//! * `CommitDoesNotExistException` (HTTP 400) — The specified commit does not exist or no
//!   commit was specified, and the specified repository has no default branch.
//! * `CommitMessageLengthExceededException` (HTTP 400) — The commit message is too long.
//!   Provide a shorter string.
//! * `CommitRequiredException` (HTTP 400) — A commit was not specified.
//! * `ConcurrentReferenceUpdateException` (HTTP 400) — The merge cannot be completed because
//!   the target branch has been modified. Another user might have modified the target branch
//!   while the merge was in progress. Wait a few minutes, and then try again.
//! * `EncryptionIntegrityChecksFailedException` (HTTP 500) — An encryption integrity check
//!   failed.
//! * `EncryptionKeyAccessDeniedException` (HTTP 400) — An encryption key could not be
//!   accessed.
//! * `EncryptionKeyDisabledException` (HTTP 400) — The encryption key is disabled.
//! * `EncryptionKeyNotFoundException` (HTTP 400) — No encryption key was found.
//! * `EncryptionKeyUnavailableException` (HTTP 400) — The encryption key is not available.
//! * `FileContentSizeLimitExceededException` (HTTP 400) — The file cannot be added because it
//!   is too large. The maximum file size is 6 MB, and the combined file content change size
//!   is 7 MB. Consider making these changes using a Git client.
//! * `FileModeRequiredException` (HTTP 400) — The commit cannot be created because no file
//!   mode has been specified. A file mode is required to update mode permissions for a file.
//! * `FolderContentSizeLimitExceededException` (HTTP 400) — The commit cannot be created
//!   because at least one of the overall changes in the commit results in a folder whose
//!   contents exceed the limit of 6 MB. Either reduce the number and size of your changes, or
//!   split the changes across multiple folders.
//! * `InvalidCommitException` (HTTP 400) — The specified commit is not valid.
//! * `InvalidConflictDetailLevelException` (HTTP 400) — The specified conflict detail level
//!   is not valid.
//! * `InvalidConflictResolutionException` (HTTP 400) — The specified conflict resolution list
//!   is not valid.
//! * `InvalidConflictResolutionStrategyException` (HTTP 400) — The specified conflict
//!   resolution strategy is not valid.
//! * `InvalidEmailException` (HTTP 400) — The specified email address either contains one or
//!   more characters that are not allowed, or it exceeds the maximum number of characters
//!   allowed for an email address.
//! * `InvalidFileModeException` (HTTP 400) — The specified file mode permission is not valid.
//!   For a list of valid file mode permissions, see PutFile.
//! * `InvalidMergeOptionException` (HTTP 400) — The specified merge option is not valid for
//!   this operation. Not all merge strategies are supported for all operations.
//! * `InvalidPathException` (HTTP 400) — The specified path is not valid.
//! * `InvalidReplacementContentException` (HTTP 400) — Automerge was specified for resolving
//!   the conflict, but the replacement type is not valid or content is missing.
//! * `InvalidReplacementTypeException` (HTTP 400) — Automerge was specified for resolving the
//!   conflict, but the specified replacement type is not valid.
//! * `InvalidRepositoryNameException` (HTTP 400) — A specified repository name is not valid.
//!   This exception occurs only when a specified repository name is not valid. Other
//!   exceptions occur when a required repository parameter is missing, or when a specified
//!   repository does not exist.
//! * `ManualMergeRequiredException` (HTTP 400) — The pull request cannot be merged
//!   automatically into the destination branch. You must manually merge the branches and
//!   resolve any conflicts.
//! * `MaximumConflictResolutionEntriesExceededException` (HTTP 400) — The number of allowed
//!   conflict resolution entries was exceeded.
//! * `MaximumFileContentToLoadExceededException` (HTTP 400) — The number of files to load
//!   exceeds the allowed limit.
//! * `MaximumItemsToCompareExceededException` (HTTP 400) — The number of items to compare
//!   between the source or destination branches and the merge base has exceeded the maximum
//!   allowed.
//! * `MergeOptionRequiredException` (HTTP 400) — A merge option or stategy is required, and
//!   none was provided.
//! * `MultipleConflictResolutionEntriesException` (HTTP 400) — More than one conflict
//!   resolution entries exists for the conflict. A conflict can have only one conflict
//!   resolution entry.
//! * `NameLengthExceededException` (HTTP 400) — The user name is not valid because it has
//!   exceeded the character limit for author names.
//! * `PathRequiredException` (HTTP 400) — The folderPath for a location cannot be null.
//! * `ReplacementContentRequiredException` (HTTP 400) — USE_NEW_CONTENT was specified, but no
//!   replacement content has been provided.
//! * `ReplacementTypeRequiredException` (HTTP 400) — A replacement type is required.
//! * `RepositoryDoesNotExistException` (HTTP 400) — The specified repository does not exist.
//! * `RepositoryNameRequiredException` (HTTP 400) — A repository name is required, but was
//!   not specified.
//! * `TipsDivergenceExceededException` (HTTP 400) — The divergence between the tips of the
//!   provided commit specifiers is too great to determine whether there might be any merge
//!   conflicts. Locally compare the specifiers using git diff or a diff tool.

#[allow(unused_imports)]
use super::types::*;

use std::collections::BTreeMap;

use crate::state::merge;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateUnreferencedMergeCommitError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-codecommit-codecommit-20150413.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateUnreferencedMergeCommitError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.codecommit#CreateUnreferencedMergeCommit")]
#[derive(Debug)]
pub enum CreateUnreferencedMergeCommitErrorModeled {
    CommitDoesNotExistException(CommitDoesNotExistException),
    CommitMessageLengthExceededException(CommitMessageLengthExceededException),
    CommitRequiredException(CommitRequiredException),
    ConcurrentReferenceUpdateException(ConcurrentReferenceUpdateException),
    EncryptionIntegrityChecksFailedException(EncryptionIntegrityChecksFailedException),
    EncryptionKeyAccessDeniedException(EncryptionKeyAccessDeniedException),
    EncryptionKeyDisabledException(EncryptionKeyDisabledException),
    EncryptionKeyNotFoundException(EncryptionKeyNotFoundException),
    EncryptionKeyUnavailableException(EncryptionKeyUnavailableException),
    FileContentSizeLimitExceededException(FileContentSizeLimitExceededException),
    FileModeRequiredException(FileModeRequiredException),
    FolderContentSizeLimitExceededException(FolderContentSizeLimitExceededException),
    InvalidCommitException(InvalidCommitException),
    InvalidConflictDetailLevelException(InvalidConflictDetailLevelException),
    InvalidConflictResolutionException(InvalidConflictResolutionException),
    InvalidConflictResolutionStrategyException(InvalidConflictResolutionStrategyException),
    InvalidEmailException(InvalidEmailException),
    InvalidFileModeException(InvalidFileModeException),
    InvalidMergeOptionException(InvalidMergeOptionException),
    InvalidPathException(InvalidPathException),
    InvalidReplacementContentException(InvalidReplacementContentException),
    InvalidReplacementTypeException(InvalidReplacementTypeException),
    InvalidRepositoryNameException(InvalidRepositoryNameException),
    ManualMergeRequiredException(ManualMergeRequiredException),
    MaximumConflictResolutionEntriesExceededException(MaximumConflictResolutionEntriesExceededException),
    MaximumFileContentToLoadExceededException(MaximumFileContentToLoadExceededException),
    MaximumItemsToCompareExceededException(MaximumItemsToCompareExceededException),
    MergeOptionRequiredException(MergeOptionRequiredException),
    MultipleConflictResolutionEntriesException(MultipleConflictResolutionEntriesException),
    NameLengthExceededException(NameLengthExceededException),
    PathRequiredException(PathRequiredException),
    ReplacementContentRequiredException(ReplacementContentRequiredException),
    ReplacementTypeRequiredException(ReplacementTypeRequiredException),
    RepositoryDoesNotExistException(RepositoryDoesNotExistException),
    RepositoryNameRequiredException(RepositoryNameRequiredException),
    TipsDivergenceExceededException(TipsDivergenceExceededException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_unreferenced_merge_commit::CreateUnreferencedMergeCommitError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateUnreferencedMergeCommitError = crate::scaffolded::Scaffolded<CreateUnreferencedMergeCommitErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateUnreferencedMergeCommitErrorModeled> for CreateUnreferencedMergeCommitError {
    fn from(modeled: CreateUnreferencedMergeCommitErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// One `conflictResolution` problem as the errors THIS operation models for it,
/// each with that shape's own sentence. The reading of what is wrong with a
/// resolution is shared ([`merge::CheckedResolution`]); which modelled failure
/// answers the caller is the operation's, because the operations that accept a
/// resolution do not all model the same set.
fn refused_resolution(problem: merge::ResolutionProblem) -> CreateUnreferencedMergeCommitError {
    use merge::ResolutionProblem as Problem;
    let modeled = match problem {
        Problem::PathRequired => {
            CreateUnreferencedMergeCommitErrorModeled::PathRequiredException(
                PathRequiredException {
                    message: Some(
                        "a file path is required for every conflict resolution entry".to_string(),
                    ),
                },
            )
        }
        Problem::InvalidPath(raw) => CreateUnreferencedMergeCommitErrorModeled::InvalidPathException(
            InvalidPathException {
                message: Some(format!("the path {raw} is not valid")),
            },
        ),
        Problem::FileModeRequired => {
            CreateUnreferencedMergeCommitErrorModeled::FileModeRequiredException(
                FileModeRequiredException {
                    message: Some(
                        "a file mode is required to update mode permissions for a file".to_string(),
                    ),
                },
            )
        }
        Problem::InvalidFileMode(mode) => {
            CreateUnreferencedMergeCommitErrorModeled::InvalidFileModeException(
                InvalidFileModeException {
                    message: Some(format!("the file mode {mode} is not valid")),
                },
            )
        }
        Problem::TooManyEntries(entries) => {
            CreateUnreferencedMergeCommitErrorModeled::MaximumConflictResolutionEntriesExceededException(
                MaximumConflictResolutionEntriesExceededException {
                    message: Some(format!(
                        "the merge carries {entries} conflict resolution entries, more than the \
                         {} this emulator accepts",
                        merge::MAX_CONFLICT_RESOLUTION_ENTRIES
                    )),
                },
            )
        }
        Problem::MultipleEntries(path) => {
            CreateUnreferencedMergeCommitErrorModeled::MultipleConflictResolutionEntriesException(
                MultipleConflictResolutionEntriesException {
                    message: Some(format!(
                        "more than one conflict resolution entry was provided for {path}"
                    )),
                },
            )
        }
        Problem::ReplacementTypeRequired(path) => {
            CreateUnreferencedMergeCommitErrorModeled::ReplacementTypeRequiredException(
                ReplacementTypeRequiredException {
                    message: Some(format!(
                        "a replacement type is required for the conflict resolution entry \
                         for {path}"
                    )),
                },
            )
        }
        Problem::InvalidReplacementType(replacement_type) => {
            CreateUnreferencedMergeCommitErrorModeled::InvalidReplacementTypeException(
                InvalidReplacementTypeException {
                    message: Some(format!(
                        "the replacement type {replacement_type} is not valid"
                    )),
                },
            )
        }
        Problem::ReplacementContentRequired(path) => {
            CreateUnreferencedMergeCommitErrorModeled::ReplacementContentRequiredException(
                ReplacementContentRequiredException {
                    message: Some(format!(
                        "{} was specified for {path} but no replacement content was provided",
                        merge::REPLACEMENT_USE_NEW_CONTENT
                    )),
                },
            )
        }
        Problem::InvalidReplacementContent {
            path,
            replacement_type,
        } => CreateUnreferencedMergeCommitErrorModeled::InvalidReplacementContentException(
            InvalidReplacementContentException {
                message: Some(format!(
                    "replacement content was provided for {path} with the replacement \
                     type {replacement_type}, which keeps a stored version"
                )),
            },
        ),
        Problem::PathNotInResult(path) => {
            CreateUnreferencedMergeCommitErrorModeled::InvalidPathException(InvalidPathException {
                message: Some(format!(
                    "the merge result does not hold the path {path}, so its \
                     file mode cannot be set"
                )),
            })
        }
    };
    modeled.into()
}

/// The `CreateUnreferencedMergeCommit` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateUnreferencedMergeCommit<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateUnreferencedMergeCommit<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateUnreferencedMergeCommit<Eng>
{
    type Input = CreateUnreferencedMergeCommitInput;
    type Output = CreateUnreferencedMergeCommitOutput;
    type Error = CreateUnreferencedMergeCommitError;

    const NAME: &'static str = "CreateUnreferencedMergeCommit";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_UNREFERENCED_MERGE_COMMIT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_UNREFERENCED_MERGE_COMMIT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {

        // The merge option decides what this call writes, so it is read first.
        // `MergeOptionTypeEnum` is enforced at the request boundary; the value the
        // enum declares that this API cannot write a commit for is refused here:
        // "This API cannot be used with the fast-forward merge strategy because
        // that strategy does not create a merge commit."
        let merge_option = input.merge_option.as_str();
        if merge_option.is_empty() {
            return Err(
                CreateUnreferencedMergeCommitErrorModeled::MergeOptionRequiredException(
                    MergeOptionRequiredException {
                        message: Some(
                            "a merge option or strategy is required, and none was provided"
                                .to_string(),
                        ),
                    },
                )
                .into(),
            );
        }
        // The fast-forward strategy is the value the enum declares that this API
        // cannot write a commit for, and it is named in the refusal; anything else
        // outside the two options that DO write one is refused the same way (the
        // enum itself is enforced at the request boundary).
        if merge_option == merge::MERGE_OPTION_FAST_FORWARD {
            return Err(
                CreateUnreferencedMergeCommitErrorModeled::InvalidMergeOptionException(
                    InvalidMergeOptionException {
                        message: Some(format!(
                            "the merge option {} cannot be used with this operation because that \
                             strategy does not create a merge commit",
                            merge::MERGE_OPTION_FAST_FORWARD
                        )),
                    },
                )
                .into(),
            );
        }
        if merge_option != merge::MERGE_OPTION_THREE_WAY && merge_option != merge::MERGE_OPTION_SQUASH
        {
            return Err(
                CreateUnreferencedMergeCommitErrorModeled::InvalidMergeOptionException(
                    InvalidMergeOptionException {
                        message: Some(format!(
                            "the merge option {merge_option} is not valid for this operation"
                        )),
                    },
                )
                .into(),
            );
        }

        // Both specifiers are required members, so only an EMPTY one reaches here:
        // a request that named no commit.
        if input.source_commit_specifier.is_empty() || input.destination_commit_specifier.is_empty()
        {
            return Err(
                CreateUnreferencedMergeCommitErrorModeled::CommitRequiredException(
                    CommitRequiredException {
                        message: Some(
                            "a source commit specifier and a destination commit specifier are \
                             required"
                                .to_string(),
                        ),
                    },
                )
                .into(),
            );
        }

        // "If unspecified, the default FILE_LEVEL is used" and the default
        // strategy "is NONE, which requires any conflicts to be resolved manually".
        let detail_level = input
            .conflict_detail_level
            .as_deref()
            .unwrap_or(merge::CONFLICT_DETAIL_FILE_LEVEL);
        if !merge::CONFLICT_DETAIL_LEVELS.contains(&detail_level) {
            return Err(
                CreateUnreferencedMergeCommitErrorModeled::InvalidConflictDetailLevelException(
                    InvalidConflictDetailLevelException {
                        message: Some(format!(
                            "the conflict detail level {detail_level} is not valid"
                        )),
                    },
                )
                .into(),
            );
        }
        let strategy = input
            .conflict_resolution_strategy
            .as_deref()
            .unwrap_or(merge::CONFLICT_RESOLUTION_NONE);
        if !merge::CONFLICT_RESOLUTION_STRATEGIES.contains(&strategy) {
            return Err(
                CreateUnreferencedMergeCommitErrorModeled::InvalidConflictResolutionStrategyException(
                    InvalidConflictResolutionStrategyException {
                        message: Some(format!(
                            "the conflict resolution strategy {strategy} is not valid"
                        )),
                    },
                )
                .into(),
            );
        }

        // `conflictResolution` is "if AUTOMERGE is the conflict resolution
        // strategy, a list of inputs to use when resolving conflicts during a
        // merge": under any other strategy the branch to take is already decided,
        // so a resolution list cannot be honoured and the request is refused
        // rather than silently ignored. Checked BEFORE the entries themselves,
        // because a list that cannot be honoured at all is the primary reason the
        // request fails, whatever one of its entries says.
        if merge::resolution_cannot_be_honoured(strategy, input.conflict_resolution.as_ref()) {
            return Err(
                CreateUnreferencedMergeCommitErrorModeled::InvalidConflictResolutionException(
                    InvalidConflictResolutionException {
                        message: Some(merge::invalid_conflict_resolution_message(strategy)),
                    },
                )
                .into(),
            );
        }
        let resolution =
            merge::CheckedResolution::check(input.conflict_resolution.as_ref()).map_err(refused_resolution)?;

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            // An unreachable store is an internal mounting fault, not a modelled
            // CodeCommit failure; every handler in this crate refuses it this way.
            crate::scaffolded::Scaffolded::validation("the repository store is unavailable")
        })?;
        let mut store = state.lock();
        let repository = store
            .repositories
            .get_mut(&input.repository_name)
            .ok_or_else(|| {
                CreateUnreferencedMergeCommitErrorModeled::RepositoryDoesNotExistException(
                    RepositoryDoesNotExistException {
                        message: Some(crate::state::repository_does_not_exist_message(
                            &input.repository_name,
                        )),
                    },
                )
            })?;

        // Each specifier is "the branch, tag, HEAD, or other fully qualified
        // reference used to identify a commit", resolved the one way this crate
        // resolves a commit specifier.
        let commit_does_not_exist = |specifier: &str| {
            CreateUnreferencedMergeCommitErrorModeled::CommitDoesNotExistException(
                CommitDoesNotExistException {
                    message: Some(crate::state::commit_does_not_exist_message(specifier)),
                }
            )
        };
        let (source_id, source_files) = {
            let commit = repository
                .resolve_commit(&input.source_commit_specifier)
                .ok_or_else(|| commit_does_not_exist(&input.source_commit_specifier))?;
            (commit.commit_id.clone(), commit.files.clone())
        };
        let (destination_id, destination_files) = {
            let commit = repository
                .resolve_commit(&input.destination_commit_specifier)
                .ok_or_else(|| commit_does_not_exist(&input.destination_commit_specifier))?;
            (commit.commit_id.clone(), commit.files.clone())
        };

        // The three-way merge: the tree of the merge base against the two tips.
        //
        // This is caller-driven CPU work — history walks and, for a file both
        // sides changed, a diff3 of three whole blobs — and it runs under this
        // service's store lock, so it is held to a wall-clock deadline
        // (`MergeBudget`, checked per path and per history walk) and moved off the
        // async worker, exactly as this crate's other caller-driven CPU work (the
        // git transport's pack reading and the commit projection) is.
        let mut budget = merge::MergeBudget::for_one_request();
        // What a merge ran out of, as the errors this operation models for it —
        // the same reading of the same allowance `GetMergeOptions` answers with,
        // so one pair of commits is refused the same way whether the merge is
        // reported or written.
        let out_of_allowance = |problem: merge::MergeProblem| -> CreateUnreferencedMergeCommitError {
            match problem {
                // No merge base at all: the two tips share no commit, so there is
                // no common ancestor to merge either side's changes against.
                merge::MergeProblem::NoMergeBase => {
                    CreateUnreferencedMergeCommitErrorModeled::TipsDivergenceExceededException(
                        TipsDivergenceExceededException {
                            message: Some(format!(
                                "the divergence between {} and {} is too great to determine \
                                 whether there might be any merge conflicts",
                                input.source_commit_specifier, input.destination_commit_specifier
                            )),
                        },
                    )
                    .into()
                }
                merge::MergeProblem::ItemsToCompare => {
                    CreateUnreferencedMergeCommitErrorModeled::MaximumItemsToCompareExceededException(
                        MaximumItemsToCompareExceededException {
                            message: Some(format!(
                                "merging {} into {} exceeds the {} items this emulator compares \
                                 against the merge base at once",
                                input.source_commit_specifier,
                                input.destination_commit_specifier,
                                merge::MAX_ITEMS_TO_COMPARE_PER_REQUEST
                            )),
                        },
                    )
                    .into()
                }
                merge::MergeProblem::Deadline => {
                    CreateUnreferencedMergeCommitErrorModeled::MaximumItemsToCompareExceededException(
                        MaximumItemsToCompareExceededException {
                            message: Some(format!(
                                "merging {} into {} took longer than the {} seconds this emulator \
                                 spends merging two commits",
                                input.source_commit_specifier,
                                input.destination_commit_specifier,
                                merge::MAX_MERGE_SECONDS
                            )),
                        },
                    )
                    .into()
                }
                merge::MergeProblem::ContentToLoad => {
                    CreateUnreferencedMergeCommitErrorModeled::MaximumFileContentToLoadExceededException(
                        MaximumFileContentToLoadExceededException {
                            message: Some(format!(
                                "merging {} into {} line by line exceeds the {} bytes of file \
                                 content this emulator loads at once",
                                input.source_commit_specifier,
                                input.destination_commit_specifier,
                                merge::MAX_CONTENT_BYTES_PER_REQUEST
                            )),
                        },
                    )
                    .into()
                }
            }
        };
        let (base_files, mut merged) = crate::service::off_the_async_worker(|| {
            let base_files =
                merge::merge_base_files(repository, &source_id, &destination_id, &mut budget)?;
            let merged = merge::merge_snapshots(
                repository,
                &base_files,
                &source_files,
                &destination_files,
                detail_level,
                strategy,
                &mut budget,
            )?;
            Ok::<_, merge::MergeProblem>((base_files, merged))
        })
        .map_err(out_of_allowance)?;

        // Then the caller's own resolutions, which AUTOMERGE is what carries.
        //
        // An entry is applied to the path it names whether or not the merge found
        // that path conflicting: `conflictResolution` names the content the merge
        // RESULT holds for a path, and the conflict set this operation computed is
        // not reported to the caller (`DescribeMergeConflicts` and
        // `BatchDescribeMergeConflicts` are the operations that report it), so a
        // caller cannot know which paths the emulator found conflicting and an
        // entry for a settled path is honoured rather than refused.
        if strategy == merge::CONFLICT_RESOLUTION_AUTOMERGE {
            resolution
                .apply(&mut merged, &base_files, &source_files, &destination_files)
                .map_err(refused_resolution)?;
            // A resolution can write a file at a path the merged tree holds as a
            // folder, and can equally settle a collision the merge itself found,
            // so the file-against-folder verdict is recomputed over the resolved
            // result.
            merged
                .detect_object_type_conflicts(&budget)
                .map_err(out_of_allowance)?;
        }

        // Anything still conflicting is a merge no strategy and no resolution
        // settled, which is exactly what the modelled refusal says.
        if !merged.unresolved().is_empty() {
            return Err(
                CreateUnreferencedMergeCommitErrorModeled::ManualMergeRequiredException(
                    ManualMergeRequiredException {
                        message: Some(crate::state::MANUAL_MERGE_REQUIRED_MESSAGE.to_string()),
                    },
                )
                .into(),
            );
        }

        // The snapshot the merge commit carries, and the bytes the merge produced
        // that the object store does not hold yet.
        let mut files: BTreeMap<String, crate::state::FileEntry> = BTreeMap::new();
        let mut new_blobs: Vec<Vec<u8>> = Vec::new();
        for (path, file) in std::mem::take(&mut merged.files) {
            if let Some(content) = file.content {
                new_blobs.push(content);
            }
            files.insert(path, file.entry);
        }

        // The folders the merge emptied are kept only when the caller asked for
        // it, with the placeholder `crate::state::keep_empty_folders` writes — the
        // one reading of that rule this crate has, which leaves a name the merged
        // tree holds as a FILE alone rather than writing a placeholder under it.
        if input.keep_empty_folders {
            let emptied: Vec<&str> = source_files
                .keys()
                .chain(destination_files.keys())
                .filter(|path| !files.contains_key(path.as_str()))
                .map(String::as_str)
                .collect();
            for _ in crate::state::keep_empty_folders(&mut files, emptied) {
                // The placeholder entry names the EMPTY blob, which the store must
                // hold for the tree to encode.
                new_blobs.push(Vec::new());
            }
        }

        // The emulator's own bound on the snapshot each commit stores, checked FIRST
        // so the folder walk below never runs over a tree this emulator would not
        // store anyway.
        if files.len() > crate::state::MAX_FILES_PER_TREE {
            return Err(crate::scaffolded::Scaffolded::validation(format!(
                "this emulator holds at most {} files per repository tree",
                crate::state::MAX_FILES_PER_TREE
            )));
        }

        // Then CodeCommit's documented 6 MB folder limit, the other bound
        // `CreateCommit` holds a written tree to, charged from the crate's one
        // reading of that rule (`merge::oversized_changed_folder`).
        if let Some((folder, _)) = merge::oversized_changed_folder(&files, &destination_files) {
            return Err(
                CreateUnreferencedMergeCommitErrorModeled::FolderContentSizeLimitExceededException(
                    FolderContentSizeLimitExceededException {
                        message: Some(format!(
                            "the commit cannot be created because the folder {folder} \
                             would exceed the limit of {} bytes",
                            crate::state::MAX_FOLDER_CONTENT_BYTES
                        )),
                    },
                )
                .into(),
            );
        }
        // And the bound on the STORE this commit's objects grow, charged before
        // the write exactly as `CreateCommit` charges it.
        let added_bytes = new_blobs
            .iter()
            .map(Vec::len)
            .fold(0usize, usize::saturating_add);
        if repository.objects.len().saturating_add(new_blobs.len())
            > crate::state::MAX_OBJECTS_PER_REPOSITORY
            || repository.object_bytes().saturating_add(added_bytes)
                > crate::state::MAX_OBJECT_BYTES_PER_REPOSITORY
        {
            return Err(crate::scaffolded::Scaffolded::validation(format!(
                "the commit would take this repository past the {} objects and {} object bytes this emulator holds",
                crate::state::MAX_OBJECTS_PER_REPOSITORY,
                crate::state::MAX_OBJECT_BYTES_PER_REPOSITORY
            )));
        }

        // "The name of the author who created the unreferenced commit. This
        // information is used as both the author and committer for the commit."
        let now = std::time::SystemTime::now();
        // The identity whose call writes the commit, which the commit records where
        // the request names no author.
        let actor = crate::state::caller_identity_arn(&*self.engine, ctx);
        let user = crate::state::commit_identity(
            input.author_name.clone(),
            input.email.clone(),
            &actor,
            now,
        );
        // The merge option decides the PARENTS, not the tree: a three-way merge
        // records both tips, with the destination — the side being merged into —
        // first, while a squash merge collapses the source history and records the
        // destination tip alone.
        let parents = if merge_option == merge::MERGE_OPTION_SQUASH {
            vec![destination_id.clone()]
        } else {
            vec![destination_id.clone(), source_id.clone()]
        };

        for content in new_blobs {
            repository.insert_blob(content);
        }
        let (commit_id, tree_id) = repository
            .commit_snapshot(
                files,
                parents,
                user.clone(),
                user,
                input.commit_message.as_deref(),
            )
            .ok_or_else(|| {
                // Every entry names a blob this store holds, so the tree always
                // encodes; a snapshot that did not is an internal inconsistency,
                // refused like an unreachable store.
                crate::scaffolded::Scaffolded::validation(
                    "the repository store holds a file whose blob id is not an object name",
                )
            })?;
        // No ref names this commit — that is what UNREFERENCED means — so it is
        // recorded as a retained unreferenced commit, which is what keeps the id
        // this response hands back readable through `GetCommit` and serveable to a
        // `git fetch` instead of released as unreachable by the next write.
        repository.retain_unreferenced(commit_id.clone());
        // The merge this call made is recorded against the pair of commits it
        // merged, so the commit it wrote is what `GetMergeCommit` reports for
        // that pair instead of being lost with this response.
        repository.record_merge_commit(&source_id, &destination_id, &commit_id);
        // Nothing else moved: no branch was created or advanced, and the
        // repository's own `lastModifiedDate` reports when the repository was last
        // modified, which an unreferenced commit does not do.
        repository.prune_history();

        Ok(CreateUnreferencedMergeCommitOutput {
            commit_id: Some(commit_id),
            tree_id: Some(tree_id),
        })
    }
}
