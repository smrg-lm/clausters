//! The carriers: binding each one and draining it into the same handler.
//!
//! UDP, TCP, WebSocket, MIDI and the shared-memory ring all end at
//! [`OscServer::handle_packet`], which is why nothing above this module knows
//! which one a message arrived on -- only [`ClientId`] distinguishes them, and
//! only so a reply goes back the way it came. Every `drain_*` is non-blocking
//! and returns [`Flow`], so one turn of the loop serves all of them.

use super::*;

impl OscServer {
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.udp()?.local_addr()
    }

    /// The UDP socket, or the error a socket-front operation reports on a
    /// headless server.
    pub(in crate::osc::server) fn udp(&self) -> io::Result<&UdpSocket> {
        self.socket
            .as_ref()
            .ok_or_else(|| io::Error::other("headless server: no UDP front"))
    }

    /// The address a thread sends its zero-length wake datagram to
    /// ([`crate::osc::wake`]): our own UDP address, with an unspecified bind
    /// address read as loopback on the same port, since a datagram has to be
    /// aimed somewhere reachable.
    pub(in crate::osc::server) fn wake_target(&self) -> io::Result<SocketAddr> {
        let mut target = self.udp()?.local_addr()?;
        if target.ip().is_unspecified() {
            target.set_ip(match target {
                SocketAddr::V4(_) => std::net::Ipv4Addr::LOCALHOST.into(),
                SocketAddr::V6(_) => std::net::Ipv6Addr::LOCALHOST.into(),
            });
        }
        Ok(target)
    }

    /// Starts accepting length-prefixed OSC over TCP on `addr` (server track M /
    /// a stream client). The run loop drains the connections every iteration and a
    /// zero-length UDP datagram to our own address wakes it the moment a frame
    /// arrives, so TCP requests don't wait for the GC tick. Returns the bound
    /// TCP address.
    pub fn listen_tcp(&mut self, addr: impl ToSocketAddrs) -> io::Result<SocketAddr> {
        // Reader threads wake the loop by pinging the UDP socket.
        let wake_target = self.wake_target()?;
        let slots = self.client_slots();
        let hub = crate::osc::tcp::TcpHub::bind(addr, wake_target, self.max_frame, slots)?;
        let bound = hub.local_addr();
        self.tcp = Some(hub);
        Ok(bound)
    }

    /// Starts accepting OSC over WebSocket on `addr`. Same loop multiplexing and
    /// zero-length-UDP wake as [`Self::listen_tcp`]: the run loop drains
    /// WebSocket frames every iteration and a connection thread pings our UDP
    /// socket the moment a frame arrives. Returns the bound address; connect a
    /// browser with `ws://<addr>/`.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn listen_ws(&mut self, addr: impl ToSocketAddrs) -> io::Result<SocketAddr> {
        let wake_target = self.wake_target()?;
        let slots = self.client_slots();
        let hub = crate::osc::ws::WsHub::bind(addr, wake_target, self.max_frame, slots)?;
        let bound = hub.local_addr();
        self.ws = Some(hub);
        Ok(bound)
    }

    /// opens a virtual MIDI input port named `port_name`. The `midir`
    /// input thread wakes the loop with a zero-length UDP datagram (same
    /// mechanism as TCP), so MIDI messages are served without waiting for the
    /// GC tick. See [`crate::midi::live`].
    #[cfg(feature = "midi")]
    pub fn listen_midi(&mut self, port_name: &str) -> io::Result<()> {
        let wake_target = self.wake_target()?;
        let hub =
            crate::midi::live::MidiHub::open(port_name, wake_target).map_err(io::Error::other)?;
        self.midi = Some(hub);
        Ok(())
    }

    /// attaches the ring endpoint of an IPC segment. The run loop then
    /// drains it on every iteration; to keep ring latency low without a
    /// cross-process semaphore (v1 trade-off), the socket timeout — the
    /// loop's tick — is shortened.
    pub fn attach_ipc(&mut self, peer: crate::server::ipc::IpcPeer) -> io::Result<()> {
        if let Some(socket) = &self.socket {
            socket.set_read_timeout(Some(Duration::from_millis(2)))?;
        }
        self.segment = Some(std::sync::Arc::clone(peer.segment()));
        self.ipc = Some(peer);
        Ok(())
    }

    /// Attaches a segment this server does **not** serve the rings of: it
    /// reads the clocks and the buses out of it and maps the samples the
    /// owner publishes, while its clients reach it over its own sockets.
    ///
    /// This is what the RT server does in the editor's arrangement — it holds
    /// the devices and plays samples somebody else owns, so killing it takes
    /// no take with it.
    pub fn attach_segment(&mut self, segment: std::sync::Arc<crate::server::ipc::Segment>) {
        self.segment = Some(segment);
    }

    /// Says where the segment's file is, which is what a buffer's **region** is
    /// named from (`dsp::region`), and makes this server the **owner** of the
    /// samples: every buffer it installs gets a directory row and a region
    /// beside the segment. Without it a server with a segment still keeps its
    /// buffers in its own memory: the ring is a transport, and sharing the
    /// samples needs a path a peer can open.
    ///
    /// Exactly one process may own the samples, because there is one
    /// directory and the buffer numbers in it are one space; the caller is the
    /// one that took [`Segment::claim_control`](crate::server::ipc::Segment::claim_control).
    pub fn share_buffers_at(&mut self, path: std::path::PathBuf) {
        self.shm_path = Some(path);
        self.owns_samples = true;
    }

    /// The reader's half of [`Self::share_buffers_at`]: this server maps the
    /// samples the owner published, and publishes none of its own.
    ///
    /// Every live row is mapped now — a server started against a segment that
    /// already holds a session's takes has them all — and a buffer the owner
    /// allocates *later* arrives by `/buffer_attach`, which is the same rule
    /// the whole design follows: samples never travel, but allocation and
    /// lifetime are messages.
    #[cfg(unix)]
    pub fn attach_samples_at(&mut self, path: std::path::PathBuf) -> usize {
        self.shm_path = Some(path);
        self.owns_samples = false;
        let mut found = 0;
        let buffers = self.translator.buffers.len();
        for index in 0..buffers {
            if self.attach_shared_buffer(index).is_ok() {
                found += 1;
            }
        }
        found
    }

    /// Maps the owner's buffer `index` into this server's pool, so its engine
    /// plays **the very cells** the owner is editing.
    ///
    /// `Err` when there is no shared segment, when the directory row is empty,
    /// or when the region behind it cannot be opened — each said in its own
    /// words, because they are three different situations for whoever asked.
    #[cfg(unix)]
    pub fn attach_shared_buffer(&mut self, index: usize) -> Result<(), String> {
        let (Some(segment), Some(path)) = (self.segment.clone(), self.shm_path.clone()) else {
            return Err("this server has no shared segment".into());
        };
        if self.owns_samples {
            return Err("this server owns the samples; it has nothing to map".into());
        }
        let (_, buffer) = segment
            .map_buffer(&path, index)
            .ok_or_else(|| format!("no shared buffer {index}"))?;
        self.install_buffer(index, buffer)
    }

    /// Off Unix the samples are unreachable: a region is a file another
    /// process opens, and there is no equivalent — so a server there says so
    /// rather than pretending. The wasm engine in a page is the case this is
    /// really about, and a page keeps `/buffer_getRange`.
    #[cfg(not(unix))]
    pub fn attach_shared_buffer(&mut self, _index: usize) -> Result<(), String> {
        Err("sharing samples needs a Unix segment".into())
    }

    /// handles every packet waiting in the attached ring. Same
    /// validation path as UDP (`decode_packet`); ring bytes are untrusted.
    pub(in crate::osc::server) fn drain_ring(&mut self) -> Flow {
        if self.ipc.is_none() {
            return Flow::Continue;
        }
        loop {
            let Some(ipc) = &self.ipc else { unreachable!() };
            let mut buf = std::mem::take(&mut self.recv_buf);
            let popped = ipc.try_pop(&mut buf);
            self.recv_buf = buf;
            let Some((peer, len)) = popped else {
                return Flow::Continue;
            };
            let packet = match crate::osc::decode_packet(&self.recv_buf[..len]) {
                Ok(packet) => packet,
                Err(e) => {
                    warn!("malformed OSC packet from ring client {peer}: {e}");
                    continue;
                }
            };
            if let Flow::Quit = self.handle_packet(packet, ClientId::Ring(peer)) {
                return Flow::Quit;
            }
        }
    }

    /// Handles every complete TCP frame currently queued. Same validation path
    /// as UDP (`decode_packet`); TCP bytes are untrusted. Replies route back to
    /// the originating connection via [`ClientId::Tcp`].
    pub(in crate::osc::server) fn drain_tcp(&mut self) -> Flow {
        loop {
            // Scope the `&mut self.tcp` borrow so `handle_packet(&mut self)` and
            // its replies (which read `self.tcp`) can run.
            let next = match &mut self.tcp {
                Some(hub) => hub.next_frame(),
                None => return Flow::Continue,
            };
            let Some((id, bytes)) = next else {
                return Flow::Continue;
            };
            let packet = match crate::osc::decode_packet(&bytes) {
                Ok(packet) => packet,
                Err(e) => {
                    warn!("malformed OSC packet from tcp client {id}: {e}");
                    continue;
                }
            };
            let flow = self.handle_packet(packet, ClientId::Tcp(id));
            self.collect_garbage();
            self.collect_nrt_results();
            #[cfg(feature = "faust")]
            self.collect_faust_results();
            if let Flow::Quit = flow {
                return Flow::Quit;
            }
        }
    }

    /// No WebSocket hub exists on wasm32; the stub keeps the run loop's shape.
    #[cfg(target_arch = "wasm32")]
    pub(in crate::osc::server) fn drain_ws(&mut self) -> Flow {
        Flow::Continue
    }

    /// Handles every complete WebSocket frame currently queued. Same validation
    /// path as UDP (`decode_packet`); WebSocket bytes are untrusted. Replies
    /// route back to the originating connection via [`ClientId::Ws`].
    #[cfg(not(target_arch = "wasm32"))]
    pub(in crate::osc::server) fn drain_ws(&mut self) -> Flow {
        loop {
            // Scope the `&mut self.ws` borrow so `handle_packet(&mut self)` and
            // its replies (which read `self.ws`) can run.
            let next = match &mut self.ws {
                Some(hub) => hub.next_frame(),
                None => return Flow::Continue,
            };
            let Some((id, bytes)) = next else {
                return Flow::Continue;
            };
            let packet = match crate::osc::decode_packet(&bytes) {
                Ok(packet) => packet,
                Err(e) => {
                    warn!("malformed OSC packet from ws client {id}: {e}");
                    continue;
                }
            };
            let flow = self.handle_packet(packet, ClientId::Ws(id));
            self.collect_garbage();
            self.collect_nrt_results();
            #[cfg(feature = "faust")]
            self.collect_faust_results();
            if let Flow::Quit = flow {
                return Flow::Quit;
            }
        }
    }

    /// translates every queued live-MIDI message into engine commands and
    /// ships them. Each message is self-contained (one note/control event), so
    /// it is realized like the immediate OSC forms: `translate_midi` (which
    /// reuses the `/synth_new`/`/node_set`/`/node_free` path and keeps the tree mirror in
    /// sync), then ship the batch. MIDI never quits the server.
    #[cfg(feature = "midi")]
    pub(in crate::osc::server) fn drain_midi(&mut self) {
        let mut cmds = Vec::new();
        while let Some(msg) = self.midi.as_ref().and_then(|hub| hub.try_next()) {
            cmds.clear();
            if let Err(e) = self.translator.translate_midi(msg, &mut cmds) {
                warn!("midi: {e}");
                continue;
            }
            for cmd in cmds.drain(..) {
                if self.handle.send(cmd).is_err() {
                    warn!("midi: command FIFO full");
                    break;
                }
            }
        }
        self.collect_garbage();
    }

    #[cfg(not(feature = "midi"))]
    pub(in crate::osc::server) fn drain_midi(&mut self) {}
}
