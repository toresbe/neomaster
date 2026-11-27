//! Named Masterlink protocol values: node addresses, telegram and payload types, payload subtypes
//! ("versions") and the layout of the payloads AudioMaster and SourceCenter send.
//!
//! Port of libpc2 `masterlink/protocol.hpp` (toresbe/libpc2 9656831). Where the names come from:
//! - **B&O**: B&O's own names, from BeoIpcML.exe (BeoMaster 5 software 7.04). Its debug-name
//!   function (FUN_0421a1f4) names telegram/payload pairs; its log strings name some fields.
//! - **decompiled**: the meaning follows from the code in BeoIpcML.exe that builds or parses that
//!   telegram (addresses given, e.g. FUN_0422c3b8 builds SOURCE_STATUS), not from a name.
//! - **observed**: seen on the wire in USB captures or live Masterlink sniffs, meaning inferred.
//! - **TODO**: the meaning is not known. The value is what B&O's software sends.
//!
//! The telegram layout, as BeoIpcML builds it:
//!
//! ```text
//! dest src 01 TYPE dest_src src_src 00 PAYLOAD_TYPE SIZE SUBTYPE payload[SIZE] checksum 00
//! ```
//!
//! SUBTYPE selects the meaning and format of the payload (e.g. LINK_CONNECT_STATUS subtype 5 =
//! CONNECTED_SIGNAL, 7 = DISCONNECTED_SIGNAL), so each payload type has its own set of subtypes.
//!
//! See also reports/masterlink-protocol.md and reports/pc2-control.md in the rea-local workspace.

/// Masterlink node addresses. Room nodes are 0x01-0x13.
pub mod node {
    /// B&O: (broadcast) "ALL"
    pub const ALL: u8 = 0x80;
    /// B&O: ALL_SOURCE_USERS (every node that can listen to a source)
    pub const ALL_SOURCE_USERS: u8 = 0x83;
    /// V_MASTER
    pub const VIDEO_MASTER: u8 = 0xc0;
    /// A_MASTER
    pub const AUDIO_MASTER: u8 = 0xc1;
    /// PC_1; the MLGW calls a BeoMaster 5 at this address SOURCE_CENTER
    pub const PC_1: u8 = 0xc2;
    pub const SOURCE_CENTER: u8 = PC_1;
}

/// A readable name for a node address (libpc2 `MasterlinkTelegram::node_name`).
pub fn node_name(n: u8) -> String {
    match n {
        0x80 | 0x83 => "ALL".into(),
        0xc0 => "V_MASTER".into(),
        0xc1 => "A_MASTER".into(),
        0xc2 => "PC_1".into(),
        _ => format!("NODE_{n:02X}"),
    }
}

/// Telegram types (byte 3).
pub mod telegram_type {
    pub const COMMAND: u8 = 0x0a;
    pub const REQUEST: u8 = 0x0b;
    pub const STATUS: u8 = 0x14;
    pub const INFO: u8 = 0x2c;
    /// Not seen so far, but a forum post suggests it exists.
    pub const TIME: u8 = 0x40;
    pub const CONFIG: u8 = 0x5e;
}

