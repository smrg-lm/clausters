"""What the server's MIDI plays: its bindings, and MIDI 2.0 packets sent in.

A **binding** says which instrument a MIDI channel plays and how its messages
reach that instrument's controls. It is the server's, not this handle's: the
same binding hears the server's live MIDI input (``--midi``) and the MIDI
messages a transport's event lane plays (`ServerTransport.lane_set`'s
``midi`` and ``ump`` lists), and a server with a data directory keeps it
across restarts. So a binding is made once, wherever the notes come from, and
`midi_query` reads back what a server already has.

`midi_ump` is the other half: MIDI 2.0 packets played now, as the live input
plays its messages.
"""

from dataclasses import dataclass, field

from ..node import ROOT_NODE_ID, AddAction, _target_id


@dataclass
class MidiBinding:
    """One binding, as `Server.midi_query` reads it back.

    ``kind`` is ``"channel"`` for a binding of one channel and ``"zone"`` for
    an MPE zone, whose ``channel`` is its master and whose ``members`` is how
    many member channels the device last said it has (0 for a channel).
    ``controls`` maps each selector that names a control -- ``note``, ``vel``
    and ``gate`` always, then ``bend``, ``pressure``, ``poly``, ``timbre``,
    ``lift``, ``ccN`` and ``progN`` where mapped -- to the name it drives (a
    ``progN`` names an instrument). ``timbre_cc`` is a zone's timbre
    controller, ``None`` on a channel."""

    kind: str
    channel: int
    members: int
    instrument: str
    target: int
    action: int
    gate: bool
    controls: "dict[str, str]" = field(default_factory=dict)
    timbre_cc: "int | None" = None

    def __str__(self) -> str:
        where = (f"zone on {self.channel} ({self.members} members)"
                 if self.kind == "zone" else f"channel {self.channel}")
        mapped = ", ".join(f"{k} -> {v}" for k, v in self.controls.items())
        return f"{where}: {self.instrument} [{mapped}]"


def parse_midi_binding(args) -> "MidiBinding | None":
    """One ``/midi_query.reply``: ``kind channel members instrument target
    addAction gate``, then ``selector name`` pairs and, on a zone, a trailing
    ``timbreCc <int>``. A ``"none"`` reply -- a channel asked for and bound to
    nothing -- is ``None``."""
    if str(args[0]) == "none":
        return None
    binding = MidiBinding(kind=str(args[0]), channel=int(args[1]), members=int(args[2]),
                          instrument=str(args[3]), target=int(args[4]),
                          action=int(args[5]), gate=bool(int(args[6])))
    i = 7
    while i + 1 < len(args):
        selector = str(args[i])
        if selector == "timbreCc":
            binding.timbre_cc = int(args[i + 1])
        else:
            binding.controls[selector] = str(args[i + 1])
        i += 2
    return binding


def _int32(word: int) -> int:
    """A packet's 32-bit word as the signed int32 OSC carries, the same bits."""
    word = int(word) & 0xFFFFFFFF
    return word - (1 << 32) if word >= 0x80000000 else word


