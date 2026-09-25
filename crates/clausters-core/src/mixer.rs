//! **What a multitrack is as nodes and groups** -- its node system,
//! written once.
//!
//! An automation whose target says `gain` has to drive the *same* `gain` the
//! track's knob shows, and until something says what a track and a clip **are**
//! on the server those are two words that happen to be spelled alike. This
//! module is that something: the defs a multitrack is made of, and the names their
//! surfaces answer to.
//!
//! It is here, and not in a client, for the reason [`crate::patch`] is here --
//! every client that plays a multitrack needs the identical wiring, and a wiring
//! written twice is two answers waiting to differ. A caller states the facts
//! (how many channels a track has, which buffer a box reads) and gets the
//! specs; the caller sends them, stamps the ids and instantiates, because ids
//! are the caller's and nothing here knows a node's number.
//!
//! # The shape, and why it is the field's
//!
//! Every fixed-channel DAW has the same channel strip, and the order is not
//! arbitrary -- it is what makes gain staging, sends and metering mean what
//! people expect:
//!
//! ```text
//! in -> [pre-fader inserts] -> fader (+ mute) -> pan/width -> [post inserts]
//!    -> sends -> out
//! ```
//!
//! A **clip** is that strip over readers; a **track** is that strip over clips;
//! the **master** is that strip over tracks. Three instances of one idea, which
//! is why they are three GraphDefs over one [`strip_def`] rather than three
//! hand-wired graphs. A clip's gain and a track's gain are *both* real and they
//! are different stages: the first corrects the take, the second mixes it.
//!
//! # What is deliberately not here yet
//!
//! **Effects**: `mt.track` and `mt.clip` declare the slot and leave it empty,
//! because a send to nowhere and an insert of nothing cannot be heard and so
//! cannot be checked. The slot is a nested-graph slot, so an effect may itself
//! be a GraphDef and a compound effect needs no new mechanism.
//!
//! **Widths past stereo**: [`strip_def`] is written for 1 and 2 channels, which
//! is what a track declares today. The general N->M downmix (BS.775 and its
//! relatives) is named where the widths are checked and refused.

use serde_json::{Value, json};

/// The prefix every def here is named with. Deliberately not `track` or `clip`
/// on their own: a def name is global, and those two words are the most
/// ambiguous ones available.
pub const PREFIX: &str = "mt";

// ---- the port vocabulary ----
//
// A port is a path. These are the names an `Automation`'s target resolves
// against, the names a header knob writes, and the names a `/node_set` uses --
// one set, which is the whole point of the module.

/// Linear gain. The fader of whatever strip it is on.
pub const GAIN: &str = "gain";
/// Position, `-1..1`. A **pan** over a mono source (an equal-power law that
/// centres at -3 dB) and a **balance** over a stereo one (which attenuates one
/// side): the same word for two laws, because they are the same gesture and
/// confusing them is the classic mixer bug -- so which one applies follows from
/// the source's width and never from a second name.
pub const PAN: &str = "pan";
/// Stereo width, `0..2`. `1` leaves the image alone; `0` collapses it to mono.
pub const WIDTH: &str = "width";
/// Silenced: `0` or `1`, and it is a **control of the fader**, not a state of
/// the group. A mute is automated like every other curve, a hard cut between
/// two samples is a click, and stopping the group would take the tail of an
/// effect and the pre-fader sends with it. Turning a silent group off is a
/// separate question -- one about cost, not about sound.
pub const MUTE: &str = "mute";
/// Where a box starts on the transport, in frames.
pub const AT: &str = "at";
/// How long a box lasts, in frames.
pub const SPAN: &str = "span";
/// The **second of the source** a box's own zero reads.
///
/// Seconds and not frames, because the frame it lands on is the *buffer's*
/// question and the buffer is what knows its own sample rate: the reader
/// crosses it with `BufSampleRate`, the way it crosses the playing speed with
/// `BufRateScale`. A caller that converted here would need a table of every
/// source's rate to hand a number the server can already work out.
pub const START: &str = "start";
/// **How fast a box reads its source**, as a factor over playing it at its own
/// pitch: `1` is the recording as it was made, `2` an octave up and half as
/// long. The region's `playrate`, and nothing else.
///
/// It is a factor and not the reading speed itself because the reading speed is
/// two things multiplied and only one of them is anybody's decision. The other
/// is the source's own rate against the engine's (`BufRateScale`), which the
/// reader reads off the buffer -- so a 44.1 kHz take in a 48 kHz session plays
/// at its true pitch with nothing said, which is what `PlayBuf` has always
/// meant by `BufRateScale(buf) * rate`.
pub const RATE: &str = "rate";
/// The buffer a box reads.
///
/// **Not initial-rate**, deliberately: `BufRd` looks the buffer up once a
/// block, so this is an ordinary control and changing which buffer a box reads
/// is a `/node_set` rather than a new node. That matters because re-cutting a
/// join *is* a new buffer -- a stitched one is replaced rather than edited -- and
/// a box that had to be rebuilt for it would stop and restart every time a hand
/// moved a seam.
pub const BUF: &str = "buf";
/// Which channel of the source a reader takes.
pub const CHAN: &str = "chan";
/// Whether the window wraps past the end of its source.
pub const LOOP: &str = "loop";

