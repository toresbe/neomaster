//! Masterlink audio master behaviour: bus arbitration, source distribution, listeners.
//!
//! Port of libpc2 `masterlink/audio_master.{hpp,cpp}`. Modelled on USB captures of B&O's own
//! BeoSound 5 software acting as audio master (see reports/masterlink-protocol.md in the rea-local
//! workspace). Names in comments follow B&O's telegram names recovered from BeoIpcML.exe.

use std::collections::BTreeSet;
use std::time::Duration;

use log::{debug, info, warn};

use crate::host::Host;
use crate::protocol::{self as ml, node, payload_type as pt, telegram_type as tt};
use crate::telegram::Telegram;
use crate::Time;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BusState {
    /// not distributing, not listening to the video master
    Idle,
    /// asked the video master whether the audio bus is free
    Arbitrating,
    /// our source is on the audio bus
    Distributing,
    /// we play a video master source (e.g. TV) from the bus
    ListeningToVm,
}

/// How the LOCK_MANAGER bus lock is taken before TV / all-standby:
/// - `Firmware` (default): the PC2 firmware runs the lock protocol and consumes all 0x5C
///   telegrams whenever the host started it with "80 <node> 00". We ask with 24, wait for
///   IN 44 01 ([`AudioMaster::bus_lock_granted`]), act, then release with e4 01: B&O's sequence.
/// - `Software`: run the 0x5C exchange ourselves. Only meaningful if the PC2 forwards 0x5C
///   telegrams, i.e. it was started with "80 <node> <non-zero>" (promiscuous mode).
/// - `Off`: no lock (the 2018 BM5 captures show no lock traffic; their replays use this).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockMode {
    Off,
    Firmware,
    Software,
}

/// Bus arbitration with the video master, as observed between a BeoSound 3000 and a Beosystem 3:
/// LINK_INFO? -> AUDIO_BUS? -> (if busy) REQUEST LINK_CONNECT_COMMAND_0 DISCONNECT_SIGNAL
/// "cur -> wanted", wait for LINK_CONNECT_STATUS DISCONNECTED_SIGNAL -> COMMAND STANDBY_0 on the
/// audio link, wait for COMMAND 0x1C (bus released) -> distribute. Each wait times out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Arbitration {
    None,
    LinkInfo,
    AudioBus,
    ChangeRequest,
    Claim,
}

/// What to do once the bus lock is ours (libpc2 kept these as closures).
#[derive(Clone, Copy, Debug)]
enum LockAction {
    ListenToVideoSource(u8),
    StopListeningToVm,
    AllStandby,
}

pub struct AudioMaster {
    system_configuration: u8,
    bus_state: BusState,
    /// source on the bus (distributing) or requested
    source: u8,
    /// what we play locally, 0 = nothing
    local_source: u8,
    /// link rooms connected to our source
    listener_nodes: BTreeSet<u8>,
    /// rooms waiting for replies during arbitration
    pending_requesters: BTreeSet<u8>,
    arbitration: Arbitration,
    arbitration_deadline: Time,
    vm_busy_source: u8,
    lock_mode: LockMode,
    holds_lock: bool,
    lock_pending: bool,
    lock_deadline: Time,
    lock_waiters: Vec<LockAction>,
    /// video source we asked for / listen to
    vm_source: u8,
    started: bool,
    next_clock: Time,
}

impl AudioMaster {
    /// `system_configuration` is B&O's SystemConfiguration, i.e. the mode byte sent in the PC2's
    /// F6 option telegram (audio-master mode sends 5, "A.OPT = 0"). B&O's software varies one
    /// LINK_INFO byte with it, so we do too.
    pub const DEFAULT_SYSTEM_CONFIGURATION: u8 = 5;
    pub const VM_REPLY_TIMEOUT: Duration = Duration::from_millis(1000);
    /// observed: ~7 min 10 s
    pub const CLOCK_INTERVAL: Duration = Duration::from_secs(430);
    /// software lock mode
    pub const LOCK_TIMEOUT: Duration = Duration::from_millis(500);
    /// firmware lock mode (> 1.5 s)
    pub const FIRMWARE_LOCK_TIMEOUT: Duration = Duration::from_millis(2000);

