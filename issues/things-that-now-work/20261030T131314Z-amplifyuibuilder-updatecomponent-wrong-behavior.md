Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0df9dc9e6fa18e0bd9ddfa393114d8e439af5a61

# Write handlers sample the wall clock BEFORE taking the state lock, so a stored modifiedAt can move backwards

_Filed: 2026-10-30T13:13:14Z_
_Operation: UpdateComponent | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0df9dc9e6fa18e0bd9ddfa393114d8e439af5a61`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every handler of this crate that stamps a record reads `SystemTime::now()` OUTSIDE the critical section and then blocks on `state.lock()`, writing the stale instant into the record it finally owns. Two concurrent `UpdateComponent` calls against the same component can therefore commit out of order: the caller that sampled the EARLIER instant can be the one that acquires the lock LAST, so `component.modified_at` ends up older than the value the previously-committed update stored, and the record no longer reports the instant of the last call that changed it. The same pattern is in update_theme.rs, create_component.rs, create_theme.rs and start_codegen_job.rs.

This contradicts the invariant the crate states for exactly this member ("`modifiedAt` never goes backwards", "`modifiedAt`, which reports the last call that changed the component") and it is inconsistent with the one clock reader in the crate that gets it right: `GetCodegenJob` takes the lock first and samples the clock after, so the instant it settles with is the instant observed while holding the store. Nothing in the handler between the sample and the lock needs the earlier instant — the merge, the stamp and the response are all computed under the lock — so the sample is simply taken in the wrong place.

The existing tests are sequential, so they cannot see it: they pin the invariant that the code can break rather than the window that breaks it.

## Evidence

The clock is sampled before the lock, and the sampled instant is what is stored:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/update_component.rs:247-248
    let now = ::std::time::SystemTime::now();
    let mut store = state.lock();
  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/update_component.rs:338
    component.modified_at = now;

The same shape in the theme update (and in create_component.rs:200-202, create_theme.rs:159-161, start_codegen_job.rs:161-164):

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/update_theme.rs:198-199
    let now = ::std::time::SystemTime::now();
    let mut store = state.lock();
  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/update_theme.rs:229
    theme.modified_at = now;

The sibling clock reader that samples INSIDE the critical section:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/get_codegen_job.rs:158-161
    let mut store = state.lock();
    // The only write a read performs: it applies the transition the clock already
    // owed at this instant, which was owed whether or not anybody called.
    store.settle_codegen_jobs(::std::time::SystemTime::now());

The invariant the crate states for the member this writes:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/tests/all/theme_updates.rs:9-15
    //! 1. Updates accumulate on the ONE stored record: each call replaces the members it
    //!    carries, the members it leaves out keep whatever the PREVIOUS update left (not what
    //!    the create stored), the record's identity never moves, and `modifiedAt` never goes
    //!    backwards.
  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/update_component.rs:10-12
    //! `clientToken` needs no replay record here: a merge is idempotent in the members it
    //! writes, so repeating one leaves the same record (only `modifiedAt`, which reports the
    //! last call that changed the component, moves).
  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/state.rs:242-245
    /// When the component last changed. Creating it is the change that first set
    /// it, so it starts at [`StoredComponent::created_at`] and is moved by every
    /// `UpdateComponent` that merges into the record.

Test coverage today: tests/all/theme_updates.rs (accumulating updates, `modifiedAt` monotonicity) and tests/all/component_lifecycle.rs exercise one call at a time, so the ordering window is untested; moving the sample under the lock leaves every one of their assertions unchanged.

## Steps to Reproduce

1. Create a component: POST /app/d1234abcd/environment/staging/components with componentToCreate = {name: "Hero", componentType: "Text", properties: {}, variants: [], overrides: {}, bindingProperties: {}} — note the returned id.
2. Issue two PATCH /app/d1234abcd/environment/staging/components/{id} requests CONCURRENTLY (e.g. two clients, or two tokio tasks against the router), one with {"name":"A"} and one with {"name":"B"}.
3. Both handlers run `let now = SystemTime::now()` (update_component.rs:247) before contending for `state.lock()` (line 248), so the one that sampled the earlier instant can be the one that writes last.
4. GET /app/d1234abcd/environment/staging/components/{id}: when that interleaving happens, `modifiedAt` is EARLIER than the value the previous PATCH's response reported, i.e. the record's `modifiedAt` went backwards and no longer names the last call that changed the component.
5. Compare GetCodegenJob (get_codegen_job.rs:158-161), which takes the lock and only then reads the clock, so the instant it writes is the one observed while holding the store.