/// The slot a clip's readers fill: one per channel of the source.
pub const SOURCE_SLOT: &str = "source";
/// The slot a track's clips of a **mono** source fill. There is one clip slot
/// per source width, because a slot names one def and a mono take and a stereo
/// take are not the same wiring: the first is panned into the track and the
/// second is balanced. Which slot a box goes in is the source's width, which is
/// the one fact about the box that decides it.
pub fn clip_slot(inputs: usize) -> String {
    format!("clips.{inputs}")
}
/// The slot a multitrack's tracks fill -- a slot of the tracks' group
/// ([`tracks_graph`]), not of the multitrack itself.
pub const TRACK_SLOT: &str = "tracks";
/// **The slot the tracks' group fills**, once per multitrack: the subtree the
/// transport governs. The master around it is not, so it goes on running
/// while the transport is stopped.
pub const TRANSPORT_SLOT: &str = "transport";
/// The slot an effect chain fills. Declared and empty -- see the module docs.
pub const FX_SLOT: &str = "fx";

/// The bus a nested graph is handed by whoever instantiates it.
pub const OUT_BUS: &str = "out";
/// The bus a strip mixes its sources on, private to each instance.
pub const MIX_BUS: &str = "mix";
/// The bus a strip **writes**, private to each instance: what this strip alone
/// sounds like, after its own fader.
///
/// A strip could write straight into whatever it feeds, and until something
/// wanted to *listen to one strip* that was enough. A meter is that something:
/// every track writes into the master's mix bus, so a meter there would read
/// the whole multitrack and call it the track. So the output has a place of its own
/// and what leaves it is a [`send_def`] -- one node per strip, which is also
/// exactly the shape an extra send wants.
pub const POST_BUS: &str = "post";
/// The port a meter's first channel is told to write to. A port and not a
/// private bus: a host reads the level, so the host says where it lands.
pub const METER_OUT0: &str = "meter/out0";
/// The port a meter's second channel is told to write to.
pub const METER_OUT1: &str = "meter/out1";
/// The port a meter's fall is set on, in decibels per second.
pub const METER_DECAY_PORT: &str = "meter/decay";
/// The port a meter's peak hold is set on, in seconds.
pub const METER_HOLD_PORT: &str = "meter/hold";
/// The port a strip's output gain is set on: unity is the ordinary wire to
/// whatever the strip feeds.
pub const SEND_GAIN: &str = "send/gain";

/// The slot a strip's meters fill.
///
/// A slot and not a member, so a multitrack nobody is looking at costs no meters at
/// all -- and so both halves of what a meter shows (the level, and the mark
/// that waits) are two instances of one def rather than a second def.
pub const METER_SLOT: &str = "meters";

/// The widths a strip is written for. Past stereo is a downmix table and a
/// decision about which one, and it is refused rather than guessed.
pub const MAX_CHANNELS: usize = 2;

/// The name of the reader def: what one channel of one box sounds as.
pub fn reader_name() -> String {
    format!("{PREFIX}.reader")
}

/// The name of the meter def for a strip of `channels`.
pub fn meter_name(channels: usize) -> String {
    format!("{PREFIX}.meter.{channels}")
}

/// The name of a **track's** meter def for a strip of `channels`: the meter,
/// closed by the transport's declick so it reads zero once a stop has frozen
/// it ([`track_meter_def`]).
pub fn track_meter_name(channels: usize) -> String {
    format!("{PREFIX}.trackmeter.{channels}")
}

/// The name of the send def for `channels`.
pub fn send_name(channels: usize) -> String {
    format!("{PREFIX}.send.{channels}")
}

/// The name of the curve def: what makes an automation a control.
pub fn curve_name() -> String {
    format!("{PREFIX}.curve")
}

/// The name of the strip def for `inputs` channels in and `outputs` out.
pub fn strip_name(inputs: usize, outputs: usize) -> String {
    format!("{PREFIX}.strip.{inputs}x{outputs}")
}

/// The name of the clip graph for a source of `inputs` channels on a track of
/// `outputs`.
pub fn clip_name(inputs: usize, outputs: usize) -> String {
    format!("{PREFIX}.clip.{inputs}x{outputs}")
}

/// The name of the track graph for a track of `channels`.
pub fn track_name(channels: usize) -> String {
    format!("{PREFIX}.track.{channels}")
}

/// The name of the tracks' group for a multitrack of `channels`.
pub fn tracks_name(channels: usize) -> String {
    format!("{PREFIX}.tracks.{channels}")
}

/// The name of the master's way out for `channels`: a send with the
/// transport's declick.
pub fn out_name(channels: usize) -> String {
    format!("{PREFIX}.out.{channels}")
}

/// The name of the multitrack graph for one of `channels`.
pub fn multitrack_name(channels: usize) -> String {
    format!("{PREFIX}.multitrack.{channels}")
}

/// Refuses a width this module has no wiring for, saying which it has.
fn check(channels: usize, what: &str) -> Result<(), String> {
    if channels == 0 || channels > MAX_CHANNELS {
        return Err(format!(
            "{what}: {channels} channels; the strip is written for 1 and {MAX_CHANNELS} \
             (a wider one is a downmix table, and which table is a decision)"
        ));
    }
    Ok(())
}

// ---- the defs ----

/// One control of a def spec, with its default. The audio editor's defs use
/// it too.
pub(crate) fn control(name: &str, default: f32) -> Value {
    json!({ "name": name, "default": default })
}

fn lagged(name: &str, default: f32) -> Value {
    // Ten milliseconds: long enough that a jump between two samples is a ramp
    // rather than a click, short enough that a hand does not hear it.
    json!({ "name": name, "default": default, "lag": 0.01 })
}

