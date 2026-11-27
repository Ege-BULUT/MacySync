//! One transfer once its request is approved, the same for the window and the command line:
//!
//!   sender     scans, ticks what it offers, publishes offer.sealed (to the receiver's key)
//!   receiver   opens the offer, ticks what it takes, publishes picks.sealed (to the sender's key)
//!   sender     listens, publishes key.sealed (stream key and addresses, to the receiver), serves the picks
//!   receiver   fetches them into ~/MacySync/<transfer>/staging, puts them in place, writes the brief
//!
//! The sender keeps session.json current, the receiver receiver.json.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::bus::{self, Answer, Bus, Device, Party, Request, Session};
use crate::scan::{Node, Offer};
use crate::wire::{self, Endpoint};
use crate::{seal, setup};

/// The leaf ids the receiver takes.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Picks {
    pub ids: Vec<String>,
}

/// What the receiver is doing, in receiver.json.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct ReceiverDoc {
    /// picking, waiting, receiving, placing, placed, failed
    pub state: String,
    pub bytes: u64,
    pub items_done: u64,
    pub items: u64,
    pub note: String,
    pub updated: u64,
}

/// Progress on this Mac, shared with the thread doing the work.
#[derive(Clone, Debug, Default)]
pub struct Live {
    pub state: String,
    pub bytes: u64,
    pub total: u64,
    pub items_done: usize,
    pub items: usize,
    pub current: String,
    pub network: String,
    /// When the bytes started to move (unix seconds), for the average speed.
    pub started: u64,
    pub note: String,
    pub done: bool,
    pub failed: bool,
    /// Receiver: the folder with the brief, and how placing went.
    pub dir: Option<PathBuf>,
    pub placed: usize,
    pub problems: usize,
}

pub type Shared = Arc<Mutex<Live>>;

/// Sealed files are stored as base64 text: GitHub's contents API reads some binary files as text in a guessed
/// charset and hands back other bytes.
pub fn put_sealed<T: Serialize>(b: &Bus, path: &str, to: &[u8; 32], t: &T, msg: &str) -> Result<(), String> {
    b.put(path, bus::b64(&seal::seal(to, &serde_json::to_vec(t).unwrap())).as_bytes(), None, msg).map(|_| ())
}

pub fn get_sealed<T: DeserializeOwned>(b: &Bus, path: &str, secret: &[u8; 32]) -> Result<Option<T>, String> {
    let Some((text, _)) = b.get(path)? else { return Ok(None) };
    let bytes = bus::unb64(&String::from_utf8_lossy(&text)).ok_or(format!("{path} is not base64"))?;
    let plain = seal::open(secret, &bytes)?;
    serde_json::from_slice(&plain).map(Some).map_err(|e| format!("{path}: {e}"))
}

/// Approves a request: makes the transfer folder with its session.json and records the answer.
pub fn approve(b: &Bus, mut r: Request, sha: &str, names: &std::collections::HashMap<String, String>, yes: bool) -> Result<Request, String> {
    if yes {
        let (sender, receiver) = r.sides();
        let at = bus::now();
        let dir = bus::transfer_dir(at, sender, receiver);
        let party = |id: &str| Party { id: id.into(), name: names.get(id).cloned().unwrap_or_default() };
        let s = Session { sender: party(sender), receiver: party(receiver), state: "offering".into(), started: at, started_utc: bus::utc(at), updated: at, ..Session::default() };
        b.put_json(&format!("{dir}/session.json"), &s, None, &format!("transfer {sender} to {receiver}"))?;
        r.transfer = dir;
    }
    r.answer = if yes { Answer::Accepted } else { Answer::Declined };
    b.put_json(&r.path(), &r, Some(sha), &format!("{:?} {}", r.answer, r.path()))?;
    Ok(r)
}

