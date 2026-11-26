//! The launcher: a borderless, see-through window shaped like two lobes and a bridge. This Mac is always on
//! the left, the other Mac on the right, each drawn as its own MacBook. While files move, the electric lines
//! run from the sender to the receiver on both Macs.

use std::collections::HashSet;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rune::tok::TextStyle;
use rune::{Align, App, AppSpec, Color, Cursor, Cx, El, LineCap, Path, ScrollSpec, Shape, Side, TextEdit, Tx, Waker, WindowOptions, col, el, row, spacer, text};
use rune_kit::{Kit, PaletteSeed, TextInput};

use crate::bus::{self, Answer, Ask, Bus, Device, Request, Session};
use crate::look::{self, H, W, full};
use crate::me::{self, Config, Hardware, Look};
use crate::scan::{self, Node, Offer};
use crate::session::{self, Live, Picks, ReceiverDoc};
use crate::{setup, tail};
use crate::wire::Endpoint;

// Metallic blue and metallic grey on a graphite ground. Rune colours are linear light: these are the sRGB hex
// values in the comments, decoded.
const BG: Color = Color { r: 0.00304, g: 0.00439, b: 0.00700, a: 0.97 }; // #0a0e14
const PANEL: Color = Color { r: 0.00561, g: 0.00913, b: 0.01681, a: 0.84 }; // #111823
const PANEL2: Color = Color { r: 0.00913, g: 0.01600, b: 0.03071, a: 1.0 }; // #182231
const FIELD: Color = Color { r: 0.00243, g: 0.00368, b: 0.00605, a: 1.0 }; // #080c12
const EDGE: Color = Color { r: 0.01938, g: 0.03190, b: 0.05613, a: 1.0 }; // #263243
const STEEL: Color = Color { r: 0.20508, g: 0.25415, b: 0.33245, a: 1.0 }; // #7d8a9c
const SILVER: Color = Color { r: 0.54572, g: 0.60383, b: 0.68669, a: 1.0 }; // #c3ccd8
const BLUE: Color = Color { r: 0.01850, g: 0.12477, b: 0.58408, a: 1.0 }; // #2563c9
const BLUE2: Color = Color { r: 0.07227, g: 0.33716, b: 1.00000, a: 1.0 }; // #4c9dff
const ICE: Color = Color { r: 0.39157, g: 0.65837, b: 1.00000, a: 1.0 }; // #a8d4ff
const INK: Color = Color { r: 0.77582, g: 0.83077, b: 0.89627, a: 1.0 }; // #e4ebf3
const SOFT: Color = Color { r: 0.36625, g: 0.43966, b: 0.53948, a: 1.0 }; // #a3b1c2
const MUTED: Color = Color { r: 0.10224, g: 0.13843, b: 0.19120, a: 1.0 }; // #5a6879
const DIM: Color = Color { r: 0.02416, g: 0.03820, b: 0.06125, a: 1.0 }; // #2b3746
const OK: Color = Color { r: 0.02843, g: 0.52100, b: 0.16513, a: 1.0 }; // #2fbf71
const ERR: Color = Color { r: 1.00000, g: 0.08866, b: 0.13843, a: 1.0 }; // #ff5468

/// Where everything sits: assets/layout.json, which tools/layout-editor.html edits. MACYSYNC_LAYOUT points at
/// another copy (the editor's preview renders with it).
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug)]
pub struct Layout {
    pub outline: Vec<[f32; 2]>,
    pub elements: Elements,
    pub conduit: [[f32; 2]; 4],
    pub flow: FlowGuide,
}

#[derive(serde::Deserialize, serde::Serialize, Clone, Copy, Debug)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(serde::Deserialize, serde::Serialize, Clone, Debug)]
pub struct Elements {
    pub title: Rect,
    /// A laptop's box is its 16-inch size: the lid is 0.655 of the width tall, the hinge sits there.
    pub laptop_left: Rect,
    pub laptop_right: Rect,
    pub left_panel: Rect,
    pub right_panel: Rect,
    pub controls: Rect,
    pub dialog: Rect,
    /// The Send and Receive buttons between the two Macs; a layout without them centres them on the conduit.
    #[serde(default)]
    pub actions: Option<Rect>,
}

impl Elements {
    pub fn actions(&self) -> Rect {
        self.actions.unwrap_or_else(|| {
            let [a, c1, c2, b] = layout().conduit;
            let m = bezier((a, c1, c2, b), 0.5);
            Rect { x: m[0] - 60.0, y: m[1] - 75.0, w: 120.0, h: 150.0 }
        })
    }
}

#[derive(serde::Deserialize, serde::Serialize, Clone, Copy, Debug)]
pub struct Band {
    pub y: f32,
    pub spread: f32,
}

/// The background lines: where they fan out over each lobe and squeeze through the bridge, and the x ranges
/// where they bend from one band to the next.
#[derive(serde::Deserialize, serde::Serialize, Clone, Copy, Debug)]
pub struct FlowGuide {
    pub left: Band,
    pub bridge: Band,
    pub right: Band,
    pub into_bridge: [f32; 2],
    pub into_right: [f32; 2],
}

static LAYOUT: std::sync::RwLock<Option<&'static Layout>> = std::sync::RwLock::new(None);

