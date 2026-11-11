Issue-Severity: HIGH
Issue-Gap-Class: event-propagation
Issue-Commit: 1d0d0c28dcee365463f48da791546814f4205587

# PutTaxInheritance's management-account gate is unsatisfiable from outside the process: nothing in src/ ever records an organization, so every wire request is refused and GetTaxInheritance can only report OptOut

_Filed: 2026-10-29T10:05:32Z_
_Operation: PutTaxInheritance | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `1d0d0c28dcee365463f48da791546814f4205587`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PutTaxInheritance` refuses every call whose caller is not the management account of the organization, and the standing it judges by is the store field `organization_management_account_id`, which is `None` by `Default` and is assigned NOWHERE in `src/`. The only assignment in the repository is in this crate's own test harness, which reaches into the Rust struct. For any client that speaks only HTTP, the gate therefore answers 400 `ValidationException` to EVERY `PutTaxInheritance` request, whatever body it sends, and `store.tax_inheritance` has no reachable writer — so `GetTaxInheritance` can only ever report the hard-coded default `OptOut` and the whole `OptIn` branch of the modelled two-value `HeritageStatus` enum is unobservable.

The crate itself states where the missing fact is supposed to come from: the field's own documentation says the organization "is established through AWS Organizations, which this emulator models in its own service and whose state holds no organization until one is created". That other service's emulator DOES record the caller as the organization's management account on `CreateOrganization` (`services/service-com-amazonaws-organizations-awsorganizationsv20161128/src/ops/create_organization.rs:252`, `:287-314`), and both services are mounted on the same shared engine — but nothing in this crate ever consults it, and no operation of this service enrolls an account either. The cross-service fact that makes the gate satisfiable is simply never propagated, so a realistic workflow (create the organization, then record the organization-level tax-inheritance election, then read it back) cannot be completed over the wire.

This is not a request to drop the gate: the refusal of a non-management caller, its `errorCode`, message and absent `fieldList` are pinned by `tests/all/tax_inheritance_election.rs` and must stay. The defect is that the gate's precondition has no in-`src` path to becoming true, which contradicts the store's own documented lifecycle for the field it protects.

## Evidence

The gate, and the standing it reads:
- `src/ops/put_tax_inheritance.rs:182-201`: "let caller = ::engine::emulator_account_id();" / "if !store.is_organization_management_account(caller) {" ... "return Err(PutTaxInheritanceErrorModeled::ValidationException(crate::state::validation_of_the_call(crate::state::ERROR_CODE_FIELD_VALIDATION_FAILED, crate::state::TAX_INHERITANCE_MANAGEMENT_ACCOUNT_ONLY)).into());"
- `src/state.rs:657-659`: "pub fn is_organization_management_account(&self, account_id: &str) -> bool { self.organization_management_account_id.as_deref() == Some(account_id) }"
- `src/state.rs:614`: "pub organization_management_account_id: Option<String>," on a `#[derive(Debug, Default)]` struct (`src/state.rs:582-583`), i.e. `None` at start-up.

Both sides of the repo inconsistency, quoted:
- `src/state.rs:594-596` documents the election field's lifecycle as driven by this operation: "`None` until `PutTaxInheritance` records one, for as long as which `GetTaxInheritance` reports [`DEFAULT_HERITAGE_STATUS`]; a write replaces the value, because the account has exactly one election." The write that is supposed to end that `None` can never run.
- `src/state.rs:607-613` names the source of the fact the gate needs and admits nothing supplies it: "`None` by default: an organization is established through AWS Organizations, which this emulator models in its own service and whose state holds no organization until one is created, so the presented account is a standalone account and is nobody's organization management account. Set it to an account id to model that account as the management account".
- The cross-service writer that exists and is never consulted: `services/service-com-amazonaws-organizations-awsorganizationsv20161128/src/ops/create_organization.rs:252` "let master_account_id = ::engine::emulator_account_id().to_string();" and `:287` "let management_account = crate::state::AccountRecord {" ... `:314` ".insert(master_account_id, management_account);".
- `grep -n organization_management_account_id` over this crate returns only `src/state.rs:601, 614, 655, 658` (the field, its docs and the reader) plus ONE assignment, in the test harness: `tests/all/tax_registration_lifecycle.rs:87` "state.lock().organization_management_account_id = account_id.map(std::string::ToString::to_string);". No handler writes it.

Test coverage: every success-path assertion for this operation goes through that in-process back door — `tests/all/tax_inheritance_election.rs:41-45` "fn management_account_emulator() ... record_management_account(&engine, Some(engine::emulator_account_id()));" — so `an_election_that_was_recorded_is_the_election_that_is_reported`, `a_recorded_election_is_shared_state` and `a_write_that_names_no_election_records_nothing` all pass while no HTTP-only client can reach the 200 they assert. The refusal arm (`an_election_is_recorded_only_by_the_organizations_management_account`, lines 226-288) must keep passing unchanged.

## Steps to Reproduce

1. Mount this service on a shared `engine::InMemoryEngine` (as `tests/all/tax_registration_lifecycle.rs:45-52` does) together with the Organizations emulator, and use ONLY HTTP requests from here on — no direct mutation of the Rust store.
2. POST /GetTaxInheritance with {} — observe 200 {"heritageStatus":"OptOut"}.
3. POST /PutTaxInheritance with {"heritageStatus":"OptIn"} — observe 400 ValidationException, errorCode FieldValidationFailed, message "Only AWS orgs management account is allowed to set tax inheritance.", because `organization_management_account_id` is still `None`.
4. Call the Organizations emulator's CreateOrganization on the same engine (which records `emulator_account_id()` as the organization's management account, create_organization.rs:252/287-314), then repeat POST /PutTaxInheritance {"heritageStatus":"OptIn"} — observe the same 400: this service never consults that state and has no other way to learn the organization.
5. POST /GetTaxInheritance again — still {"heritageStatus":"OptOut"}: no request sequence a client can send makes the read report OptIn, so the modelled success of PutTaxInheritance and the OptIn value of HeritageStatus are both unreachable over the wire.
