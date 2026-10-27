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
    /// The analyses placed in a texture slot, by widget, in order.
    textures: Vec<(i32, Vec<crate::spectrogram::Stft>)>,
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
        // As both fronts do: a list's questions are the host's own.
        if crate::host::disk::list_key_of(&msg).is_some() {
            return self.host.ask_disk(msg);
        }
        self.sent.borrow_mut().push(msg);
    }
    fn redraw_window(&self, _def_id: i32) {}
    fn window_open(&self, _def_id: i32) -> bool {
        true
    }
    fn place_slot(&mut self, _def_id: i32, widget_id: i32, data: Loaded) -> Option<usize> {
        if let Loaded::Stfts(stfts) = data {
            self.textures.push((widget_id, stfts));
        }
        None
    }
    fn release_slot(&mut self, _def_id: i32, _widget_id: i32) {}
    fn take_owed(&self, _def_id: i32) -> Vec<(i32, Owed)> {
        Vec::new()
    }
    fn stale_columns(&self, _def_id: i32, _widget_id: i32, _frames: usize) -> bool {
        false
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

/// **An answerer in the same process**, answering at once: what a front's
/// disk thread does, without the thread.
#[derive(Clone, Default)]
struct Local(std::rc::Rc<RefCell<LocalDisk>>);

#[derive(Default)]
struct LocalDisk {
    answerer: crate::host::disk::answer::Answerer,
    io: crate::host::disk::native::Disk,
    replies: Vec<OscMessage>,
    defined: Vec<i32>,
}

impl crate::host::disk::DiskLink for Local {
    fn define(&self, key: i32, list: std::sync::Arc<crate::host::disk::List>) {
        let mut d = self.0.borrow_mut();
        d.defined.push(key);
        d.answerer.define(key, list);
    }
    fn forget(&self, key: i32) {
        self.0.borrow_mut().answerer.forget(key);
    }
    fn ask(&self, msg: OscMessage) {
        let mut d = self.0.borrow_mut();
        let LocalDisk { answerer, io, .. } = &mut *d;
        if let Some(reply) = crate::host::disk::native::answer(answerer, io, &msg) {
            d.replies.push(reply);
        }
    }
}

impl Probe {
    /// Hands every answer the local disk has back to the front, until it
    /// has none.
    fn pump(&mut self, local: &Local) -> usize {
        let mut n = 0;
        loop {
            let replies = std::mem::take(&mut local.0.borrow_mut().replies);
            if replies.is_empty() {
                return n;
            }
            for reply in replies {
                n += 1;
                self.on_server_reply(Leg::Server, &reply);
            }
        }
    }

    /// The list key the view at widget 40 was given.
    fn list_key(&self) -> i32 {
        self.host
            .window_def(1)
            .and_then(|t| t.find(40))
            .and_then(|w| w.kind.as_element())
            .and_then(|el| el.samples())
            .and_then(|s| s.source_buffer())
            .expect("a key")
    }
}

/// A float W64 of `frames` mono samples rising from -0.5 to 0.5, with its
/// peaks; answers the paths.
fn ramp_file(dir: &std::path::Path, name: &str, frames: usize) -> (String, String) {
    let samples: Vec<f32> = (0..frames)
        .map(|i| i as f32 / frames as f32 - 0.5)
        .collect();
    let path = dir
        .join(format!("{name}.w64"))
        .to_string_lossy()
        .into_owned();
    let mut w = clausters_core::pcm::Writer::create(&path, 1, 48_000.0, "float").unwrap();
    w.write(&samples).unwrap();
    w.finalize().unwrap();
    let peaks = dir
        .join(format!("{name}.peaks"))
        .to_string_lossy()
        .into_owned();
    clausters_core::peaks::MultiPyramid::build_interleaved(&samples, 1, 256)
        .write_cache(&peaks)
        .unwrap();
    (path, peaks)
}

fn segments(parts: &[(&str, &str, usize, usize, usize)]) -> String {
    let parts: Vec<String> = parts
        .iter()
        .map(|(path, peaks, start, frames, of)| {
            format!(
                r#"{{"path":"{path}","peaks":"{peaks}","start":{start},"frames":{frames},
                    "source_channels":1,"source_frames":{of},"source_rate":48000}}"#
            )
        })
        .collect();
    format!(
        r#"{{"channels":1,"sample_rate":48000,"parts":[{}]}}"#,
        parts.join(",")
    )
}