    pub fn new(system_configuration: u8) -> Self {
        AudioMaster {
            system_configuration,
            bus_state: BusState::Idle,
            source: 0,
            local_source: 0,
            listener_nodes: BTreeSet::new(),
            pending_requesters: BTreeSet::new(),
            arbitration: Arbitration::None,
            arbitration_deadline: Time::ZERO,
            vm_busy_source: 0,
            lock_mode: LockMode::Firmware,
            holds_lock: false,
            lock_pending: false,
            lock_deadline: Time::ZERO,
            lock_waiters: Vec::new(),
            vm_source: 0,
            started: false,
            next_clock: Time::ZERO,
        }
    }

    // ------------------------------------------------------------ telegram builders

    #[allow(clippy::too_many_arguments)]
    fn make_from(dest: u8, src: u8, ttype: u8, ptype: u8, subtype: u8, payload: Vec<u8>, src_src: u8) -> Telegram {
        Telegram {
            dest_node: dest,
            src_node: src,
            telegram_type: ttype,
            dest_src: 0,
            src_src,
            payload_type: ptype,
            subtype,
            payload,
        }
    }

    /// Most telegrams: from the audio master address (C1).
    fn make(dest: u8, ttype: u8, ptype: u8, subtype: u8, payload: Vec<u8>) -> Telegram {
        Self::make_from(dest, node::AUDIO_MASTER, ttype, ptype, subtype, payload, 0)
    }

    /// STATUS LINK_INFO: "audio master present"
    fn send_link_info_reply(&self, host: &mut dyn Host, to: u8) {
        host.send_telegram(Self::make(
            to,
            tt::STATUS,
            pt::LINK_INFO,
            ml::link_info::SUBTYPE,
            ml::link_info::answer(ml::link_info::node_kind::AUDIO_MASTER, true),
        ));
    }

    /// STATUS AUDIO_BUS: free (empty) or busy with our source
    fn send_audio_bus_status(&self, host: &mut dyn Host, to: u8) {
        if self.bus_state == BusState::Distributing {
            host.send_telegram(Self::make(
                to,
                tt::STATUS,
                pt::AUDIO_BUS,
                ml::audio_bus::SUBTYPE_BUSY,
                ml::audio_bus::busy(self.source, false),
            ));
        } else {
            host.send_telegram(Self::make(to, tt::STATUS, pt::AUDIO_BUS, ml::audio_bus::SUBTYPE_FREE, vec![]));
        }
    }

    /// SOURCE_SYSTEM_STATUS to ALL_SOURCE_USERS, as sent when distribution starts
    fn send_source_status_active(host: &mut dyn Host, src: u8) {
        host.send_telegram(Self::make_from(
            node::ALL_SOURCE_USERS,
            node::AUDIO_MASTER,
            tt::STATUS,
            pt::SOURCE_STATUS,
            ml::source_status::SUBTYPE_SYSTEM,
            ml::source_status::system_status(src, ml::source_status::State::Playing),
            src,
        ));
    }

    /// The "stopped" form, sent when everything is switched off while distributing
    fn send_source_status_stopped(host: &mut dyn Host, src: u8) {
        host.send_telegram(Self::make_from(
            node::ALL_SOURCE_USERS,
            node::AUDIO_MASTER,
            tt::STATUS,
            pt::SOURCE_STATUS,
            ml::source_status::SUBTYPE_SYSTEM,
            ml::source_status::stopped(src),
            src,
        ));
    }