/// The layout file the app follows: MACYSYNC_LAYOUT, else this checkout's assets/layout.json when the app was
/// built from it on this Mac. Elsewhere there is none and the copy built in is used.
fn layout_file() -> Option<std::path::PathBuf> {
    std::env::var_os("MACYSYNC_LAYOUT").map(Into::into)
        .or_else(|| Some(std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/layout.json"))).filter(|p| p.exists()))
}

fn read_layout(path: &std::path::Path) -> Result<Layout, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

pub fn layout() -> &'static Layout {
    if let Some(l) = *LAYOUT.read().unwrap() {
        return l;
    }
    *LAYOUT.write().unwrap().get_or_insert_with(|| {
        let l = match layout_file() {
            Some(p) => read_layout(&p),
            None => serde_json::from_str(include_str!("../assets/layout.json")).map_err(|e| e.to_string()),
        };
        Box::leak(Box::new(l.unwrap_or_else(|e| panic!("layout.json: {e}"))))
    })
}

/// Takes `path` again when its time changed since `seen`: a save in the layout editor shows in the running app.
/// A file caught half written is skipped; the next change brings it in.
fn reload_layout(path: &std::path::Path, seen: &mut Option<std::time::SystemTime>) -> bool {
    let Ok(t) = std::fs::metadata(path).and_then(|m| m.modified()) else { return false };
    if seen.replace(t).is_none_or(|old| old == t) {
        return false;
    }
    match read_layout(path) {
        // ponytail: each reload leaks the old layout (about 2 KB), fine for an editing session.
        Ok(l) => {
            *LAYOUT.write().unwrap() = Some(Box::leak(Box::new(l)));
            true
        }
        Err(_) => false,
    }
}

const LID: f32 = 0.655;

/// (hinge centre x, hinge y, 16-inch width) of a laptop box.
fn hinge(r: Rect) -> (f32, f32, f32) {
    (r.x + r.w / 2.0, r.y + r.w * LID, r.w)
}
const POLL_S: f32 = 5.0;
/// How long a picked Mac takes to grow from the grid into its place.
const PICK_S: f32 = 0.55;

pub fn is_tooling_arg(a: &str) -> bool {
    ["--snap", "--script", "--tree", "--list-scenes", "--scene", "--size", "--video", "--desktop", "--native", "--trace"].contains(&a)
}

enum Job {
    Connected(Result<(Arc<Bus>, String), String>),
    Registered(Result<(), String>),
    Polled(Result<Snapshot, String>),
    Asked(Result<Request, String>),
    Answered(Result<Request, String>),
    Scanned(Node),
    /// The offer or the picks went out.
    Published(Result<(), String>),
    Moved(Result<(), String>),
    Reported,
    /// MacySync's own Tailscale node: None while it isn't running.
    Tail(Option<tail::Status>),
}

#[derive(Default)]
struct Snapshot {
    devices: Vec<Device>,
    requests: Vec<(Request, String)>,
    session: Option<Session>,
    /// The transfer the sealed documents below belong to.
    transfer: String,
    offer: Option<Offer>,
    picks: Option<Picks>,
    key: Option<Endpoint>,
}

/// 03 PICK and 04 MOVE, for the accepted transfer.
#[derive(Default)]
struct Pick {
    transfer: String,
    /// Set once the first look at the bus is in, so an old accepted transfer doesn't switch tabs on launch.
    seen: bool,
    /// What's ticked here: this Mac's scan when it sends, the offer when it receives.
    tree: Option<Node>,
    ticks: HashSet<String>,
    open: HashSet<String>,
    scanning: bool,
    /// The offer (sender) or the picks (receiver) went out.
    sent: bool,
    offer: Option<Offer>,
    picks: Option<Picks>,
    live: Option<session::Shared>,
    reported: f32,
    reported_state: String,
}

pub struct S {
    cfg: Config,
    hw: Hardware,
    tr: bool,
    now: f32,
    tab: u8,
    name: TextEdit,
    bus_edit: TextEdit,
    model_open: bool,
    agent_open: bool,
    bus: Option<Arc<Bus>>,
    login: String,
    devices: Vec<Device>,
    requests: Vec<(Request, String)>,
    peer: String,
    /// The request this Mac is answering in the dialog.
    incoming: Option<(Request, String)>,
    session: Option<Session>,
    polling: bool,
    polled_at: f32,
    busy: String,
    err: String,
    jobs: (Sender<Job>, Receiver<Job>),
    waker: Option<Waker>,
    demo: bool,
    flow: Flow,
    flowing: bool,
    /// The right side lists the account's Macs to pick from.
    finding: bool,
    found_at: f32,
    /// Whether the bus has been read once since it connected.
    polled: bool,
    hovered: String,
    /// How lit each listed Mac's screen is, 0 (off) to 1.
    glow: std::collections::HashMap<String, f32>,
    /// The grid box a picked Mac grows from, and when.
    picked: Option<(Rect, f32)>,
    /// 1 while the pointer is on Send, -1 on Receive: the lines show which way it would go.
    hover_dir: f32,
    /// When the layout file was last read, and when it was last looked at.
    layout_seen: Option<std::time::SystemTime>,
    layout_checked: f32,
    pick: Pick,
    tail: Option<tail::Status>,
    tail_at: f32,
}

/// Where the dashes are along their curves, as in WavySync: they speed up towards `dir` (1 runs left to
/// right) while work runs, and glide to a quarter-cycle stop after.
#[derive(Default)]
struct Flow {
    pos: f32,
    v: f32,
    dir: f32,
    stop: Option<(f32, f32)>,
}

const FLOW_V: f32 = 0.3;
const DASHES: usize = 4;

impl Flow {
    fn env(&self) -> f32 {
        (self.v.abs() / FLOW_V).clamp(0.0, 1.0)
    }

    fn step(&mut self, on: bool, dt: f32) {
        if on {
            self.stop = None;
            let a = FLOW_V / 0.8 * dt;
            self.v += (self.dir * FLOW_V - self.v).clamp(-a, a);
        } else if self.v != 0.0 {
            let sign = self.v.signum();
            let (target, decel) = *self.stop.get_or_insert_with(|| {
                let q = 1.0 / DASHES as f32;
                let t = sign * ((sign * (self.pos + self.v * 0.5)) / q).ceil() * q;
                (t, self.v * self.v / (2.0 * (t - self.pos).abs().max(1e-4)))
            });
            self.v = sign * (self.v.abs() - decel * dt).max(0.0);
            if self.v == 0.0 || sign * (self.pos + self.v * dt - target) >= 0.0 {
                *self = Flow { dir: self.dir, ..Flow::default() };
                return;
            }
        }
        self.pos += self.v * dt;
    }
}

fn init() -> S {
    let cfg = Config::load();
    S {
        tr: cfg.lang == "tr",
        hw: Hardware::default(),
        now: 0.0,
        tab: if cfg.registered { 2 } else { 1 },
        name: TextEdit::new(cfg.name.clone()),
        bus_edit: TextEdit::new(cfg.bus.clone()),
        model_open: false,
        agent_open: false,
        bus: None,
        login: String::new(),
        devices: Vec::new(),
        requests: Vec::new(),
        peer: String::new(),
        incoming: None,
        session: None,
        polling: false,
        polled_at: -100.0,
        busy: String::new(),
        err: String::new(),
        jobs: channel(),
        waker: None,
        demo: false,
        flow: Flow::default(),
        flowing: false,
        finding: false,
        found_at: 0.0,
        polled: false,
        hovered: String::new(),
        glow: Default::default(),
        picked: None,
        hover_dir: 0.0,
        layout_seen: None,
        layout_checked: 0.0,
        pick: Pick::default(),
        tail: None,
        tail_at: -100.0,
        cfg,
    }
}

impl S {
    fn t(&self, en: &str, tr: &str) -> String {
        if self.tr { tr.into() } else { en.into() }
    }

    fn spawn(&mut self, busy: String, f: impl FnOnce() -> Job + Send + 'static) {
        if self.demo {
            return;
        }
        if !busy.is_empty() {
            self.busy = busy;
        }
        let tx = self.jobs.0.clone();
        let w = self.waker.clone();
        std::thread::spawn(move || {
            let _ = tx.send(f());
            if let Some(w) = w {
                w();
            }
        });
    }

    fn me(&self) -> Device {
        let (_, public) = me::keys();
        Device {
            id: self.cfg.id.clone(),
            name: self.cfg.name.clone(),
            look: self.cfg.look.clone(),
            key: bus::b64(&public),
            chip: self.hw.chip.clone(),
            ram_gb: self.hw.ram_gb,
            disk_gb: self.hw.disk_gb,
            user: std::env::var("USER").unwrap_or_default(),
            seen: bus::now(),
            version: env!("CARGO_PKG_VERSION").into(),
        }
    }

    fn connect(&mut self) {
        let want = self.bus_edit.text.trim().to_string();
        self.spawn(self.t("Signing in to GitHub…", "GitHub'a bağlanılıyor…"), move || {
            Job::Connected((|| {
                let token = me::token().ok_or("Not signed in: run `gh auth login`, or set MACYSYNC_TOKEN.")?;
                let login = bus::github_login(&token)?;
                let repo = if want.is_empty() { format!("{login}/macysync-bus") } else { want };
                bus::ensure_repo(&token, &repo)?;
                Ok((Arc::new(Bus::new(&repo, &token)), login))
            })())
        });
    }

    fn register(&mut self) {
        let (Some(b), d) = (self.bus.clone(), self.me()) else { return };
        let path = format!("devices/{}.json", d.id);
        self.spawn(self.t("Saving this Mac to the bus…", "Bu Mac bus'a kaydediliyor…"), move || {
            Job::Registered(b.put_json(&path, &d, None, &format!("device {} ({})", d.name, d.id)).map(|_| ()))
        });
    }

    fn set_look(&mut self, look: Look) {
        self.cfg.look = look;
        self.cfg.save();
        if self.cfg.registered {
            self.register();
        }
    }

    fn poll_bus(&mut self) {
        let Some(b) = self.bus.clone() else { return };
        self.polling = true;
        let me = self.cfg.id.clone();
        let transfer = self.active_request().map(|r| r.transfer.clone()).filter(|t| !t.is_empty());
        let (p, here) = (&self.pick, self.tab >= 3 && transfer.as_ref() == Some(&self.pick.transfer));
        let want_offer = here && self.sending() == Some(false) && p.offer.is_none();
        let want_picks = here && self.sending() == Some(true) && p.sent && p.picks.is_none();
        let want_key = here && self.sending() == Some(false) && p.sent && p.live.is_none();
        self.spawn(String::new(), move || {
            Job::Polled((|| {
                let mut snap = Snapshot::default();
                for n in b.list("devices")?.iter().filter(|n| n.ends_with(".json")) {
                    if let Some((d, _)) = b.get_json::<Device>(&format!("devices/{n}"))? {
                        snap.devices.push(d);
                    }
                }
                for n in b.list("requests")?.iter().filter(|n| n.ends_with(".json") && n.contains(&me)) {
                    if let Some((r, sha)) = b.get_json::<Request>(&format!("requests/{n}"))? {
                        snap.requests.push((r, sha));
                    }
                }
                if let Some(t) = transfer {
                    snap.session = b.get_json::<Session>(&format!("{t}/session.json"))?.map(|s| s.0);
                    let secret = me::keys().0;
                    if want_offer {
                        snap.offer = session::get_sealed(&b, &format!("{t}/offer.sealed"), &secret)?;
                    }
                    if want_picks {
                        snap.picks = session::get_sealed(&b, &format!("{t}/picks.sealed"), &secret)?;
                    }
                    if want_key {
                        snap.key = session::get_sealed(&b, &format!("{t}/key.sealed"), &secret)?;
                    }
                    snap.transfer = t;
                }
                Ok(snap)
            })())
        });
    }

    /// The newest request between this Mac and another that is pending or accepted.
    fn active_request(&self) -> Option<&Request> {
        self.requests.iter().map(|(r, _)| r).filter(|r| matches!(r.answer, Answer::Pending | Answer::Accepted)).max_by_key(|r| r.at)
    }

    /// Lists the Macs on the account on the right, signing in first if needed.
    fn find(&mut self) {
        self.finding = true;
        self.found_at = self.now;
        self.picked = None;
        self.peer.clear();
        if self.bus.is_none() {
            self.connect();
        } else {
            self.polled_at = -100.0;
        }
    }

    /// The Mac clicked in the grid grows from `from` into the right-hand place.
    fn pick(&mut self, id: String, from: Rect) {
        self.peer = id;
        self.picked = Some((from, self.now));
        self.hovered.clear();
    }

    /// Asks the other Mac for a session in which this one sends (`Ask::Send`) or receives.
    fn ask(&mut self, ask: Ask) {
        let (Some(b), peer) = (self.bus.clone(), self.peer.clone()) else { return };
        if peer.is_empty() {
            return;
        }
        self.hover_dir = 0.0;
        let r = Request {
            ask,
            from: self.cfg.id.clone(),
            to: peer,
            at: bus::now(),
            answer: Answer::Pending,
            transfer: String::new(),
        };
        self.spawn(self.t("Sending the request…", "İstek gönderiliyor…"), move || {
            Job::Asked(b.put_json(&r.path(), &r, None, &format!("{:?} request {} to {}", r.ask, r.from, r.to)).map(|_| r))
        });
    }

    fn answer(&mut self, yes: bool) {
        let (Some(b), Some((r, sha))) = (self.bus.clone(), self.incoming.take()) else { return };
        let mut names: std::collections::HashMap<String, String> = self.devices.iter().map(|d| (d.id.clone(), d.name.clone())).collect();
        names.insert(self.cfg.id.clone(), self.cfg.name.clone());
        self.spawn(self.t("Answering…", "Yanıtlanıyor…"), move || Job::Answered(session::approve(&b, r, &sha, &names, yes)));
    }

    /// The accepted request whose transfer folder exists.
    fn transfer(&self) -> Option<&Request> {
        self.active_request().filter(|r| r.answer == Answer::Accepted && !r.transfer.is_empty())
    }

    /// Sends the offer (sender) or the picks (receiver) to the other Mac, sealed to its key.
    fn publish(&mut self) {
        let (Some(b), Some(r), Some(key), Some(tree)) = (self.bus.clone(), self.transfer().cloned(), self.peer_device().and_then(|d| d.key_bytes()), self.pick.tree.clone()) else { return };
        let t = r.transfer;
        self.pick.sent = true;
        if self.sending() == Some(true) {
            let offer = session::offer(&me::home_dir(), &tree, &self.pick.ticks);
            self.pick.offer = Some(offer.clone());
            self.spawn(self.t("Sending the offer…", "Teklif gönderiliyor…"), move || Job::Published((|| {
                session::put_sealed(&b, &format!("{t}/offer.sealed"), &key, &offer, "offer")?;
                session::update_session(&b, &t, |s| s.state = "picking".into())
            })()));
        } else {
            let picks = Picks { ids: tree.leaves().iter().filter(|l| self.pick.ticks.contains(&l.id)).map(|l| l.id.clone()).collect() };
            let n = picks.ids.len() as u64;
            self.pick.picks = Some(picks.clone());
            self.spawn(self.t("Sending your picks…", "Seçimlerin gönderiliyor…"), move || Job::Published((|| {
                session::put_sealed(&b, &format!("{t}/picks.sealed"), &key, &picks, "picks")?;
                session::write_receiver(&b, &t, &ReceiverDoc { state: "waiting".into(), items: n, ..ReceiverDoc::default() })
            })()));
        }
    }

    /// Starts sending the picks (sender) or fetching them from `ep` (receiver) on a thread of its own.
    fn start_moving(&mut self, ep: Option<Endpoint>) {
        let (Some(b), Some(offer), Some(picks), Some(r), Some(other)) = (self.bus.clone(), self.pick.offer.clone(), self.pick.picks.clone(), self.transfer().cloned(), self.peer_device().cloned()) else { return };
        let live: session::Shared = Default::default();
        self.pick.live = Some(live.clone());
        self.tab = 4;
        let home = me::home_dir();
        self.spawn(String::new(), move || Job::Moved(match ep {
            None => session::serve(&b, &r.transfer, &other, &offer, &picks, &home, live),
            Some(ep) => session::receive(&ep, &offer, &picks, &session::local_dir(&home, &r.transfer), &other.name, &home, live),
        }));
    }

    fn land(&mut self, job: Job) {
        self.busy.clear();
        match job {
            Job::Connected(Ok((b, login))) => {
                self.err.clear();
                self.cfg.bus = b.repo.clone();
                self.bus_edit = TextEdit::new(b.repo.clone());
                self.login = login;
                self.bus = Some(b);
                self.cfg.save();
                self.register();
            }
            Job::Registered(Ok(())) => {
                self.err.clear();
                if !self.cfg.registered {
                    self.cfg.registered = true;
                    self.cfg.save();
                    self.tab = 2;
                }
                self.polled_at = -100.0;
            }
            Job::Polled(r) => {
                self.polling = false;
                match r {
                    Ok(snap) => {
                        self.polled = true;
                        self.devices = snap.devices.into_iter().filter(|d| d.id != self.cfg.id).collect();
                        self.devices.sort_by(|a, b| b.seen.cmp(&a.seen));
                        self.requests = snap.requests;
                        self.session = snap.session;
                        if self.incoming.is_none() {
                            self.incoming = self.requests.iter().find(|(r, _)| r.to == self.cfg.id && r.answer == Answer::Pending && bus::now().saturating_sub(r.at) < 86400).cloned();
                        }
                        if let Some(r) = self.active_request().cloned() {
                            let other = if r.from == self.cfg.id { &r.to } else { &r.from };
                            self.peer = other.clone();
                            self.finding = false;
                        }
                        if snap.transfer == self.pick.transfer {
                            if let Some(o) = snap.offer.filter(|_| self.pick.offer.is_none()) {
                                self.pick.ticks = session::default_ticks(&o.tree);
                                self.pick.tree = Some(o.tree.clone());
                                self.pick.offer = Some(o);
                            }
                            if let Some(p) = snap.picks.filter(|_| self.pick.picks.is_none()) {
                                self.pick.picks = Some(p);
                                self.start_moving(None);
                            }
                            if let Some(ep) = snap.key.filter(|_| self.pick.live.is_none()) {
                                self.start_moving(Some(ep));
                            }
                        }
                    }
                    Err(e) => self.err = e,
                }
            }
            Job::Asked(Ok(r)) | Job::Answered(Ok(r)) => {
                self.err.clear();
                if let Some(i) = self.requests.iter().position(|(x, _)| x.path() == r.path()) {
                    self.requests.remove(i);
                }
                self.requests.push((r, String::new()));
                self.polled_at = -100.0;
            }
            Job::Scanned(n) => {
                self.pick.scanning = false;
                self.pick.ticks = session::default_ticks(&n);
                self.pick.tree = Some(n);
            }
            Job::Published(r) => {
                self.polled_at = -100.0;
                if let Err(e) = r {
                    self.pick.sent = false;
                    if self.sending() == Some(false) {
                        self.pick.picks = None;
                    }
                    self.err = e;
                }
            }
            Job::Moved(r) => {
                self.polled_at = -100.0;
                if let Err(e) = r {
                    self.err = e;
                }
            }
            Job::Reported => {}
            Job::Tail(t) => self.tail = t,
            Job::Connected(Err(e)) | Job::Registered(Err(e)) | Job::Asked(Err(e)) | Job::Answered(Err(e)) => self.err = e,
        }
    }

    fn peer_device(&self) -> Option<&Device> {
        self.devices.iter().find(|d| d.id == self.peer)
    }

    /// Whether this Mac sends in the current session; None before one is asked for. Each session picks its
    /// own direction, so any Mac can send to or take from any other.
    fn sending(&self) -> Option<bool> {
        self.active_request().map(|r| r.sides().0 == self.cfg.id)
    }
}

fn setup(s: &mut S, waker: Waker) {
    s.waker = Some(waker);
    if s.demo {
        return;
    }
    s.hw = me::detect();
    if s.cfg.registered || !s.cfg.bus.is_empty() {
        s.connect();
    }
}

fn poll(s: &mut S, now: f64, tx: &mut Tx) -> bool {
    let now = now as f32;
    let dt = (now - s.now).clamp(0.0, 0.1);
    s.now = now;
    while let Ok(job) = s.jobs.1.try_recv() {
        s.land(job);
    }
    if now - s.layout_checked > 0.4 {
        s.layout_checked = now;
        if let Some(p) = layout_file() {
            reload_layout(&p, &mut s.layout_seen);
        }
    }
    if s.polled {
        let t = s.transfer().map(|r| r.transfer.clone()).unwrap_or_default();
        if t != s.pick.transfer || !s.pick.seen {
            if !t.is_empty() && s.pick.seen {
                s.tab = 3;
            }
            s.pick = Pick { transfer: t, seen: true, ..Pick::default() };
        }
    }
    if !s.demo && s.tab == 3 && !s.pick.transfer.is_empty() && s.sending() == Some(true) && s.pick.tree.is_none() && !s.pick.scanning {
        s.pick.scanning = true;
        s.spawn(s.t("Scanning this Mac…", "Bu Mac taranıyor…"), || {
            let home = me::home_dir();
            Job::Scanned(scan::scan(&home, &["/Applications".into(), home.join("Applications")], true))
        });
    }
    if let (Some(l), Some(b)) = (s.pick.live.as_ref().map(|l| l.lock().unwrap().clone()), s.bus.clone()) {
        let finished = l.done || l.failed;
        if !l.state.is_empty() && (l.state != s.pick.reported_state || (!finished && now - s.pick.reported > 10.0)) {
            s.pick.reported = now;
            s.pick.reported_state = l.state.clone();
            let (t, sender) = (s.pick.transfer.clone(), s.sending() == Some(true));
            s.spawn(String::new(), move || {
                let _ = session::report(&b, &t, sender, &l);
                Job::Reported
            });
        }
    }
    if !s.demo && s.tab == 2 && now - s.tail_at > POLL_S {
        s.tail_at = now;
        s.spawn(String::new(), || Job::Tail(tail::status()));
    }
    if !s.demo && s.bus.is_some() && !s.polling && now - s.polled_at > POLL_S {
        s.polled_at = now;
        s.poll_bus();
    }
    let moving = s.pick.live.as_ref().is_some_and(|l| matches!(l.lock().unwrap().state.as_str(), "sending" | "receiving"));
    let live = moving || s.session.as_ref().is_some_and(|x| x.state == "sending");
    s.flow.dir = if s.hover_dir != 0.0 { s.hover_dir } else if s.sending() == Some(false) { -1.0 } else { 1.0 };
    s.flow.step(s.flowing || live || !s.busy.is_empty() || s.hover_dir != 0.0, dt);
    let mut lighting = false;
    for d in &s.devices {
        let target = if s.finding && s.hovered == d.id { 1.0 } else { 0.0 };
        let g = s.glow.entry(d.id.clone()).or_insert(0.0);
        let step = dt / 0.18;
        *g += (target - *g).clamp(-step, step);
        lighting |= *g != target;
    }
    if s.picked.is_some_and(|(_, t0)| now - t0 >= PICK_S) {
        s.picked = None;
        s.finding = false;
    }
    let moving = s.flow.v != 0.0 || lighting || s.picked.is_some() || (s.finding && now - s.found_at < 1.2);
    tx.wake_after(Duration::from_millis(if moving { 0 } else { 250 }));
    true
}

// Drawing ----------------------------------------------------------------------------------------------

fn style(size: f32, weight: f32) -> TextStyle {
    TextStyle::new(size, weight)
}
fn label(s: impl Into<String>, size: f32, weight: f32, c: Color) -> El {
    text(s.into()).font(style(size, weight)).color(c)
}
fn mono(s: impl Into<String>, size: f32, c: Color) -> El {
    text(s.into()).font(style(size, 500.0).mono()).color(c)
}
fn caps(s: impl Into<String>, c: Color) -> El {
    text(s.into()).font(style(11.0, 700.0).mono().tracking(1.4)).color(c)
}

/// The outline through the layout's points, smoothed (Catmull-Rom; the editor draws it the same way): a tall
/// lobe on the left for this Mac, a bridge, and a lower lobe on the right for the other Mac.
fn segments() -> Vec<([f32; 2], [f32; 2], [f32; 2], [f32; 2])> {
    let o = &layout().outline;
    let n = o.len();
    (0..n)
        .map(|i| {
            let (p0, p1, p2, p3) = (o[(i + n - 1) % n], o[i], o[(i + 1) % n], o[(i + 2) % n]);
            let c1 = [p1[0] + (p2[0] - p0[0]) / 6.0, p1[1] + (p2[1] - p0[1]) / 6.0];
            let c2 = [p2[0] - (p3[0] - p1[0]) / 6.0, p2[1] - (p3[1] - p1[1]) / 6.0];
            (p1, c1, c2, p2)
        })
        .collect()
}

fn body_path() -> Path {
    segments().iter().fold(Path::new().move_to(layout().outline[0]), |p, (_, a, b, c)| p.cubic_to(*a, *b, *c)).close()
}

fn bezier((p0, a, b, c): ([f32; 2], [f32; 2], [f32; 2], [f32; 2]), t: f32) -> [f32; 2] {
    let u = 1.0 - t;
    let k = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
    [k[0] * p0[0] + k[1] * a[0] + k[2] * b[0] + k[3] * c[0], k[0] * p0[1] + k[1] * a[1] + k[2] * b[1] + k[3] * c[1]]
}

/// Distance from `p` to the outline, positive inside.
fn depth(poly: &[[f32; 2]], p: [f32; 2]) -> f32 {
    let (mut inside, mut d) = (false, f32::MAX);
    for (i, a) in poly.iter().enumerate() {
        let b = poly[(i + 1) % poly.len()];
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]) {
            inside = !inside;
        }
        let (vx, vy) = (b[0] - a[0], b[1] - a[1]);
        let t = (((p[0] - a[0]) * vx + (p[1] - a[1]) * vy) / (vx * vx + vy * vy).max(1e-6)).clamp(0.0, 1.0);
        d = d.min((a[0] + vx * t - p[0]).hypot(a[1] + vy * t - p[1]));
    }
    if inside { d } else { -d }
}