/// **One channel of one box**: a reader that plays a span of a buffer at the
/// place on the transport the box sits, and silence everywhere else.
///
/// It is *resident*: it is created when the box is, and it stays. Where the
/// transport is playing from is `TransportPos`'s to say, so moving the box is a
/// `/node_set` of `at` and a locate is no message at all -- the reader is
/// wherever the transport says it is. That is the whole reason a multitrack plays
/// itself rather than being driven.
///
/// The window is a gate rather than a schedule for the same reason: `at` and
/// `span` are read every block, so an edit while it sounds lands on the next
/// block and nothing has to be re-armed.
///
/// # The transport is in engine samples and the buffer is in its own frames
///
/// The two are the same number only while the source was recorded at the rate
/// the engine runs at, and a reader that added them together said they always
/// were: a 44.1 kHz take in a 48 kHz session played 8.8% sharp, and its window
/// began at the wrong frame by the same ratio. So the phase is crossed rather
/// than added -- `BufRateScale(buf)` frames of source per engine sample, times
/// [`RATE`], and [`START`] crossed by `BufSampleRate(buf)`. Both come off the
/// buffer itself, which is the one thing that knows what rate its samples were
/// written at; it is `PlayBuf(buf, BufRateScale(buf) * rate)` spelled inside
/// the def that plays a box, and it needs nothing said by a client.
pub fn reader_def() -> Value {
    json!({
        "name": reader_name(),
        "controls": [
            control(OUT_BUS, 0.0),
            control(BUF, 0.0),
            control(CHAN, 0.0),
            control(AT, 0.0),
            control(SPAN, 0.0),
            control(START, 0.0),
            control(LOOP, 0.0),
            control(RATE, 1.0),
            lagged(GAIN, 1.0),
        ],
        "ugens": [
            // 0: engine samples since this box began; negative before it starts.
            {"kind": "TransportPos", "inputs": [{"control": 3}]},
            // 1..3: inside the window? The gate is the transport's, so it is
            // asked in the transport's own samples and no rate reaches it.
            {"kind": "BinaryOpUGen", "op": "ge", "inputs": [{"ugen": 0}, {"const": 0.0}]},
            {"kind": "BinaryOpUGen", "op": "lt", "inputs": [{"ugen": 0}, {"control": 4}]},
            {"kind": "Mul", "inputs": [{"ugen": 1}, {"ugen": 2}]},
            // 4..6: frames of the source per engine sample -- the buffer's own
            // rate against the engine's, times the box's playrate -- and the
            // frames that many make since the box began.
            {"kind": "BufRateScale", "inputs": [{"control": 1}]},
            {"kind": "Mul", "inputs": [{"ugen": 4}, {"control": 7}]},
            {"kind": "Mul", "inputs": [{"ugen": 0}, {"ugen": 5}]},
            // 7..9: the window's own start, a second of the source crossed by
            // the buffer's rate, and the frame that is, read interpolated.
            {"kind": "BufSampleRate", "inputs": [{"control": 1}]},
            {"kind": "Mul", "inputs": [{"ugen": 7}, {"control": 5}]},
            {"kind": "Add", "inputs": [{"ugen": 6}, {"ugen": 8}]},
            {"kind": "BufRd", "inputs": [
                {"control": 1}, {"control": 2}, {"ugen": 9}, {"control": 6}
            ]},
            // 11..13: gated, levelled, out.
            {"kind": "Mul", "inputs": [{"ugen": 10}, {"ugen": 3}]},
            {"kind": "Mul", "inputs": [{"ugen": 11}, {"control": 8}]},
            {"kind": "Out", "inputs": [{"control": 0}, {"ugen": 12}]}
        ]
    })
}

/// The fall of a meter, in decibels per second. The field's value for a peak
/// meter, and a number rather than a taste: the slope on screen is what makes
/// the picture readable as a *rate*.
pub const METER_DECAY: f32 = 20.0;

/// How long a peak mark stays put before it begins to fall, in seconds. Long
/// enough that an eye arriving late still finds it.
pub const METER_HOLD: f32 = 1.5;

/// How many frames one sample of a curve's table covers.
///
/// One block at the usual rate. A curve is read once a block anyway --
/// [`curve_def`] writes a control bus, and a control bus is a value per block --
/// so a finer table would be samples nothing can hear the difference of, and a
/// coarser one would step where a fader should glide.
pub const CURVE_STEP: f64 = 64.0;

/// **What makes an automation a control**: a table read at the transport's own
/// position, written to a control bus.
///
/// The alternative was a message per block from the client, and it fails for a
/// reason worth stating: a locate would leave the curve wherever the last
/// message put it, so every seek would need the whole thing re-sent, and the
/// resolution of the automation would be the resolution of a socket. Read from
/// the transport instead, the curve is simply *at* wherever the transport is --
/// a locate costs nothing at all, and it works the same offline.
///
/// `BufRd` clamps a phase past either end when it is not looping, which is
/// exactly what a curve should do: before its first point it holds the first
/// value and after its last it holds the last.
pub fn curve_def() -> Value {
    json!({
        "name": curve_name(),
        "controls": [
            control(OUT_BUS, 0.0),
            control(BUF, 0.0),
            control(AT, 0.0),
            control("step", CURVE_STEP as f32),
        ],
        "ugens": [
            // 0..1: how far into the curve the transport is, in table samples.
            {"kind": "TransportPos", "inputs": [{"control": 2}]},
            {"kind": "Div", "inputs": [{"ugen": 0}, {"control": 3}]},
            // 2..3: the value there, onto the control bus the port is mapped to.
            {"kind": "BufRd", "inputs": [
                {"control": 1}, {"const": 0.0}, {"ugen": 1}, {"const": 0.0}
            ]},
            {"kind": "OutCtl", "inputs": [{"control": 0}, {"ugen": 2}]}
        ]
    })
}