    /// Idle: our PC source (from the PC_1 address), not playing
    fn send_source_status_idle(host: &mut dyn Host) {
        host.send_telegram(Self::make_from(
            node::ALL_SOURCE_USERS,
            node::PC_1,
            tt::STATUS,
            pt::SOURCE_STATUS,
            ml::source_status::SUBTYPE_SYSTEM,
            ml::source_status::system_status(ml::source::PC, ml::source_status::State::Idle),
            ml::source::PC,
        ));
    }

    /// The two replies a room gets once it is connected to our source: CONNECTED_SIGNAL and
    /// PRI_SYSTEM_STATUS
    fn send_connect_replies(host: &mut dyn Host, to: u8, src: u8) {
        host.send_telegram(Self::make(
            to,
            tt::STATUS,
            pt::LINK_CONNECT_0,
            ml::link_connect::signal::CONNECTED,
            ml::link_connect::connected(src),
        ));
        host.send_telegram(Self::make(
            to,
            tt::STATUS,
            pt::PRI_SYSTEM_STATUS,
            ml::pri_system::SUBTYPE_AUDIO,
            ml::pri_system::playing(src),
        ));
    }

    /// STATUS PROGRAMMING_STATUS (clock) to ALL: 0a 00 03 hh mm ss 00 DD MM YY <priority> (BCD)
    fn send_clock(host: &mut dyn Host) {
        let t = host.local_time();
        host.send_telegram(Self::make(node::ALL, tt::STATUS, pt::PROGRAMMING_STATUS, ml::clock::SUBTYPE, ml::clock::payload(t)));
    }

    /// REQUEST LINK_INFO to the video master, as the audio master
    fn send_link_info_request_to_vm(&self, host: &mut dyn Host) {
        host.send_telegram(Self::make(
            node::VIDEO_MASTER,
            tt::REQUEST,
            pt::LINK_INFO,
            ml::link_info::SUBTYPE,
            ml::link_info::request(ml::link_info::node_kind::AUDIO_MASTER, self.system_configuration),
        ));
    }

    /// COMMAND STANDBY_0 on the audio link: take the audio bus from the video master
    fn send_take_bus_from_vm(host: &mut dyn Host) {
        host.send_telegram(Self::make(
            node::VIDEO_MASTER,
            tt::COMMAND,
            pt::STANDBY_0,
            ml::standby::SUBTYPE_0,
            ml::standby::take_audio_bus(),
        ));
    }

    // ------------------------------------------------------------ public API

    /// Startup broadcasts: idle source status, clock, LINK_INFO probe to the video master.
    pub fn start(&mut self, host: &mut dyn Host, now: Time) {
        Self::send_source_status_idle(host);
        Self::send_clock(host);
        // Probe for a video master; its answer is just noted.
        self.send_link_info_request_to_vm(host);
        self.next_clock = now + Self::CLOCK_INTERVAL;
        self.started = true;
    }

    /// Call periodically (every 100-500 ms): timeouts and the periodic clock broadcast.
    pub fn tick(&mut self, host: &mut dyn Host, now: Time) {
        if self.started && now >= self.next_clock {
            Self::send_clock(host);
            self.next_clock = now + Self::CLOCK_INTERVAL;
        }
        if self.lock_pending && now >= self.lock_deadline {
            // Software: nobody answered, so nobody holds it. Firmware: no IN 44 01 (the PC2 may loop
            // REQUEST/COMMAND v5 forever if nobody answers); act anyway rather than block TV/standby.
            if self.lock_mode == LockMode::Firmware {
                warn!("No bus lock grant (IN 44) from the PC2; going ahead");
            }
            self.lock_acquired(host);
        }
        if self.bus_state == BusState::Arbitrating && now >= self.arbitration_deadline {
            if self.arbitration == Arbitration::ChangeRequest {
                info!("Video master did not answer the change request; claiming the bus");
                Self::send_take_bus_from_vm(host);
                self.arbitration = Arbitration::Claim;
                self.arbitration_deadline = now + Self::VM_REPLY_TIMEOUT;
            } else {
                // No (more) answers from a video master: assume there is none / it let go, and take the bus.
                info!("No reply from video master; taking the audio bus");
                self.start_distribution(host);
            }
        }
    }