/// Payload types (byte 7), with B&O's names. Older libpc2 names in brackets.
pub mod payload_type {
    /// B&O: LINK_INFO_COMMAND / LINK_INFO_STATUS [MASTER_PRESENT]
    pub const LINK_INFO: u8 = 0x04;
    /// [DISPLAY_DATA]
    pub const DISPLAY_DATA: u8 = 0x06;
    /// no B&O name; the audio bus arbitration question/answer [AUDIO_BUS]
    pub const AUDIO_BUS: u8 = 0x08;
    /// [METADATA]
    pub const METADATA: u8 = 0x0b;
    /// B&O: SOURCE_CONTROL (a Beo4 key for a source) [BEO4_KEY]
    pub const SOURCE_CONTROL: u8 = 0x0d;
    /// B&O: STANDBY_0 (stop / take the bus) [STANDBY]
    pub const STANDBY_0: u8 = 0x10;
    /// B&O: STANDBY_1 (leave / all standby) [RELEASE]
    pub const STANDBY_1: u8 = 0x11;
    /// B&O: STANDBY_2
    pub const STANDBY_2: u8 = 0x12;
    /// no B&O name; observed: confirms the audio bus was handed over
    pub const BUS_RELEASED: u8 = 0x1c;
    /// B&O: IR_SIMUL_LOW (a simulated Beo4 key press on another node)
    pub const IR_SIMUL_LOW: u8 = 0x20;
    /// no B&O name; observed: what a master plays locally
    pub const LOCAL_SOURCE: u8 = 0x30;
    /// B&O: PROGRAMMING_STATUS (the clock broadcast) [CLOCK]
    pub const PROGRAMMING_STATUS: u8 = 0x40;
    /// B&O: LINK_CONNECT_COMMAND_0 / LINK_CONNECT_STATUS [TRACK_INFO]
    pub const LINK_CONNECT_0: u8 = 0x44;
    /// B&O: LINK_CONNECT_COMMAND_1 (a room asks for a source) [GOTO_SOURCE]
    pub const LINK_CONNECT_1: u8 = 0x45;
    /// B&O: LINK_CONNECT_COMMAND_2
    pub const LINK_CONNECT_2: u8 = 0x46;
    /// B&O: SL_CONFIG_STATUS
    pub const SL_CONFIG: u8 = 0x48;
    /// B&O: LOCK_MANAGER_COMMAND / LOCK_MANAGER_STATUS (the bus lock, "function key")
    pub const LOCK_MANAGER: u8 = 0x5c;
    /// B&O: SOURCE_CONNECT_COMMAND / SOURCE_CONNECT_STATUS [DISTRIBUTION_REQUEST]
    pub const SOURCE_CONNECT: u8 = 0x6c;
    /// B&O: PRI_SYSTEM_STATUS [TRACK_INFO_LONG]
    pub const PRI_SYSTEM_STATUS: u8 = 0x82;
    /// B&O: SOURCE_STATUS [STATUS_INFO]
    pub const SOURCE_STATUS: u8 = 0x87;
}

/// Masterlink source IDs (libpc2 `Masterlink::source`). Several IDs have two names.
pub mod source {
    pub const TV: u8 = 0x0b;
    pub const V_MEM: u8 = 0x15;
    pub const V_TAPE: u8 = 0x15;
    pub const V_TAPE2: u8 = 0x16;
    pub const DVD2: u8 = 0x16;
    pub const DTV: u8 = 0x1f;
    pub const SAT: u8 = 0x1f;
    pub const DVD: u8 = 0x29;
    pub const V_AUX: u8 = 0x33;
    pub const DTV2: u8 = 0x33;
    pub const V_AUX2: u8 = 0x3e;
    pub const DOORCAM: u8 = 0x3e;
    /// The BeoMaster 5's own PC source.
    pub const PC: u8 = 0x47;
    pub const RADIO: u8 = 0x6f;
    pub const A_MEM: u8 = 0x79;
    pub const A_TAPE: u8 = 0x79;
    /// N.MUSIC on a BeoMaster 5.
    pub const A_MEM2: u8 = 0x7a;
    pub const N_MUSIC: u8 = A_MEM2;
    pub const CD: u8 = 0x8d;
    pub const A_AUX: u8 = 0x97;
    pub const N_RADIO: u8 = 0xa1;
    pub const PHONO: u8 = 0xa1;
}

/// The audio and video links, as flag pairs in STANDBY_0, STANDBY_1 v2 and LINK_CONNECT payloads
/// (decompiled: FUN_04229034, FUN_04229358, FUN_04228e2c).
pub mod link {
    pub const OFF: u8 = 0x00;
    pub const ON: u8 = 0x01;
}

/// LINK_INFO (0x04): "Is there a master?" REQUEST and its STATUS answer.
/// Decompiled: FUN_04223034 builds the request, FUN_042233a0 the answer.
pub mod link_info {
    /// request and answer
    pub const SUBTYPE: u8 = 4;
    /// BeoIpcML's own node to V_MASTER, payload "01"; TODO
    pub const SUBTYPE_OWN_NODE_PROBE: u8 = 23;

    /// Payload byte 0: which kind of node is talking. In a request it is the requester's kind; in
    /// an answer it is the master the answer speaks for (01 if sent as C1, 02 if sent as C0).
    pub mod node_kind {
        /// BeoIpcML SystemConfiguration 4, 5, 6
        pub const AUDIO_MASTER: u8 = 0x01;
        /// SystemConfiguration 2
        pub const VIDEO_MASTER: u8 = 0x02;
        /// SystemConfiguration 1; TODO: name
        pub const CONFIG_1: u8 = 0x03;
        /// observed from room nodes; TODO: BeoIpcML never sends it
        pub const LINK_ROOM_08: u8 = 0x08;
        /// SystemConfiguration 3, 7-10 (link rooms, source center)
        pub const LINK_ROOM: u8 = 0x0a;
    }