/// **A list of segments over files is drawn the way a buffer is, with no
/// server**: its key's questions go to the host's own answerer, the view asks
/// its summary at the level it is drawn at and gets it whole, and an edit's
/// new list keeps the picture until the new one has come.
#[test]
fn a_list_of_segments_is_drawn_from_its_files_with_no_server() {
    let dir = std::env::temp_dir().join(format!("clausters-gui-list-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let frames = 1 << 20;
    let (path, peaks) = ramp_file(&dir, "a", frames);

    let mut probe = Probe::default();
    let local = Local::default();
    probe.host.set_disk_link(Box::new(local.clone()));
    let def = format!(
        r#"{{"type":"window","title":"w","children":[
             {{"id":40,"type":"signal","view":"trace","segments":{}}}]}}"#,
        segments(&[(&path, &peaks, 0, frames, frames)])
    );
    probe.host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: GUI_DEF.into(),
            args: vec![OscType::Int(1), OscType::String(def.clone())],
        }),
        from(),
    );
    let key = probe.list_key();
    assert!(crate::host::disk::is_list_key(key));
    assert_eq!(local.0.borrow().defined, vec![key], "told the answerer");

    // The bulk walk a front makes: the query is the host's to answer.
    let query = probe.fetches.want(1, 40, key).expect("a query");
    probe.to_server(query);
    probe.pump(&local);
    probe.drawn(760.0);
    let replies = probe.pump(&local);
    assert!(replies > 0 && replies < 8, "{replies} replies");
    assert_eq!(probe.sent(), Vec::new(), "nothing went to a server");
    let (len, peak) = probe.picture().expect("drawn");
    assert_eq!(len, frames);
    assert!((peak - 0.5).abs() < 1e-3, "{peak}");

    // **A redefine names the same list**: the key and the picture carry,
    // and the answerer is told nothing new.
    probe.host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: GUI_DEF.into(),
            args: vec![OscType::Int(1), OscType::String(def)],
        }),
        from(),
    );
    assert_eq!(probe.list_key(), key);
    assert_eq!(local.0.borrow().defined, vec![key]);
    assert!(probe.picture().is_some(), "carried");

    // **An edit: a cut** -- the first half again, then the whole. The view
    // keeps its picture until the new one has come whole.
    let edited = segments(&[
        (&path, &peaks, 0, frames / 2, frames),
        (&path, &peaks, 0, frames, frames),
    ]);
    probe.host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: GUI_SET.into(),
            args: vec![
                OscType::Int(40),
                OscType::String("segments".into()),
                OscType::String(edited),
            ],
        }),
        from(),
    );
    assert_eq!(probe.list_key(), key, "the same take");
    assert_eq!(
        local.0.borrow().defined,
        vec![key, key],
        "told the new list"
    );
    let reload = probe
        .host
        .window_def_mut(1)
        .and_then(|t| t.find_mut(40))
        .and_then(|w| w.kind.as_samples_mut())
        .and_then(|s| s.wants_reload());
    assert!(reload.is_some(), "the view is owed the new list");
    let query = probe.fetches.want(1, 40, key).expect("asked again");
    probe.to_server(query);
    probe.pump(&local);
    assert_eq!(
        probe.picture().map(|p| p.0),
        Some(frames),
        "the old picture stays"
    );
    probe.drawn(760.0);
    probe.pump(&local);
    assert_eq!(
        probe.picture().map(|p| p.0),
        Some(frames + frames / 2),
        "the new one, whole"
    );
    assert_eq!(probe.sent(), Vec::new());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A float W64 of `frames` mono samples of a sine at `hz`, with its peaks;