/// **What a strip's output reads as**: `channels` channels in, one control bus
/// out per channel, with the ballistics that make a level legible.
///
/// A meter is **one number a block**, which is exactly what a control bus
/// carries -- so a metered strip costs one node and one bus per channel, and a
/// host reads a *range* of buses rather than one per message. A finer answer
/// would be samples nothing can read.
///
/// It is written by width rather than per channel because it is a **slot**
/// member, and a slot names one def: every instance of it is wired the same
/// way, so one instance has to cover the strip's whole width. Two of them give
/// both halves of what a meter shows: one with no hold is the level, one with
/// [`METER_HOLD`] is the mark that waits to be read. The ballistics are applied
/// here rather than by whoever draws, so two clients cannot draw two different
/// falls off one signal.
pub fn meter_def(channels: usize) -> Result<Value, String> {
    check(channels, "meter")?;
    Ok(meter_spec(&meter_name(channels), channels, MAX_CHANNELS))
}

/// **A track's meter**: [`meter_def`], closed while the transport's declick is
/// under half.
///
/// A track is governed, so a stop freezes its meter with whatever it last
/// wrote, and a meter that gets no time to fall would go on claiming that
/// level. With the transport's ramp the track keeps running for the length of
/// the ramp after the stop -- which is also when a client's zeroing of the
/// bus lands -- so the meter closes itself on the way down, and the last value
/// it writes before the freeze is zero. The master's meter is outside the
/// governed group and falls on its own; it is [`meter_def`].
pub fn track_meter_def(channels: usize) -> Result<Value, String> {
    check(channels, "meter")?;
    Ok(meter_spec_gated(
        &track_meter_name(channels),
        channels,
        MAX_CHANNELS,
        true,
    ))
}

/// **The meter, written once**: `channels` levels read off `in0..`, each onto
/// the control bus its `out` names, with the field's ballistics. `slots` is how
/// many `in`/`out` pairs the def declares, at least `channels` -- the
/// multitrack's strips declare two whatever their width, so one wiring fits
/// both -- and the rest of the controls come after them: `decay`, then `hold`.
///
/// Public because it is the audio editor's meter too
/// ([`crate::audio_editor::meter_def`]): the same algorithm under another
/// application's name.
pub fn meter_spec(name: &str, channels: usize, slots: usize) -> Value {
    meter_spec_gated(name, channels, slots, false)
}

/// [`meter_spec`], and with `gated` each level times whether the transport's
/// declick is above half (`TransportFade > 0.5`): see [`track_meter_def`].
fn meter_spec_gated(name: &str, channels: usize, slots: usize, gated: bool) -> Value {
    let slots = slots.max(channels);
    let mut controls = Vec::new();
    for channel in 0..slots {
        controls.push(control(&format!("in{channel}"), 0.0));
    }
    for channel in 0..slots {
        controls.push(control(&format!("out{channel}"), 0.0));
    }
    controls.push(control("decay", METER_DECAY));
    controls.push(control("hold", 0.0));
    let (decay, hold) = (2 * slots as u32, 2 * slots as u32 + 1);
    let mut ugens = Vec::new();
    // The gate, first when there is one: at control rate, so it is the
    // declick's level at the start of each slice.
    let gate = gated.then(|| {
        ugens.push(json!({"kind": "TransportFade", "rate": "kr", "inputs": []}));
        ugens.push(json!({"kind": "BinaryOpUGen", "op": "gt", "rate": "kr",
                          "inputs": [{"ugen": 0}, {"const": 0.5}]}));
        1u32
    });
    for channel in 0..channels {
        // Three UGens per channel (four gated), found by where each one lands
        // rather than by a stride: a second channel reading the first
        // channel's node does not fail -- every index is a valid UGen -- it
        // silently meters the wrong one.
        let read = ugens.len() as u32;
        ugens.push(json!({"kind": "In", "inputs": [{"control": channel}]}));
        ugens.push(json!({"kind": "Meter", "inputs": [
            {"ugen": read}, {"control": decay}, {"control": hold}
        ]}));
        let mut level = read + 1;
        if let Some(gate) = gate {
            ugens.push(json!({"kind": "Mul", "inputs": [{"ugen": level}, {"ugen": gate}]}));
            level = ugens.len() as u32 - 1;
        }
        ugens.push(json!({"kind": "OutCtl", "inputs": [
            {"control": slots + channel}, {"ugen": level}
        ]}));
    }
    json!({ "name": name, "controls": controls, "ugens": ugens })
}

/// **What leaves a strip**: the strip's own output bus, at a gain, onto the bus
/// it feeds.
///
/// The wire from a track to the master is one of these at unity, and that is
/// not a detour: a strip writes to its own [`POST_BUS`] so that a meter has
/// something to read that is *this strip* (see there), and something has to
/// carry it the rest of the way. Having a gain on it means an extra send --
/// post-fader, since `post` is after the fader -- is another instance of this
/// def pointed at another bus, and needs no new mechanism when sends are taken.
pub fn send_def(channels: usize) -> Result<Value, String> {
    check(channels, "send")?;
    let controls = vec![
        control("in0", 0.0),
        control("in1", 0.0),
        control("out0", 0.0),
        control("out1", 0.0),
        lagged(GAIN, 1.0),
    ];
    let mut ugens = Vec::new();
    for channel in 0..channels {
        // Three UGens per channel, so the stride is three -- see `meter_def`,
        // which had the same slip: at two, the right channel wrote the bus it
        // read and never passed through the gain.
        let read = 3 * channel as u32;
        ugens.push(json!({"kind": "In", "inputs": [{"control": channel}]}));
        ugens.push(json!({"kind": "Mul", "inputs": [{"ugen": read}, {"control": 4}]}));
        ugens.push(json!({"kind": "Out", "inputs": [
            {"control": 2 + channel}, {"ugen": read + 1}
        ]}));
    }
    Ok(json!({ "name": send_name(channels), "controls": controls, "ugens": ugens }))
}

