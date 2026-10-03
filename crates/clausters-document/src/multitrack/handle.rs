//! **One verb of a multitrack, as JSON** -- the door a client's handle speaks
//! through, the same over the C ABI and wasm.
//!
//! A script holds the multitrack an editor edits, not a copy of it, so what
//! crosses here is a question about one structure or one edit of it: a track,
//! a region, a curve read by its id, the ids a container holds in order, and an
//! edit in the multitrack's vocabulary ([`super::edit`]) applied with the
//! payload that puts it back. Nothing here is a second vocabulary: every write
//! is a [`MultitrackIntent`], so a change made through a handle is the same
//! edit a gesture makes, and can be recorded and undone the same way.

use serde_json::{Value, json};

use super::edit::{self, MultitrackIntent};
use super::{Multitrack, picture};
use crate::NodeId;
use crate::intent::{Against, Rules};

/// **One verb of a multitrack.** `request` is `{"verb": ...}`:
///
/// - `"state"`: the multitrack, whole.
/// - `"ids"` with `of`: `{"ids": [id]}`, in the order shown --
///   `"tracks"`; `"takeLanes"` with `track`; `"regions"` with `takeLane`, in
///   position order; `"automation"` with `track` or `region`; `"markers"`. A
///   container that is not there answers `{"ids": null}`.
/// - `"track"` with `id`: the track, or `null`.
/// - `"takeLane"` with `id`: `{"track", "takeLane"}` -- the track's id and the
///   take lane -- or `null`.
/// - `"region"` with `id`: `{"track", "takeLane", "region"}`, or `null`.
/// - `"automation"` with `id`: `{"track"?, "region"?, "automation"}` -- which
///   holds it, and the curve -- or `null`.
/// - `"marker"` with `id`: the marker, or `null`.
/// - `"mint"` with `count`: `{"ids": [id]}`, ids nothing in the multitrack
///   names, in order. Minted from what is there: an id an undo took away may
///   come back, which is safe because recording any edit drops the redo that
///   could have brought the old one back.
/// - `"apply"` with `intent`: the edit applied against what the multitrack says
///   now, answering `{"applied", "current"?, "reason"?}` -- `current` the edit
///   that puts it back, read before this one landed, unless `"inverse": false`
///   -- or `{"error"}` for an intent that does not read.
///
/// A request that does not read answers `{"error": ...}`. [`mutates`] says
/// which requests change the multitrack.
pub fn call_json(multitrack: &mut Multitrack, request: &str) -> String {
    let Ok(request) = serde_json::from_str::<Value>(request) else {
        return json!({"error": "the request is not JSON"}).to_string();
    };
    let id = || request.get("id").and_then(Value::as_u64).map(NodeId);
    let answer = match request.get("verb").and_then(Value::as_str) {
        Some("state") => serde_json::to_value(&*multitrack).unwrap_or(Value::Null),
        Some("ids") => json!({ "ids": ids(multitrack, &request) }),
        Some("track") => id()
            .and_then(|id| multitrack.track(id))
            .map_or(Value::Null, |t| serde_json::to_value(t).unwrap_or(Value::Null)),
        Some("takeLane") => id()
            .and_then(|id| multitrack.take_lane(id))
            .map_or(Value::Null, |(track, lane)| {
                json!({"track": track.id, "takeLane": lane})
            }),
        Some("region") => id()
            .and_then(|id| multitrack.locate(id))
            .map_or(Value::Null, |(track, lane, region)| {
                json!({"track": track.id, "takeLane": lane.id, "region": region})
            }),
        Some("automation") => id().map_or(Value::Null, |id| automation(multitrack, id)),
        Some("marker") => id()
            .and_then(|id| multitrack.markers.iter().find(|m| m.id == id))
            .map_or(Value::Null, |m| serde_json::to_value(m).unwrap_or(Value::Null)),
        Some("mint") => {
            let count = request.get("count").and_then(Value::as_u64).unwrap_or(1);
            let first = fresh(multitrack);
            json!({ "ids": (first..first + count).collect::<Vec<_>>() })
        }
        Some("apply") => {
            let intent = request.get("intent").cloned().unwrap_or(Value::Null);
            let inverse = request
                .get("inverse")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            match serde_json::from_value::<MultitrackIntent>(intent) {
                Ok(intent) => {
                    let current = inverse
                        .then(|| edit::current(multitrack, &intent))
                        .flatten();
                    let outcome =
                        edit::apply(multitrack, &intent, &Against::unstated(), &Rules::none());
                    let mut answer = json!({ "applied": outcome.applied });
                    if let Some(current) = current.filter(|_| outcome.applied) {
                        answer["current"] = edit::payload(&current).0;
                    }
                    if let Some(reason) = outcome.reason {
                        answer["reason"] = json!(reason);
                    }
                    answer
                }
                Err(error) => json!({ "error": error.to_string() }),
            }
        }
        _ => json!({"error": "no such verb"}),
    };
    answer.to_string()
}

/// Whether `request` changes the multitrack -- for a door that answers a
/// sizing pass and has to leave nothing behind.
pub fn mutates(request: &str) -> bool {
    serde_json::from_str::<Value>(request)
        .ok()
        .and_then(|r| r.get("verb").and_then(Value::as_str).map(|v| v == "apply"))
        .unwrap_or(false)
}