    /// Payload byte 1 of a request. BeoIpcML sends 02 in SystemConfiguration 5 ("A.OPT = 0") and
    /// 01 otherwise (decompiled, FUN_04223034). Confirmed in the USB captures: every configuration
    /// 5 capture sends "01 02 00", the configuration 6 ones "01 01 00". Answers always carry 01.
    /// TODO: what the byte means (the PC2 firmware also answers 02 for a sleeping PC).
    pub const BYTE1_DEFAULT: u8 = 0x01;
    pub const BYTE1_A_OPT_0: u8 = 0x02;
    pub const fn request_byte1(system_configuration: u8) -> u8 {
        if system_configuration == 5 {
            BYTE1_A_OPT_0
        } else {
            BYTE1_DEFAULT
        }
    }
    /// Payload byte 2: 00 in a request; in an answer, whether that master exists (BeoIpcML
    /// answers 01 as C1 only if it is configured as audio master, and always 01 as C0).
    pub const ABSENT: u8 = 0x00;
    pub const PRESENT: u8 = 0x01;

    pub fn request(from: u8, system_configuration: u8) -> Vec<u8> {
        vec![from, request_byte1(system_configuration), ABSENT]
    }
    pub fn answer(as_kind: u8, is_present: bool) -> Vec<u8> {
        vec![as_kind, BYTE1_DEFAULT, if is_present { PRESENT } else { ABSENT }]
    }
}

/// AUDIO_BUS (0x08): "Are you using the audio bus?"
/// Decompiled: FUN_04224950 (request), FUN_04224e7c (answer).
pub mod audio_bus {
    /// empty payload
    pub const SUBTYPE_REQUEST: u8 = 1;
    /// answer, empty payload: not distributing
    pub const SUBTYPE_FREE: u8 = 4;
    /// answer: distributing, payload below
    pub const SUBTYPE_BUSY: u8 = 6;
    /// Busy payload: 02 01 <flag> <source>. TODO: the meaning of 02 and 01. <flag> is a BeoIpcML
    /// object field (always 00 in captures). A BeoSound 3000 appends a fifth byte, 01.
    pub const BUSY_BYTE0: u8 = 0x02;
    pub const BUSY_BYTE1: u8 = 0x01;
    /// TODO
    pub const BUSY_FLAG_DEFAULT: u8 = 0x00;

    pub fn busy(src: u8) -> Vec<u8> {
        vec![BUSY_BYTE0, BUSY_BYTE1, BUSY_FLAG_DEFAULT, src]
    }
}

/// 0x30 (local source). Decompiled: FUN_04229e30 (request), FUN_042311d8 (answer). No B&O name.
pub mod local_source {
    /// empty payload
    pub const SUBTYPE_REQUEST: u8 = 2;
    /// answer, empty payload: nothing plays locally
    pub const SUBTYPE_NONE: u8 = 4;
    /// answer: payload 08 <source>
    pub const SUBTYPE_PLAYING: u8 = 6;
    /// TODO: meaning (constant in BeoIpcML)
    pub const PLAYING_MARKER: u8 = 0x08;

    pub fn playing(src: u8) -> Vec<u8> {
        vec![PLAYING_MARKER, src]
    }
}

/// LINK_CONNECT (0x44 / 0x45): a room connecting to (or being moved off) a master's source.
/// For 0x44 the subtype is B&O's "ConnectType" (log text: "prTGtrxLINK_CONNECT_STATUS: Invalid
/// ConnectType", "RxD: STA:CONNECTED_SIGNAL", "Transmitting LINK_CONNECT_COMMAND /
/// DISCONNECT_SIGNAL"). Decompiled: FUN_04228e2c (0x45 request), FUN_04230ae0 (0x44 status),
/// FUN_04230ec0 (0x44 request).
pub mod link_connect {
    use super::link;

    /// The subtype ("ConnectType").
    pub mod signal {
        /// LINK_CONNECT_COMMAND_1: "connect me to <source>"
        pub const CONNECT: u8 = 1;
        /// LINK_CONNECT_COMMAND_0, B&O: DISCONNECT_SIGNAL ("<cur> -> <wanted>, please")
        pub const DISCONNECT: u8 = 3;
        /// LINK_CONNECT_STATUS, B&O: CONNECTED_SIGNAL
        pub const CONNECTED: u8 = 5;
        /// LINK_CONNECT_STATUS; inferred: B&O's DISCONNECTED_SIGNAL (source changed / given up)
        pub const DISCONNECTED: u8 = 7;
        /// LINK_CONNECT_STATUS with a 16-bit value; TODO
        pub const STATUS_9: u8 = 9;
    }

