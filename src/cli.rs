//! The same transfer without the window, for a Mac reached over SSH:
//!
//!   macysync --give <mac id> [--only apps/wave,secrets/ssh]    ask that Mac to receive, then send
//!   macysync --accept [--only …]                               approve the request waiting here, then do this side
//!
//! `--only` takes tree ids or their starts (`macysync --scan` shows the tree); without it the sender offers and
//! the receiver takes everything not marked off.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::bus::{self, Answer, Ask, Bus, Device, Request};
use crate::me::{self, Config};
use crate::scan::{self, Offer};
use crate::session::{self, Live, Picks, ReceiverDoc};
use crate::wire::Endpoint;

struct Ctx {
    bus: Arc<Bus>,
    cfg: Config,
    secret: [u8; 32],
}

fn connect() -> Result<Ctx, String> {
    let mut cfg = Config::load();
    let token = me::token().ok_or("Not signed in: run `gh auth login`, or set MACYSYNC_TOKEN.")?;
    if cfg.bus.is_empty() {
        cfg.bus = format!("{}/macysync-bus", bus::github_login(&token)?);
        bus::ensure_repo(&token, &cfg.bus)?;
        cfg.save();
    }
    let b = Arc::new(Bus::new(&cfg.bus, &token));
    let (secret, public) = me::keys();
    let hw = me::detect();
    // Registers (again) so the other Mac has this one's current key and look.
    let d = Device {
        id: cfg.id.clone(), name: cfg.name.clone(), look: cfg.look.clone(), key: bus::b64(&public), chip: hw.chip, ram_gb: hw.ram_gb,
        disk_gb: hw.disk_gb, user: std::env::var("USER").unwrap_or_default(), seen: bus::now(), version: env!("CARGO_PKG_VERSION").into(),
    };
    b.put_json(&format!("devices/{}.json", d.id), &d, None, &format!("device {} ({})", d.name, d.id))?;
    if !cfg.registered {
        cfg.registered = true;
        cfg.save();
    }
    println!("{} #{} on {}", cfg.name, cfg.id, cfg.bus);
    Ok(Ctx { bus: b, cfg, secret })
}

fn device(b: &Bus, id: &str) -> Result<Device, String> {
    b.get_json::<Device>(&format!("devices/{id}.json"))?.map(|d| d.0).ok_or(format!("no Mac #{id} on the bus"))
}

/// Polls `f` every 5 s until it gives something; GitHub doesn't count the unchanged answers.
fn wait<T>(what: &str, limit: Duration, mut f: impl FnMut() -> Result<Option<T>, String>) -> Result<T, String> {
    println!("{what}…");
    let start = Instant::now();
    loop {
        if let Some(t) = f()? {
            return Ok(t);
        }
        if start.elapsed() > limit {
            return Err(format!("gave up: {what}"));
        }
        std::thread::sleep(Duration::from_secs(5));
    }
}

fn gb(b: u64) -> String {
    format!("{:.2} GB", b as f64 / 1e9)
}

pub fn give(peer: &str, only: Vec<String>) -> Result<(), String> {
    let c = connect()?;
    let other = device(&c.bus, peer)?;
    let r = Request { ask: Ask::Send, from: c.cfg.id.clone(), to: other.id.clone(), at: bus::now(), answer: Answer::Pending, transfer: String::new() };
    c.bus.put_json(&r.path(), &r, None, &format!("Send request {} to {}", r.from, r.to))?;
    let r = wait(&format!("waiting for {} to approve", other.name), Duration::from_secs(3600), || {
        Ok(c.bus.get_json::<Request>(&r.path())?.map(|x| x.0).filter(|x| x.answer != Answer::Pending))
    })?;
    if r.answer != Answer::Accepted {
        return Err(format!("{} said {:?}", other.name, r.answer));
    }
    sender(&c, &r, &other, &only)
}

pub fn accept(only: Vec<String>) -> Result<(), String> {
    let c = connect()?;
    let me = c.cfg.id.clone();
    let mut mine: Vec<(Request, String)> = vec![];
    for n in c.bus.list("requests")?.into_iter().filter(|n| n.ends_with(".json") && n.contains(&me)) {
        if let Some(x) = c.bus.get_json::<Request>(&format!("requests/{n}"))? {
            mine.push(x);
        }
    }
    let pending = mine.iter().filter(|(r, _)| r.to == me && r.answer == Answer::Pending && bus::now().saturating_sub(r.at) < 86400).max_by_key(|(r, _)| r.at).cloned();
    let (r, sha) = pending.ok_or("no request is waiting for this Mac")?;
    let peer = if r.from == me { r.to.clone() } else { r.from.clone() };
    let other = device(&c.bus, &peer)?;
    let names = [(me.clone(), c.cfg.name.clone()), (other.id.clone(), other.name.clone())].into_iter().collect();
    let r = session::approve(&c.bus, r, &sha, &names, true)?;
    println!("approved {:?} from {}: {}", r.ask, other.name, r.transfer);
    if r.sides().0 == me { sender(&c, &r, &other, &only) } else { receiver(&c, &r, &other, &only) }
}

