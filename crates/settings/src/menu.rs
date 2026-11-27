//! The stock SETTINGS menu as data (captured on the BM5 on 27 Nov 2026, `work/bm5-ui/INDEX.md`).
//!
//! GO on SETTINGS in the MODE list opens 7 sections; RIGHT LEFT RIGHT LEFT then GO on SETTINGS
//! opens the hidden menu with 9. [`sections`] builds them from the [`Settings`] and the
//! [`Facts`] the core gathers (uptime, addresses, versions), so conditional items come and go as
//! settings change: DELAY or PLAY TIMER by AM option, the source-name items only as audio
//! master, CD naming only with a CD drive.
//!
//! An item is one of: a [`ItemKind::Choice`] (LEFT opens the picker, the wheel moves, GO keeps
//! the choice and RIGHT leaves it), a read-only [`ItemKind::Value`] (shown beside the item while
//! highlighted, as the stock TIME item does), or [`ItemKind::Unavailable`] for stock items that
//! neomaster does not do (software update, recovery, Spotify accounts...). The ui crate draws
//! this; [`Settings::apply`] carries out a choice.

use crate::names::{Slot, SourceName, AM_OPTIONS};
use crate::{PowerSaving, Settings, IR_TIMEOUT_RANGE, LANGUAGES, N_RADIO_BUFFER_RANGE};

/// What the core knows about the machine, for the info sections. Each list is (label, value)
/// rows in display order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    pub system_info: Vec<(String, String)>,
    pub product_info: Vec<(String, String)>,
    pub network_info: Vec<(String, String)>,
    /// The B&O USB CD drive is attached (gates CD naming).
    pub cd_drive: bool,
    /// Local date and time as TIME shows it, "2026-11-27 19:26:24".
    pub time: String,
    /// The machine's host name (when `network.host_name` is empty).
    pub host_name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SectionId {
    SystemInfo,
    SystemSettings,
    Maintenance,
    MusicServices,
    ServiceSettings,
    ProductInfo,
    NetworkInfo,
    NetworkSettings,
    Roles,
}

impl SectionId {
    pub fn title(self) -> &'static str {
        match self {
            SectionId::SystemInfo => "SYSTEM INFO",
            SectionId::SystemSettings => "SYSTEM SETTINGS",
            SectionId::Maintenance => "MAINTENANCE",
            SectionId::MusicServices => "MUSIC SERVICES",
            SectionId::ServiceSettings => "SERVICE SETTINGS",
            SectionId::ProductInfo => "PRODUCT INFO",
            SectionId::NetworkInfo => "NETWORK INFO",
            SectionId::NetworkSettings => "NETWORK SETTINGS",
            SectionId::Roles => "ROLES",
        }
    }
}

/// The sections on the arc, top to bottom (below MODE).
pub fn section_ids(hidden: bool) -> &'static [SectionId] {
    use SectionId::*;
    if hidden {
        &[
            SystemInfo,
            SystemSettings,
            Maintenance,
            MusicServices,
            ServiceSettings,
            ProductInfo,
            NetworkInfo,
            NetworkSettings,
            Roles,
        ]
    } else {
        &[SystemInfo, SystemSettings, Maintenance, MusicServices, ServiceSettings, NetworkInfo, Roles]
    }
}