    /// When [`AudioMaster::tick`] next has something to do: the clock broadcast, a lock or an
    /// arbitration timeout.
    pub fn next_deadline(&self) -> Option<Time> {
        let clock = self.started.then_some(self.next_clock);
        let lock = self.lock_pending.then_some(self.lock_deadline);
        let arbitration = (self.bus_state == BusState::Arbitrating).then_some(self.arbitration_deadline);
        [clock, lock, arbitration].into_iter().flatten().min()
    }

    pub fn set_lock_mode(&mut self, mode: LockMode) {
        self.lock_mode = mode;
    }

    /// Compatibility with libpc2: true = software, false = off.
    pub fn set_lock_participation(&mut self, on: bool) {
        self.set_lock_mode(if on { LockMode::Software } else { LockMode::Off });
    }

    /// The PC2 reported IN 44 01: the firmware holds the bus lock for us.
    pub fn bus_lock_granted(&mut self, host: &mut dyn Host) {
        if self.lock_mode == LockMode::Firmware && self.lock_pending {
            self.lock_acquired(host);
        }
    }

    pub fn holds_bus_lock(&self) -> bool {
        self.holds_lock
    }

    /// We now play `src` locally (e.g. a source picked on the panel). Link rooms listening to us
    /// are moved to it.
    pub fn set_local_source(&mut self, host: &mut dyn Host, src: u8) {
        self.local_source = src;
        if self.bus_state == BusState::Distributing && src != 0 && src != self.source {
            self.change_source(host, src);
        }
    }

    /// Ask the video master for one of its sources (e.g. TV) and play it from the bus.
    pub fn listen_to_video_source(&mut self, host: &mut dyn Host, src: u8, now: Time) {
        if self.bus_state == BusState::Distributing {
            self.stop_distribution(host, false);
        }
        self.vm_source = src;
        // As captured: (lock), LINK_INFO to the VM, then LINK_CONNECT_COMMAND_1 for the source. The
        // VM then asks us LINK_INFO and AUDIO_BUS (answered in handle()) and confirms with
        // CONNECTED_SIGNAL.
        self.with_lock(host, LockAction::ListenToVideoSource(src), now);
    }

    /// Local standby: stop listening to the video master. Distribution to link rooms that still
    /// listen continues, as B&O's software does.
    pub fn local_standby(&mut self, host: &mut dyn Host, now: Time) {
        self.local_source = 0;
        if self.bus_state == BusState::ListeningToVm || self.vm_source != 0 {
            // B&O takes the bus lock around releasing the video master's source (24 ... e4 01)
            host.set_ml_listening(false);
            self.with_lock(host, LockAction::StopListeningToVm, now);
        }
        // Rooms still listening keep getting our source (observed: "I power off the BS5 locally;
        // CD continues to play on both Beolink and Beoport").
        if self.bus_state == BusState::Distributing && self.listener_nodes.is_empty() {
            self.stop_distribution(host, false);
        }
    }

    /// "All standby": tell every node to switch off, and stop everything.
    pub fn all_standby(&mut self, host: &mut dyn Host, now: Time) {
        self.with_lock(host, LockAction::AllStandby, now);
    }

    pub fn state(&self) -> BusState {
        self.bus_state
    }
    pub fn current_source(&self) -> u8 {
        self.source
    }
    pub fn listeners(&self) -> &BTreeSet<u8> {
        &self.listener_nodes
    }

    // ------------------------------------------------------------ lock

