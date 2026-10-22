//! The replies read through a front with no window behind it: what is asked of
//! the server, and what each view holds after the answers.

use std::cell::RefCell;

use clausters_core::osc::{OscMessage, OscPacket, OscType};

use super::Front;
use crate::host::fetch::BufferFetches;
use crate::host::frame::Owed;
use crate::host::instance::Leg;
use crate::host::widget::element::Loaded;
use crate::host::{ClientId, GUI_DEF, GUI_SET, Host};

/// A front that keeps what it was told to send, and a window that is open.
#[derive(Default)]
struct Probe {
    host: Host,
    fetches: BufferFetches,
    sent: RefCell<Vec<OscMessage>>,
}

impl Front for Probe {
    fn host(&self) -> &Host {
        &self.host
    }
    fn host_mut(&mut self) -> &mut Host {
        &mut self.host
    }
    fn fetches(&mut self) -> &mut BufferFetches {
        &mut self.fetches
    }
    fn to_server(&self, msg: OscMessage) {
        self.sent.borrow_mut().push(msg);
    }
    fn redraw_window(&self, _def_id: i32) {}
    fn window_open(&self, _def_id: i32) -> bool {
        true
    }
    fn place_slot(&mut self, _def_id: i32, _widget_id: i32, _data: Loaded) -> Option<usize> {
        None
    }
    fn release_slot(&mut self, _def_id: i32, _widget_id: i32) {}
    fn take_owed(&self, _def_id: i32) -> Vec<(i32, Owed)> {
        Vec::new()
    }
    fn held(&self) -> Vec<i32> {
        Vec::new()
    }
}

fn from() -> ClientId {
    ClientId::Udp(std::net::SocketAddr::from((
        std::net::Ipv4Addr::LOCALHOST,
        9000,
    )))
}

const TEN_MINUTES: usize = 10 * 60 * 48_000;

impl Probe {
    /// A window drawing take 7 as a trace.
    fn open() -> Self {
        let mut probe = Probe::default();
        probe.host.handle_packet(
            OscPacket::Message(OscMessage {
                addr: GUI_DEF.into(),
                args: vec![
                    OscType::Int(1),
                    OscType::String(
                        r#"{"type":"window","title":"w","children":[
                             {"id":40,"type":"signal","view":"trace","buffer":7}]}"#
                            .into(),
                    ),
                ],
            }),
            from(),
        );
        probe
    }

    /// What was sent since the last look, by address.
    fn sent(&self) -> Vec<OscMessage> {
        std::mem::take(&mut *self.sent.borrow_mut())
    }

    /// The bulk walk a front makes, then the server's answer to its query.
    fn told(&mut self, frames: usize) {
        self.fetches.want(1, 40, 7);
        self.on_server_reply(
            Leg::Server,
            &OscMessage {
                addr: "/buffer_query.reply".into(),
                args: vec![
                    OscType::Int(7),
                    OscType::Int(frames as i32),
                    OscType::Int(1),
                    OscType::Float(48_000.0),
                ],
            },
        );
    }

    /// A draw of the window at `width` physical pixels, and what it owes.
    fn drawn(&mut self, width: f32) {
        if let Some(w) = self.host.window_def(1).and_then(|t| t.find(40)) {
            w.kind.as_element().unwrap().note_width(width);
        }
        self.ask_owed_spans(&[1]);
    }

    /// The server answering one `/buffer_peaks`, mono, every bucket `value`.
    fn answer(&mut self, ask: &OscMessage, value: f32, take: usize) {
        let ints: Vec<i64> = ask
            .args
            .iter()
            .map(|a| match a {
                OscType::Int(v) => *v as i64,
                _ => 0,
            })
            .collect();
        let (bucket, start, frames) = (ints[1] as usize, ints[2] as usize, ints[3] as usize);
        let end = (start + frames).min(take);
        let buckets = (end - start).div_ceil(bucket).min(8 * 1024 / 12);
        let mut blob = Vec::new();
        for _ in 0..buckets {
            for v in [-value, value, value * value] {
                blob.extend_from_slice(&v.to_le_bytes());
            }
        }
        self.on_server_reply(
            Leg::Server,
            &OscMessage {
                addr: "/buffer_peaks.reply".into(),
                args: vec![
                    OscType::Int(7),
                    OscType::Long(start as i64),
                    OscType::Int(bucket as i32),
                    OscType::Blob(blob),
                ],
            },
        );
    }

    /// Answers every summary request until none is left, counting them.
    fn answer_all(&mut self, value: f32, take: usize) -> usize {
        let mut asks = 0;
        loop {
            let pending: Vec<_> = self
                .sent()
                .into_iter()
                .filter(|m| m.addr == "/buffer_peaks")
                .collect();
            if pending.is_empty() {
                return asks;
            }
            for ask in pending {
                asks += 1;
                self.answer(&ask, value, take);
            }
        }
    }

    /// The take's length and its peak, as the view holds it.
    fn picture(&self) -> Option<(usize, f32)> {
        let el = self.host.window_def(1)?.find(40)?.kind.signal()?;
        let crate::host::elements::signal::Source::Data(d) = &el.source else {
            return None;
        };
        let body = d.body.as_ref()?;
        Some((body.total_samples(), body.column(0, 1.0e9, 0.0, 1.0e9).1))
    }
}

