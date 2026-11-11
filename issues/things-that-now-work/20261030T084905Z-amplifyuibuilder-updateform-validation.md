Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ba1310c994fc47a47280538753d01db9a79290df

# Form writes store any string as a sectional element's type/orientation, where the model documents a closed set of valid values

_Filed: 2026-10-30T08:49:05Z_
_Operation: UpdateForm | Class: validation_

> _Note: this issue was identified against the code base at commit `ba1310c994fc47a47280538753d01db9a79290df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A form's `sectionalElements` map is stored exactly as the request supplied it, with no check of the three members whose valid values the model spells out in prose (`SectionalElement$type`, `$orientation`, `$level`). `UpdateForm` assigns `patch.sectional_elements` straight onto the record, and `CreateForm` moves `data.sectional_elements` straight into `StoredForm`; neither consults the documented value sets. So `PATCH .../forms/{id}` with `{"sectionalElements": {"x": {"type": "Banana", "orientation": "sideways", "level": 99}}}` answers HTTP 200 and `GetForm`/`ExportForms` then report a form whose sectional element has a type no renderer of this API knows.

These three members target plain `String`/`Integer` in the model (not enums), so the framework's validating walk cannot refuse them — exactly the situation this crate already handles by hand elsewhere: `PutMetadataFlag` refuses a `featureName` outside the catalogue the service holds because "the model cannot express which names exist", and `CreateComponent`/`UpdateComponent` refuse a reserved `tags` property key because that is "a rule the model itself cannot express". The form write paths are the ones that skip that treatment.

Filed against `UpdateForm` because the (CreateForm, validation) pair is already taken by the name-conflict issue; the gap is the same on both write paths and the fix belongs to both (one shared check in `crate::state`, like `carries_reserved_name`/`is_known_metadata_feature`).

## Evidence

The model states the valid values for each member (smithy_query(query="shape",shape="SectionalElement")):

  "type": {"target": "String", "required": true, "doc": "<p>The type of sectional element. Valid values are <code>Heading</code>, <code>Text</code>,\n      and <code>Divider</code>.</p>"}
  "level": {"target": "Integer", "doc": "<p>Specifies the size of the font for a <code>Heading</code> sectional element. Valid values\n      are <code>1 | 2 | 3 | 4 | 5 | 6</code>.</p>"}
  "orientation": {"target": "String", "doc": "<p>Specifies the orientation for a <code>Divider</code> sectional element. Valid values are\n        <code>horizontal</code> or <code>vertical</code>.</p>"}

The handler under review stores whatever arrived, with no check:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/update_form.rs:216-218
        if let Some(sectional_elements) = patch.sectional_elements {
            form.sectional_elements = sectional_elements;
        }

The create path does the same (its whole `handle` body, lines 158-225, contains no validation block at all):

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/create_form.rs:208
            sectional_elements: data.sectional_elements,

The stored shape keeps the members as free-form values, so nothing downstream narrows them either:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/types/shared_types.rs:978-987
    #[smithy_macros::smithy(shape = "com.amazonaws.amplifyuibuilder#SectionalElement")]
    pub struct SectionalElement {
        pub r#type: String,
        ...
        pub level: Option<i32>,
        pub orientation: Option<String>,

The crate's own precedent for a documented-but-unmodellable value set, in a sibling handler:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/put_metadata_flag.rs:200-212
        // `featureName` is an `@httpLabel` targeting a plain `String`, so the
        // model cannot express which names exist: the service holds a fixed
        // catalogue of feature flags ... and a name outside it is an invalid
        // parameter rather than a new feature to store.
        if !crate::state::is_known_metadata_feature(&input.feature_name) { ... InvalidParameterException ... }

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/create_component.rs:162-166
        // `Component$properties`, `$overrides`, `$bindingProperties` and
        // `$collectionProperties` each document "You can't specify `tags` as a
        // valid property" for that map — a rule the model itself cannot express —
        // so a request naming it is refused rather than stored.

Test coverage today: every form test uses only documented values, so a check leaves them green — tests/all/form_update.rs:245-253 (`{"type": "Divider", "orientation": "horizontal"}`, `{"type": "Text"}`), tests/all/form_lifecycle.rs:244-246 (`Heading`/`Divider`/`Text`), and tests/all/form_lifecycle.rs:659-725 (`field_and_sectional_element_maps_round_trip_whatever_their_keys`, which varies the map KEY while always sending `{"type": "Heading", ...}` / `{"type": "Divider"}`). No test pins acceptance of an undocumented type, orientation or level.

## Steps to Reproduce

1. POST /app/d1234abcd/environment/staging/forms with formToCreate = {"name":"Contact","dataType":{"dataSourceType":"Custom","dataTypeName":"X"},"formActionType":"create","fields":{},"style":{},"sectionalElements":{},"schemaVersion":"1"} — the form is created (200).
2. PATCH /app/d1234abcd/environment/staging/forms/{id} with updatedForm = {"sectionalElements":{"x":{"type":"Banana","orientation":"sideways","level":99}}}.
3. Observed: HTTP 200, and GET /app/d1234abcd/environment/staging/forms/{id} reports sectionalElements.x.type == "Banana", orientation == "sideways", level == 99 — none of them a value the model documents as valid ("Valid values are Heading, Text, and Divider" / "horizontal or vertical" / "1 | 2 | 3 | 4 | 5 | 6").
4. The same payload passed as `formToCreate` on CreateForm is accepted too, so the gap is on both form write paths.
5. Compare with the sibling prose-rule checks: PUT /app/d1234abcd/environment/staging/metadata/features/notAFeature is refused 400 ("Feature notAFeature is not valid"), and a `tags` key in componentToCreate.properties is refused 400.