    fn run_lock_action(&mut self, host: &mut dyn Host, action: LockAction) {
        match action {
            LockAction::ListenToVideoSource(src) => {
                self.send_link_info_request_to_vm(host);
                host.send_telegram(Self::make(
                    node::VIDEO_MASTER,
                    tt::REQUEST,
                    pt::LINK_CONNECT_1,
                    ml::link_connect::signal::CONNECT,
                    ml::link_connect::connect_request(src),
                ));
            }
            LockAction::StopListeningToVm => self.stop_listening_to_vm(host, true),
            LockAction::AllStandby => {
                host.send_telegram(Self::make(node::ALL, tt::COMMAND, pt::STANDBY_1, ml::standby::SUBTYPE_1_ALL, vec![]));
                if self.bus_state == BusState::ListeningToVm {
                    self.stop_listening_to_vm(host, false);
                }
                if matches!(self.bus_state, BusState::Distributing | BusState::Arbitrating) {
                    self.stop_distribution(host, true);
                }
                self.local_source = 0;
            }
        }
    }

    /// LOCK_MANAGER: broadcast REQUEST; the current holder answers STATUS KEY_TRANSFER (or
    /// TRANSFER_IMPOSSIBLE if it still needs the key); we confirm with COMMAND KEY_RECEIVED to the
    /// giver. If nobody answers, nobody holds it.
    fn with_lock(&mut self, host: &mut dyn Host, action: LockAction, now: Time) {
        if self.lock_mode == LockMode::Off || self.holds_lock {
            self.run_lock_action(host, action);
            return;
        }
        self.lock_waiters.push(action);
        if self.lock_pending {
            return;
        }
        self.lock_pending = true;
        if self.lock_mode == LockMode::Firmware {
            self.lock_deadline = now + Self::FIRMWARE_LOCK_TIMEOUT;
            host.request_bus_lock(); // 24
        } else {
            self.lock_deadline = now + Self::LOCK_TIMEOUT;
            host.send_telegram(Self::make(
                node::ALL,
                tt::REQUEST,
                pt::LOCK_MANAGER,
                ml::lock_manager::SUBTYPE_REQUEST,
                vec![ml::lock_manager::PAYLOAD],
            ));
        }
    }

    fn lock_acquired(&mut self, host: &mut dyn Host) {
        self.lock_pending = false;
        self.holds_lock = true;
        for action in std::mem::take(&mut self.lock_waiters) {
            self.run_lock_action(host, action);
        }
        if self.lock_mode == LockMode::Firmware {
            // B&O releases right after its telegrams (e4 01) rather than waiting out the firmware's hold
            host.release_bus_lock();
            self.holds_lock = false;
        }
    }

    fn handle_lock(&mut self, host: &mut dyn Host, t: &Telegram) {
        let to_us = t.dest_node == node::AUDIO_MASTER;
        if t.telegram_type == tt::REQUEST && t.dest_node == node::ALL {
            // Someone wants the lock; answer if we hold it
            if self.holds_lock && t.src_node != node::AUDIO_MASTER {
                host.send_telegram(Self::make(
                    t.src_node,
                    tt::STATUS,
                    pt::LOCK_MANAGER,
                    ml::lock_manager::SUBTYPE_KEY_TRANSFER,
                    vec![ml::lock_manager::PAYLOAD],
                ));
            }
        } else if t.telegram_type == tt::STATUS
            && to_us
            && self.lock_pending
            && t.subtype == ml::lock_manager::SUBTYPE_KEY_TRANSFER
        {
            // The holder hands it to us (TRANSFER_IMPOSSIBLE leaves us waiting until the lock timeout)
            host.send_telegram(Self::make(
                t.src_node,
                tt::COMMAND,
                pt::LOCK_MANAGER,
                ml::lock_manager::SUBTYPE_KEY_RECEIVED,
                vec![ml::lock_manager::PAYLOAD],
            ));
            self.lock_acquired(host);
        } else if t.telegram_type == tt::COMMAND && to_us {
            // We handed it over
            self.holds_lock = false;
        }
    }

    // ------------------------------------------------------------ flow