    /// Payload byte 0 (decompiled: the 0x45 builder sets 02 together with the audio-link flag and
    /// 01 together with the video-link flag).
    pub mod link_type {
        pub const VIDEO: u8 = 0x01;
        pub const AUDIO: u8 = 0x02;
    }
    /// TODO: constant in every builder
    pub const BYTE2: u8 = 0x00;
    /// TODO: constant in every builder
    pub const BYTE3: u8 = 0x02;

    /// Payload byte 6. TODO: in DISCONNECT_SIGNAL B&O uses 02 = plain (8 bytes) and 03 = with the
    /// wanted source appended (13 bytes); in statuses it is a parameter (00 when connected, 02 in
    /// the change notice to listeners, 03 in the answer to a DISCONNECT_SIGNAL).
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    #[repr(u8)]
    pub enum Mode {
        None = 0x00,
        Changed = 0x02,
        WithWantedSource = 0x03,
    }
    /// Bytes 7 and 11 of the 13-byte "with wanted source" form: 01 ... <byte 11> <wanted>.
    /// TODO: B&O's builder writes 01 and 02; a BeoSound 3000 sent ff and 03 in its place.
    pub const WANTED_FORM_BYTE7: u8 = 0x01;
    /// as a VM sent it (and BeoIpcML)
    pub const WANTED_BYTE11_VIDEO_MASTER: u8 = 0x02;
    /// as a BeoSound 3000 sent it
    pub const WANTED_BYTE11_AUDIO_MASTER: u8 = 0xff;
    /// in the answer to the request
    pub const WANTED_BYTE11_ANSWER: u8 = 0x03;

    /// 02 <src> 00 02 <audio> <video>
    pub fn head(src: u8) -> Vec<u8> {
        vec![link_type::AUDIO, src, BYTE2, BYTE3, link::ON, link::OFF]
    }
    /// LINK_CONNECT_COMMAND_1 (subtype connect): ask a master for its source, over the audio link.
    pub fn connect_request(src: u8) -> Vec<u8> {
        let mut p = head(src);
        p.push(0x00); // TODO (decompiled: always 00)
        p
    }
    /// LINK_CONNECT_STATUS (subtype connected), 8 bytes.
    pub fn connected(src: u8) -> Vec<u8> {
        let mut p = head(src);
        p.extend_from_slice(&[Mode::None as u8, 0x00]);
        p
    }
    /// The 13-byte form: head, <mode>, then 01 00 00 00 / 00 00 00 00, <byte 11>, <second source>.
    pub fn with_second_source(src: u8, mode: Mode, wanted_form: bool, byte11: u8, second: u8) -> Vec<u8> {
        let mut p = head(src);
        p.extend_from_slice(&[mode as u8, if wanted_form { WANTED_FORM_BYTE7 } else { 0x00 }, 0x00, 0x00, 0x00, byte11, second]);
        p
    }
    /// LINK_CONNECT_STATUS (subtype disconnected) to a listener: "<old> is gone, now <new>".
    pub fn changed(old_src: u8, new_src: u8) -> Vec<u8> {
        with_second_source(old_src, Mode::Changed, false, 0x00, new_src)
    }
}

/// SOURCE_STATUS (0x87). Decompiled: code at 0x422c3b8 builds all forms; B&O names subtypes 1-4
/// (FUN_0421a1f4).
pub mod source_status {
    /// B&O: STATIC_SOURCE_STATUS
    pub const SUBTYPE_STATIC: u8 = 1;
    /// B&O: DYNAMIC_SOURCE_STATUS
    pub const SUBTYPE_DYNAMIC: u8 = 2;
    /// B&O: ASCII_SOURCE_STATUS
    pub const SUBTYPE_ASCII: u8 = 3;
    /// B&O: SOURCE_SYSTEM_STATUS (the one we send)
    pub const SUBTYPE_SYSTEM: u8 = 4;

    /// Source state, payload byte 11. Inferred: the field is the same BeoIpcML object field as
    /// PRI_SYSTEM_STATUS byte 3, and its three observed values match the three forms.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    #[repr(u8)]
    pub enum State {
        Idle = 0x00,
        Playing = 0x02,
        /// the "everything switched off" form
        Stopped = 0x06,
    }