const LINES: usize = 15;
const SAMPLES: usize = 200;

/// A sampled curve from one lobe to the other, and how deep inside the body each sample is (Rune has no
/// clipping, so the lines are cut and faded by this).
struct FlowLine {
    pts: Vec<[f32; 2]>,
    depth: Vec<f32>,
    inside: Vec<std::ops::Range<usize>>,
    offset: f32,
    len: f32,
    speed: f32,
}

fn hash(n: u32) -> f32 {
    let mut x = n.wrapping_mul(0x9E37_79B9) ^ 0x85EB_CA6B;
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    (x & 0xFFFF) as f32 / 65535.0
}

/// The lines for the current layout, made again when the layout changes.
fn flow_lines() -> &'static [FlowLine] {
    static LINES: std::sync::Mutex<Option<(usize, &'static [FlowLine])>> = std::sync::Mutex::new(None);
    let key = layout() as *const Layout as usize;
    let mut cache = LINES.lock().unwrap();
    if let Some((k, lines)) = *cache
        && k == key
    {
        return lines;
    }
    let lines: &'static [FlowLine] = Box::leak(make_flow_lines().into_boxed_slice());
    *cache = Some((key, lines));
    lines
}

fn make_flow_lines() -> Vec<FlowLine> {
    let poly: Vec<[f32; 2]> = segments().into_iter().flat_map(|s| (1..=16).map(move |i| bezier(s, i as f32 / 16.0))).collect();
    (0..LINES)
        .map(|i| {
            let o = -1.0 + 2.0 * i as f32 / (LINES - 1) as f32;
            // They fan out over each lobe and squeeze through the bridge.
            let f = layout().flow;
            let (yl, yb, yr) = (f.left.y + o * f.left.spread, f.bridge.y + o * f.bridge.spread, f.right.y + o * f.right.spread);
            let wobble = 4.0 + 6.0 * hash(i as u32 * 7 + 1);
            let shift = hash(i as u32 * 13 + 5) * std::f32::consts::TAU;
            let pts: Vec<[f32; 2]> = (0..SAMPLES)
                .map(|j| {
                    let x = -20.0 + 1320.0 * j as f32 / (SAMPLES - 1) as f32;
                    let sm = |a: f32, b: f32| {
                        let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
                        t * t * (3.0 - 2.0 * t)
                    };
                    let y = yl + (yb - yl) * sm(f.into_bridge[0], f.into_bridge[1]) + (yr - yb) * sm(f.into_right[0], f.into_right[1]) + wobble * (x / 160.0 + shift).sin();
                    [x, y]
                })
                .collect();
            let depth: Vec<f32> = pts.iter().map(|p| depth(&poly, *p)).collect();
            let mut inside = Vec::new();
            let mut from = None;
            for (j, d) in depth.iter().chain([&-1.0]).enumerate() {
                match (from, *d > 6.0) {
                    (None, true) => from = Some(j),
                    (Some(a), false) => {
                        if j - a > 1 {
                            inside.push(a..j);
                        }
                        from = None;
                    }
                    _ => {}
                }
            }
            let (offset, len) = (hash(i as u32 * 61 + 9), 0.03 + 0.024 * hash(i as u32 * 17 + 3));
            FlowLine { pts, depth, inside, offset, len, speed: if i % 3 == 0 { 2.0 } else { 1.0 } }
        })
        .collect()
}