    fn connect_request(&mut self, host: &mut dyn Host, room: u8, src: u8, now: Time) {
        if self.bus_state == BusState::Distributing && src == self.source {
            // Already on the bus: just connect the room.
            self.listener_nodes.insert(room);
            Self::send_connect_replies(host, room, src);
            return;
        }
        if self.bus_state == BusState::Distributing {
            // A room picks another of our sources: move everybody over.
            if !host.start_source(src) {
                warn!("Host refused source {src:#04x}");
                return;
            }
            self.listener_nodes.insert(room);
            self.change_source(host, src);
            return;
        }
        if self.bus_state == BusState::Arbitrating {
            self.pending_requesters.insert(room);
            self.source = src;
            return;
        }
        if !host.start_source(src) {
            warn!("Host refused source {src:#04x}");
            return;
        }
        if self.bus_state == BusState::ListeningToVm {
            // We were playing the VM's source: take the bus back first (STANDBY_0 on the audio link).
            Self::send_take_bus_from_vm(host);
            host.set_ml_listening(false);
            self.bus_state = BusState::Idle;
            self.vm_source = 0;
        }
        self.pending_requesters.insert(room);
        self.begin_arbitration(host, src, now);
    }

    fn begin_arbitration(&mut self, host: &mut dyn Host, src: u8, now: Time) {
        self.source = src;
        self.bus_state = BusState::Arbitrating;
        self.arbitration = Arbitration::LinkInfo;
        self.arbitration_deadline = now + Self::VM_REPLY_TIMEOUT;
        self.send_link_info_request_to_vm(host);
    }

    fn arbitration_step_link_info_ok(&mut self, host: &mut dyn Host, now: Time) {
        self.arbitration = Arbitration::AudioBus;
        self.arbitration_deadline = now + Self::VM_REPLY_TIMEOUT;
        host.send_telegram(Self::make(node::VIDEO_MASTER, tt::REQUEST, pt::AUDIO_BUS, ml::audio_bus::SUBTYPE_REQUEST, vec![]));
    }

    fn start_distribution(&mut self, host: &mut dyn Host) {
        self.arbitration = Arbitration::None;
        self.bus_state = BusState::Distributing;
        Self::send_source_status_active(host, self.source);
        host.set_ml_distribution(true);
        for room in std::mem::take(&mut self.pending_requesters) {
            self.listener_nodes.insert(room);
            Self::send_connect_replies(host, room, self.source);
        }
    }

    fn stop_distribution(&mut self, host: &mut dyn Host, announce_stopped: bool) {
        let was = self.source;
        if announce_stopped && was != 0 {
            Self::send_source_status_stopped(host, was);
        }
        host.set_ml_distribution(false);
        host.stop_source();
        self.bus_state = BusState::Idle;
        self.arbitration = Arbitration::None;
        self.listener_nodes.clear();
        self.pending_requesters.clear();
        self.source = 0;
        Self::send_source_status_idle(host);
    }

    fn change_source(&mut self, host: &mut dyn Host, new_src: u8) {
        let old_src = self.source;
        // DISCONNECTED_SIGNAL "<old> -> <new>" to each listener
        if old_src != 0 {
            for &room in &self.listener_nodes {
                host.send_telegram(Self::make(
                    room,
                    tt::STATUS,
                    pt::LINK_CONNECT_0,
                    ml::link_connect::signal::DISCONNECTED,
                    ml::link_connect::changed(old_src, new_src),
                ));
            }
        }
        self.source = new_src;
        Self::send_source_status_active(host, new_src);
        for &room in &self.listener_nodes {
            Self::send_connect_replies(host, room, new_src);
        }
    }

    fn stop_listening_to_vm(&mut self, host: &mut dyn Host, tell_vm: bool) {
        if tell_vm {
            // As captured on standby while on TV: LINK_INFO, then STANDBY_1 (we leave the audio link) to the VM
            self.send_link_info_request_to_vm(host);
            host.send_telegram(Self::make(
                node::VIDEO_MASTER,
                tt::COMMAND,
                pt::STANDBY_1,
                ml::standby::SUBTYPE_1_NODE,
                ml::standby::leave_audio(),
            ));
        }
        host.set_ml_listening(false);
        self.vm_source = 0;
        if self.bus_state == BusState::ListeningToVm {
            self.bus_state = BusState::Idle;
        }
    }