/// Changes session.json (the sender's document).
pub fn update_session(b: &Bus, transfer: &str, f: impl FnOnce(&mut Session)) -> Result<(), String> {
    let path = format!("{transfer}/session.json");
    let (mut s, sha) = b.get_json::<Session>(&path)?.ok_or("the transfer has no session.json")?;
    f(&mut s);
    s.updated = bus::now();
    b.put_json(&path, &s, Some(&sha), &format!("session {}", s.state)).map(|_| ())
}

pub fn write_receiver(b: &Bus, transfer: &str, doc: &ReceiverDoc) -> Result<(), String> {
    let mut d = doc.clone();
    d.updated = bus::now();
    b.put_json(&format!("{transfer}/receiver.json"), &d, None, &format!("receiver {}", d.state)).map(|_| ())
}

/// How often a running transfer writes its progress to the bus; a state change writes at once. Each write is a
/// commit on the bus repo: every 10 s made ~720 commits an hour, every 2 min makes 30. Live progress travels
/// through the tunnel itself; the bus copy is what the other Mac's TRANSFER panel shows.
pub const REPORT_S: u64 = 120;

/// Puts this side's progress on the bus: session.json for the sender, receiver.json for the receiver.
pub fn report(b: &Bus, transfer: &str, sender: bool, l: &Live) -> Result<(), String> {
    if !sender {
        let d = ReceiverDoc { state: l.state.clone(), bytes: l.bytes, items_done: l.items_done as u64, items: l.items as u64, note: l.note.clone(), updated: 0 };
        return write_receiver(b, transfer, &d);
    }
    let secs = bus::now().saturating_sub(l.started).max(1);
    update_session(b, transfer, |s| {
        s.state = l.state.clone();
        s.bytes = l.bytes;
        s.total = l.total;
        s.items = l.items as u64;
        s.items_done = l.items_done as u64;
        s.avg_bytes_per_s = l.bytes / secs;
        s.network = l.network.clone();
        s.note = l.note.clone();
    })
}

// Ticks ---------------------------------------------------------------------------------------------------

/// Every leaf not marked `off`.
pub fn default_ticks(tree: &Node) -> HashSet<String> {
    tree.leaves().into_iter().filter(|l| !l.off).map(|l| l.id.clone()).collect()
}

/// The leaves under any of `prefixes` (an id or the start of one, "apps/wave"), `off` ones included.
pub fn ticks_for(tree: &Node, prefixes: &[String]) -> HashSet<String> {
    tree.leaves().into_iter().filter(|l| prefixes.iter().any(|p| l.id == *p || l.id.starts_with(&format!("{p}/")))).map(|l| l.id.clone()).collect()
}

/// (ticked leaves, all leaves) under `n`.
pub fn count(n: &Node, ticks: &HashSet<String>) -> (usize, usize) {
    let l = n.leaves();
    (l.iter().filter(|l| ticks.contains(&l.id)).count(), l.len())
}

/// Ticks every leaf under `n`, or unticks them all when they already are.
pub fn toggle(n: &Node, ticks: &mut HashSet<String>) {
    let (on, all) = count(n, ticks);
    for l in n.leaves() {
        if on == all { ticks.remove(&l.id) } else { ticks.insert(l.id.clone()) };
    }
}

pub fn ticked_size(tree: &Node, ticks: &HashSet<String>) -> u64 {
    tree.leaves().iter().filter(|l| ticks.contains(&l.id)).map(|l| l.size).sum()
}

pub fn offer(home: &Path, tree: &Node, ticks: &HashSet<String>) -> Offer {
    Offer { home: home.display().to_string(), skip_rebuildable: true, tree: tree.pruned(ticks).unwrap_or_default() }
}

/// The offered leaves the receiver picked; a pick that wasn't offered is ignored.
pub fn items(offer: &Offer, picks: &Picks) -> Vec<Node> {
    let keep: HashSet<&String> = picks.ids.iter().collect();
    offer.tree.leaves().into_iter().filter(|l| keep.contains(&l.id)).cloned().collect()
}

/// Where the receiver keeps a transfer: ~/MacySync/<time>_<sender>_<receiver>.
pub fn local_dir(home: &Path, transfer: &str) -> PathBuf {
    home.join("MacySync").join(transfer.rsplit('/').next().unwrap_or(transfer))
}

