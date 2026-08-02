//! The component bundle: its manifest, its two kinds of hole, and the resolver.
//!
//! A **bundle** is the directory of persisted data that one component mounts —
//! the def payloads, the GuiDef record, the presets, the samples — and the
//! manifest (`bundle.json`) that declares what mounting it needs. One directory
//! runs on three legs: a browser tab, `clausters-gui --standalone`, and a
//! loopback host against a running server. This module is the part that has to
//! agree between them, so it lives here rather than in any one client.
//!
//! # The two kinds of hole
//!
//! Mounting the *same* bundle twice on one page must not collide, so the GuiDef
//! record on disk is a **template** with placeholders, told apart by sigil:
//!
//! ```text
//! "@lfo", "@graph"   a symbol    — an id the caller allocates when mounting
//! "$freq", "$title"  a parameter — a value the tag supplies, or a preset's,
//!                                  or the declared default
//! ```
//!
//! Widget ids are deliberately **not** symbols: the template numbers its widgets
//! `1..N` and [`resolve`] offsets them by a base the caller allocates, so twelve
//! widgets do not mean twelve placeholders.
//!
//! **Placeholders live only in the GuiDef record** (and in its `boot` list).
//! That is the invariant the format is built on: the def payloads under `defs/`
//! contain no holes, so they are byte-identical between two mounted instances
//! and are sent to the server once. It forces one authoring rule, which is the
//! right rule anyway — *a bus, a node or a buffer reaches a def as a control,
//! never as a baked constant.* [`check_def_payload`] is how a writer enforces
//! it.
//!
//! A doubled sigil escapes it: `"$$5"` resolves to the literal `"$5"`, so prose
//! that wants a `$` or an `@` can still be written.
//!
//! # Two pure steps, with the allocation in between
//!
//! ```text
//! requirements(manifest)  ->  { widgets, nodes, buses, buffers }
//!         ... the caller allocates from its own allocators ...
//! resolve(manifest, template, allocation, params)  ->  { def_id, tree, boot }
//! ```
//!
//! Nothing here allocates an id, keeps state or knows a transport: what comes
//! out is the same `/gui_def` tree and `boot` messages as a hand-written bundle,
//! so nothing is added to the `/gui_*` protocol.
//!
//! [`validate`] is the same machinery pointed the other way, for the writers: a
//! dry run over the declared defaults, so a bundle that would fail to mount
//! fails to be written.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

mod format;
mod resolve;

// The format's types are this module's public surface, named
// `bundle::Manifest` the way they always were; the resolution machinery is
// private and reached only by the passes below.
pub use format::*;
use resolve::*;

/// What one instance of `manifest` needs allocated.
///
/// Pure bookkeeping: it reads the declaration and hands it back in the shape a
/// caller allocates from. The caller then builds an [`Allocation`] and calls
/// [`resolve`].
pub fn requirements(manifest: &Manifest) -> Requirements {
    Requirements {
        widgets: manifest.widgets,
        nodes: manifest.symbols.nodes.clone(),
        buses: manifest.symbols.buses.clone(),
        buffers: manifest.symbols.buffers.clone(),
    }
}

/// [`requirements`], with the template on hand to measure what the manifest
/// does not declare.
///
/// A written bundle numbers its widgets `1..N` and declares `widgets`, so the
/// two agree. A bundle written **before** the contract declares nothing, and
/// its saved ids are whatever the author picked — `1`, `10`, `20`. Offsetting
/// those by a one-wide block would make two instances overlap, so the block is
/// sized to the ids actually used ([`widget_span`]) instead.
pub fn requirements_for(manifest: &Manifest, template: Option<&Template>) -> Requirements {
    let mut out = requirements(manifest);
    if let Some(template) = template {
        out.widgets = out.widgets.max(widget_span(template));
    }
    out
}

/// The width of the id block a template needs: its highest local widget id,
/// the root's included. (Ids are offset by `widget_base - 1`, so a template
/// using `1..=20` needs twenty ids however few widgets it holds.)
pub fn widget_span(template: &Template) -> usize {
    fn walk(node: &Value, high: &mut i64) {
        let Some(map) = node.as_object() else { return };
        if let Some(id) = map.get("id").and_then(Value::as_i64) {
            *high = (*high).max(id);
        }
        if let Some(children) = map.get("children").and_then(Value::as_array) {
            for child in children {
                walk(child, high);
            }
        }
    }
    let mut high = template.id as i64;
    walk(&template.gui, &mut high);
    high.max(1) as usize
}