/// An id past everything the multitrack names -- [`picture::fresh_id`], and
/// past its markers too, since a marker is looked up by its id as well.
fn fresh(multitrack: &Multitrack) -> u64 {
    let markers = multitrack.markers.iter().map(|m| m.id.0 + 1).max();
    picture::fresh_id(multitrack).max(markers.unwrap_or(0))
}

/// The ids a container holds, in the order shown, or `None` when it is not
/// there.
fn ids(multitrack: &Multitrack, request: &Value) -> Option<Vec<u64>> {
    let named = |key: &str| request.get(key).and_then(Value::as_u64).map(NodeId);
    match request.get("of").and_then(Value::as_str)? {
        "tracks" => Some(multitrack.tracks.iter().map(|t| t.id.0).collect()),
        "takeLanes" => multitrack
            .track(named("track")?)
            .map(|t| t.take_lanes.iter().map(|l| l.id.0).collect()),
        "regions" => multitrack
            .take_lane(named("takeLane")?)
            .map(|(_, l)| l.regions.iter().map(|r| r.id.0).collect()),
        "automation" => {
            if let Some(track) = named("track") {
                return multitrack
                    .track(track)
                    .map(|t| t.automation.iter().map(|a| a.id.0).collect());
            }
            multitrack
                .locate(named("region")?)
                .map(|(_, _, r)| r.automation.iter().map(|a| a.id.0).collect())
        }
        "markers" => Some(multitrack.markers.iter().map(|m| m.id.0).collect()),
        _ => None,
    }
}

/// The curve with this id and what holds it: `{"track", "automation"}` for a
/// track's, `{"region", "automation"}` for a region's.
fn automation(multitrack: &Multitrack, id: NodeId) -> Value {
    for track in &multitrack.tracks {
        if let Some(curve) = track.automation.iter().find(|a| a.id == id) {
            return json!({"track": track.id, "automation": curve});
        }
        for region in track.take_lanes.iter().flat_map(|l| l.regions.iter()) {
            if let Some(curve) = region.automation.iter().find(|a| a.id == id) {
                return json!({"region": region.id, "automation": curve});
            }
        }
    }
    Value::Null
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multitrack::{Content, Marker, Region, TakeLane, Track};
    use crate::timebase::Second;
    use crate::{Lifetime, SegmentRef, SegmentSource, SourceId, SourceRef};

    fn multitrack() -> Multitrack {
        let mut lane = TakeLane::new(NodeId(2));
        lane.place(Region::new(
            NodeId(3),
            Second(1.0),
            Second(2.0),
            Content::window(SegmentRef {
                source: SegmentSource::Samples(SourceRef {
                    source: SourceId(9),
                    lifetime: Lifetime::Session,
                    generation: 0,
                    range: None,
                }),
                start: 0.0,
                duration: 2.0,
            }),
        ));
        let mut track = Track::new(NodeId(1), NodeId(2)).named("drums");
        track.take_lanes = vec![lane];
        let mut mt = Multitrack::new();
        mt.tracks.push(track);
        mt.add_marker(Marker::new(NodeId(40), Second(0.5)));
        mt
    }

    fn call(mt: &mut Multitrack, request: Value) -> Value {
        serde_json::from_str(&call_json(mt, &request.to_string())).unwrap()
    }

    /// **A structure is read by its id**, with the ids of what holds it, and a
    /// container lists what it holds in the order shown.
    #[test]
    fn a_region_is_read_with_where_it_is() {
        let mut mt = multitrack();
        let found = call(&mut mt, json!({"verb": "region", "id": 3}));
        assert_eq!(found["track"], 1);
        assert_eq!(found["takeLane"], 2);
        assert_eq!(found["region"]["position"], 1.0);
        assert_eq!(
            call(
                &mut mt,
                json!({"verb": "ids", "of": "regions", "takeLane": 2})
            )["ids"],
            json!([3])
        );
        assert_eq!(
            call(
                &mut mt,
                json!({"verb": "ids", "of": "takeLanes", "track": 7})
            )["ids"],
            Value::Null,
            "no such track"
        );
        assert_eq!(
            call(&mut mt, json!({"verb": "track", "id": 99})),
            Value::Null
        );
    }

    /// **An edit answers the edit that puts it back**, read before it landed.
    #[test]
    fn an_edit_answers_its_inverse() {
        let mut mt = multitrack();
        let moved = call(
            &mut mt,
            json!({"verb": "apply", "intent": {
                "intent": "placeregion", "region": 3, "track": 1, "take_lane": 2,
                "position": 4.0}}),
        );
        assert_eq!(moved["applied"], true);
        assert_eq!(moved["current"]["position"], 1.0);
        assert_eq!(mt.locate(NodeId(3)).unwrap().2.position, Second(4.0));
        let back = call(
            &mut mt,
            json!({"verb": "apply", "intent": moved["current"]}),
        );
        assert_eq!(back["applied"], true);
        assert_eq!(mt.locate(NodeId(3)).unwrap().2.position, Second(1.0));
        assert!(mutates(r#"{"verb": "apply"}"#) && !mutates(r#"{"verb": "state"}"#));
    }

    /// **A minted id is one nothing names** -- past the markers as well.
    #[test]
    fn minted_ids_are_past_everything_named() {
        let mut mt = multitrack();
        assert_eq!(
            call(&mut mt, json!({"verb": "mint", "count": 2}))["ids"],
            json!([41, 42])
        );
    }
}