fn sender(c: &Ctx, r: &Request, to: &Device, only: &[String]) -> Result<(), String> {
    let t = r.transfer.clone();
    let home = me::home_dir();
    println!("scanning this Mac…");
    let tree = scan::scan(&home, &["/Applications".into(), home.join("Applications")], true);
    let ticks = if only.is_empty() { session::default_ticks(&tree) } else { session::ticks_for(&tree, only) };
    let offer = session::offer(&home, &tree, &ticks);
    println!("offering {} items, {}", ticks.len(), gb(offer.tree.size));
    for l in offer.tree.leaves() {
        println!("  {} ({})", l.id, gb(l.size));
    }
    let key = to.key_bytes().ok_or("the other Mac has no key on the bus")?;
    session::put_sealed(&c.bus, &format!("{t}/offer.sealed"), &key, &offer, "offer")?;
    session::update_session(&c.bus, &t, |s| s.state = "picking".into())?;
    let picks: Picks = wait(&format!("waiting for {} to pick", to.name), Duration::from_secs(3600), || session::get_sealed(&c.bus, &format!("{t}/picks.sealed"), &c.secret))?;
    let items = session::items(&offer, &picks);
    println!("{} picked {} items, {}", to.name, items.len(), gb(items.iter().map(|i| i.size).sum()));
    let live: session::Shared = Arc::new(Mutex::new(Live::default()));
    let (b, t2, to2, l2) = (c.bus.clone(), t.clone(), to.clone(), live.clone());
    let worker = std::thread::spawn(move || session::serve(&b, &t2, &to2, &offer, &picks, &home, l2));
    let start = Instant::now();
    let mut last_write = Instant::now() - Duration::from_secs(60);
    loop {
        let l = live.lock().unwrap().clone();
        let finished = l.done || l.failed || worker.is_finished();
        println!("{} {} / {}  {}/{} items  {}  {}", l.state, gb(l.bytes), gb(l.total), l.items_done, l.items, l.network, l.current);
        if finished || last_write.elapsed() > Duration::from_secs(10) {
            last_write = Instant::now();
            let secs = start.elapsed().as_secs().max(1);
            let _ = session::update_session(&c.bus, &t, |s| {
                s.state = l.state.clone();
                s.bytes = l.bytes;
                s.total = l.total;
                s.items = l.items as u64;
                s.items_done = l.items_done as u64;
                s.avg_bytes_per_s = l.bytes / secs;
                s.network = l.network.clone();
                s.note = l.note.clone();
            });
        }
        if finished {
            break;
        }
        std::thread::sleep(Duration::from_secs(3));
    }
    worker.join().map_err(|_| "the sending thread stopped".to_string())?
}

fn receiver(c: &Ctx, r: &Request, from: &Device, only: &[String]) -> Result<(), String> {
    let t = r.transfer.clone();
    let offer: Offer = wait(&format!("waiting for {} to offer", from.name), Duration::from_secs(3600), || session::get_sealed(&c.bus, &format!("{t}/offer.sealed"), &c.secret))?;
    let ticks = if only.is_empty() { session::default_ticks(&offer.tree) } else { session::ticks_for(&offer.tree, only) };
    let picks = Picks { ids: offer.tree.leaves().iter().filter(|l| ticks.contains(&l.id)).map(|l| l.id.clone()).collect() };
    println!("taking {} of {} offered items, {}", picks.ids.len(), offer.tree.leaves().len(), gb(session::ticked_size(&offer.tree, &ticks)));
    let key = from.key_bytes().ok_or("the other Mac has no key on the bus")?;
    session::put_sealed(&c.bus, &format!("{t}/picks.sealed"), &key, &picks, "picks")?;
    let mut doc = ReceiverDoc { state: "waiting".into(), items: picks.ids.len() as u64, ..ReceiverDoc::default() };
    session::write_receiver(&c.bus, &t, &doc)?;
    let ep: Endpoint = wait(&format!("waiting for {} to start sending", from.name), Duration::from_secs(3600), || session::get_sealed(&c.bus, &format!("{t}/key.sealed"), &c.secret))?;
    let home = me::home_dir();
    let dir: PathBuf = session::local_dir(&home, &t);
    let live: session::Shared = Arc::new(Mutex::new(Live::default()));
    let (l2, d2, name) = (live.clone(), dir.clone(), from.name.clone());
    let worker = std::thread::spawn(move || session::receive(&ep, &offer, &picks, &d2, &name, &home, l2));
    let mut last_write = Instant::now();
    loop {
        let l = live.lock().unwrap().clone();
        let finished = l.done || l.failed || worker.is_finished();
        println!("{} {} / {}  {}/{} items  {}  {}", l.state, gb(l.bytes), gb(l.total), l.items_done, l.items, l.current, l.note);
        if finished || last_write.elapsed() > Duration::from_secs(10) {
            last_write = Instant::now();
            doc = ReceiverDoc { state: l.state.clone(), bytes: l.bytes, items_done: l.items_done as u64, items: l.items as u64, note: l.note.clone(), updated: 0 };
            let _ = session::write_receiver(&c.bus, &t, &doc);
        }
        if finished {
            println!("placed {} items, {} problems; the brief is in {}", l.placed, l.problems, dir.display());
            break;
        }
        std::thread::sleep(Duration::from_secs(3));
    }
    worker.join().map_err(|_| "the receiving thread stopped".to_string())?
}