/// **What leaves the master for the hardware**: the send, times the
/// transport's declick level (`TransportFade`).
///
/// The master is outside the governed group, so it runs through a stop; with
/// the transport's ramp (`/transport_fade`) the tracks go on playing while the
/// level falls, and this is where it is applied -- once, after the mix, so a
/// stop and a play fade the whole multitrack and nothing clicks. A track's
/// own send is [`send_def`], unfaded, since it runs inside the governed group.
pub fn out_def(channels: usize) -> Result<Value, String> {
    check(channels, "out")?;
    let controls = vec![
        control("in0", 0.0),
        control("in1", 0.0),
        control("out0", 0.0),
        control("out1", 0.0),
        lagged(GAIN, 1.0),
    ];
    let mut ugens = vec![json!({"kind": "TransportFade", "inputs": []})];
    let level = 1u32;
    ugens.push(json!({"kind": "Mul", "inputs": [{"ugen": 0}, {"control": 4}]}));
    for channel in 0..channels {
        let read = ugens.len() as u32;
        ugens.push(json!({"kind": "In", "inputs": [{"control": channel}]}));
        ugens.push(json!({"kind": "Mul", "inputs": [{"ugen": read}, {"ugen": level}]}));
        ugens.push(json!({"kind": "Out", "inputs": [
            {"control": 2 + channel}, {"ugen": read + 1}
        ]}));
    }
    Ok(json!({ "name": out_name(channels), "controls": controls, "ugens": ugens }))
}

/// **The channel strip**: `inputs` channels in, gain and mute, then the image,
/// then `outputs` channels out.
///
/// One def for the clip, the track and the master, because they are one shape.
/// `mute` multiplies rather than stopping anything (see [`MUTE`]), and `pan`
/// is a pan law over a mono input and a balance over a stereo one (see
/// [`PAN`]) -- so a mono take on a stereo track is centred at -3 dB a side,
/// which is what every mixer does and what naming one channel would not.
///
/// Each channel is its own `Out`, because a UGen has one output; the bus
/// indices come in as separate controls (`out0`, `out1`), which is what
/// `"mix:1"` in a GraphDef exists to fill.
pub fn strip_def(inputs: usize, outputs: usize) -> Result<Value, String> {
    check(inputs, "strip inputs")?;
    check(outputs, "strip outputs")?;
    let mut controls = vec![control("in0", 0.0), control("in1", 0.0)];
    controls.push(control("out0", 0.0));
    controls.push(control("out1", 0.0));
    controls.push(lagged(GAIN, 1.0));
    controls.push(lagged(MUTE, 0.0));
    controls.push(lagged(PAN, 0.0));
    controls.push(lagged(WIDTH, 1.0));
    // Control indices, by the order above.
    const IN0: u32 = 0;
    const IN1: u32 = 1;
    const OUT0: u32 = 2;
    const OUT1: u32 = 3;
    const GAIN_C: u32 = 4;
    const MUTE_C: u32 = 5;
    const PAN_C: u32 = 6;
    const WIDTH_C: u32 = 7;

    let mut ugens: Vec<Value> = Vec::new();
    let push = |u: Value, ugens: &mut Vec<Value>| -> u32 {
        ugens.push(u);
        (ugens.len() - 1) as u32
    };

    // The fader: gain, then mute as `1 - mute` so it is a control like any
    // other and rides the same lag.
    let open = push(
        json!({"kind": "BinaryOpUGen", "op": "sub",
               "inputs": [{"const": 1.0}, {"control": MUTE_C}]}),
        &mut ugens,
    );
    let level = push(
        json!({"kind": "Mul", "inputs": [{"ugen": open}, {"control": GAIN_C}]}),
        &mut ugens,
    );

    let read0 = push(
        json!({"kind": "In", "inputs": [{"control": IN0}]}),
        &mut ugens,
    );
    let left_in = push(
        json!({"kind": "Mul", "inputs": [{"ugen": read0}, {"ugen": level}]}),
        &mut ugens,
    );
    let right_in = if inputs == 2 {
        let read1 = push(
            json!({"kind": "In", "inputs": [{"control": IN1}]}),
            &mut ugens,
        );
        Some(push(
            json!({"kind": "Mul", "inputs": [{"ugen": read1}, {"ugen": level}]}),
            &mut ugens,
        ))
    } else {
        None
    };

    let (left, right) = match (inputs, outputs, right_in) {
        // Mono in, stereo out: a **pan**, with the law. Not the same signal
        // twice -- that is 3 dB too loud in the middle and is the mistake this
        // names on purpose.
        (1, 2, _) => {
            let l = push(
                json!({"kind": "Pan2", "inputs": [
                    {"ugen": left_in}, {"control": PAN_C}, {"const": 1.0}, {"const": 0.0}]}),
                &mut ugens,
            );
            let r = push(
                json!({"kind": "Pan2", "inputs": [
                    {"ugen": left_in}, {"control": PAN_C}, {"const": 1.0}, {"const": 1.0}]}),
                &mut ugens,
            );
            (l, Some(r))
        }
        // Stereo in, stereo out: a **balance**, then the width.
        //
        // A balance *attenuates one side* and leaves the centre alone, which is
        // what `Balance2` is not: that applies the equal-power pan law to a
        // stereo pair, so a centred strip comes out 3 dB down and three strips
        // in series -- clip, track, master -- take 9 dB off a mix for
        // nothing. So the law is written here: `min(1, 1 -/+ pan)`, unity at the
        // centre and silence at the far end.
        (2, 2, Some(right_in)) => {
            let lg = push(
                json!({"kind": "BinaryOpUGen", "op": "sub",
                       "inputs": [{"const": 1.0}, {"control": PAN_C}]}),
                &mut ugens,
            );
            let lg = push(
                json!({"kind": "BinaryOpUGen", "op": "min",
                       "inputs": [{"ugen": lg}, {"const": 1.0}]}),
                &mut ugens,
            );
            let rg = push(
                json!({"kind": "Add", "inputs": [{"const": 1.0}, {"control": PAN_C}]}),
                &mut ugens,
            );
            let rg = push(
                json!({"kind": "BinaryOpUGen", "op": "min",
                       "inputs": [{"ugen": rg}, {"const": 1.0}]}),
                &mut ugens,
            );
            let bl = push(
                json!({"kind": "Mul", "inputs": [{"ugen": left_in}, {"ugen": lg}]}),
                &mut ugens,
            );
            let br = push(
                json!({"kind": "Mul", "inputs": [{"ugen": right_in}, {"ugen": rg}]}),
                &mut ugens,
            );
            let wl = push(
                json!({"kind": "StereoWidth", "inputs": [
                    {"ugen": bl}, {"ugen": br}, {"control": WIDTH_C}, {"const": 0.0}]}),
                &mut ugens,
            );
            let wr = push(
                json!({"kind": "StereoWidth", "inputs": [
                    {"ugen": bl}, {"ugen": br}, {"control": WIDTH_C}, {"const": 1.0}]}),
                &mut ugens,
            );
            (wl, Some(wr))
        }
        // Stereo in, mono out: the sum, which is the honest downmix at this
        // width and the only one anybody agrees on.
        (2, 1, Some(right_in)) => {
            let sum = push(
                json!({"kind": "Add", "inputs": [{"ugen": left_in}, {"ugen": right_in}]}),
                &mut ugens,
            );
            (sum, None)
        }
        // Mono in, mono out: the signal.
        (1, 1, _) => (left_in, None),
        _ => unreachable!("check() refused every other width"),
    };

    ugens.push(json!({"kind": "Out", "inputs": [{"control": OUT0}, {"ugen": left}]}));
    if outputs == 2 {
        let right = right.expect("a stereo output has a right channel");
        ugens.push(json!({"kind": "Out", "inputs": [{"control": OUT1}, {"ugen": right}]}));
    }

    Ok(json!({ "name": strip_name(inputs, outputs), "controls": controls, "ugens": ugens }))
}

