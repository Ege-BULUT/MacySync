//! Files go straight from Mac to Mac over TCP (same network or Tailscale), inside a `seal` stream under the
//! transfer's key; GitHub only carries the key, sealed. One connection carries every item still missing:
//!
//!   receiver -> sender   sealed stream: {"need": [item ids]}
//!   sender -> receiver   sealed stream: per item, [u32 len][header json] then [u32 len][tar bytes]… [u32 0]
//!
//! Each item is a tar of its paths taken from `/`, unpacked by the receiver into its own staging folder. A cut
//! connection loses only the item in flight: the receiver asks again for what it hasn't finished.

use std::io::{self, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::scan::{Node, REBUILDABLE};
use crate::seal;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Endpoint {
    pub key: Vec<u8>,
    pub port: u16,
    pub addrs: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct Hello {
    need: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct Header {
    id: String,
    size: u64,
}

/// This Mac's IPv4 addresses, Tailscale's (100.x) last so a shared network is tried first.
pub fn addresses() -> Vec<String> {
    let out = Command::new("ifconfig").output().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
    let mut v: Vec<String> = out.lines().filter_map(|l| l.trim().strip_prefix("inet ")).filter_map(|l| l.split_whitespace().next()).filter(|a| !a.starts_with("127.")).map(String::from).collect();
    v.extend(crate::tail::ip());
    v.sort_by_key(|a| a.starts_with("100."));
    v.dedup();
    v
}

fn tailnet(ip: std::net::IpAddr) -> bool {
    matches!(ip, std::net::IpAddr::V4(ip) if ip.octets()[0] == 100 && (64..128).contains(&ip.octets()[1]))
}

/// MacySync's own Tailscale node hands its incoming connections over on 127.0.0.1.
pub fn network_of(addr: &SocketAddr) -> &'static str {
    if tailnet(addr.ip()) || addr.ip().is_loopback() { "Tailscale" } else { "LAN" }
}

fn put_u32(w: &mut impl Write, n: u32) -> io::Result<()> {
    w.write_all(&n.to_be_bytes())
}

fn get_u32(r: &mut impl Read) -> io::Result<Option<u32>> {
    let mut b = [0u8; 4];
    match r.read_exact(&mut b) {
        Ok(()) => Ok(Some(u32::from_be_bytes(b))),
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(None),
        Err(e) => Err(e),
    }
}

/// What the sender has done so far.
#[derive(Clone, Debug, Default)]
pub struct Progress {
    pub bytes: u64,
    pub items_done: usize,
    pub current: String,
    pub network: String,
    pub finished: bool,
}

pub fn listen() -> io::Result<(TcpListener, u16)> {
    let l = TcpListener::bind("0.0.0.0:0")?;
    let port = l.local_addr()?.port();
    Ok((l, port))
}

/// Sends `items` (leaves of the offer) to whoever connects with the key, until the receiver needs nothing.
pub fn serve(listener: TcpListener, key: [u8; 32], items: Vec<Node>, home: &Path, skip_rebuildable: bool, on: impl Fn(&Progress)) -> Result<(), String> {
    let mut p = Progress::default();
    let mut sent: std::collections::HashSet<String> = Default::default();
    loop {
        let (stream, peer) = listener.accept().map_err(|e| e.to_string())?;
        stream.set_nodelay(true).ok();
        p.network = network_of(&peer).into();
        let Ok(mut hello_in) = seal::Reader::new(&key, stream.try_clone().map_err(|e| e.to_string())?) else { continue };
        let mut raw = Vec::new();
        // A connection that can't prove it holds the key is dropped, and the sender waits for the next.
        if hello_in.read_to_end(&mut raw).is_err() {
            continue;
        }
        let Ok(hello) = serde_json::from_slice::<Hello>(&raw) else { continue };
        if hello.need.is_empty() {
            p.finished = true;
            on(&p);
            return Ok(());
        }
        let mut out = seal::Writer::new(&key, io::BufWriter::with_capacity(1 << 20, stream)).map_err(|e| e.to_string())?;
        let result: io::Result<()> = (|| {
            for item in items.iter().filter(|i| hello.need.contains(&i.id)) {
                p.current = item.name.clone();
                on(&p);
                let head = serde_json::to_vec(&Header { id: item.id.clone(), size: item.size }).unwrap();
                put_u32(&mut out, head.len() as u32)?;
                out.write_all(&head)?;
                let mut tar = tar_item(item, home, skip_rebuildable)?;
                let mut stdout = tar.stdout.take().unwrap();
                let mut buf = vec![0u8; 1 << 20];
                let mut last = Instant::now();
                loop {
                    let n = stdout.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    put_u32(&mut out, n as u32)?;
                    out.write_all(&buf[..n])?;
                    p.bytes += n as u64;
                    if last.elapsed() > Duration::from_millis(500) {
                        on(&p);
                        last = Instant::now();
                    }
                }
                tar.wait()?;
                put_u32(&mut out, 0)?;
                if sent.insert(item.id.clone()) {
                    p.items_done += 1;
                }
                on(&p);
            }
            out.finish()?;
            Ok(())
        })();
        // A cut connection: wait for the receiver to come back and ask for what is left.
        if let Err(e) = result {
            p.current = format!("connection lost ({e}); waiting for the other Mac");
            on(&p);
        }
    }
}

/// tar of an item's paths, relative to `/` so the receiver can map the old home folder onto its own. A
/// generated item runs its command first and sends the output as `macysync-generated/<file>`.
fn tar_item(item: &Node, home: &Path, skip_rebuildable: bool) -> io::Result<std::process::Child> {
    let mut c = Command::new("tar");
    c.arg("-cf").arg("-");
    if skip_rebuildable {
        for r in REBUILDABLE {
            c.args(["--exclude", r]);
        }
    }
    if !item.command.is_empty() {
        let dir = std::env::temp_dir().join(format!("macysync-gen-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("macysync-generated"))?;
        std::fs::write(dir.join("macysync-generated").join(&item.file), run_for(&item.command, Duration::from_secs(90)))?;
        c.arg("-C").arg(&dir).arg(format!("macysync-generated/{}", item.file));
    } else {
        c.args(["-C", "/"]);
        let rel: Vec<String> = item.paths.iter().map(|p| expand(home, p)).filter(|p| p.exists() || p.is_symlink()).map(|p| p.to_string_lossy().trim_start_matches('/').to_string()).collect();
        if rel.is_empty() {
            c.args(["-T", "/dev/null"]);
        }
        c.args(rel);
    }
    c.stdout(Stdio::piped()).stderr(Stdio::null()).spawn()
}

/// A command's output, or what it wrote before `limit`: a command that waits on a dialog nobody answers (a
/// keychain read) must not hold the whole transfer.
fn run_for(command: &str, limit: Duration) -> Vec<u8> {
    let out = std::env::temp_dir().join(format!("macysync-run-{}-{}", std::process::id(), seal::random::<4>().iter().map(|b| format!("{b:02x}")).collect::<String>()));
    let Ok(file) = std::fs::File::create(&out) else { return vec![] };
    let Ok(mut child) = Command::new("/bin/zsh").args(["-lc", command]).stdout(file).stderr(Stdio::null()).spawn() else { return vec![] };
    let start = Instant::now();
    while child.try_wait().ok().flatten().is_none() {
        if start.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let bytes = std::fs::read(&out).unwrap_or_default();
    let _ = std::fs::remove_file(&out);
    bytes
}

pub fn expand(home: &Path, p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => PathBuf::from(p),
    }
}

/// Connects to the sender and unpacks each needed item into `staging/<item>`; `done` lists items already
/// unpacked (they are skipped and grow as items finish). Returns when the sender has sent all it was asked.
pub fn fetch(ep: &Endpoint, need: Vec<String>, staging: &Path, on: impl Fn(&str, u64)) -> Result<Vec<String>, String> {
    let key: [u8; 32] = ep.key.clone().try_into().map_err(|_| "bad transfer key")?;
    let mut last_err = String::from("no address to try");
    let stream = ep.addrs.iter().find_map(|a| {
        let addr = (a.as_str(), ep.port).to_socket_addrs().ok()?.next()?;
        let direct = TcpStream::connect_timeout(&addr, Duration::from_secs(4));
        // A tailnet address this Mac has no route to goes through MacySync's own node.
        let r = if direct.is_err() && tailnet(addr.ip()) { crate::tail::connect(addr, Duration::from_secs(4)) } else { direct };
        r.map_err(|e| last_err = format!("{a}: {e}")).ok()
    });
    let stream = stream.ok_or_else(|| format!("can't reach the other Mac ({last_err})"))?;
    stream.set_nodelay(true).ok();
    let mut hello = seal::Writer::new(&key, stream.try_clone().map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    hello.write_all(&serde_json::to_vec(&Hello { need: need.clone() }).unwrap()).map_err(|e| e.to_string())?;
    hello.finish().map_err(|e| e.to_string())?;
    if need.is_empty() {
        return Ok(vec![]);
    }
    let mut inp = seal::Reader::new(&key, BufReader::with_capacity(1 << 20, stream)).map_err(|e| e.to_string())?;
    let mut done = Vec::new();
    while let Some(len) = get_u32(&mut inp).map_err(|e| e.to_string())? {
        let mut head = vec![0u8; len as usize];
        inp.read_exact(&mut head).map_err(|e| e.to_string())?;
        let h: Header = serde_json::from_slice(&head).map_err(|e| e.to_string())?;
        let dest = staging.join(safe(&h.id));
        let _ = std::fs::remove_dir_all(&dest);
        std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
        let mut tar = Command::new("tar").args(["-xpf", "-", "-C"]).arg(&dest).stdin(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
        let mut stdin = tar.stdin.take().unwrap();
        let mut buf = vec![0u8; 1 << 20];
        let mut got = 0u64;
        loop {
            let n = get_u32(&mut inp).map_err(|e| e.to_string())?.ok_or("the transfer was cut")? as usize;
            if n == 0 {
                break;
            }
            if n > buf.len() {
                return Err("bad chunk".into());
            }
            inp.read_exact(&mut buf[..n]).map_err(|e| e.to_string())?;
            stdin.write_all(&buf[..n]).map_err(|e| format!("unpacking {}: {e}", h.id))?;
            got += n as u64;
            on(&h.id, got);
        }
        drop(stdin);
        let o = tar.wait_with_output().map_err(|e| e.to_string())?;
        if !o.status.success() {
            // Some files can't be written (macOS-protected ones): the item still counts; the log says which.
            let _ = std::fs::write(dest.with_extension("tar-errors.txt"), &o.stderr);
        }
        std::fs::write(dest.with_extension("done"), b"").map_err(|e| e.to_string())?;
        done.push(h.id);
    }
    Ok(done)
}

/// An item id as one folder name.
pub fn safe(id: &str) -> String {
    id.replace('/', "__")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real round trip over localhost: a folder and a generated file arrive, a wrong key gets nothing.
    #[test]
    fn items_cross_and_unpack() {
        let base = std::env::temp_dir().join(format!("macysync-wire-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let home = base.join("home");
        std::fs::create_dir_all(home.join(".config/karabiner")).unwrap();
        std::fs::write(home.join(".config/karabiner/karabiner.json"), b"{\"profiles\":[]}").unwrap();
        std::fs::create_dir_all(home.join("proj/node_modules")).unwrap();
        std::fs::write(home.join("proj/main.rs"), b"fn main() {}").unwrap();
        std::fs::write(home.join("proj/node_modules/x.js"), b"junk").unwrap();
        let items = vec![
            Node { id: "apps/karabiner/config".into(), name: "Config".into(), paths: vec!["~/.config/karabiner".into()], ..Node::default() },
            Node { id: "home/proj".into(), name: "proj".into(), paths: vec!["~/proj".into()], ..Node::default() },
            Node { id: "tools/echo".into(), name: "echo".into(), command: "echo brew \\\"jq\\\"".into(), file: "Brewfile".into(), ..Node::default() },
        ];
        let key = seal::random::<32>();
        let (l, port) = listen().unwrap();
        let h2 = home.clone();
        let server = std::thread::spawn(move || serve(l, key, items, &h2, true, |_| {}));
        let ep = Endpoint { key: key.to_vec(), port, addrs: vec!["127.0.0.1".into()] };
        let staging = base.join("staging");

        // Someone without the key: refused, and the sender keeps waiting.
        let wrong = Endpoint { key: seal::random::<32>().to_vec(), ..ep.clone() };
        assert!(fetch(&wrong, vec!["home/proj".into()], &staging, |_, _| {}).is_err());

        let done = fetch(&ep, vec!["apps/karabiner/config".into(), "home/proj".into(), "tools/echo".into()], &staging, |_, _| {}).unwrap();
        assert_eq!(done.len(), 3);
        let h = home.to_string_lossy().trim_start_matches('/').to_string();
        let k = staging.join(safe("apps/karabiner/config")).join(&h).join(".config/karabiner/karabiner.json");
        assert_eq!(std::fs::read(k).unwrap(), b"{\"profiles\":[]}");
        let p = staging.join(safe("home/proj")).join(&h).join("proj");
        assert!(p.join("main.rs").exists());
        assert!(!p.join("node_modules").exists(), "rebuildable folders stay behind");
        let g = std::fs::read_to_string(staging.join(safe("tools/echo")).join("macysync-generated/Brewfile")).unwrap();
        assert_eq!(g.trim(), "brew \"jq\"");

        // Nothing left to ask for: the sender ends.
        fetch(&ep, vec![], &staging, |_, _| {}).unwrap();
        server.join().unwrap().unwrap();
        let _ = std::fs::remove_dir_all(&base);
    }
}
