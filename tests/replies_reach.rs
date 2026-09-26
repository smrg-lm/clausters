//! A UGen's replies reach whoever they are for, live and offline alike: a
//! `Poll` posts the same line in an offline render as on the live server.

#![cfg(feature = "synth")]

use std::io::Write;
use std::sync::{Arc, Mutex};

use clausters::rosc::{OscMessage, OscType};
use clausters::server::render::{RenderConfig, Score, render_to_vec};
use clausters::synthdef::SynthDefSpec;
use serde_json::json;

const SR: f32 = 48_000.0;

fn msg(addr: &str, args: Vec<OscType>) -> OscMessage {
    OscMessage {
        addr: addr.into(),
        args,
    }
}

fn def(ugens: serde_json::Value) -> OscType {
    OscType::Blob(
        serde_json::to_vec(
            &serde_json::from_value::<SynthDefSpec>(json!({"name": "d", "ugens": ugens})).unwrap(),
        )
        .unwrap(),
    )
}

/// A writer the test reads back.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// An offline render posts a `Poll`'s line as the live server does: the
/// label and the value, on the OSC log target at `info`.
#[test]
fn an_offline_poll_posts_its_line() {
    let log = Captured::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_max_level(tracing::Level::INFO)
        .without_time()
        .finish();
    let events = vec![
        (
            0.0,
            vec![
                msg(
                    "/def_send",
                    vec![
                        OscType::String("synth".into()),
                        def(json!([
                            {"kind": "Impulse", "inputs": [{"const": 10.0}]},
                            {"kind": "Poll",
                             "inputs": [{"ugen": 0}, {"const": 0.25}, {"const": -1.0}],
                             "label": "level"}
                        ])),
                    ],
                ),
                msg(
                    "/synth_new",
                    vec![
                        OscType::String("d".into()),
                        OscType::Int(100),
                        OscType::Int(0),
                        OscType::Int(0),
                    ],
                ),
            ],
        ),
        (0.05, vec![msg("/node_free", vec![OscType::Int(100)])]),
    ];
    let cfg = RenderConfig {
        sample_rate: SR as f64,
        channels: 1,
        ..RenderConfig::default()
    };
    tracing::subscriber::with_default(subscriber, || {
        render_to_vec(&Score::new(events).unwrap(), &cfg).unwrap();
    });
    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    assert!(text.contains("level: 0.25"), "{text}");
}