/// The buses a strip's `in`/`out` controls are wired to, given a bus name and a
/// width. Channel 1 of a mono bus does not exist, so it is left at 0 -- the def
/// never reads it.
fn strip_wiring(input: &str, inputs: usize, output: &str, outputs: usize) -> Value {
    json!({
        "in0": format!("{input}:0"),
        "in1": if inputs == 2 { format!("{input}:1") } else { format!("{input}:0") },
        "out0": format!("{output}:0"),
        "out1": if outputs == 2 { format!("{output}:1") } else { format!("{output}:0") },
    })
}

/// The buses a meter reads, given the strip's output bus and its width.
fn meter_wiring(input: &str, channels: usize) -> Value {
    json!({
        "in0": format!("{input}:0"),
        "in1": if channels == 2 { format!("{input}:1") } else { format!("{input}:0") },
    })
}

/// The surface a track and the master share: the four controls of their own
/// strip, the send's gain, and the meter slot's -- which is what lets a host
/// say **where** it wants the level written without learning a private bus.
fn surface_of(meter: usize) -> Value {
    let mut surface = json!({
        GAIN:  [{"member": 0, "control": GAIN}],
        PAN:   [{"member": 0, "control": PAN}],
        WIDTH: [{"member": 0, "control": WIDTH}],
        MUTE:  [{"member": 0, "control": MUTE}],
        SEND_GAIN: [{"member": 1, "control": GAIN}],
    });
    let object = surface.as_object_mut().expect("an object");
    for (port, control) in [
        (METER_OUT0, "out0"),
        (METER_OUT1, "out1"),
        (METER_DECAY_PORT, "decay"),
        (METER_HOLD_PORT, "hold"),
    ] {
        object.insert(
            port.to_string(),
            json!([{"member": meter, "control": control}]),
        );
    }
    surface
}

/// **A clip**: readers onto a private bus, then a strip onto the bus the track
/// hands it.
///
/// The readers are a **slot** because a source has as many channels as it has
/// and a box may be re-cut while it sounds; the effect chain is a slot for the
/// same reason and is empty today. The clip does not decide where it goes: `out`
/// is external, so one clip def is every clip on every track.
pub fn clip_graph(inputs: usize, outputs: usize) -> Result<Value, String> {
    check(inputs, "clip inputs")?;
    check(outputs, "clip outputs")?;
    Ok(json!({
        "name": clip_name(inputs, outputs),
        "buses": [
            {"name": "src", "rate": "audio", "channels": inputs},
            {"name": OUT_BUS, "rate": "audio", "channels": outputs, "external": true},
        ],
        "members": [
            // 0: the strip. Shared: one per clip, however many readers there are.
            {"def": strip_name(inputs, outputs),
             "controls": strip_wiring("src", inputs, OUT_BUS, outputs)},
            // 1: one reader per channel of the source.
            {"def": reader_name(), "slot": SOURCE_SLOT,
             "controls": {OUT_BUS: "src"}},
        ],
        "surface": {
            GAIN:  [{"member": 0, "control": GAIN}],
            PAN:   [{"member": 0, "control": PAN}],
            WIDTH: [{"member": 0, "control": WIDTH}],
            MUTE:  [{"member": 0, "control": MUTE}],
            // The reader's own, which is the slot's surface rather than the
            // clip's: a box with two channels has two of each.
            AT:    [{"member": 1, "control": AT}],
            SPAN:  [{"member": 1, "control": SPAN}],
            START: [{"member": 1, "control": START}],
            BUF:   [{"member": 1, "control": BUF}],
            CHAN:  [{"member": 1, "control": CHAN}],
            LOOP:  [{"member": 1, "control": LOOP}],
            "source/gain": [{"member": 1, "control": GAIN}],
        },
        "defaults": { GAIN: 1.0, WIDTH: 1.0 }
    }))
}