/// The parameter values a mount supplies, in the order they win.
///
/// Resolution is **attribute → preset → declared default**, so a preset is a
/// named bundle of values and an attribute is a local override. Both maps may
/// hold names the manifest does not declare (a tag carries `class` and `style`
/// like any element); those are ignored rather than refused.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ParamInput {
    #[serde(default)]
    pub attributes: Map<String, Value>,
    #[serde(default)]
    pub preset: Map<String, Value>,
}

impl ParamInput {
    /// Just the attributes — the common mount, with no preset named.
    pub fn from_attributes(attributes: Map<String, Value>) -> ParamInput {
        ParamInput {
            attributes,
            preset: Map::new(),
        }
    }
}

/// Fills one instance of a template: offsets its widget ids, substitutes every
/// hole, and lifts its `boot` list out.
///
/// `allocation` is what the caller allocated for the [`requirements`]; `params`
/// is what the tag and its preset supplied. Every declared parameter is typed
/// and range-checked here, so a bad value is an error at mount rather than a
/// surprise on a bus.
pub fn resolve(
    manifest: &Manifest,
    template: &Template,
    allocation: &Allocation,
    params: &ParamInput,
) -> Result<Resolved, Error> {
    check_symbol_namespaces(&manifest.symbols)?;
    let values = merge_params(manifest, params)?;
    let ctx = Ctx {
        symbols: &manifest.symbols,
        allocation,
        params: &values,
    };

    let Value::Object(root) = &template.gui else {
        return Err(Error::BadTemplate("the root is not an object".into()));
    };
    check_widget_ids(template)?;
    let needed = widget_span(template);
    if manifest.widgets != 0 && manifest.widgets < needed {
        return Err(Error::WidgetBlock {
            declared: manifest.widgets,
            needed,
        });
    }

    let offset = allocation.widget_base - 1;
    let mut root = root.clone();
    let boot = match root.remove("boot") {
        Some(Value::Array(list)) => resolve_boot(&list, &ctx)?,
        _ => Vec::new(),
    };
    let tree = resolve_node(&root, offset, &ctx)?;

    Ok(Resolved {
        def_id: template.id + offset,
        tree,
        boot,
        params: values,
    })
}

/// Checks a manifest against its own template, the way a writer needs it
/// checked: the same resolution a mount performs, run over a synthetic
/// allocation and the declared defaults.
///
/// So a bundle whose template names an undeclared symbol, whose parameter
/// defaults do not type-check, or whose widget count is wrong, fails here —
/// at the point it would be written, not at the point someone mounts it.
pub fn validate(manifest: &Manifest, template: &Template) -> Result<(), Error> {
    check_symbol_namespaces(&manifest.symbols)?;
    let mut allocation = Allocation {
        widget_base: 1,
        ..Allocation::default()
    };
    // Distinct dummy ids, so a substitution that swaps two namespaces still
    // shows up as a wrong value rather than passing by coincidence.
    for (i, name) in manifest.symbols.nodes.iter().enumerate() {
        allocation.nodes.insert(name.clone(), 1000 + i as i32);
    }
    for (i, bus) in manifest.symbols.buses.iter().enumerate() {
        allocation.buses.insert(bus.name.clone(), 2000 + i as i32);
    }
    for (i, buffer) in manifest.symbols.buffers.iter().enumerate() {
        allocation.buffers.insert(buffer.clone(), 3000 + i as i32);
    }
    resolve(manifest, template, &allocation, &ParamInput::default())?;
    Ok(())
}