    /// Bytes 2-5: two 16-bit masks. The second is 0x1fbe, built by OR-ing bits 1-5, 7-12 one by
    /// one; the first is a function of it that is 0000 in every capture. TODO: what the bits mean.
    pub const MASK_A_HI: u8 = 0x00;
    pub const MASK_A_LO: u8 = 0x00;
    pub const MASK_B_HI: u8 = 0x1f;
    pub const MASK_B_LO: u8 = 0xbe;

    /// SOURCE_SYSTEM_STATUS for `src` in state `state` (idle or playing): 31 bytes.
    /// The TODO bytes are constants or caller parameters in BeoIpcML; the values are what it sends.
    #[rustfmt::skip]
    pub fn system_status(src: u8, state: State) -> Vec<u8> {
        vec![src, 0x01,                     // 0x01: TODO (constant)
             MASK_A_HI, MASK_A_LO, MASK_B_HI, MASK_B_LO,
             0x01,                          // TODO (constant)
             0x00, 0x00,                    // 16-bit field (BeoIpcML +0x50; also in PRI_SYSTEM)
             0x00,                          // field +0x4d (also PRI_SYSTEM byte 2); TODO
             0xff,                          // TODO (constant)
             state as u8,
             0x01,                          // 1 = audio source, 2 = video (for PC: 1-3); TODO
             0x00,                          // caller parameter; TODO
             0x03,                          // TODO (constant)
             0x01,                          // caller parameter; TODO
             0x01, 0x01, 0x03, 0x00, 0x02,  // TODO (constants)
             0x00, 0x00, 0x00,
             0x00,                          // caller parameter (same as byte 13); TODO
             0x01,                          // caller parameter; TODO
             0x00, 0x00, 0x00, 0x00,
             0x00]                          // caller parameter; TODO
    }

    /// The "stopped" form (26 bytes): the fields are ff and the state is stopped.
    #[rustfmt::skip]
    pub fn stopped(src: u8) -> Vec<u8> {
        vec![src, 0x01, MASK_A_HI, MASK_A_LO, MASK_B_HI, MASK_B_LO, 0x01,
             0xff, 0xff, 0xff, 0xff,
             State::Stopped as u8,
             0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
             0x00, 0x00, 0x00,
             0x00,                          // caller parameter (as byte 13 above); TODO
             0x01]                          // caller parameter (as byte 25 above); TODO
    }
}

/// PRI_SYSTEM_STATUS (0x82). Decompiled: code at 0x422c2fc. Payload:
/// `06 <src> <field 4d> <state> 00 00 00 <16-bit field 50> 01`, where <field 4d>, <state> and the
/// 16-bit field are the same values as in SOURCE_STATUS.
pub mod pri_system {
    /// sent for an audio source (BeoIpcML: field 45 set)
    pub const SUBTYPE_AUDIO: u8 = 1;
    /// otherwise; observed from a video master
    pub const SUBTYPE_VIDEO: u8 = 2;
    /// TODO: constant in BeoIpcML
    pub const BYTE0: u8 = 0x06;
    /// TODO: constant in BeoIpcML
    pub const LAST_BYTE: u8 = 0x01;

    /// For a source that is playing (the only form we send).
    pub fn playing(src: u8) -> Vec<u8> {
        vec![BYTE0, src, 0x00, super::source_status::State::Playing as u8, 0x00, 0x00, 0x00, 0x00, 0x00, LAST_BYTE]
    }
}

/// PROGRAMMING_STATUS (0x40): the clock broadcast. Decompiled: FUN_042257a0. Payload (BCD):
/// `0a 00 03 hh mm ss 00 DD MM YY <ClockPriority>`.
pub mod clock {
    pub const SUBTYPE: u8 = 11;
    /// TODO: constant in BeoIpcML
    pub const BYTE0: u8 = 0x0a;
    /// TODO: constant
    pub const BYTE1: u8 = 0x00;
    /// TODO: constant
    pub const BYTE2: u8 = 0x03;
    /// TODO: constant
    pub const BYTE6: u8 = 0x00;
    /// Last byte: the "ClockPriority" registry value (HKCU\Software\Bang & Olufsen\Beolink PC).
    /// 02 in every capture.
    pub const DEFAULT_PRIORITY: u8 = 0x02;