    // ------------------------------------------------------------ incoming

    /// Feed every Masterlink telegram received from the PC2.
    pub fn handle(&mut self, host: &mut dyn Host, t: &Telegram, now: Time) {
        use ml::link_connect as lc;
        let from = t.src_node;
        let to_us = t.dest_node == node::AUDIO_MASTER;
        let from_vm = from == node::VIDEO_MASTER;

        if t.payload_type == pt::LOCK_MANAGER {
            if self.lock_mode == LockMode::Software {
                self.handle_lock(host, t);
            }
            return;
        }

        if t.telegram_type == tt::REQUEST {
            if !to_us {
                return;
            }
            match t.payload_type {
                pt::LINK_INFO => self.send_link_info_reply(host, from),
                pt::AUDIO_BUS => self.send_audio_bus_status(host, from),
                pt::LOCAL_SOURCE => {
                    // "playing <src>" when we play something locally without distributing it; else "none"
                    if self.local_source != 0 && self.bus_state != BusState::Distributing {
                        host.send_telegram(Self::make(
                            from,
                            tt::STATUS,
                            pt::LOCAL_SOURCE,
                            ml::local_source::SUBTYPE_PLAYING,
                            ml::local_source::playing(self.local_source),
                        ));
                    } else {
                        host.send_telegram(Self::make(
                            from,
                            tt::STATUS,
                            pt::LOCAL_SOURCE,
                            ml::local_source::SUBTYPE_NONE,
                            vec![],
                        ));
                    }
                }
                pt::LINK_CONNECT_1 => {
                    // CONNECT: payload byte 1 is the source
                    if t.payload.len() >= 2 {
                        let src = t.payload[1];
                        self.connect_request(host, from, src, now);
                        // byte 2: a channel to start at (a timer's N.RADIO preset)
                        if let Some(channel) = lc::requested_channel(&t.payload) {
                            host.source_channel(src, channel);
                        }
                    }
                }
                pt::LINK_CONNECT_0 => {
                    // DISCONNECT_SIGNAL "<cur> -> <wanted>" (13-byte form, <wanted> last): the video
                    // master wants the bus for <wanted>. Answer with DISCONNECTED_SIGNAL and tell our
                    // listeners too; it will then claim the bus with STANDBY_0 on the audio link.
                    if t.payload.len() >= 13 {
                        let cur = if self.bus_state == BusState::Distributing { self.source } else { t.payload[1] };
                        let wanted = t.payload[12];
                        for &room in &self.listener_nodes {
                            host.send_telegram(Self::make(
                                room,
                                tt::STATUS,
                                pt::LINK_CONNECT_0,
                                lc::signal::DISCONNECTED,
                                lc::changed(cur, wanted),
                            ));
                        }
                        host.send_telegram(Self::make(
                            from,
                            tt::STATUS,
                            pt::LINK_CONNECT_0,
                            lc::signal::DISCONNECTED,
                            lc::with_second_source(cur, lc::Mode::WithWantedSource, true, lc::WANTED_BYTE11_ANSWER, wanted),
                        ));
                    }
                }
                other => debug!("Unhandled Masterlink request, payload type {other:#04x}"),
            }
            return;
        }

        if t.telegram_type == tt::STATUS && from_vm && to_us {
            // STATUS from the video master
            if t.payload_type == pt::LINK_INFO
                && self.bus_state == BusState::Arbitrating
                && self.arbitration == Arbitration::LinkInfo
            {
                self.arbitration_step_link_info_ok(host, now);
            } else if t.payload_type == pt::AUDIO_BUS
                && self.bus_state == BusState::Arbitrating
                && self.arbitration == Arbitration::AudioBus
            {
                if t.payload.is_empty() {
                    self.start_distribution(host); // free: the VM is not using the bus
                } else {
                    // busy "02 01 00 <src> [01]": the VM distributes <src>. Ask it to switch to ours
                    // (DISCONNECT_SIGNAL <src> -> ours).
                    self.vm_busy_source = if t.payload.len() >= 4 { t.payload[3] } else { 0 };
                    info!("Video master is using the audio bus; requesting a change");
                    host.send_telegram(Self::make(
                        node::VIDEO_MASTER,
                        tt::REQUEST,
                        pt::LINK_CONNECT_0,
                        lc::signal::DISCONNECT,
                        lc::with_second_source(
                            self.vm_busy_source,
                            lc::Mode::WithWantedSource,
                            true,
                            lc::WANTED_BYTE11_AUDIO_MASTER,
                            self.source,
                        ),
                    ));
                    self.arbitration = Arbitration::ChangeRequest;
                    self.arbitration_deadline = now + Self::VM_REPLY_TIMEOUT;
                }
            } else if t.payload_type == pt::LINK_CONNECT_0
                && t.subtype == lc::signal::DISCONNECTED
                && self.bus_state == BusState::Arbitrating
                && self.arbitration == Arbitration::ChangeRequest
            {
                // The VM agreed (DISCONNECTED_SIGNAL): claim the bus
                Self::send_take_bus_from_vm(host);
                self.arbitration = Arbitration::Claim;
                self.arbitration_deadline = now + Self::VM_REPLY_TIMEOUT;
            } else if t.payload_type == pt::LINK_CONNECT_0 && self.vm_source != 0 {
                // VM confirms the source we asked for: play it from the bus
                self.bus_state = BusState::ListeningToVm;
                host.set_ml_listening(true);
            }
            return;
        }

        if t.telegram_type == tt::COMMAND {
            match t.payload_type {
                pt::STANDBY_1 => {
                    if t.dest_node == node::ALL || t.payload.is_empty() {
                        // "All standby" (subtype_1_all, to ALL)
                        if self.bus_state == BusState::ListeningToVm {
                            self.stop_listening_to_vm(host, false);
                        }
                        if matches!(self.bus_state, BusState::Distributing | BusState::Arbitrating) {
                            self.stop_distribution(host, true);
                        }
                        self.local_source = 0;
                        host.link_standby();
                    } else if to_us {
                        // One room leaves (subtype_1_node, "<audio link> <video link>")
                        self.listener_nodes.remove(&from);
                        self.pending_requesters.remove(&from);
                        if self.bus_state == BusState::Distributing && self.listener_nodes.is_empty() && self.local_source == 0 {
                            self.stop_distribution(host, false);
                        }
                    }
                }
                pt::STANDBY_0 if from_vm && to_us => {
                    // The video master takes the audio bus; confirm with COMMAND 0x1C (bus released)
                    if matches!(self.bus_state, BusState::Distributing | BusState::Arbitrating) {
                        self.stop_distribution(host, false);
                        host.send_telegram(Self::make(
                            node::VIDEO_MASTER,
                            tt::COMMAND,
                            pt::BUS_RELEASED,
                            ml::bus_released::SUBTYPE_BS3000,
                            vec![ml::bus_released::BYTE0],
                        ));
                    }
                }
                pt::BUS_RELEASED if from_vm && to_us => {
                    // The VM confirms it let go of the bus
                    if self.bus_state == BusState::Arbitrating && self.arbitration == Arbitration::Claim {
                        self.start_distribution(host);
                    }
                }
                pt::SOURCE_CONTROL if to_us && t.payload.len() >= 2 => {
                    // "<source> <Beo4 key>"
                    host.source_key(t.payload[0], t.payload[1]);
                }
                _ => {}
            }
        }
    }
}

impl Default for AudioMaster {
    fn default() -> Self {
        Self::new(Self::DEFAULT_SYSTEM_CONFIGURATION)
    }
}