/// Refuses a placeholder in a def payload — the invariant that keeps def
/// payloads shareable between two mounted instances. A writer calls this on
/// every `/def_send synth` and `/def_send graph` spec before emitting it.
pub fn check_def_payload(payload: &Value) -> Result<(), Error> {
    match payload {
        Value::String(s) => match placeholder(s) {
            Some(Hole::Symbol(_)) | Some(Hole::Param(_)) => Err(Error::PlaceholderInDef(s.clone())),
            _ => Ok(()),
        },
        Value::Array(items) => items.iter().try_for_each(check_def_payload),
        Value::Object(map) => map.values().try_for_each(check_def_payload),
        _ => Ok(()),
    }
}

// --- the shape the bindings carry ---------------------------------------
//
// [`resolve`] and [`validate`] take three and two arguments; a C or wasm door
// carrying each as its own pointer pair would be a wide surface that the two
// doors could then drift apart on. So the envelope is declared here, once, and
// both doors are the same two lines over it.

/// One [`requirements_for`] call: the manifest, and the template when the
/// caller has it (a pre-contract bundle's id block is measured from it).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RequirementsRequest {
    pub manifest: Manifest,
    #[serde(default)]
    pub template: Option<Template>,
}

/// One [`resolve`] call as a single JSON object — what the wasm and C doors
/// carry.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ResolveRequest {
    pub manifest: Manifest,
    pub template: Template,
    pub allocation: Allocation,
    #[serde(default)]
    pub params: ParamInput,
}

/// One [`validate`] call, plus the def payloads to check for holes — the whole
/// pre-flight a writer runs before emitting a directory.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ValidateRequest {
    pub manifest: Manifest,
    pub template: Template,
    /// The `/def_send synth` and `/def_send graph` payloads, each parsed. Checked by
    /// [`check_def_payload`].
    #[serde(default)]
    pub defs: Vec<Value>,
}

/// [`requirements_for`], from the envelope the bindings carry.
pub fn requirements_request(request: &RequirementsRequest) -> Requirements {
    requirements_for(&request.manifest, request.template.as_ref())
}

/// [`resolve`], from the envelope the bindings carry.
pub fn resolve_request(request: &ResolveRequest) -> Result<Resolved, Error> {
    resolve(
        &request.manifest,
        &request.template,
        &request.allocation,
        &request.params,
    )
}

/// [`validate`] plus [`check_def_payload`] over every payload, from the
/// envelope the bindings carry.
pub fn validate_request(request: &ValidateRequest) -> Result<(), Error> {
    validate(&request.manifest, &request.template)?;
    request.defs.iter().try_for_each(check_def_payload)
}