/// **A track**: clips onto a private mix bus, then a strip onto the track's own
/// output bus, and a send from there onto the bus the master hands it.
///
/// The clips are a slot of **nested graphs**, which is what a clip being a
/// thing with its own gain, its own image and its own chain amounts to; the
/// track's own effects are the same slot shape and are empty today.
///
/// The strip does not write into the master directly, and the node in between
/// is what makes a meter mean anything -- see [`POST_BUS`] and [`send_def`].
pub fn track_graph(channels: usize) -> Result<Value, String> {
    check(channels, "track")?;
    Ok(json!({
        "name": track_name(channels),
        "buses": [
            {"name": MIX_BUS, "rate": "audio", "channels": channels},
            {"name": POST_BUS, "rate": "audio", "channels": channels},
            {"name": OUT_BUS, "rate": "audio", "channels": channels, "external": true},
        ],
        "members": [
            {"def": strip_name(channels, channels),
             "controls": strip_wiring(MIX_BUS, channels, POST_BUS, channels)},
            {"def": send_name(channels),
             "controls": strip_wiring(POST_BUS, channels, OUT_BUS, channels)},
            {"def": clip_name(1, channels), "kind": "graph", "slot": clip_slot(1),
             "controls": {OUT_BUS: MIX_BUS}},
            {"def": clip_name(2, channels), "kind": "graph", "slot": clip_slot(2),
             "controls": {OUT_BUS: MIX_BUS}},
            {"def": track_meter_name(channels), "slot": METER_SLOT,
             "controls": meter_wiring(POST_BUS, channels)},
        ],
        "surface": surface_of(4),
        "defaults": { GAIN: 1.0, WIDTH: 1.0 }
    }))
}

/// **The tracks' group**: every track onto the bus it is handed (`out`, the
/// master's mix bus), and nothing else.
///
/// It exists for the transport. A multitrack's tracks follow it and its master
/// does not -- the master's meter must fall on a pause and its declick must run
/// across a stop -- so the tracks are one subtree the transport governs, and
/// the master is around it. A slot and not a member, because the transport is
/// bound by node id and a slot's id is the caller's; one per multitrack.
pub fn tracks_graph(channels: usize) -> Result<Value, String> {
    check(channels, "tracks")?;
    Ok(json!({
        "name": tracks_name(channels),
        "buses": [
            {"name": OUT_BUS, "rate": "audio", "channels": channels, "external": true},
        ],
        "members": [
            {"def": track_name(channels), "kind": "graph", "slot": TRACK_SLOT,
             "controls": {OUT_BUS: OUT_BUS}},
        ],
    }))
}

/// **The multitrack**: tracks onto the master bus, then the master strip onto its
/// own output, and a send from there onto the hardware.
///
/// The master is the same strip as everything else, which is the point -- the
/// last fader in the chain is not a different kind of thing, and it is metered
/// by the same slot for the same reason.
///
/// **The tracks are inside a slot of this** rather than instances beside it,
/// and that is not a nicety: a track's output is a bus, the master's mix bus is
/// private to the master's instance, and a graph instantiated on its own could
/// never name it. Containment is what lets the master hand each track the bus
/// it writes to -- so a whole multitrack is *one* `/graph_new`, and every track,
/// clip and reader after it is a slot added to what is already sounding.
///
/// **The slot is the tracks' group** ([`tracks_graph`], [`TRANSPORT_SLOT`]),
/// which is what the transport governs: a stop freezes the tracks and leaves
/// the master strip, its meter and its way out ([`out_def`]) running, so the
/// meter falls and the declick is heard across the stop.
pub fn multitrack_graph(channels: usize) -> Result<Value, String> {
    check(channels, "multitrack")?;
    Ok(json!({
        "name": multitrack_name(channels),
        "buses": [
            {"name": MIX_BUS, "rate": "audio", "channels": channels},
            {"name": POST_BUS, "rate": "audio", "channels": channels},
        ],
        "members": [
            {"def": strip_name(channels, channels),
             "controls": strip_wiring(MIX_BUS, channels, POST_BUS, channels)},
            {"def": out_name(channels),
             "controls": strip_wiring(POST_BUS, channels, "OUT", channels)},
            {"def": tracks_name(channels), "kind": "graph", "slot": TRANSPORT_SLOT,
             "controls": {OUT_BUS: MIX_BUS}},
            {"def": meter_name(channels), "slot": METER_SLOT,
             "controls": meter_wiring(POST_BUS, channels)},
        ],
        "surface": surface_of(3),
        "defaults": { GAIN: 1.0, WIDTH: 1.0 }
    }))
}