/// A setting a choice writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Language,
    TimeZone,
    IrTimeout,
    PowerSaving,
    DebugLogging,
    NRadioBuffer,
    DisableMots,
    AudioMaster,
    AmOption,
    SlotName(Slot),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemKind {
    Choice {
        key: Key,
        options: Vec<String>,
        current: usize,
    },
    Value(String),
    Unavailable,
    /// A list of items one level down (stock TRANSCODING, MUSIC SERVICES › Spotify).
    Submenu(Vec<Item>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub label: String,
    pub kind: ItemKind,
}

impl Item {
    fn choice(label: &str, key: Key, options: Vec<String>, current: usize) -> Item {
        Item { label: label.into(), kind: ItemKind::Choice { key, options, current } }
    }
    fn value(label: &str, value: impl Into<String>) -> Item {
        Item { label: label.into(), kind: ItemKind::Value(value.into()) }
    }
    fn unavailable(label: &str) -> Item {
        Item { label: label.into(), kind: ItemKind::Unavailable }
    }

    /// The current value as text, if the item has one.
    pub fn value_text(&self) -> Option<&str> {
        match &self.kind {
            ItemKind::Choice { options, current, .. } => options.get(*current).map(String::as_str),
            ItemKind::Value(v) => Some(v),
            ItemKind::Unavailable => Some("NOT AVAILABLE"),
            ItemKind::Submenu(_) => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Body {
    /// A table of (label, value) rows.
    Info(Vec<(String, String)>),
    Items(Vec<Item>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub id: SectionId,
    pub body: Body,
}

const YES_NO: [&str; 2] = ["YES", "NO"];
/// TIME ZONE choices. The stock picker lists every Windows zone; this is the European set plus
/// UTC, and the configured zone is added if it is not among them.
pub const TIME_ZONES: [&str; 12] = [
    "UTC",
    "Europe/London",
    "Europe/Dublin",
    "Europe/Lisbon",
    "Europe/Copenhagen",
    "Europe/Oslo",
    "Europe/Stockholm",
    "Europe/Berlin",
    "Europe/Amsterdam",
    "Europe/Paris",
    "Europe/Helsinki",
    "Europe/Athens",
];

fn strings<'a>(v: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    v.into_iter().map(String::from).collect()
}

fn yes_no(label: &str, key: Key, on: bool) -> Item {
    Item::choice(label, key, strings(YES_NO), if on { 0 } else { 1 })
}

fn time_zones(current: &str) -> (Vec<String>, usize) {
    let mut v = strings(TIME_ZONES);
    if !v.iter().any(|z| z == current) {
        v.push(current.to_string());
    }
    let i = v.iter().position(|z| z == current).unwrap_or(0);
    (v, i)
}

/// Build one section.
pub fn section(id: SectionId, s: &Settings, facts: &Facts, hidden: bool) -> Section {
    use SectionId::*;
    let body = match id {
        SystemInfo => {
            let mut rows = facts.system_info.clone();
            if !hidden {
                // The normal menu only shows the storage line.
                rows.retain(|(k, _)| k == "AVAILABLE STORAGE");
            }
            Body::Info(rows)
        }
        ProductInfo => Body::Info(facts.product_info.clone()),
        NetworkInfo => {
            let mut rows =
                vec![("HOST NAME".to_string(), host_name(s, facts)), ("LOCATION".to_string(), s.network.location.clone())];
            rows.extend(facts.network_info.iter().cloned());
            rows.push(("MUSIC SERVER".into(), music_server(s)));
            Body::Info(rows)
        }
        SystemSettings => {
            let lang = LANGUAGES.iter().position(|l| *l == s.system.language).unwrap_or(2);
            let (zones, zi) = time_zones(&s.system.time_zone);
            let mut v = vec![
                Item::choice("LANGUAGE", Key::Language, strings(LANGUAGES), lang),
                Item::choice("TIME ZONE", Key::TimeZone, zones, zi),
                Item::value("TIME", facts.time.clone()),
            ];
            if hidden {
                let r: Vec<String> = IR_TIMEOUT_RANGE.map(|n| n.to_string()).collect();
                let cur = (s.system.ir_timeout - IR_TIMEOUT_RANGE.start()) as usize;
                v.push(Item::choice("IR TIMEOUT", Key::IrTimeout, r, cur));
                v.push(Item::choice(
                    "POWER SAVING",
                    Key::PowerSaving,
                    strings(["AUTOMATIC", "OFF"]),
                    (s.system.power_saving == PowerSaving::Off) as usize,
                ));
                // Stock lists NO before YES here.
                v.push(Item::choice("DEBUG LOGGING", Key::DebugLogging, strings(["NO", "YES"]), s.system.debug_logging as usize));
            }
            Body::Items(v)
        }
        Maintenance => {
            let about = Item::value("ABOUT", format!("NEOMASTER {}", env!("CARGO_PKG_VERSION")));
            let v = if hidden {
                vec![
                    Item::unavailable("CLEAN-UP"),
                    Item::unavailable("UPDATE SOFTWARE"),
                    Item::unavailable("AUTOMATIC SOFTWARE UPDATE"),
                    Item::unavailable("SUBMIT LOGS TO BANG & OLUFSEN"),
                    Item::unavailable("REBOOT"),
                    Item::unavailable("SERVICE MENU"),
                    Item::unavailable("START RECOVERY"),
                    Item::unavailable("DELETE DATABASES"),
                    Item::unavailable("LATEST RELEASE INFO"),
                    about,
                ]
            } else {
                vec![
                    Item::unavailable("UPDATE SOFTWARE"),
                    Item::unavailable("AUTOMATIC SOFTWARE UPDATE"),
                    Item::unavailable("LATEST RELEASE INFO"),
                    about,
                ]
            };
            Body::Items(v)
        }
        MusicServices => Body::Items(vec![
            Item {
                label: "SPOTIFY".into(),
                kind: ItemKind::Submenu(vec![Item::unavailable("ADD ACCOUNT"), Item::unavailable("LANGUAGE")]),
            },
            Item::value("MUSIC ASSISTANT", music_server(s)),
        ]),
        ServiceSettings => {
            let r: Vec<String> = N_RADIO_BUFFER_RANGE.map(|n| n.to_string()).collect();
            let cur = (s.service.n_radio_buffer - N_RADIO_BUFFER_RANGE.start()) as usize;
            let mut v = vec![
                Item::choice("N.RADIO BUFFER", Key::NRadioBuffer, r, cur),
                yes_no("DISABLE MOTS", Key::DisableMots, s.service.disable_mots),
            ];
            if hidden {
                v.push(Item {
                    label: "TRANSCODING".into(),
                    kind: ItemKind::Submenu(vec![Item::unavailable("TRANSCODING"), Item::unavailable("CLEAR CACHE")]),
                });
                v.push(Item::unavailable("MUSIC STORAGE"));
                v.push(Item::value("MUSIC SERVER", music_server(s)));
                v.push(Item::unavailable("REBUILD MUSIC SERVER"));
            }
            Body::Items(v)
        }
        NetworkSettings => Body::Items(vec![
            Item::unavailable("CLIENT PASSWORD"),
            Item::value("HOST NAME", host_name(s, facts)),
            Item::value("LOCATION", s.network.location.clone()),
        ]),
        Roles => Body::Items(roles_items(s, facts)),
    };
    Section { id, body }
}

/// The Roles items, in BS5's order (`ConfigReader::GetCurrentRolesMenu`, FUN_005f580d).
fn roles_items(s: &Settings, facts: &Facts) -> Vec<Item> {
    let r = &s.roles;
    let mut v = Vec::new();
    let opt0 = r.am_option == 0;
    if r.audio_master && !opt0 {
        v.push(Item::unavailable("PLAY TIMER"));
    }
    v.push(yes_no("AUDIO MASTER", Key::AudioMaster, r.audio_master));
    let opts: Vec<String> = AM_OPTIONS.iter().map(|(o, _)| o.to_string()).collect();
    let cur = AM_OPTIONS.iter().position(|(o, _)| *o == r.am_option).unwrap_or(0);
    v.push(Item::choice("AM OPTION", Key::AmOption, opts, cur));
    if !r.audio_master || opt0 {
        // DELAY's unit and range are not decoded yet.
        v.push(Item::value("DELAY", r.delay.to_string()));
    }
    if r.audio_master {
        let mut slots = vec![Slot::NMusic, Slot::NRadio];
        if facts.cd_drive {
            slots.push(Slot::CdPlay);
        }
        for slot in slots {
            let offered = r.offered(slot);
            let cur = offered.iter().position(|n| *n == r.names.get(slot)).unwrap_or(0);
            let label = match slot {
                Slot::NMusic => "N.MUSIC",
                Slot::NRadio => "N.RADIO",
                Slot::CdPlay => "CD",
                Slot::MusicService => "MUSIC SERVICES",
            };
            v.push(Item::choice(label, Key::SlotName(slot), offered.iter().map(|n| n.label().to_string()).collect(), cur));
        }
    }
    v
}

fn host_name(s: &Settings, facts: &Facts) -> String {
    if s.network.host_name.is_empty() {
        facts.host_name.clone()
    } else {
        s.network.host_name.clone()
    }
}

fn music_server(s: &Settings) -> String {
    if s.music_assistant.url.is_empty() {
        "NOT CONFIGURED".into()
    } else {
        s.music_assistant.url.clone()
    }
}

/// All sections of the open menu.
pub fn sections(s: &Settings, facts: &Facts, hidden: bool) -> Vec<Section> {
    section_ids(hidden).iter().map(|id| section(*id, s, facts, hidden)).collect()
}

impl Settings {
    /// Carry out a choice made in a picker: option `index` of the item with `key`. Returns
    /// false if nothing changed (same value, or a choice the rules refuse).
    pub fn apply(&mut self, key: Key, index: usize) -> bool {
        let before = self.clone();
        match key {
            Key::Language => {
                if let Some(l) = LANGUAGES.get(index) {
                    self.system.language = l.to_string();
                }
            }
            Key::TimeZone => {
                let (zones, _) = time_zones(&self.system.time_zone);
                if let Some(z) = zones.get(index) {
                    self.system.time_zone = z.clone();
                }
            }
            Key::IrTimeout => {
                if let Some(n) = IR_TIMEOUT_RANGE.clone().nth(index) {
                    self.system.ir_timeout = n;
                }
            }
            Key::PowerSaving => {
                self.system.power_saving = if index == 1 { PowerSaving::Off } else { PowerSaving::Automatic };
            }
            Key::DebugLogging => self.system.debug_logging = index == 1,
            Key::NRadioBuffer => {
                if let Some(n) = N_RADIO_BUFFER_RANGE.clone().nth(index) {
                    self.service.n_radio_buffer = n;
                }
            }
            Key::DisableMots => self.service.disable_mots = index == 0,
            Key::AudioMaster => {
                self.roles.set_audio_master(index == 0);
            }
            Key::AmOption => {
                if let Some((o, _)) = AM_OPTIONS.get(index) {
                    self.roles.am_option = *o;
                }
            }
            Key::SlotName(slot) => {
                let offered: Vec<SourceName> = self.roles.offered(slot);
                if let Some(n) = offered.get(index) {
                    self.roles.set_name(slot, *n);
                }
            }
        }
        *self != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn titles(hidden: bool) -> Vec<&'static str> {
        section_ids(hidden).iter().map(|s| s.title()).collect()
    }

    fn items(s: &Settings, id: SectionId, hidden: bool) -> Vec<Item> {
        match section(id, s, &Facts::default(), hidden).body {
            Body::Items(v) => v,
            Body::Info(_) => panic!("{id:?} is an info section"),
        }
    }

    fn labels(v: &[Item]) -> Vec<&str> {
        v.iter().map(|i| i.label.as_str()).collect()
    }

    #[test]
    fn seven_normal_and_nine_hidden_sections() {
        assert_eq!(
            titles(false),
            ["SYSTEM INFO", "SYSTEM SETTINGS", "MAINTENANCE", "MUSIC SERVICES", "SERVICE SETTINGS", "NETWORK INFO", "ROLES"]
        );
        assert_eq!(titles(true).len(), 9);
        assert!(titles(true).contains(&"PRODUCT INFO") && titles(true).contains(&"NETWORK SETTINGS"));
    }

    #[test]
    fn hidden_system_settings_add_three_items() {
        let s = Settings::default();
        assert_eq!(labels(&items(&s, SectionId::SystemSettings, false)), ["LANGUAGE", "TIME ZONE", "TIME"]);
        assert_eq!(
            labels(&items(&s, SectionId::SystemSettings, true)),
            ["LANGUAGE", "TIME ZONE", "TIME", "IR TIMEOUT", "POWER SAVING", "DEBUG LOGGING"]
        );
        let ir = &items(&s, SectionId::SystemSettings, true)[3];
        assert_eq!(ir.value_text(), Some("6"));
    }

    #[test]
    fn roles_items_follow_the_master_state() {
        let mut s = Settings::default();
        assert_eq!(labels(&items(&s, SectionId::Roles, false)), ["AUDIO MASTER", "AM OPTION", "DELAY"]);
        assert!(s.apply(Key::AudioMaster, 0));
        assert_eq!(labels(&items(&s, SectionId::Roles, false)), ["AUDIO MASTER", "AM OPTION", "DELAY", "N.MUSIC", "N.RADIO"]);
        assert!(s.apply(Key::AmOption, 2)); // option 2
        assert_eq!(s.roles.system_configuration(), 6);
        assert_eq!(
            labels(&items(&s, SectionId::Roles, false)),
            ["PLAY TIMER", "AUDIO MASTER", "AM OPTION", "N.MUSIC", "N.RADIO"]
        );
        let cd = section(SectionId::Roles, &s, &Facts { cd_drive: true, ..Facts::default() }, false);
        let Body::Items(v) = cd.body else { unreachable!() };
        assert_eq!(v.last().unwrap().label, "CD");
    }

    #[test]
    fn apply_choices() {
        let mut s = Settings::default();
        assert!(s.apply(Key::IrTimeout, 0));
        assert_eq!(s.system.ir_timeout, 2);
        assert!(!s.apply(Key::IrTimeout, 0));
        assert!(s.apply(Key::NRadioBuffer, 4));
        assert_eq!(s.service.n_radio_buffer, 6);
        assert!(s.apply(Key::DisableMots, 0));
        assert!(s.service.disable_mots);
        assert!(s.apply(Key::Language, 1));
        assert_eq!(s.system.language, "DEUTSCH");
        assert!(!s.apply(Key::SlotName(Slot::NMusic), 0), "names are fixed while not audio master");
        s.apply(Key::AudioMaster, 0);
        assert!(s.apply(Key::SlotName(Slot::NMusic), 0));
        assert_eq!(s.roles.names.n_music, SourceName::AMem);
        assert_eq!(s.roles.source_map().n_music, 0x79);
    }

    #[test]
    fn normal_system_info_shows_storage_only() {
        let f = Facts {
            system_info: vec![("BM5 UPTIME".into(), "1d".into()), ("AVAILABLE STORAGE".into(), "10 MB".into())],
            ..Facts::default()
        };
        let s = Settings::default();
        assert_eq!(
            section(SectionId::SystemInfo, &s, &f, false).body,
            Body::Info(vec![("AVAILABLE STORAGE".into(), "10 MB".into())])
        );
        let Body::Info(rows) = section(SectionId::SystemInfo, &s, &f, true).body else { unreachable!() };
        assert_eq!(rows.len(), 2);
    }
}