    /// Local wall-clock time for the broadcast. `month` is 1-12, `year` the full year.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct LocalTime {
        pub hour: u8,
        pub minute: u8,
        pub second: u8,
        pub day: u8,
        pub month: u8,
        pub year: u16,
    }

    pub fn bcd(v: u8) -> u8 {
        ((v / 10) << 4) | (v % 10)
    }

    pub fn payload(t: LocalTime) -> Vec<u8> {
        vec![
            BYTE0,
            BYTE1,
            BYTE2,
            bcd(t.hour),
            bcd(t.minute),
            bcd(t.second),
            BYTE6,
            bcd(t.day),
            bcd(t.month),
            bcd((t.year % 100) as u8),
            DEFAULT_PRIORITY,
        ]
    }
}

/// STANDBY_0 / STANDBY_1 (0x10 / 0x11). Decompiled: FUN_04229034 (STANDBY_0), FUN_04225594
/// (STANDBY_1 to ALL), FUN_04229358 (STANDBY_1 v2).
pub mod standby {
    use super::link;

    /// STANDBY_0: <audio link> <video link> <reason>
    pub const SUBTYPE_0: u8 = 3;
    /// STANDBY_1 to ALL, empty: "all standby"
    pub const SUBTYPE_1_ALL: u8 = 1;
    /// STANDBY_1: <audio link> <video link>, one node leaves
    pub const SUBTYPE_1_NODE: u8 = 2;

    /// STANDBY_0 byte 2. B&O sends 1 normally; when it closes a link because of the device type
    /// it sends 2 for an audio device and 3 for a video device.
    pub mod reason {
        /// TODO: name. Observed when a master takes the bus
        pub const STANDBY: u8 = 0x01;
        /// observed from listeners after a DISCONNECTED_SIGNAL
        pub const CLOSE_AUDIO_LINK: u8 = 0x02;
        pub const CLOSE_VIDEO_LINK: u8 = 0x03;
    }

    /// STANDBY_0 on the audio link: a master takes the audio bus from the other master.
    pub fn take_audio_bus() -> Vec<u8> {
        vec![link::ON, link::OFF, reason::STANDBY]
    }
    /// STANDBY_1 subtype 2 on the audio link: we leave.
    pub fn leave_audio() -> Vec<u8> {
        vec![link::ON, link::OFF]
    }
}

/// 0x1C (bus released). No B&O name. COMMAND payload: 01 [nodes...]. Decompiled: FUN_04225010
/// sends subtype 1 with 01 followed by a list of nodes (a video master sent "01 04" while node 04
/// was listening). A BeoSound 3000 sent subtype 2 with just "01", which is what we copy.
pub mod bus_released {
    pub const SUBTYPE_WITH_NODES: u8 = 1;
    pub const SUBTYPE_BS3000: u8 = 2;
    /// TODO: constant
    pub const BYTE0: u8 = 0x01;
}

/// LOCK_MANAGER (0x5C): the bus lock ("function key"). Decompiled: FUN_04223f74
/// (REQUEST/COMMAND), FUN_04223fe8 (STATUS); the PC2 firmware runs the same exchange
/// (pc2-firmware.md). Payload is always "01".
pub mod lock_manager {
    /// REQUEST to ALL: who holds the lock?
    pub const SUBTYPE_REQUEST: u8 = 1;
    /// STATUS from the holder
    pub const SUBTYPE_HOLDER: u8 = 2;
    /// STATUS, other form; TODO
    pub const SUBTYPE_HOLDER_ALT: u8 = 3;
    /// COMMAND to the holder: hand it over
    pub const SUBTYPE_TAKE: u8 = 4;
    /// COMMAND, other form; TODO
    pub const SUBTYPE_COMMAND_ALT: u8 = 5;
    pub const PAYLOAD: u8 = 0x01;
}

/// SOURCE_CONNECT (0x6C). Decompiled: FUN_042338d8 builds the STATUS (subtype 8) with a one- or
/// two-byte payload.
pub mod source_connect {
    pub const SUBTYPE_STATUS: u8 = 8;
    /// inferred from the flow; always 01 in captures
    pub const CONNECTED: u8 = 0x01;
}

/// IR_SIMUL_LOW (0x20): a Beo4 key pressed on another node: `00 01 ff ff <key>`. Observed (as
/// libpc2's send_beo4_code()).
pub mod ir_simul {
    pub const SUBTYPE: u8 = 2;
    pub fn key(beo4_key: u8) -> Vec<u8> {
        vec![0x00, 0x01, 0xff, 0xff, beo4_key] // TODO: the first four bytes
    }
}

/// SOURCE_CONTROL (0x0D): COMMAND subtype 1, payload `<source> <Beo4 key>`. Decompiled: FUN_04228f48.
pub mod source_control {
    pub const SUBTYPE: u8 = 1;
}