/// Every def a multitrack of these widths needs, **in the order they must be sent**:
/// the SynthDefs first, then the graphs that name them, then the graphs that
/// name those.
///
/// One call rather than a list a caller assembles, because the order is a rule
/// and a caller that got it wrong would find out at instantiation, in another
/// process, as a missing member.
pub fn defs_for(widths: &[(usize, usize)], master: usize) -> Result<Defs, String> {
    check(master, "multitrack")?;
    let mut strips: Vec<(usize, usize)> = widths.to_vec();
    strips.push((master, master));
    for &(_, outputs) in widths {
        strips.push((outputs, outputs));
        for inputs in 1..=MAX_CHANNELS {
            strips.push((inputs, outputs));
        }
    }
    for inputs in 1..=MAX_CHANNELS {
        strips.push((inputs, master));
    }
    strips.sort_unstable();
    strips.dedup();

    let mut synth = vec![reader_def(), curve_def()];
    for &(inputs, outputs) in &strips {
        synth.push(strip_def(inputs, outputs)?);
    }
    let mut graph = Vec::new();
    // A track declares a clip slot per source width, so every one of those clip
    // graphs has to exist before it -- not only the widths a multitrack happens to
    // use today, since a take of the other width is one import away.
    let mut tracks: Vec<usize> = widths.iter().map(|&(_, out)| out).collect();
    tracks.push(master);
    tracks.sort_unstable();
    tracks.dedup();
    for &channels in &tracks {
        // The meter and the send are per strip width, and every track width
        // and the master's need theirs before the graph that names them.
        synth.push(meter_def(channels)?);
        synth.push(track_meter_def(channels)?);
        synth.push(send_def(channels)?);
        synth.push(out_def(channels)?);
        for inputs in 1..=MAX_CHANNELS {
            graph.push(clip_graph(inputs, channels)?);
        }
    }
    for &channels in &tracks {
        graph.push(track_graph(channels)?);
    }
    graph.push(tracks_graph(master)?);
    graph.push(multitrack_graph(master)?);
    Ok(Defs { synth, graph })
}

/// The defs a multitrack needs, split by the family they are sent as.
pub struct Defs {
    /// `/def_send synth`, in order.
    pub synth: Vec<Value>,
    /// `/def_send graph`, in order: a graph never names one that comes after it.
    pub graph: Vec<Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The port vocabulary is the same set everywhere it is spoken.** A knob,
    /// a curve and a `/node_set` write the same name, and this is what says so
    /// before an automation resolves against nothing.
    #[test]
    fn every_strip_answers_the_same_four_ports() {
        for graph in [
            clip_graph(1, 2).unwrap(),
            track_graph(2).unwrap(),
            multitrack_graph(2).unwrap(),
        ] {
            let surface = graph["surface"].as_object().expect("a surface");
            for port in [GAIN, PAN, WIDTH, MUTE] {
                assert!(surface.contains_key(port), "{} lacks {port}", graph["name"]);
            }
        }
    }

    /// **A width nobody wrote a downmix for is refused, not guessed.**
    #[test]
    fn a_width_past_stereo_is_refused() {
        let err = strip_def(6, 2).unwrap_err();
        assert!(
            err.contains("downmix"),
            "it says what the decision is: {err}"
        );
        assert!(
            strip_def(0, 2).is_err(),
            "and a strip of no channels is not one"
        );
    }

    /// **A mono source on a stereo strip is panned, not copied.** The same
    /// signal on both sides is 3 dB too loud in the middle, and it is the
    /// mistake worth a test rather than a comment.
    #[test]
    fn a_mono_strip_pans_into_stereo() {
        let def = strip_def(1, 2).unwrap();
        let kinds: Vec<&str> = def["ugens"]
            .as_array()
            .unwrap()
            .iter()
            .map(|u| u["kind"].as_str().unwrap())
            .collect();
        assert_eq!(kinds.iter().filter(|k| **k == "Pan2").count(), 2);
        assert_eq!(
            kinds.iter().filter(|k| **k == "In").count(),
            1,
            "one source channel"
        );
        assert_eq!(
            kinds.iter().filter(|k| **k == "Out").count(),
            2,
            "two sides"
        );
    }

    /// **A stereo source is balanced and given a width**, which is a different
    /// law from a pan and is why one word covers both -- and the balance leaves
    /// the centre alone, which is the whole difference and what three strips in
    /// series would otherwise cost 9 dB for.
    #[test]
    fn a_stereo_strip_balances_and_widens() {
        let def = strip_def(2, 2).unwrap();
        let kinds: Vec<&str> = def["ugens"]
            .as_array()
            .unwrap()
            .iter()
            .map(|u| u["kind"].as_str().unwrap())
            .collect();
        assert!(kinds.contains(&"StereoWidth"));
        assert!(!kinds.contains(&"Pan2"), "a balance is not a pan");
        assert!(
            !kinds.contains(&"Balance2"),
            "`Balance2` is the pan law over a pair, which is 3 dB down at centre"
        );
    }

    /// **The send order is a rule, not a caller's guess**: a graph never names
    /// one that has not been sent.
    #[test]
    fn the_defs_come_back_in_an_order_that_resolves() {
        let defs = defs_for(&[(1, 2), (2, 2)], 2).unwrap();
        let synth: Vec<String> = defs
            .synth
            .iter()
            .map(|d| d["name"].as_str().unwrap().to_string())
            .collect();
        assert!(synth.contains(&reader_name()));
        assert!(synth.contains(&curve_name()));
        assert!(synth.contains(&strip_name(1, 2)));

        let mut sent: Vec<String> = synth;
        for graph in &defs.graph {
            for member in graph["members"].as_array().unwrap() {
                let named = member["def"].as_str().unwrap().to_string();
                assert!(sent.contains(&named), "{named} is named before it is sent");
            }
            sent.push(graph["name"].as_str().unwrap().to_string());
        }
        assert!(sent.contains(&multitrack_name(2)));
    }
}