// The two sides ------------------------------------------------------------------------------------------

/// Sender, after the picks arrive: listens, publishes the stream key to the receiver and sends the picks until
/// the receiver has all of them.
pub fn serve(b: &Bus, transfer: &str, receiver: &Device, offer: &Offer, picks: &Picks, home: &Path, live: Shared) -> Result<(), String> {
    let r = serving(b, transfer, receiver, offer, picks, home, live.clone());
    let mut l = live.lock().unwrap();
    match &r {
        Ok(()) => {
            l.state = "done".into();
            l.done = true;
        }
        Err(e) => {
            l.state = "failed".into();
            l.failed = true;
            l.note = e.clone();
        }
    }
    r
}

fn serving(b: &Bus, transfer: &str, receiver: &Device, offer: &Offer, picks: &Picks, home: &Path, live: Shared) -> Result<(), String> {
    let to = receiver.key_bytes().ok_or("the other Mac has no key on the bus")?;
    let items = items(offer, picks);
    let (listener, port) = wire::listen().map_err(|e| e.to_string())?;
    let key = seal::random::<32>();
    let ep = Endpoint { key: key.to_vec(), port, addrs: wire::addresses() };
    {
        let mut l = live.lock().unwrap();
        l.state = "sending".into();
        l.started = bus::now();
        l.total = items.iter().map(|i| i.size).sum();
        l.items = items.len();
    }
    put_sealed(b, &format!("{transfer}/key.sealed"), &to, &ep, "stream key")?;
    wire::serve(listener, key, items, home, offer.skip_rebuildable, move |p| {
        let mut l = live.lock().unwrap();
        l.bytes = p.bytes;
        l.items_done = p.items_done;
        l.current = p.current.clone();
        l.network = p.network.clone();
    })
}

/// Receiver, once the stream key is there: fetches every pick (asking again after a cut), lets the sender
/// go, puts the items in place and writes AGENTS.md and GOAL.md into the transfer's folder.
pub fn receive(ep: &Endpoint, offer: &Offer, picks: &Picks, dir: &Path, from: &str, home: &Path, live: Shared) -> Result<(), String> {
    let r = receiving(ep, offer, picks, dir, from, home, live.clone());
    if let Err(e) = &r {
        let mut l = live.lock().unwrap();
        l.state = "failed".into();
        l.failed = true;
        l.note = e.clone();
    }
    r
}