/// **A take's summary is asked at the level the view is drawn at**, once it
/// has been drawn, and comes in a few replies whatever the take's length.
#[test]
fn a_summary_is_asked_at_the_drawn_level_and_placed_whole() {
    let mut probe = Probe::open();
    probe.told(TEN_MINUTES);
    assert!(
        probe.sent().iter().all(|m| m.addr != "/buffer_peaks"),
        "nothing says how wide the view is before it is drawn"
    );
    probe.drawn(760.0);
    let asked = probe.sent();
    let ask = asked
        .iter()
        .find(|m| m.addr == "/buffer_peaks")
        .expect("drawn, it asks");
    assert_eq!(
        ask.args[1],
        OscType::Int(32_768),
        "the coarsest level its columns resolve"
    );
    assert_eq!(probe.picture(), None);
    // The first of the two replies is not drawn: half a summary is not one.
    probe.answer(ask, 0.5, TEN_MINUTES);
    assert_eq!(probe.picture(), None, "nothing until it is whole");
    let rest = probe.answer_all(0.5, TEN_MINUTES);
    assert_eq!(rest, 1, "879 buckets of mono: two replies");
    assert_eq!(probe.picture(), Some((TEN_MINUTES, 0.5)));
    probe.drawn(760.0);
    assert!(probe.sent().is_empty(), "a still window asks for nothing");
}

/// **An edit keeps the picture until the new one has come whole**: the
/// owner's `reload` asks again, the view goes on drawing the take it had,
/// and the take after the edit replaces it at once -- never a picture filling
/// in from the left.
#[test]
fn an_edit_keeps_the_picture_until_the_new_one_is_whole() {
    let mut probe = Probe::open();
    probe.told(TEN_MINUTES);
    probe.drawn(760.0);
    probe.answer_all(0.5, TEN_MINUTES);
    assert_eq!(probe.picture(), Some((TEN_MINUTES, 0.5)));

    // A paste: the owner restitched the take, a second longer, and says so.
    probe.host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: GUI_SET.into(),
            args: vec![
                OscType::Int(40),
                OscType::String("reload".into()),
                OscType::Int(1),
            ],
        }),
        from(),
    );
    let asks_again = probe
        .host
        .window_def_mut(1)
        .and_then(|t| t.find_mut(40))
        .and_then(|w| w.kind.as_samples_mut())
        .and_then(|s| s.wants_reload());
    assert!(asks_again.is_some(), "the reload is a read");
    assert_eq!(probe.picture(), Some((TEN_MINUTES, 0.5)), "drawn meanwhile");

    let longer = TEN_MINUTES + 48_000;
    probe.told(longer);
    probe.drawn(760.0);
    let asked = probe.sent();
    let ask = asked
        .iter()
        .find(|m| m.addr == "/buffer_peaks")
        .expect("asked at its level");
    probe.answer(ask, 0.75, longer);
    assert_eq!(
        probe.picture(),
        Some((TEN_MINUTES, 0.5)),
        "the take before the edit, still whole, while the new one comes"
    );
    let rest = probe.answer_all(0.75, longer);
    assert!(rest <= 2, "a few replies, whatever the length: {rest}");
    assert_eq!(
        probe.picture(),
        Some((longer, 0.75)),
        "the new take, at once"
    );
}