/// answers the paths and the samples.
fn sine_file(
    dir: &std::path::Path,
    name: &str,
    frames: usize,
    hz: f32,
) -> (String, String, Vec<f32>) {
    let samples: Vec<f32> = (0..frames)
        .map(|i| 0.5 * (std::f32::consts::TAU * hz * i as f32 / 48_000.0).sin())
        .collect();
    let path = dir
        .join(format!("{name}.w64"))
        .to_string_lossy()
        .into_owned();
    let mut w = clausters_core::pcm::Writer::create(&path, 1, 48_000.0, "float").unwrap();
    w.write(&samples).unwrap();
    w.finalize().unwrap();
    let peaks = dir
        .join(format!("{name}.peaks"))
        .to_string_lossy()
        .into_owned();
    clausters_core::peaks::MultiPyramid::build_interleaved(&samples, 1, 256)
        .write_cache(&peaks)
        .unwrap();
    (path, peaks, samples)
}

/// **A list's spectrogram is analyzed where it is looked at**: the view is
/// given a dark column over the whole take, its first draw owes the columns
/// of what it shows, and those come from the files through the list's parts
/// -- what is analyzed is what plays, a fade included -- with nothing asked
/// of a server. A redefine keeps the columns and owes the new list's.
#[test]
fn a_lists_spectrogram_is_analyzed_from_its_files_over_the_span_shown() {
    let dir = std::env::temp_dir().join(format!("clausters-gui-columns-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let frames = 1 << 16;
    let (a, a_peaks, a_samples) = sine_file(&dir, "low", frames, 1_000.0);
    let (b, b_peaks, b_samples) = sine_file(&dir, "high", frames, 6_000.0);
    let fade = 4_000;
    let list = format!(
        r#"{{"channels":1,"sample_rate":48000,"parts":[
             {{"path":"{a}","peaks":"{a_peaks}","start":0,"frames":{frames},"fade_out":{fade},
               "source_channels":1,"source_frames":{frames},"source_rate":48000}},
             {{"path":"{b}","peaks":"{b_peaks}","start":0,"frames":{frames},"fade_in":{fade},
               "source_channels":1,"source_frames":{frames},"source_rate":48000}}]}}"#
    );

    let parsed =
        crate::host::disk::List::parse(&serde_json::from_str(&list).unwrap()).expect("a list");
    let mut probe = Probe::default();
    let local = Local::default();
    probe.host.set_disk_link(Box::new(local.clone()));
    let def = format!(
        r#"{{"type":"window","title":"w","children":[
             {{"id":40,"type":"signal","view":"spectrogram","window_size":512,
               "segments":{list}}}]}}"#
    );
    probe.host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: GUI_DEF.into(),
            args: vec![OscType::Int(1), OscType::String(def)],
        }),
        from(),
    );
    let key = probe.list_key();
    let query = probe.fetches.want(1, 40, key).expect("a query");
    probe.to_server(query);
    probe.pump(&local);
    let (widget, dark) = probe.textures.pop().expect("a texture placed");
    assert_eq!(widget, 40);
    assert!(
        dark[0].is_span() && dark[0].n_frames() == 1,
        "one dark column"
    );
    assert_eq!(dark[0].total_samples(), 2 * frames);

    // **The first draw owes the columns of what it shows**: the whole
    // take, 200 pixels wide.
    let nav = crate::viewport::View {
        start: 0.0,
        len: (2 * frames) as f64,
    };
    let owed = crate::host::frame::columns_owed_of(
        &dark[0],
        Some(&dark[0]),
        None,
        &nav,
        200.0,
        (512, 256, 0),
    )
    .expect("the columns are owed");
    let crate::host::frame::Owed::Columns(span) = owed else {
        panic!("{owed:?}")
    };
    // About a column to the pixel, at the power of two under it.
    let per_px = (2 * frames) / 200;
    assert!(span.hop.is_power_of_two() && span.hop <= per_px && span.hop * 2 > per_px);
    assert_eq!(span.start, 0);
    assert_eq!(span.window, 512);
    let ask = probe.fetches.columns.want(key, 1, 40, span).expect("asked");
    probe.to_server(ask);
    let replies = probe.pump(&local);
    assert!(replies >= 1, "{replies}");
    assert_eq!(probe.sent(), Vec::new(), "nothing went to a server");
    let (_, columns) = probe.textures.pop().expect("the columns placed");
    let stft = &columns[0];
    assert_eq!(
        (stft.n_frames(), stft.hop(), stft.origin()),
        (span.columns, span.hop, 0)
    );
    // And held, they owe nothing more.
    assert!(
        crate::host::frame::columns_owed_of(stft, Some(stft), None, &nav, 200.0, (512, 256, 0))
            .is_none()
    );

    // **Each column is the analysis of what plays there**, each bin the
    // largest it was over the windows every half window inside it: a window
    // over a seam or a fade is the list's samples through its parts and
    // fades, analyzed as a whole signal is; one inside a part is that part's
    // file at the nearest point of a grid of the half window laid over the
    // file, which is how an edit's or a zoom's columns are found again
    // (`disk::answer`).
    let grid = 256;
    let source_of = |p: &crate::host::disk::ListPart| {
        if p.path.as_ref() == a {
            &a_samples
        } else {
            &b_samples
        }
    };
    let inside = |first: usize| {
        parsed.part_at(first).filter(|p| {
            let s = &p.span;
            !s.resampled()
                && first >= s.start() + s.fade_in
                && first + 512 <= s.end().saturating_sub(s.fade_out)
                && (s.src_start + first - s.start() + grid / 2) / grid * grid + 512
                    <= s.src_frames()
        })
    };
    let played = |f: usize| {
        let part = parsed.part_at(f);
        part.map_or(0.0, |p| {
            let src = if p.path.as_ref() == a {
                &a_samples
            } else {
                &b_samples
            };
            p.span.read_with(f - p.span.start(), 0, |f, _| {
                src.get(f).copied().unwrap_or(0.0)
            })
        })
    };
    let (hann, gain) = clausters_core::stft::analysis_window(512);
    let (mut w, mut sp, mut col) = (vec![0.0; 512], vec![0.0; 256], vec![0.0; 256]);
    for c in 0..stft.n_frames() {
        let mut want = vec![0u8; 256];
        let end = ((c + 1) * span.hop).min(2 * frames).max(c * span.hop + 1);
        for first in (c * span.hop..end).step_by(grid) {
            let frame: Vec<f32> = match inside(first) {
                Some(p) => {
                    let s = &p.span;
                    let at = (s.src_start + first - s.start() + grid / 2) / grid * grid;
                    source_of(&p)[at..at + 512].to_vec()
                }
                None => (first..first + 512)
                    .map(|f| if f < 2 * frames { played(f) } else { 0.0 })
                    .collect(),
            };
            clausters_core::stft::column_into(&frame, &hann, gain, &mut w, &mut sp, &mut col);
            for (o, m) in want.iter_mut().zip(&col) {
                *o = (*o).max(clausters_core::stft::quantize(*m));
            }
        }
        let got: Vec<u8> = stft
            .column(c)
            .iter()
            .map(|m| clausters_core::stft::quantize(*m))
            .collect();
        assert_eq!(got, want, "column {c}");
    }
    // The low part is loudest near 1 kHz, the high one near 6 kHz.
    let loudest = |c: usize| {
        let col = stft.column(c);
        (0..col.len())
            .max_by(|&i, &j| col[i].total_cmp(&col[j]))
            .unwrap() as f32
            * 48_000.0
            / 512.0
    };
    assert!((loudest(10) - 1_000.0).abs() < 200.0, "{}", loudest(10));
    assert!((loudest(stft.n_frames() - 20) - 6_000.0).abs() < 200.0);

    // **A redefine owes the new list's columns**, keeping these, over the
    // whole of the take as long as it is now: a paste that made it longer
    // was asked for over the length it had, and the part it gained stayed
    // one column stretched to the end.
    for now in [frames, 2 * frames, 3 * frames] {
        let owed = crate::host::frame::columns_owed_of(
            stft,
            Some(stft),
            Some(now),
            &nav,
            200.0,
            (512, 256, 0),
        )
        .expect("the new list's columns");
        let crate::host::frame::Owed::Columns(again) = owed else {
            panic!("{owed:?}")
        };
        assert_eq!((again.start, again.total), (0, now));
        assert!(again.hop * (again.columns - 1) >= now, "{again:?}");
    }
    // **Zoomed in, the span shown is owed over the whole take held**: a
    // tenth of it, at a power of two near a column to the pixel.
    let near = crate::viewport::View {
        start: frames as f64,
        len: (2 * frames / 10) as f64,
    };
    let owed =
        crate::host::frame::columns_owed_of(stft, Some(stft), None, &near, 200.0, (512, 256, 0))
            .expect("the span's columns");
    let crate::host::frame::Owed::Columns(close) = owed else {
        panic!("{owed:?}")
    };
    assert!(
        close.start < frames + 1
            && close.start + close.hop * close.columns > frames + 2 * frames / 10
    );
    assert!(close.hop.is_power_of_two() && close.hop < span.hop);
    // **What is on screen is asked alone first**, when nothing held draws
    // it: a span comes whole, and the view alone comes in half the time.
    assert_eq!(close.start, frames / close.hop * close.hop, "{close:?}");
    let of = |ask: &crate::host::columns::ColumnSpan| {
        crate::spectrogram::Stft::of_span(
            vec![0.0; ask.columns * 256],
            256,
            ask.start,
            ask.hop,
            512,
            48_000.0,
            2 * frames,
        )
        .unwrap()
        .with_analysis(256, 0)
    };
    // **Then the span reaches half a view past either edge**, and is asked
    // again once the view comes within a quarter of a view of what is held:
    // a zoom out or a pan reveals what is beside the view while the next
    // span comes, instead of the whole take's columns.
    let owed = crate::host::frame::columns_owed_of(
        &of(&close),
        Some(stft),
        None,
        &near,
        200.0,
        (512, 256, 0),
    )
    .expect("the margins, once the view is held");
    let crate::host::frame::Owed::Columns(far) = owed else {
        panic!("{owed:?}")
    };
    assert!(
        far.start <= frames - frames / 10 && far.start + frames / 10 >= frames - frames / 5,
        "{far:?}"
    );
    assert_eq!(far.hop, close.hop);
    let held = of(&far);
    let owes = |at: f64| {
        let view = crate::viewport::View {
            start: at,
            len: near.len,
        };
        crate::host::frame::columns_owed_of(&held, Some(stft), None, &view, 200.0, (512, 256, 0))
            .is_some()
    };
    assert!(!owes(near.start), "the view held, with its margins");
    assert!(
        owes(near.start + near.len * 0.3),
        "within a quarter of a view of the edge held"
    );
    // **No column is finer than the analysis's own hop**: a view of a few
    // frames is drawn by the windows on its grid, one a block.
    let tiny = crate::viewport::View {
        start: frames as f64,
        len: 4.0,
    };
    let owed =
        crate::host::frame::columns_owed_of(stft, Some(stft), None, &tiny, 200.0, (512, 256, 0))
            .expect("the windows under a few frames");
    let crate::host::frame::Owed::Columns(tiny) = owed else {
        panic!("{owed:?}")
    };
    assert_eq!(tiny.hop, 256, "{tiny:?}");
    // **A whole take analyzed as the file's spectral summary is asked a
    // summary column apart at least**, so it is read from the summary.
    let summary = (
        clausters_core::spectra::WINDOW,
        clausters_core::spectra::HOP,
        0,
    );
    let owed = crate::host::frame::columns_owed_of(stft, Some(stft), None, &nav, 200.0, summary)
        .expect("the summary's columns");
    let crate::host::frame::Owed::Columns(whole) = owed else {
        panic!("{owed:?}")
    };
    assert_eq!(whole.start, 0);
    assert!(
        whole.hop >= clausters_core::spectra::COLUMN_FRAMES,
        "{whole:?}"
    );
    // **Another analysis owes the whole take again**: a window, a hop or a
    // shape the held columns are not of.
    for analysis in [(2048, 1024, 0), (512, 128, 0), (512, 256, 4)] {
        let owed =
            crate::host::frame::columns_owed_of(stft, Some(stft), None, &nav, 200.0, analysis)
                .expect("the columns of the analysis asked for");
        let crate::host::frame::Owed::Columns(again) = owed else {
            panic!("{owed:?}")
        };
        assert_eq!((again.start, again.window), (0, analysis.0), "{analysis:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