fn backdrop(s: &S) -> Vec<El> {
    let env = s.flow.env();
    let body = body_path();
    let mut out = vec![full(Shape::new().fill_path(&body, BG)).window_drag().key("body")];
    let (mut faint, mut glow, mut core) = (Shape::new(), Shape::new(), Shape::new());
    for (i, l) in flow_lines().iter().enumerate() {
        let tint = if i % 4 == 1 { SILVER } else { BLUE2 };
        for r in &l.inside {
            faint = faint.stroke(l.pts[r.clone()].to_vec(), 1.0, Color { a: 0.07 + 0.06 * env, ..tint });
        }
        for j in 0..DASHES {
            let u = (l.offset + j as f32 / DASHES as f32 + l.speed * s.flow.pos).rem_euclid(1.0);
            let last = (SAMPLES - 1) as f32;
            let (a, b) = ((u * last) as usize, (((u + l.len) * last) as usize).min(SAMPLES - 1));
            if b <= a + 1 {
                continue;
            }
            let fade = ((l.depth[a..=b].iter().copied().fold(f32::MAX, f32::min) - 2.0) / 28.0).clamp(0.0, 1.0);
            if fade == 0.0 {
                continue;
            }
            let pts = l.pts[a..=b].to_vec();
            glow = glow.stroke(pts.clone(), 7.0, Color { a: (0.16 + 0.5 * env) * fade, ..tint });
            core = core.stroke(pts, 1.6, Color { a: (0.36 + 0.55 * env) * fade, ..tint });
        }
    }
    out.push(full(faint).key("faint"));
    out.push(full(glow).blur(6.0).key("glow"));
    out.push(full(core).blur(0.6).key("core"));
    out.push(full(Shape::new().stroke_path(&body, 2.0, LineCap::Round, EDGE)).key("edge"));
    out.push(full(Shape::new().stroke_path(&body, 3.0, LineCap::Round, Color { a: 0.26 * env, ..BLUE2 })).blur(9.0).key("outline-glow"));
    out.push(full(Shape::new().stroke_path(&body, 1.0, LineCap::Round, Color { a: 0.35 + 0.3 * env, ..STEEL })).key("outline-line"));
    out
}

/// The conduit between the laptops: two steel rails, and chevrons that run from the sender to the receiver.
fn conduit(s: &S) -> Vec<El> {
    let env = s.flow.env();
    let [a, c1, c2, b] = layout().conduit;
    let rail = |dy: f32| Path::new().move_to([a[0], a[1] + dy]).cubic_to([c1[0], c1[1] + dy], [c2[0], c2[1] + dy], [b[0], b[1] + dy]);
    let mut shape = Shape::new();
    for dy in [-16.0, 16.0] {
        shape = shape.stroke_path(&rail(dy), 2.0, LineCap::Round, Color { a: 0.85, ..STEEL });
    }
    let mut out = vec![
        full(Shape::new().stroke_path(&rail(0.0), 30.0, LineCap::Round, Color { a: 0.55, ..FIELD })).key("conduit-bed"),
        full(shape).key("conduit-rails"),
    ];
    let n = 7;
    let mut chev = Shape::new();
    for k in 0..n {
        let u = ((k as f32 + s.flow.pos * 3.0 * s.flow.dir.signum() * s.flow.dir) / n as f32).rem_euclid(1.0);
        let seg = (a, c1, c2, b);
        let p = bezier(seg, u);
        let q = bezier(seg, (u + 0.01).min(1.0));
        let d = if s.flow.dir >= 0.0 { 1.0 } else { -1.0 };
        let ang = (q[1] - p[1]).atan2(q[0] - p[0]);
        let (c, sn) = (ang.cos() * d, ang.sin() * d);
        let pt = |x: f32, y: f32| [p[0] + x * c - y * sn, p[1] + x * sn + y * c];
        let alpha = (0.25 + 0.75 * env) * (std::f32::consts::PI * u).sin();
        chev = chev.stroke(vec![pt(-5.0, -8.0), pt(4.0, 0.0), pt(-5.0, 8.0)], 2.4, Color { a: alpha, ..if k % 2 == 0 { BLUE2 } else { ICE } });
    }
    out.push(full(chev.clone()).blur(4.0).key("chev-glow"));
    out.push(full(chev).key("chev"));
    out
}

fn device_on_screen(s: &S, key: &str, d: Option<&Device>, look: &Look, place: Rect, role: String, this: bool) -> Vec<El> {
    let Some((m, c)) = look::resolve(look) else { return vec![] };
    let (cx, hy, width) = hinge(place);
    let drawn = look::laptop(key, m, c, cx, hy, width);
    let [x, y, w, h] = drawn.screen;
    let mut out = drawn.layers;
    let name = d.map(|d| d.name.clone()).unwrap_or_else(|| s.t("No Mac picked", "Mac seçilmedi"));
    let id = d.map(|d| d.id.clone()).unwrap_or_default();
    let spec = d.map(|d| if d.ram_gb > 0 { format!("{} · {} GB · {} GB", d.chip.trim_start_matches("Apple "), d.ram_gb, d.disk_gb) } else { String::new() }).unwrap_or_default();
    let tint = if this { BLUE2 } else { SILVER };
    let mut screen = col().absolute(x, y).size(w, h).padding(14.0).gap(4.0)
        .child(row().w_full().center_y().child(caps(role, tint)).child(spacer()).child(mono(if this { s.t("THIS MAC", "BU MAC") } else { String::new() }, 9.5, MUTED)))
        .child(spacer())
        .child(label(name, 19.0, 700.0, if d.is_some() { INK } else { MUTED }))
        .child(mono(look::short_name(m), 10.5, SOFT))
        .child(mono(format!("{} · {spec}", c.name), 10.0, MUTED));
    if !id.is_empty() {
        screen = screen.child(el().mt(4.0).fill(PANEL2).radius(5.0).px(6.0).py(1.0).child(mono(format!("#{id}"), 10.0, ICE)));
    }
    out.push(screen.key(format!("{key}-screen")));
    out
}

/// A small MacBook: its screen is off until `glow` lights it and shows the Mac's name. `alpha` fades it all.
fn mini(key: &str, d: &Device, r: Rect, glow: f32, alpha: f32) -> Vec<El> {
    let Some((m, c)) = look::resolve(&d.look) else { return vec![] };
    let (cx, hy, width) = hinge(r);
    let drawn = look::laptop(key, m, c, cx, hy, width);
    let [x, y, w, h] = drawn.screen;
    let mut out: Vec<El> = drawn.layers.into_iter().map(|e| e.opacity(alpha)).collect();
    if glow > 0.0 {
        let halo = el().key(format!("{key}-halo")).absolute(x - w * 0.08, y - w * 0.08).size(w * 1.16, h + w * 0.16).radius(w * 0.08)
            .fill(Color { a: 0.45 * glow, ..BLUE2 }).blur(w * 0.09).opacity(alpha);
        out.insert(1.min(out.len()), halo);
        out.push(el().key(format!("{key}-lit")).absolute(x, y).size(w, h).radius(w * 0.012).opacity(alpha * glow)
            .linear_gradient(2.4, [(0.0, Color { r: 0.03, g: 0.12, b: 0.42, a: 1.0 }), (0.55, Color { r: 0.012, g: 0.05, b: 0.18, a: 1.0 }), (1.0, Color { r: 0.05, g: 0.2, b: 0.55, a: 1.0 })]));
        let size = (w / (d.name.chars().count().max(5) as f32 * 0.6)).min(h * 0.3).min(24.0);
        out.push(col().key(format!("{key}-name")).absolute(x, y).size(w, h).center().gap(size * 0.2).opacity(alpha * glow)
            .child(label(d.name.clone(), size, 700.0, INK))
            .child(mono(format!("#{}", d.id), (size * 0.5).max(8.0), ICE)));
    } else {
        // The display switched off: plain black glass.
        out.push(el().key(format!("{key}-off")).absolute(x, y).size(w, h).radius(w * 0.012).fill(Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }).opacity(alpha));
    }
    out
}

/// Laptop boxes for `n` Macs in the right-hand place, rows of up to three, a short last row centred.
fn grid_cells(n: usize) -> Vec<Rect> {
    let r = layout().elements.laptop_right;
    let cols = match n {
        0 | 1 => 1,
        2 | 4 => 2,
        _ => 3,
    };
    let rows = n.div_ceil(cols).max(1);
    let (ax, ay, aw, ah) = (r.x - 40.0, r.y + 40.0, r.w + 80.0, r.h - 40.0);
    let (cw, ch) = (aw / cols as f32, ah / rows as f32);
    let w = (cw * 0.78).min(ch * 0.85 / 0.935).min(170.0);
    (0..n).map(|i| {
        let (c, row_) = (i % cols, i / cols);
        let in_row = (n - row_ * cols).min(cols);
        let x0 = ax + (aw - in_row as f32 * cw) / 2.0 + c as f32 * cw;
        Rect { x: x0 + (cw - w) / 2.0, y: ay + row_ as f32 * ch + (ch - w * 0.935) / 2.0, w, h: w * 0.935 }
    }).collect()
}