fn receiving(ep: &Endpoint, offer: &Offer, picks: &Picks, dir: &Path, from: &str, home: &Path, live: Shared) -> Result<(), String> {
    let items = items(offer, picks);
    let staging = dir.join("staging");
    std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let _ = std::fs::write(dir.join("offer.json"), serde_json::to_vec_pretty(offer).unwrap());
    let _ = std::fs::write(dir.join("picks.json"), serde_json::to_vec_pretty(picks).unwrap());
    let landed = |id: &str| staging.join(format!("{}.done", wire::safe(id))).exists();
    {
        let mut l = live.lock().unwrap();
        l.state = "receiving".into();
        l.started = bus::now();
        l.total = items.iter().map(|i| i.size).sum();
        // Picked up after a stop: what already arrived counts.
        l.bytes = items.iter().filter(|i| landed(&i.id)).map(|i| i.size).sum();
        l.items = items.len();
        l.dir = Some(dir.to_path_buf());
    }
    let names: std::collections::HashMap<String, String> = items.iter().map(|i| (i.id.clone(), i.name.clone())).collect();
    let mut tries = 0;
    loop {
        let need: Vec<String> = items.iter().filter(|i| !landed(&i.id)).map(|i| i.id.clone()).collect();
        {
            let mut l = live.lock().unwrap();
            l.items_done = items.len() - need.len();
        }
        if need.is_empty() {
            break;
        }
        // Bytes so far: what earlier items took, plus the item in flight.
        let base = Mutex::new((String::new(), 0u64, live.lock().unwrap().bytes));
        let l2 = live.clone();
        let r = wire::fetch(ep, need, &staging, |id, got| {
            let mut b = base.lock().unwrap();
            let mut l = l2.lock().unwrap();
            if b.0 != id {
                if !b.0.is_empty() {
                    l.items_done += 1;
                }
                b.2 += b.1;
                b.0 = id.to_string();
            }
            b.1 = got;
            l.bytes = b.2 + got;
            l.current = names.get(id).cloned().unwrap_or_else(|| id.to_string());
        });
        if let Err(e) = r {
            tries += 1;
            live.lock().unwrap().note = e.clone();
            if tries > 20 {
                return Err(e);
            }
            std::thread::sleep(Duration::from_secs(3));
        }
    }
    // Nothing left to ask for: the sender stops listening.
    let _ = wire::fetch(ep, vec![], &staging, |_, _| {});
    live.lock().unwrap().state = "placing".into();
    let placed = setup::place(&staging, dir, &offer.home, home, &items);
    setup::write_brief(dir, from, &offer.home, home, &placed).map_err(|e| e.to_string())?;
    let mut l = live.lock().unwrap();
    l.state = "placed".into();
    l.placed = placed.iter().filter(|p| p.problems.is_empty()).count();
    l.problems = placed.iter().map(|p| p.problems.len()).sum();
    l.done = true;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> Node {
        let leaf = |id: &str, size: u64, off: bool| Node { id: id.into(), name: id.into(), size, off, paths: vec![format!("~/{id}")], ..Node::default() };
        Node { children: vec![
            Node { id: "apps".into(), children: vec![
                Node { id: "apps/wave".into(), children: vec![leaf("apps/wave/app", 10, false), leaf("apps/wave/data", 5, false)], ..Node::default() },
                Node { id: "apps/tailscale".into(), children: vec![leaf("apps/tailscale/app", 7, true)], ..Node::default() },
            ], ..Node::default() },
            Node { id: "secrets".into(), children: vec![leaf("secrets/keychain", 1, false)], ..Node::default() },
        ], ..Node::default() }
    }

    #[test]
    fn ticks_follow_the_tree() {
        let t = tree();
        let mut ticks = default_ticks(&t);
        assert_eq!(ticks.len(), 3, "the identity-bound app starts unticked");
        let apps = t.find("apps").unwrap();
        assert_eq!(count(apps, &ticks), (2, 3));
        toggle(apps, &mut ticks);
        assert_eq!(count(apps, &ticks), (3, 3), "a partly ticked group ticks all");
        toggle(apps, &mut ticks);
        assert_eq!(count(apps, &ticks), (0, 3), "a fully ticked group unticks all");
        let only = ticks_for(&t, &["apps/wave".into(), "secrets/keychain".into()]);
        assert_eq!(only.len(), 3);
        assert!(!ticks_for(&t, &["apps/wav".into()]).contains("apps/wave/app"), "a prefix means whole id segments");
    }

    #[test]
    fn only_offered_items_can_be_picked() {
        let t = tree();
        let o = offer(Path::new("/Users/old"), &t, &ticks_for(&t, &["apps/wave".into()]));
        assert_eq!(o.tree.leaves().len(), 2);
        let p = Picks { ids: vec!["apps/wave/data".into(), "secrets/keychain".into()] };
        let got: Vec<String> = items(&o, &p).into_iter().map(|n| n.id).collect();
        assert_eq!(got, vec!["apps/wave/data".to_string()], "the keychain wasn't offered");
    }

    #[test]
    fn sealed_documents_open_only_for_their_mac() {
        let secret = seal::random::<32>();
        let other = seal::random::<32>();
        let p = Picks { ids: vec!["a".into()] };
        let sealed = seal::seal(&seal::public(&secret), &serde_json::to_vec(&p).unwrap());
        assert_eq!(serde_json::from_slice::<Picks>(&seal::open(&secret, &sealed).unwrap()).unwrap(), p);
        assert!(seal::open(&other, &sealed).is_err());
    }
}