class ServerMidi:
    """The MIDI half of `Server`; never instantiated on its own.

    The bindings are sent and not awaited, like `clausters.defs.node.Node.map`:
    the server takes them in order, so a lane set after a bind plays through
    it, and one it refuses -- an instrument it does not hold, a channel a zone
    covers -- answers ``/fail``."""

    def midi_bind(self, channel: int, instrument: str, target=ROOT_NODE_ID,
                  action: AddAction = AddAction.HEAD, gate: bool = False):
        """Bind MIDI ``channel`` to the def named ``instrument``
        (``/midi_bind``): each note on it makes a node of that def at
        ``target`` by ``action``, its note number on ``freq`` and its velocity
        on ``amp`` until `midi_map` says otherwise. A SynthDef, a FaustDef and
        a GraphDef are bound alike (a GraphDef's shared members are made here,
        and each note is a voice of them). ``channel`` is 0-255: the 16 MIDI
        1.0 channels and the extended MIDI 2.0 group x channel space.

        A note-off frees the node, or, with ``gate``, sets its ``gate`` control
        to 0, for a def whose envelope releases. The binding reaches both the
        live input and a transport lane's MIDI messages; binding a channel
        again replaces it. Returns ``self``."""
        self.send_msg("/midi_bind", int(channel), str(instrument), _target_id(target),
                      int(action), 1 if gate else 0)
        return self

    def midi_bind_zone(self, master: int, members: int, instrument: str,
                       target=ROOT_NODE_ID, action: AddAction = AddAction.HEAD,
                       gate: bool = False):
        """Bind an **MPE zone** to ``instrument`` (``/midi_bindZone``): the
        lower zone on master channel 0 (members ascending from 1) or the upper
        on 15 (descending from 14), ``members`` member channels, 0 to wait for
        the device's own layout -- which wins when it arrives. Each note is a
        voice whose own channel's bend, pressure and timbre are its own: a
        voice starts with ``freq``, ``amp``, ``press`` (the pressure) and
        ``slide`` (the timbre) -- the built-in ``default``'s controls -- so a
        def with those controls plays all three with no `midi_map`. Refused
        when the zone would reach a channel `midi_bind` holds. Returns
        ``self``."""
        self.send_msg("/midi_bindZone", int(master), int(members), str(instrument),
                      _target_id(target), int(action), 1 if gate else 0)
        return self

    def midi_unbind(self, channel: int):
        """Remove ``channel``'s binding (``/midi_unbind``), freeing every voice
        still sounding on it; on a zone's master, the zone and its voices.
        Returns ``self``."""
        self.send_msg("/midi_unbind", int(channel))
        return self

    def midi_map(self, channel: int, selector: str, name: str, cc: "int | None" = None):
        """Route one kind of message on ``channel`` to the control ``name``
        (``/midi_map``). ``selector`` is ``"note"`` (the frequency control),
        ``"vel"`` (the amplitude), ``"gate"`` (the gate control), ``"bend"``,
        ``"pressure"`` (channel aftertouch), ``"poly"`` (per-note aftertouch,
        to the note's voice), ``"lift"`` (note-off velocity), ``"ccN"`` (control
        change ``N``) or ``"progN"`` (program ``N``, whose ``name`` is an
        instrument to switch to); on a zone's master, ``"timbre"`` too, with
        ``cc`` the controller that carries it (74 unless given). On a zone's
        master the map is the zone's. Returns ``self``."""
        args = [int(channel), str(selector), str(name)]
        if cc is not None:
            args.append(int(cc))
        self.send_msg("/midi_map", *args)
        return self

    def midi_ump(self, *words: int):
        """Play MIDI 2.0 packets now (``/midi_ump``), each int one 32-bit word
        of a Universal MIDI Packet, as the live input plays its messages: a
        note, controller, pressure, bend or program change through its
        channel's binding at its own resolution (a 16-bit velocity, 32-bit
        values), and a per-note bend or controller to the voice sounding on
        its channel and key. Any other packet plays nothing. Returns
        ``self``."""
        self.send_msg("/midi_ump", *[_int32(w) for w in words])
        return self

    def midi_query(self, *channels: int,
                   timeout: "float | None" = None) -> "dict[int, MidiBinding]":
        """The server's MIDI bindings (``/midi_query``), by channel: every one,
        or those of ``channels``, a channel bound to nothing left out. A zone
        is listed under its master. Blocking, RT only -- never call it from a
        routine."""
        rows = self._request_batch("/midi_query", *[int(c) for c in channels],
                                   reply="/midi_query.reply", timeout=timeout)
        out = {}
        for row in rows:
            binding = parse_midi_binding(row)
            if binding is not None:
                out[binding.channel] = binding
        return out