// --- the resolution machinery -------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn manifest() -> Manifest {
        serde_json::from_value(json!({
            "name": "fm-voice",
            "gui": "fm-voice",
            "synthdefs": ["fm-voice.voice"],
            "widgets": 4,
            "symbols": {
                "nodes": ["graph"],
                "buses": [{ "name": "lfo", "rate": "control", "channels": 1 }],
                "buffers": ["hit"]
            },
            "params": {
                "freq": { "type": "float", "default": 220.0, "min": 60.0, "max": 700.0 },
                "title": { "type": "string", "default": "FM voice" }
            }
        }))
        .unwrap()
    }

    fn template() -> Template {
        serde_json::from_value(json!({
            "id": 1,
            "gui": {
                "type": "window",
                "title": "$title",
                "boot": [["/graph_new", "fm-voice.graph", "@graph", 0, 0],
                         ["/node_set", "@graph", "freq", "$freq"]],
                "children": [
                    { "id": 2, "type": "meter", "bus": "@lfo" },
                    { "id": 3, "type": "panel", "children": [
                        { "id": 4, "type": "knob", "value": "$freq",
                          "bind": ["/node_set", "@graph", "freq"] }
                    ]}
                ]
            }
        }))
        .unwrap()
    }

    fn allocation() -> Allocation {
        Allocation {
            widget_base: 100,
            nodes: [("graph".to_string(), 1500)].into(),
            buses: [("lfo".to_string(), 17)].into(),
            buffers: [("hit".to_string(), 4)].into(),
        }
    }

    /// The manifest round-trips, and its optional halves stay out of the JSON.
    #[test]
    fn manifest_round_trips() {
        let m = manifest();
        let back: Manifest = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(m, back);
        let minimal: Manifest = serde_json::from_value(json!({ "gui": "drone" })).unwrap();
        assert_eq!(
            serde_json::to_value(&minimal).unwrap(),
            json!({"gui":"drone"})
        );
    }

    /// A manifest written before the format grew its contract still reads: the
    /// three legs keep running today's bundles.
    #[test]
    fn a_pre_contract_manifest_still_parses() {
        let m: Manifest = serde_json::from_value(json!({
            "gui": "piano", "synthdefs": ["piano_voice"], "graphdefs": []
        }))
        .unwrap();
        assert_eq!(m.gui, "piano");
        assert_eq!(m.widgets, 0);
        assert!(m.symbols.is_empty());
        assert!(m.params.is_empty());
    }

    #[test]
    fn requirements_read_the_declaration() {
        let req = requirements(&manifest());
        assert_eq!(req.widgets, 4);
        assert_eq!(req.nodes, ["graph"]);
        assert_eq!(req.buses[0].rate, BusRate::Control);
        assert_eq!(req.buffers, ["hit"]);
    }

    /// A bundle written before the contract declares no count, and its saved
    /// ids are whatever the author picked. The block is sized to those, or two
    /// instances would overlap on the widget ids they were offset into.
    #[test]
    fn an_undeclared_widget_block_is_measured_from_the_template() {
        let old: Manifest = serde_json::from_value(json!({ "gui": "piano" })).unwrap();
        let sparse: Template = serde_json::from_value(json!({
            "id": 1,
            "gui": { "type": "window", "children": [
                { "id": 10, "type": "piano" },
                { "id": 20, "type": "meter", "bus": 0 }]}
        }))
        .unwrap();
        assert_eq!(requirements(&old).widgets, 0);
        assert_eq!(requirements_for(&old, Some(&sparse)).widgets, 20);

        // Two instances, each offset by its own 20-wide block: no id is shared.
        let ids = |base: i32| {
            let out = resolve(
                &old,
                &sparse,
                &Allocation {
                    widget_base: base,
                    ..Allocation::default()
                },
                &ParamInput::default(),
            )
            .unwrap();
            let children = out.tree["children"].as_array().unwrap().clone();
            let mut ids: Vec<i64> = vec![out.def_id as i64];
            ids.extend(children.iter().map(|c| c["id"].as_i64().unwrap()));
            ids
        };
        let (first, second) = (ids(1000), ids(1020));
        assert_eq!(first, [1000, 1009, 1019]);
        assert_eq!(second, [1020, 1029, 1039]);
        assert!(first.iter().all(|id| !second.contains(id)));
    }

    /// A declared count wins when it is larger; the span never shrinks a block.
    #[test]
    fn a_declared_widget_count_is_kept() {
        let req = requirements_for(&manifest(), Some(&template()));
        assert_eq!(req.widgets, 4);
    }

    /// The whole pass: ids offset through the nesting, symbols and parameters
    /// substituted in props and in boot, the boot list lifted out of the tree.
    #[test]
    fn resolve_offsets_ids_and_fills_both_kinds_of_hole() {
        let out = resolve(
            &manifest(),
            &template(),
            &allocation(),
            &ParamInput::default(),
        )
        .unwrap();
        assert_eq!(out.def_id, 100);
        let tree = out.tree.as_object().unwrap();
        assert_eq!(tree["title"], json!("FM voice"));
        assert!(!tree.contains_key("boot"), "the boot list left the tree");

        let children = tree["children"].as_array().unwrap();
        assert_eq!(children[0]["id"], json!(101));
        assert_eq!(children[0]["bus"], json!(17));
        assert_eq!(children[1]["id"], json!(102));
        let knob = &children[1]["children"][0];
        assert_eq!(knob["id"], json!(103));
        assert_eq!(knob["value"], json!(220.0));
        assert_eq!(knob["bind"], json!(["/node_set", 1500, "freq"]));

        assert_eq!(
            out.boot,
            vec![
                vec![
                    json!("/graph_new"),
                    json!("fm-voice.graph"),
                    json!(1500),
                    json!(0),
                    json!(0)
                ],
                vec![json!("/node_set"), json!(1500), json!("freq"), json!(220.0)],
            ]
        );
    }

    /// Two mounts of one template share nothing: different ids, different
    /// buses, one def payload.
    #[test]
    fn two_instances_do_not_collide() {
        let m = manifest();
        let t = template();
        let first = resolve(&m, &t, &allocation(), &ParamInput::default()).unwrap();
        let second = resolve(
            &m,
            &t,
            &Allocation {
                widget_base: 200,
                nodes: [("graph".to_string(), 1501)].into(),
                buses: [("lfo".to_string(), 18)].into(),
                buffers: [("hit".to_string(), 5)].into(),
            },
            &ParamInput::default(),
        )
        .unwrap();
        assert_ne!(first.def_id, second.def_id);
        assert_eq!(second.tree["children"][0]["bus"], json!(18));
        assert_eq!(second.boot[0][2], json!(1501));
    }

    /// Attribute over preset over default, all three typed on the way in.
    #[test]
    fn parameters_merge_attribute_over_preset_over_default() {
        let params = ParamInput {
            // As they arrive from HTML: every attribute is a string.
            attributes: json!({ "freq": "440" }).as_object().unwrap().clone(),
            preset: json!({ "freq": 660.0, "title": "bright" })
                .as_object()
                .unwrap()
                .clone(),
        };
        let out = resolve(&manifest(), &template(), &allocation(), &params).unwrap();
        assert_eq!(out.params["freq"], json!(440.0));
        assert_eq!(out.params["title"], json!("bright"));
        assert_eq!(out.tree["title"], json!("bright"));
    }

    /// A doubled sigil is the escape: prose keeps its `$` and `@`.
    #[test]
    fn a_doubled_sigil_escapes_it() {
        let mut m = manifest();
        m.widgets = 1;
        let t: Template = serde_json::from_value(json!({
            "id": 1, "gui": { "type": "window", "title": "$$5 and @@home" }
        }))
        .unwrap();
        let out = resolve(&m, &t, &allocation(), &ParamInput::default()).unwrap();
        assert_eq!(out.tree["title"], json!("$5 and @@home"));
    }

    #[test]
    fn an_unknown_symbol_is_an_error() {
        let mut m = manifest();
        m.widgets = 1;
        let t: Template =
            serde_json::from_value(json!({ "id": 1, "gui": { "type": "meter", "bus": "@nope" } }))
                .unwrap();
        assert_eq!(
            resolve(&m, &t, &allocation(), &ParamInput::default()),
            Err(Error::UnknownSymbol("nope".into()))
        );
    }

    #[test]
    fn an_unknown_parameter_is_an_error() {
        let mut m = manifest();
        m.widgets = 1;
        let t: Template = serde_json::from_value(
            json!({ "id": 1, "gui": { "type": "window", "title": "$nope" } }),
        )
        .unwrap();
        assert_eq!(
            resolve(&m, &t, &allocation(), &ParamInput::default()),
            Err(Error::UnknownParam("nope".into()))
        );
    }

    #[test]
    fn a_parameter_with_no_default_must_be_supplied() {
        let mut m = manifest();
        m.params.insert(
            "gain".into(),
            ParamSpec {
                kind: ParamType::Float,
                default: None,
                min: None,
                max: None,
            },
        );
        assert_eq!(
            resolve(&m, &template(), &allocation(), &ParamInput::default()),
            Err(Error::MissingParam("gain".into()))
        );
        let supplied =
            ParamInput::from_attributes(json!({ "gain": "0.5" }).as_object().unwrap().clone());
        let out = resolve(&m, &template(), &allocation(), &supplied).unwrap();
        assert_eq!(out.params["gain"], json!(0.5));
    }

    #[test]
    fn a_type_mismatch_is_an_error() {
        let params =
            ParamInput::from_attributes(json!({ "freq": "loud" }).as_object().unwrap().clone());
        assert_eq!(
            resolve(&manifest(), &template(), &allocation(), &params),
            Err(Error::TypeMismatch {
                name: "freq".into(),
                expected: ParamType::Float,
                got: "the string \"loud\"".into(),
            })
        );
    }

    #[test]
    fn a_value_out_of_range_is_an_error() {
        let params =
            ParamInput::from_attributes(json!({ "freq": 9000.0 }).as_object().unwrap().clone());
        assert_eq!(
            resolve(&manifest(), &template(), &allocation(), &params),
            Err(Error::OutOfRange {
                name: "freq".into(),
                value: 9000.0,
                min: Some(60.0),
                max: Some(700.0),
            })
        );
    }

    #[test]
    fn a_declared_but_unallocated_symbol_is_an_error() {
        let bare = Allocation {
            widget_base: 100,
            ..Allocation::default()
        };
        assert_eq!(
            resolve(&manifest(), &template(), &bare, &ParamInput::default()),
            Err(Error::UnallocatedSymbol("graph".into()))
        );
    }

    /// A block narrower than the numbering is the error worth catching: two
    /// instances offset by it would overlap. A wider one is merely generous.
    #[test]
    fn too_narrow_a_widget_block_is_an_error() {
        let mut m = manifest();
        m.widgets = 2;
        assert_eq!(
            resolve(&m, &template(), &allocation(), &ParamInput::default()),
            Err(Error::WidgetBlock {
                declared: 2,
                needed: 4
            })
        );
        m.widgets = 9;
        assert!(resolve(&m, &template(), &allocation(), &ParamInput::default()).is_ok());
    }

    /// The root's id is offset with the rest, so a child numbered like it would
    /// resolve onto it.
    #[test]
    fn a_widget_id_used_twice_is_an_error() {
        let mut m = manifest();
        m.widgets = 2;
        m.symbols = SymbolTable::default();
        m.params.clear();
        let t: Template = serde_json::from_value(json!({
            "id": 1,
            "gui": { "type": "window", "children": [
                { "id": 1, "type": "knob" },
                { "id": 2, "type": "meter", "bus": 0 }]}
        }))
        .unwrap();
        assert_eq!(
            resolve(&m, &t, &allocation(), &ParamInput::default()),
            Err(Error::DuplicateWidgetId(1))
        );
    }

    #[test]
    fn one_name_in_two_namespaces_is_ambiguous() {
        let mut m = manifest();
        m.symbols.nodes.push("lfo".into());
        assert_eq!(
            resolve(&m, &template(), &allocation(), &ParamInput::default()),
            Err(Error::DuplicateSymbol("lfo".into()))
        );
    }

    /// The invariant a writer enforces: no hole ever reaches a def payload, so
    /// two instances can share the one that was sent.
    #[test]
    fn a_placeholder_in_a_def_payload_is_refused() {
        let clean = json!({ "name": "voice", "ugens": [{ "rate": 2, "inputs": [0, 1] }] });
        assert_eq!(check_def_payload(&clean), Ok(()));
        let baked = json!({ "name": "voice", "ugens": [{ "inputs": ["@lfo"] }] });
        assert_eq!(
            check_def_payload(&baked),
            Err(Error::PlaceholderInDef("@lfo".into()))
        );
    }

    /// `validate` catches at write time what a mount would have hit.
    #[test]
    fn validate_dry_runs_the_mount() {
        assert_eq!(validate(&manifest(), &template()), Ok(()));

        let mut wrong = manifest();
        wrong.symbols.nodes.clear();
        assert_eq!(
            validate(&wrong, &template()),
            Err(Error::UnknownSymbol("graph".into()))
        );

        let mut untyped = manifest();
        untyped.params.get_mut("freq").unwrap().default = Some(json!("loud"));
        assert!(matches!(
            validate(&untyped, &template()),
            Err(Error::TypeMismatch { .. })
        ));
    }

    /// A bundle that declares nothing resolves as itself, holes or not — the
    /// pre-contract bundles keep mounting.
    #[test]
    fn a_bundle_with_no_contract_resolves_unchanged() {
        let m: Manifest = serde_json::from_value(json!({ "gui": "drone" })).unwrap();
        let t: Template = serde_json::from_value(json!({
            "id": 1,
            "gui": { "type": "window", "children": [{ "id": 2, "type": "meter", "bus": 0 }] }
        }))
        .unwrap();
        let out = resolve(&m, &t, &allocation(), &ParamInput::default()).unwrap();
        assert_eq!(out.def_id, 100);
        assert_eq!(out.tree["children"][0]["id"], json!(101));
        assert_eq!(out.tree["children"][0]["bus"], json!(0));
    }
}