fn ease(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

/// The account's Macs, screens off; the one under the pointer lights up. A click grows it into its place
/// while the rest fade.
fn grid(s: &S) -> Vec<El> {
    let r = layout().elements.laptop_right;
    let pick = s.picked.map(|(from, t0)| (from, ease(((s.now - t0) / PICK_S).clamp(0.0, 1.0))));
    let fade = pick.map(|(_, t)| 1.0 - t).unwrap_or(1.0);
    let status = if !s.err.is_empty() {
        (s.err.clone(), ERR)
    } else if !s.polled {
        (s.t("Looking on GitHub…", "GitHub'da aranıyor…"), SOFT)
    } else if s.devices.is_empty() {
        (s.t("No other Mac yet: open MacySync on it and save it here.", "Henüz başka Mac yok: onda MacySync'i aç ve buraya kaydet."), SOFT)
    } else {
        (s.t("Point at a Mac to wake it, click to pick it.", "Uyandırmak için üstüne gel, seçmek için tıkla."), MUTED)
    };
    let centred = |e: El| row().w_full().child(spacer()).child(e).child(spacer());
    let mut out = vec![col().key("grid-head").absolute(r.x - 40.0, r.y).w(r.w + 80.0).gap(4.0).opacity(fade)
        .child(centred(caps(s.t("MACS ON THIS ACCOUNT", "HESAPTAKİ MAC'LER"), SILVER)))
        .child(centred(label(status.0, 11.5, 500.0, status.1)))];
    let cells = grid_cells(s.devices.len());
    for (i, (d, cell)) in s.devices.iter().zip(cells).enumerate() {
        if pick.is_some() && d.id == s.peer {
            continue;
        }
        let alpha = ((s.now - s.found_at - 0.07 * i as f32) / 0.35).clamp(0.0, 1.0) * fade;
        if alpha <= 0.0 {
            continue;
        }
        let key = format!("g-{}", d.id);
        out.extend(mini(&key, d, cell, s.glow.get(&d.id).copied().unwrap_or(0.0), alpha));
        if pick.is_none() {
            let (id, id2) = (d.id.clone(), d.id.clone());
            out.push(el().key(format!("{key}-hit")).absolute(cell.x - cell.w * 0.07, cell.y).size(cell.w * 1.14, cell.h).cursor(Cursor::Pointer)
                .on_hover_change(move |s: &mut S, inside, _| {
                    if inside {
                        s.hovered = id.clone();
                    } else if s.hovered == id {
                        s.hovered.clear();
                    }
                })
                .on_press(move |s: &mut S| s.pick(id2.clone(), cell)));
        }
    }
    if let (Some((from, t)), Some(d)) = (pick, s.peer_device()) {
        let lerp = |a: f32, b: f32| a + (b - a) * t;
        let to = Rect { x: lerp(from.x, r.x), y: lerp(from.y, r.y), w: lerp(from.w, r.w), h: lerp(from.h, r.h) };
        out.extend(mini("right", d, to, 1.0, 1.0));
    }
    out
}

/// The right-hand place before a Mac is picked: a dim laptop with its screen off and the find button.
fn find_prompt(s: &S) -> Vec<El> {
    let r = layout().elements.laptop_right;
    let ghost = Device { look: s.cfg.look.clone(), ..Device::default() };
    let mut out = mini("right", &ghost, r, 0.0, 0.3);
    let (cx, cy) = (r.x + r.w / 2.0, r.y + r.w * LID * 0.5);
    let note = if s.cfg.bus.is_empty() { s.t("signs in to GitHub", "GitHub'a bağlanır") } else { s.cfg.bus.clone() };
    let centred = |e: El| row().w_full().child(spacer()).child(e).child(spacer());
    out.push(col().key("find").absolute(cx - 150.0, cy - 30.0).w(300.0).gap(8.0)
        .child(centred(button(s.t("Find Macs on this account", "Hesaptaki Mac'leri bul"), true).key("find-btn").on_press(|s: &mut S| s.find())))
        .child(centred(mono(note, 10.0, MUTED))));
    out
}

/// Send and Receive between the two Macs: the direction is chosen per session. Pointing at one runs the
/// lines that way first.
fn actions(s: &S) -> Option<El> {
    let d = s.peer_device().filter(|_| !s.finding)?;
    let r = layout().elements.actions();
    if let Some(req) = s.active_request() {
        return (req.answer == Answer::Pending && req.from == s.cfg.id).then(|| {
            col().key("actions").absolute(r.x - 30.0, r.y + r.h / 2.0 - 52.0).w(r.w + 60.0)
                .child(row().w_full().child(spacer()).child(mono(s.t("waiting for approval…", "onay bekleniyor…"), 10.5, ICE)).child(spacer()))
        });
    }
    let pill = |key: &str, text: String, dir: f32, help: String| {
        let on = s.hover_dir == dir;
        el().key(key).w_full().h(34.0).center().radius(17.0).cursor(Cursor::Pointer)
            .fill(if on { BLUE } else { PANEL2 }).border(1.0, if on { BLUE2 } else { EDGE })
            .shadow(10.0, 24.0, Color { r: 0.0, g: 0.0, b: 0.0, a: 0.45 })
            .child(label(text, 13.0, 700.0, INK))
            .help(help)
            .on_hover_change(move |s: &mut S, inside, _| {
                if inside {
                    s.hover_dir = dir;
                } else if s.hover_dir == dir {
                    s.hover_dir = 0.0;
                }
            })
            .on_press(move |s: &mut S| s.ask(if dir > 0.0 { Ask::Send } else { Ask::Receive }))
    };
    Some(col().key("actions").absolute(r.x, r.y).size(r.w, r.h)
        .child(pill("send", format!("{}  ⟶", s.t("Send", "Gönder")), 1.0,
            format!("{} {}", s.t("This Mac sends to", "Bu Mac şuna gönderir:"), d.name)))
        .child(spacer())
        .child(pill("receive", format!("⟵  {}", s.t("Receive", "Al")), -1.0,
            format!("{} {}", s.t("This Mac takes from", "Bu Mac şundan alır:"), d.name))))
}

fn card(h: f32) -> El {
    col().w_full().h(h).fill(PANEL).border(1.0, EDGE).radius(14.0).px(14.0).py(12.0).gap(7.0)
}

fn button(s: impl Into<String>, primary: bool) -> El {
    el().px(12.0).py(7.0).radius(9.0)
        .fill(if primary { BLUE } else { PANEL2 })
        .border(1.0, if primary { BLUE2 } else { EDGE })
        .hover(if primary { BLUE2 } else { EDGE })
        .cursor(Cursor::Pointer)
        .child(label(s, 12.5, 600.0, INK))
}

fn trigger(key: &str, s: impl Into<String>) -> El {
    row().key(key).w_full().h(32.0).px(10.0).center_y().fill(FIELD).border(1.0, EDGE).radius(8.0).cursor(Cursor::Pointer)
        .child(label(s, 13.0, 500.0, INK))
        .child(spacer())
        .child(label("▾", 12.0, 600.0, MUTED))
}

fn menu(key: &str, w: f32, h: f32, items: Vec<El>) -> El {
    let list = col().w_full().gap(1.0).children(items);
    col().fill(Color { r: 0.004, g: 0.006, b: 0.01, a: 1.0 }).border(1.0, EDGE).radius(10.0).padding(4.0).w(w)
        .shadow(18.0, 40.0, Color { r: 0.0, g: 0.0, b: 0.0, a: 0.5 })
        .child(list.scroll(ScrollSpec { stick_bottom: false }).scroll_edge_fade(10.0).h(h))
        .anchored(key, Side::Bottom, Align::Start, 4.0)
}

fn menu_row(s: impl Into<String>, on: bool) -> El {
    row().w_full().px(9.0).py(6.0).radius(7.0).hover(PANEL2).cursor(Cursor::Pointer).fill(if on { PANEL2 } else { Color { a: 0.0, ..PANEL2 } })
        .child(label(s, 12.5, if on { 700.0 } else { 500.0 }, if on { ICE } else { INK }))
}

fn kit() -> Kit {
    Kit::from_palette(PaletteSeed { bg: FIELD, fg: INK, accent: BLUE })
}

fn tab_device(s: &S, k: &Kit) -> El {
    let detected = look::guess(&s.hw);
    let (m, _) = look::resolve(&s.cfg.look).unzip();
    let mut c = col().w_full().gap(8.0)
        .child(row().w_full().gap(10.0).center_y()
            .child(col().gap(4.0).child(caps(s.t("NAME", "AD"), MUTED))
                .child(TextInput::new(&s.name).width(Some(250.0)).on_edit(|s: &mut S| &mut s.name).on_change(|s: &mut S, _| {
                    s.cfg.name = s.name.text.trim().to_string();
                    s.cfg.save();
                }).build(k)))
            .child(col().gap(4.0).child(caps("ID", MUTED)).child(el().h(32.0).center_y().px(10.0).fill(PANEL2).radius(8.0).child(mono(format!("#{}", s.cfg.id), 13.0, ICE)))
                .help(s.t("Made on this Mac's first run; it never changes.", "Bu Mac'in ilk açılışında üretildi, değişmez."))))
        .child(caps(s.t("LOOK", "GÖRÜNÜM"), MUTED))
        .child(trigger("model-dd", m.map(look::short_name).unwrap_or_default()).on_press(|s: &mut S| s.model_open = !s.model_open));
    if let Some(m) = m {
        let mut sw = row().gap(8.0).center_y();
        for col_ in &m.colors {
            let on = col_.name == s.cfg.look.color;
            let base = look::hx(&col_.body_hex, STEEL);
            let model = m.id.clone();
            let name = col_.name.clone();
            sw = sw.child(el().key(format!("sw-{}", col_.name)).size(24.0, 24.0).radius(12.0).fill(base).border(if on { 2.5 } else { 1.0 }, if on { BLUE2 } else { EDGE })
                .cursor(Cursor::Pointer).help(col_.name.clone())
                .on_press(move |s: &mut S| s.set_look(Look { model: model.clone(), color: name.clone() })));
        }
        c = c.child(row().w_full().center_y().gap(10.0).child(sw).child(spacer()).child(mono(s.cfg.look.color.clone(), 11.5, SOFT)));
    }
    if !s.hw.model_id.is_empty() && detected != s.cfg.look && !detected.model.is_empty() {
        let name = look::resolve(&detected).map(|(m, c)| format!("{}, {}", look::short_name(m), c.name)).unwrap_or_default();
        c = c.child(row().w_full().center_y().gap(8.0)
            .child(label(format!("{} {name}", s.t("Detected:", "Algılanan:")), 12.0, 500.0, SOFT).wrap().grow(1.0))
            .child(button(s.t("Use it", "Bunu kullan"), false).key("use-detected").on_press(move |s: &mut S| s.set_look(detected.clone()))));
    }
    let action = if s.bus.is_none() { s.t("Sign in and save", "Bağlan ve kaydet") } else if s.cfg.registered { s.t("Save again", "Tekrar kaydet") } else { s.t("Save to the bus", "Bus'a kaydet") };
    c.child(row().w_full().center_y().gap(8.0)
        .child(TextInput::new(&s.bus_edit).placeholder(s.t("owner/macysync-bus (empty: yours)", "sahip/macysync-bus (boş: senin)")).width(Some(300.0)).on_edit(|s: &mut S| &mut s.bus_edit).build(k))
        .child(spacer())
        .child(button(action, true).key("save").on_press(|s: &mut S| if s.bus.is_none() { s.connect() } else { s.register() })))
}

fn ago(s: &S, t: u64) -> String {
    let d = bus::now().saturating_sub(t);
    if d < 90 { s.t("online", "çevrimiçi") } else if d < 3600 { format!("{} {}", d / 60, s.t("min ago", "dk önce")) } else if d < 86400 { format!("{} {}", d / 3600, s.t("h ago", "sa önce")) } else { format!("{} {}", d / 86400, s.t("d ago", "gün önce")) }
}

fn tab_connect(s: &S) -> El {
    let req = s.active_request();
    let picked = s.peer_device().is_some() && !s.finding;
    let done = [s.finding || picked, picked, req.is_some(), req.is_some_and(|r| r.answer == Answer::Accepted)];
    let steps = [
        s.t("Find the Macs on this account, on the right.", "Sağda, bu hesaptaki Mac'leri bul."),
        s.t("Click the Mac to work with.", "Çalışılacak Mac'e tıkla."),
        s.t("Press Send or Receive between the two. It holds for this session only.", "İkisinin arasında Gönder ya da Al'a bas. Yalnızca bu oturum için geçerli."),
        s.t("The other Mac approves.", "Diğer Mac onaylar."),
    ];
    let current = done.iter().position(|d| !d).unwrap_or(steps.len());
    let mut c = col().w_full().gap(10.0);
    for (i, text) in steps.into_iter().enumerate() {
        let now = i == current;
        let (mark, fg, bg) = if done[i] { ("✓".to_string(), INK, BLUE) } else if now { ((i + 1).to_string(), INK, PANEL2) } else { ((i + 1).to_string(), MUTED, FIELD) };
        c = c.child(row().w_full().gap(10.0).center_y()
            .child(el().size(22.0, 22.0).radius(11.0).center().fill(bg).border(1.0, if now { BLUE2 } else { EDGE }).child(mono(mark, 11.0, fg)))
            .child(label(text, 13.0, if now { 600.0 } else { 500.0 }, if now { INK } else if done[i] { SOFT } else { MUTED }).wrap().grow(1.0)));
    }
    let state = match req {
        Some(r) if r.answer == Answer::Pending && r.from == s.cfg.id => s.t("Waiting for the other Mac to approve…", "Diğer Mac'in onayı bekleniyor…"),
        Some(r) if r.answer == Answer::Accepted => s.t("Approved. Next: 03 PICK.", "Onaylandı. Sırada: 03 SEÇ."),
        _ => String::new(),
    };
    c.child(label(state, 12.0, 600.0, ICE).wrap().w_full()).child(spacer()).child(tail_row(s))
}

/// MacySync's own Tailscale, for Macs the network keeps apart.
fn tail_row(s: &S) -> El {
    let t = s.tail.clone().unwrap_or_default();
    let (text, action) = match t.state.as_str() {
        "Running" => (format!("{} {}", s.t("MacySync's Tailscale is on:", "MacySync'in Tailscale'i açık:"), t.ip), None),
        _ if !t.auth_url.is_empty() => (s.t("Sign in to Tailscale to put this Mac on your tailnet.", "Bu Mac'i tailnet'ine eklemek için Tailscale'e giriş yap."), Some(s.t("Sign in", "Giriş yap"))),
        "" => (s.t("Macs can't see each other here? MacySync's own Tailscale joins them, apart from any Tailscale on this Mac.",
            "Mac'ler burada birbirini görmüyor mu? MacySync'in kendi Tailscale'i bağlar, bu Mac'teki Tailscale'e dokunmaz."), Some(s.t("Use Tailscale", "Tailscale kullan"))),
        _ => (s.t("Tailscale is starting…", "Tailscale başlıyor…"), None),
    };
    let mut r = row().w_full().gap(10.0).center_y().child(label(text, 11.5, 500.0, if t.state == "Running" { ICE } else { MUTED }).wrap().grow(1.0));
    if let Some(a) = action {
        let url = t.auth_url.clone();
        r = r.child(button(a, false).key("tailscale").on_press(move |s: &mut S| {
            if url.is_empty() {
                let id = s.cfg.id.clone();
                s.tail_at = s.now - POLL_S + 1.5;
                s.spawn(s.t("Starting Tailscale…", "Tailscale başlatılıyor…"), move || Job::Tail(tail::start(&id).ok().and_then(|_| tail::status())));
            } else {
                let _ = std::process::Command::new("open").arg(&url).spawn();
            }
        }));
    }
    r
}

fn tab_soon(s: &S, what: String) -> El {
    col().w_full().gap(6.0).child(label(what, 13.0, 500.0, SOFT).wrap().w_full()).child(mono(s.t("next milestone", "sonraki adım"), 10.5, MUTED))
}

fn tick_row(s: &S, n: &Node, depth: usize, out: &mut Vec<El>) {
    let (on, all) = session::count(n, &s.pick.ticks);
    let open = s.pick.open.contains(&n.id);
    let mark = if on == all { "✓" } else if on > 0 { "–" } else { "" };
    let mut tick = el().key(format!("tick-{}", n.id)).size(16.0, 16.0).radius(4.0).center()
        .fill(if on == 0 { FIELD } else { BLUE }).border(1.0, if on == 0 { EDGE } else { BLUE2 })
        .child(mono(mark, 10.0, INK));
    if !s.pick.sent {
        let id = n.id.clone();
        tick = tick.cursor(Cursor::Pointer).on_press(move |s: &mut S| {
            let p = &mut s.pick;
            if let Some(n) = p.tree.as_ref().and_then(|t| t.find(&id)) {
                session::toggle(n, &mut p.ticks);
            }
        });
    }
    let arrow = if n.children.is_empty() {
        el().size(14.0, 16.0)
    } else {
        let id = n.id.clone();
        el().key(format!("open-{}", n.id)).size(14.0, 16.0).center().cursor(Cursor::Pointer)
            .child(label(if open { "▾" } else { "▸" }, 11.0, 600.0, MUTED))
            .on_press(move |s: &mut S| {
                if !s.pick.open.remove(&id) {
                    s.pick.open.insert(id.clone());
                }
            })
    };
    let mut r = row().w_full().h(22.0).gap(6.0).center_y().px(4.0).radius(5.0).fill(Color { a: 0.0, ..PANEL2 }).hover(PANEL2)
        .child(el().w(depth as f32 * 14.0)).child(arrow).child(tick)
        .child(label(n.name.clone(), 12.5, if depth == 0 { 650.0 } else { 500.0 }, if on == 0 { MUTED } else { INK }).grow(1.0));
    if n.secret {
        r = r.child(mono(s.t("secret", "gizli"), 9.5, BLUE2));
    }
    r = r.child(mono(fmt_bytes(n.size), 11.0, SOFT));
    if !n.note.is_empty() {
        r = r.help(n.note.clone());
    }
    out.push(r);
    if open {
        for c in &n.children {
            tick_row(s, c, depth + 1, out);
        }
    }
}

fn tab_pick(s: &S) -> El {
    if s.transfer().is_none() {
        return tab_soon(s, s.t("Approve a session in 02 CONNECT first. Then the sender ticks what it offers and the receiver what it takes: only those go.",
            "Önce 02 BAĞLAN'da bir oturum onaylanır. Sonra gönderen neyi sunduğunu, alan neyi alacağını işaretler: yalnızca onlar gider."));
    }
    let sender = s.sending() == Some(true);
    let other = s.peer_device().map(|d| d.name.clone()).unwrap_or_default();
    let Some(tree) = &s.pick.tree else {
        let msg = if !sender { format!("{} {other}…", s.t("Waiting for the offer from", "Teklif bekleniyor:")) }
            else if s.pick.scanning { s.t("Scanning this Mac: apps, tools, secrets, settings and files…", "Bu Mac taranıyor: uygulamalar, araçlar, gizli bilgiler, ayarlar ve dosyalar…") }
            else { s.t("The scan starts here.", "Tarama burada başlar.") };
        return col().w_full().gap(6.0).child(label(msg, 13.0, 500.0, SOFT).wrap().w_full());
    };
    let mut rows = Vec::new();
    for c in &tree.children {
        tick_row(s, c, 0, &mut rows);
    }
    let (on, all) = session::count(tree, &s.pick.ticks);
    let action = match (s.pick.sent, sender) {
        (false, true) => button(format!("{} {on}", s.t("Offer", "Sun")), true).key("offer").on_press(|s: &mut S| s.publish()),
        (false, false) => button(format!("{} {on}", s.t("Take", "Al")), true).key("take").on_press(|s: &mut S| s.publish()),
        (true, true) => label(format!("{} {other}…", s.t("Offered. Waiting for picks from", "Sunuldu. Seçim bekleniyor:")), 12.0, 600.0, ICE),
        (true, false) => label(format!("{} {other}…", s.t("Picked. Waiting for files from", "Seçildi. Dosyalar bekleniyor:")), 12.0, 600.0, ICE),
    };
    col().w_full().gap(6.0)
        .child(caps(if sender { s.t("TICK WHAT YOU OFFER", "SUNACAKLARINI İŞARETLE") } else { s.t("TICK WHAT YOU TAKE", "ALACAKLARINI İŞARETLE") }, MUTED))
        .child(col().w_full().gap(1.0).children(rows).scroll(ScrollSpec { stick_bottom: false }).scroll_edge_fade(10.0).h(172.0))
        .child(row().w_full().center_y().gap(8.0)
            .child(mono(format!("{on}/{all} · {}", fmt_bytes(session::ticked_size(tree, &s.pick.ticks))), 11.5, SOFT))
            .child(spacer()).child(action))
}

/// The command that starts the setup agent; Claude Code until one is chosen.
fn agent_cmd(s: &S) -> String {
    if s.cfg.agent.is_empty() { setup::AGENTS[0].1.into() } else { s.cfg.agent.clone() }
}

fn agent_name(s: &S) -> String {
    let cmd = agent_cmd(s);
    setup::AGENTS.iter().find(|(_, c)| *c == cmd).map(|(n, _)| n.to_string()).unwrap_or_else(|| "Custom".into())
}

fn tab_move(s: &S) -> El {
    let Some(l) = s.pick.live.as_ref().map(|l| l.lock().unwrap().clone()) else {
        return tab_soon(s, s.t("Nothing is moving yet. After 03 PICK the files go Mac to Mac, encrypted end to end, and an agent sets everything up.",
            "Henüz bir şey taşınmıyor. 03 SEÇ'ten sonra dosyalar uçtan uca şifreli olarak Mac'ten Mac'e gider, sonra bir agent her şeyi kurar."));
    };
    let state = match l.state.as_str() {
        "sending" => s.t("SENDING", "GÖNDERİLİYOR"),
        "receiving" => s.t("RECEIVING", "ALINIYOR"),
        "placing" => s.t("PUTTING THINGS IN PLACE", "YERLEŞTİRİLİYOR"),
        "placed" => s.t("IN PLACE", "YERLEŞTİ"),
        "done" => s.t("SENT", "GÖNDERİLDİ"),
        "failed" => s.t("STOPPED", "DURDU"),
        _ => s.t("STARTING", "BAŞLIYOR"),
    };
    let pct = if l.total > 0 { (l.bytes as f32 / l.total as f32).min(1.0) } else if l.done { 1.0 } else { 0.0 };
    let w = layout().elements.left_panel.w - 28.0;
    let mut c = col().w_full().gap(7.0)
        .child(row().w_full().center_y().child(caps(state, if l.failed { ERR } else { BLUE2 })).child(spacer())
            .child(mono(format!("{}/{} {}", l.items_done, l.items, s.t("items", "öğe")), 11.0, SOFT)))
        .child(row().w_full().child(mono(format!("{} / {}", fmt_bytes(l.bytes), fmt_bytes(l.total)), 12.5, INK)).child(spacer()).child(mono(l.network.clone(), 11.0, SOFT)))
        .child(el().w_full().h(6.0).radius(3.0).fill(FIELD).child(el().h(6.0).w(w * pct).radius(3.0).linear_gradient(1.57, [(0.0, BLUE), (1.0, ICE)])))
        .child(label(l.current.clone(), 12.0, 500.0, MUTED));
    if !l.note.is_empty() {
        c = c.child(label(l.note.clone(), 11.5, 600.0, if l.failed { ERR } else { SOFT }).wrap().w_full());
    }
    if l.state == "done" {
        c = c.child(label(s.t("Everything went across. The other Mac puts it in place and starts its agent.", "Her şey karşıya geçti. Diğer Mac yerleştirip agent'ını başlatır."), 12.5, 500.0, SOFT).wrap().w_full());
    }
    if let (true, Some(dir)) = (l.state == "placed", l.dir.clone()) {
        let dir2 = dir.clone();
        c = c.child(label(format!("{} {} · {} {}", l.placed, s.t("in place", "yerinde"), l.problems, s.t("to look at", "bakılacak")), 13.0, 650.0, INK))
            .child(label(s.t("An agent finishes the setup from AGENTS.md and GOAL.md in the transfer's folder.", "Kurulumu aktarım klasöründeki AGENTS.md ve GOAL.md ile bir agent bitirir."), 12.0, 500.0, SOFT).wrap().w_full())
            .child(row().w_full().gap(8.0).center_y()
                .child(el().w(170.0).child(trigger("agent-dd", agent_name(s)).on_press(|s: &mut S| s.agent_open = !s.agent_open)))
                .child(spacer())
                .child(button(s.t("Open folder", "Klasörü aç"), false).key("open-dir").on_press(move |_: &mut S| {
                    let _ = std::process::Command::new("open").arg(&dir2).spawn();
                }))
                .child(button(s.t("Start agent", "Agent'ı başlat"), true).key("start-agent").on_press(move |s: &mut S| {
                    if let Err(e) = setup::start_agent(&agent_cmd(s), &dir) {
                        s.err = e;
                    }
                })));
    }
    c
}

fn left_panel(s: &S, k: &Kit) -> El {
    let names = [("01", s.t("DEVICE", "CİHAZ")), ("02", s.t("CONNECT", "BAĞLAN")), ("03", s.t("PICK", "SEÇ")), ("04", s.t("MOVE", "TAŞI"))];
    let mut tabs = row().w_full().gap(4.0);
    for (i, (n, name)) in names.iter().enumerate() {
        let on = s.tab == i as u8 + 1;
        tabs = tabs.child(el().key(format!("tab-{}", i + 1)).grow(1.0).h(28.0).center().radius(5.0)
            .fill(if on { BLUE } else { PANEL }).border(1.0, if on { BLUE2 } else { EDGE }).cursor(Cursor::Pointer)
            .child(caps(format!("{n} {name}"), if on { INK } else { MUTED }))
            .on_press(move |s: &mut S| s.tab = i as u8 + 1));
    }
    let content = match s.tab {
        1 => tab_device(s, k),
        2 => tab_connect(s),
        3 => tab_pick(s),
        _ => tab_move(s),
    };
    let r = layout().elements.left_panel;
    col().absolute(r.x, r.y).w(r.w).gap(8.0).child(tabs).child(card(r.h - 36.0).child(content))
}

fn fmt_bytes(b: u64) -> String {
    let b = b as f64;
    if b >= 1e9 { format!("{:.1} GB", b / 1e9) } else if b >= 1e6 { format!("{:.0} MB", b / 1e6) } else { format!("{:.0} KB", b / 1e3) }
}

fn right_panel(s: &S) -> El {
    let r = layout().elements.right_panel;
    let mut c = card(r.h);
    match &s.session {
        Some(x) => {
            let pct = if x.total > 0 { x.bytes as f32 / x.total as f32 } else { 0.0 };
            c = c.child(row().w_full().center_y().child(caps(s.t("TRANSFER", "AKTARIM"), BLUE2)).child(spacer()).child(mono(x.state.clone(), 10.5, ICE)))
                .child(row().w_full().child(mono(format!("{} / {}", fmt_bytes(x.bytes), fmt_bytes(x.total)), 12.0, INK)).child(spacer())
                    .child(mono(format!("{}/s", fmt_bytes(x.avg_bytes_per_s)), 12.0, SOFT)))
                .child(el().w_full().h(6.0).radius(3.0).fill(FIELD).child(el().h(6.0).w((r.w - 28.0) * pct).radius(3.0).linear_gradient(1.57, [(0.0, BLUE), (1.0, ICE)])))
                .child(mono(format!("{} {} · {}", s.t("started", "başladı"), x.started_utc, x.network), 10.0, MUTED));
        }
        None => {
            let d = s.peer_device().filter(|_| !s.finding);
            let mut head = row().w_full().center_y().child(caps(s.t("OTHER MAC", "DİĞER MAC"), SILVER)).child(spacer());
            if d.is_some() && s.active_request().is_none() {
                head = head.child(button(s.t("Change", "Değiştir"), false).key("change").on_press(|s: &mut S| s.find()));
            }
            let line = match d {
                Some(d) => format!("{} · {} · {}", d.name, d.user, ago(s, d.seen)),
                None if s.finding => s.t("Click a Mac above to work with it.", "Çalışmak için yukarıdan bir Mac'e tıkla."),
                None => s.t("Find the other Mac above.", "Diğer Mac'i yukarıdan bul."),
            };
            c = c.child(head).child(label(line, 13.0, 500.0, SOFT).wrap().w_full());
        }
    }
    let status = if !s.busy.is_empty() { (s.busy.clone(), SOFT) } else if !s.err.is_empty() { (s.err.clone(), ERR) } else if s.bus.is_some() { (format!("{} {}", s.t("bus", "bus"), s.cfg.bus), MUTED) } else { (String::new(), MUTED) };
    c.child(spacer()).child(label(status.0, 11.0, 600.0, status.1).wrap().w_full())
}

fn sheet(card: El) -> El {
    let dim = Shape::new().fill_path(&body_path(), Color { r: 0.0, g: 0.001, b: 0.002, a: 0.74 });
    let r = layout().elements.dialog;
    el().absolute(0.0, 0.0).size(W, H).child(full(dim)).child(col().absolute(r.x, r.y).w(r.w).child(card))
}

fn incoming(s: &S, r: &Request) -> El {
    let who = s.devices.iter().find(|d| d.id == r.from).map(|d| d.name.clone()).unwrap_or_else(|| format!("#{}", r.from));
    let (title, body) = match r.ask {
        Ask::Send => (s.t("Receive files?", "Dosyaları al?"), format!("{who} {}", s.t("wants to send its files, settings and secrets to this Mac. You pick what to take before anything moves.",
            "dosyalarını, ayarlarını ve gizli bilgilerini bu Mac'e göndermek istiyor. Bir şey taşınmadan önce neyi alacağını sen seçersin."))),
        Ask::Receive => (s.t("Send files?", "Dosyaları gönder?"), format!("{who} {}", s.t("wants to take this Mac's files, settings and secrets. You pick what to offer before anything moves.",
            "bu Mac'in dosyalarını, ayarlarını ve gizli bilgilerini almak istiyor. Bir şey taşınmadan önce neyi sunacağını sen seçersin."))),
    };
    let card = col().w_full().fill(Color { r: 0.004, g: 0.007, b: 0.013, a: 1.0 }).border(1.0, BLUE).radius(16.0).padding(22.0).gap(12.0)
        .shadow(30.0, 80.0, Color { r: 0.0, g: 0.0, b: 0.0, a: 0.6 })
        .child(caps(s.t("REQUEST", "İSTEK"), BLUE2))
        .child(label(title, 19.0, 700.0, INK))
        .child(label(body, 13.5, 500.0, SOFT).wrap().w_full())
        .child(row().w_full().gap(10.0).child(spacer())
            .child(button(s.t("Decline", "Reddet"), false).key("decline").on_press(|s: &mut S| s.answer(false)))
            .child(button(s.t("Approve", "Onayla"), true).key("approve").on_press(|s: &mut S| s.answer(true))));
    sheet(card)
}

fn overlays(s: &S) -> Vec<El> {
    let mut out = Vec::new();
    if s.model_open && s.tab == 1 {
        let items: Vec<El> = look::catalog().iter().map(|m| {
            let id = m.id.clone();
            menu_row(look::short_name(m), m.id == s.cfg.look.model).key(format!("model-{}", m.id)).on_press(move |s: &mut S| {
                let color = look::model(&id).map(|m| m.colors[0].name.clone()).unwrap_or_default();
                let color = if look::model(&id).is_some_and(|m| m.colors.iter().any(|c| c.name == s.cfg.look.color)) { s.cfg.look.color.clone() } else { color };
                s.model_open = false;
                s.set_look(Look { model: id.clone(), color });
            })
        }).collect();
        out.push(menu("model-dd", 484.0, 260.0, items));
    }
    if s.agent_open && s.tab == 4 {
        let items: Vec<El> = setup::AGENTS.iter().map(|&(name, cmd)| {
            menu_row(name, agent_cmd(s) == cmd).key(format!("agent-{name}")).on_press(move |s: &mut S| {
                s.cfg.agent = cmd.into();
                s.cfg.save();
                s.agent_open = false;
            })
        }).collect();
        out.push(menu("agent-dd", 170.0, 150.0, items));
    }
    out
}

fn view(s: &S, _: &mut Cx<S>) -> El {
    let k = kit();
    let e = &layout().elements;
    let mut root = el().key("root").size(W, H).children(backdrop(s)).children(conduit(s));
    let me = s.me();
    let peer = s.peer_device();
    let (role_me, role_peer) = match s.sending() {
        Some(true) => (s.t("SENDER", "GÖNDEREN"), s.t("RECEIVER", "ALAN")),
        Some(false) => (s.t("RECEIVER", "ALAN"), s.t("SENDER", "GÖNDEREN")),
        None => (s.t("READY", "HAZIR"), if peer.is_some_and(|d| bus::now().saturating_sub(d.seen) < 90) { s.t("ONLINE", "ÇEVRİMİÇİ") } else { s.t("AWAY", "UZAKTA") }),
    };
    root = root.children(device_on_screen(s, "left", Some(&me), &s.cfg.look, e.laptop_left, role_me, true));
    root = match peer {
        _ if s.finding => root.children(grid(s)),
        Some(d) => root.children(device_on_screen(s, "right", Some(d), &d.look, e.laptop_right, role_peer, false)),
        None => root.children(find_prompt(s)),
    };
    root = root
        .child(row().absolute(e.title.x, e.title.y).gap(10.0).center_y().window_drag()
            .child(row().child(label("Macy", 26.0, 800.0, INK)).child(label("~", 26.0, 800.0, BLUE2)).child(label("Sync", 26.0, 800.0, SILVER)))
            .child(mono(format!("MYSY · v{}", env!("CARGO_PKG_VERSION")), 10.5, MUTED)))
        .child(row().absolute(e.controls.x, e.controls.y).gap(6.0)
            .child(button(if s.tr { "EN" } else { "TR" }, false).key("lang").help(s.t("Türkçe", "English")).on_press(|s: &mut S| {
                s.tr = !s.tr;
                s.cfg.lang = if s.tr { "tr".into() } else { "en".into() };
                s.cfg.save();
            }))
            .child(button("▁", false).key("min").on_press_tx(|_: &mut S, tx: &mut Tx| tx.minimize()))
            .child(button("✕", false).key("quit").on_press_tx(|_: &mut S, tx: &mut Tx| tx.quit())))
        .child(left_panel(s, &k))
        .child(col().absolute(e.right_panel.x, e.right_panel.y).w(e.right_panel.w).child(right_panel(s)));
    if let Some(a) = actions(s) {
        root = root.child(a);
    }
    if let Some((r, _)) = &s.incoming {
        root = root.child(incoming(s, r));
    }
    rune_kit::root(root.children(overlays(s)), Vec::<El>::new())
}

// Demo scenes for screenshots: made-up Macs, nothing from this computer, nothing sent.
fn demo(s: &mut S) {
    s.demo = true;
    s.busy.clear();
    s.err.clear();
    s.tr = false;
    s.cfg.id = "7f3a2c91".into();
    s.cfg.name = "Studio Air".into();
    s.name = TextEdit::new("Studio Air");
    s.cfg.registered = true;
    s.cfg.bus = "someone/macysync-bus".into();
    s.hw = Hardware { model_id: "MacBookPro17,1".into(), part: "MYD82".into(), chip: "Apple M1".into(), ram_gb: 8, disk_gb: 256 };
    s.cfg.look = look::guess(&s.hw);
    let pick = |fam: &str, inch: f32| look::catalog().iter().find(|m| m.family == fam && (m.screen_in - inch).abs() < 0.6).map(|m| Look { model: m.id.clone(), color: m.colors.last().unwrap().name.clone() }).unwrap_or_default();
    s.devices = vec![
        Device { id: "c04e19b7".into(), name: "Desk Pro".into(), look: pick("pro", 14.2), chip: "Apple M1".into(), ram_gb: 16, disk_gb: 512, user: "someone".into(), seen: bus::now() - 20, ..Device::default() },
        Device { id: "5b21d0aa".into(), name: "Travel Air".into(), look: pick("air", 13.6), chip: "Apple M2".into(), ram_gb: 8, disk_gb: 256, user: "someone".into(), seen: bus::now() - 7200, ..Device::default() },
        Device { id: "e8d4a130".into(), name: "Studio 16".into(), look: pick("pro", 16.2), chip: "Apple M3 Max".into(), ram_gb: 48, disk_gb: 1000, user: "someone".into(), seen: bus::now() - 40, ..Device::default() },
        Device { id: "91fb07c2".into(), name: "Old Air".into(), look: Look { model: look::catalog().iter().rev().find(|m| m.family == "air").map(|m| m.id.clone()).unwrap_or_default(), color: String::new() }, chip: "Apple M1".into(), ram_gb: 8, disk_gb: 256, user: "someone".into(), seen: bus::now() - 90000, ..Device::default() },
    ];
    s.polled = true;
    s.peer = "c04e19b7".into();
    s.tab = 2;
}

/// The grid already shown, nothing picked.
fn finding_scene(s: &mut S) {
    demo(s);
    s.peer.clear();
    s.finding = true;
    s.found_at = -5.0;
}

fn init_demo() -> S {
    let mut s = init();
    demo(&mut s);
    s
}

/// An approved session with Desk Pro, this Mac sending or receiving.
fn accepted(s: &mut S, this_sends: bool) {
    demo(s);
    let (a, b) = if this_sends { ("7f3a2c91", "c04e19b7") } else { ("c04e19b7", "7f3a2c91") };
    let r = Request { ask: Ask::Send, from: a.into(), to: b.into(), at: bus::now() - 300, answer: Answer::Accepted, transfer: bus::transfer_dir(bus::now() - 280, a, b) };
    s.pick = Pick { transfer: r.transfer.clone(), seen: true, ..Pick::default() };
    s.requests = vec![(r, String::new())];
    s.session = Some(Session { state: "picking".into(), started_utc: bus::utc(bus::now() - 280), ..Session::default() });
}

fn demo_tree() -> Node {
    let leaf = |id: &str, name: &str, size: u64, off: bool, secret: bool| Node { id: id.into(), name: name.into(), size, off, secret, ..Node::default() };
    let group = |id: &str, name: &str, children: Vec<Node>| Node { id: id.into(), name: name.into(), size: children.iter().map(|c| c.size).sum(), children, ..Node::default() };
    group("", "", vec![
        group("apps", "Apps", vec![
            group("apps/wave", "Wave", vec![leaf("apps/wave/app", "App", 310_000_000, false, false), leaf("apps/wave/settings", "Settings", 2_100_000, false, false), leaf("apps/wave/data", "Workspaces and layouts", 585_000_000, false, false)]),
            group("apps/helium", "Helium", vec![leaf("apps/helium/app", "App", 421_000_000, false, false), leaf("apps/helium/data", "Profiles, history, logins", 1_930_000_000, false, false)]),
            group("apps/tailscale", "Tailscale", vec![leaf("apps/tailscale/app", "App", 95_000_000, true, false)]),
        ]),
        group("tools", "Tools", vec![leaf("tools/brew", "Homebrew formulae and casks", 81_000, false, false)]),
        group("secrets", "Secrets", vec![leaf("secrets/ssh", "SSH keys", 12_000, false, true), leaf("secrets/keychain", "Login keychain", 2_400_000, false, true), leaf("secrets/helium-key", "Helium Storage Key", 100, true, true)]),
        group("shell", "Shell", vec![leaf("shell/zshrc", ".zshrc", 9_000, false, false), leaf("shell/gitconfig", ".gitconfig", 1_000, false, false)]),
        group("ai", "AI", vec![leaf("ai/claude-code", "Claude Code", 6_100_000_000, false, false)]),
        group("files", "Files", vec![leaf("files/documents", "Documents", 14_200_000_000, false, false), leaf("files/projects", "projects", 22_700_000_000, false, false)]),
    ])
}

fn pick_scene(s: &mut S, this_sends: bool, sent: bool) {
    accepted(s, this_sends);
    let t = demo_tree();
    s.pick.ticks = session::default_ticks(&t);
    s.pick.open = ["apps", "apps/wave", "secrets"].map(String::from).into();
    s.pick.tree = Some(t);
    s.pick.sent = sent;
    s.tab = 3;
}

fn live(l: Live) -> Option<session::Shared> {
    Some(Arc::new(Mutex::new(l)))
}

fn sending_scene(s: &mut S, this_sends: bool) {
    accepted(s, this_sends);
    s.session = Some(Session { state: "sending".into(), started_utc: bus::utc(bus::now() - 280), bytes: 23_400_000_000, total: 81_200_000_000, avg_bytes_per_s: 86_000_000, network: "LAN".into(), ..Session::default() });
    s.pick.live = live(Live { state: if this_sends { "sending" } else { "receiving" }.into(), bytes: 23_400_000_000, total: 81_200_000_000, items_done: 14, items: 41, current: "Documents".into(), network: "LAN".into(), ..Live::default() });
    s.flowing = true;
    s.flow.dir = if this_sends { 1.0 } else { -1.0 };
    s.flow.v = s.flow.dir * FLOW_V;
    s.flow.pos = 0.37 * s.flow.dir;
    s.tab = 4;
}

fn placed_scene(s: &mut S) {
    accepted(s, false);
    s.session = Some(Session { state: "done".into(), started_utc: bus::utc(bus::now() - 1180), bytes: 81_200_000_000, total: 81_200_000_000, avg_bytes_per_s: 91_000_000, network: "LAN".into(), ..Session::default() });
    let dir = std::path::PathBuf::from("/Users/someone/MacySync/20261126-101240_c04e19b7_7f3a2c91");
    s.pick.live = live(Live { state: "placed".into(), bytes: 81_200_000_000, total: 81_200_000_000, items_done: 41, items: 41, network: "LAN".into(), placed: 38, problems: 3, done: true, dir: Some(dir), ..Live::default() });
    s.tab = 4;
}

pub fn run(demo_mode: bool) {
    if demo_mode {
        // SAFETY: still single threaded; nothing has read the environment yet.
        unsafe {
            std::env::set_var("MACYSYNC_HOME", std::env::temp_dir().join("macysync-demo"));
            std::env::set_var("MACYSYNC_TOKEN", "");
        }
    }
    let spec = AppSpec {
        setup: Some(setup),
        poll: Some(poll),
        families: ("Inter", "ui-monospace"),
        scenes: &[
            ("connect", |s| demo(s)),
            ("start", |s| { demo(s); s.peer.clear(); }),
            ("find", |s| { finding_scene(s); s.hovered = "5b21d0aa".into(); s.glow.insert("5b21d0aa".into(), 1.0); }),
            ("pick", |s| { finding_scene(s); let c = grid_cells(s.devices.len())[0]; s.pick("c04e19b7".into(), c); s.picked = Some((c, 1.5 - 0.22)); }),
            ("hover-send", |s| { demo(s); s.hover_dir = 1.0; s.flow.dir = 1.0; s.flow.v = FLOW_V; s.flow.pos = 0.3; }),
            ("device", |s| { demo(s); s.tab = 1; }),
            ("models", |s| { demo(s); s.tab = 1; s.model_open = true; }),
            ("request", |s| { demo(s); s.incoming = Some((Request { ask: Ask::Send, from: "c04e19b7".into(), to: "7f3a2c91".into(), at: bus::now(), answer: Answer::Pending, transfer: String::new() }, String::new())); }),
            ("offer", |s| pick_scene(s, true, false)),
            ("take", |s| pick_scene(s, false, false)),
            ("offered", |s| pick_scene(s, true, true)),
            ("placed", |s| placed_scene(s)),
            ("agents", |s| { placed_scene(s); s.agent_open = true; }),
            ("sending", |s| sending_scene(s, true)),
            ("receiving", |s| sending_scene(s, false)),
            ("tr", |s| { sending_scene(s, false); s.tr = true; }),
        ],
        ..AppSpec::new("MacySync", (W, H), if demo_mode { init_demo } else { init }, view)
    };
    let background = std::env::var_os("MACYSYNC_BACKGROUND").is_some();
    let options = WindowOptions { transparent: true, borderless: true, resizable: false, position: background.then_some((-4000, 100)), ..Default::default() };
    let app = App::new(spec).window_options(options);
    if background { app.open_inactive().run() } else { app.run() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_layout_replaces_the_running_one() {
        let path = std::env::temp_dir().join(format!("macysync-layout-{}.json", std::process::id()));
        let mut l: serde_json::Value = serde_json::from_str(include_str!("../assets/layout.json")).unwrap();
        std::fs::write(&path, l.to_string()).unwrap();
        let mut seen = None;
        assert!(!reload_layout(&path, &mut seen), "the first look only notes the time");
        let lines = flow_lines().as_ptr();
        l["elements"]["title"]["x"] = 333.0.into();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&path, l.to_string()).unwrap();
        assert!(reload_layout(&path, &mut seen));
        assert_eq!(layout().elements.title.x, 333.0);
        assert_ne!(flow_lines().as_ptr(), lines, "the lines follow the new layout");
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&path, "{ half written").unwrap();
        assert!(!reload_layout(&path, &mut seen), "a broken file keeps the last good layout");
        assert_eq!(layout().elements.title.x, 333.0);
        let _ = std::fs::remove_file(path);
    }
}
