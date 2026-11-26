//! MacySync's own Tailscale node, for two Macs the network keeps apart (an office or guest Wi-Fi that isolates
//! its devices). tailscaled runs in userspace mode with its state, socket and log in ~/.config/macysync/tailscale:
//! no network interface, no DNS or route changes, nothing shared with a Tailscale already on the Mac. Incoming
//! connections to its address arrive on 127.0.0.1; outgoing ones go through its SOCKS5 port.
//!
//! `tailscaled` and `tailscale` (BSD-3-Clause, tailscale.com) sit next to the macysync binary.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

fn dir() -> PathBuf {
    crate::me::dir().join("tailscale")
}

fn tool(name: &str) -> Option<PathBuf> {
    let p = std::env::current_exe().ok()?.parent()?.join(name);
    p.exists().then_some(p)
}

fn socket() -> PathBuf {
    dir().join("tailscaled.sock")
}

fn socks_port() -> Option<u16> {
    std::fs::read_to_string(dir().join("socks.port")).ok()?.trim().parse().ok()
}

fn cli(args: &[&str]) -> Result<String, String> {
    let t = tool("tailscale").ok_or("tailscale isn't next to macysync")?;
    let o = Command::new(t).arg("--socket").arg(socket()).args(args).stdin(Stdio::null()).output().map_err(|e| e.to_string())?;
    if o.status.success() { Ok(String::from_utf8_lossy(&o.stdout).into()) } else { Err(String::from_utf8_lossy(&o.stderr).trim().into()) }
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Status {
    /// NoState, NeedsLogin, Starting, Running, Stopped
    pub state: String,
    pub auth_url: String,
    pub ip: String,
}

/// None when MacySync's node isn't running.
pub fn status() -> Option<Status> {
    let v: serde_json::Value = serde_json::from_str(&cli(&["status", "--json"]).ok()?).ok()?;
    let ip = v["Self"]["TailscaleIPs"].as_array().into_iter().flatten().filter_map(|i| i.as_str()).find(|i| i.contains('.')).unwrap_or_default();
    Some(Status { state: v["BackendState"].as_str().unwrap_or_default().into(), auth_url: v["AuthURL"].as_str().unwrap_or_default().into(), ip: ip.into() })
}

/// This node's address once it is signed in.
pub fn ip() -> Option<String> {
    status().filter(|s| s.state == "Running" && !s.ip.is_empty()).map(|s| s.ip)
}

/// Starts the node if it isn't running and asks it to come up as `macysync-<id>`. A new node then waits for a
/// sign-in: `status().auth_url` is the page to open.
pub fn start(id: &str) -> Result<(), String> {
    if status().is_none() {
        let d = tool("tailscaled").ok_or("tailscaled isn't next to macysync")?;
        std::fs::create_dir_all(dir().join("state")).map_err(|e| e.to_string())?;
        let port = TcpListener::bind("127.0.0.1:0").and_then(|l| l.local_addr()).map_err(|e| e.to_string())?.port();
        std::fs::write(dir().join("socks.port"), port.to_string()).map_err(|e| e.to_string())?;
        let log = std::fs::File::create(dir().join("tailscaled.log")).map_err(|e| e.to_string())?;
        Command::new(d)
            .arg("--tun=userspace-networking")
            .arg(format!("--statedir={}", dir().join("state").display()))
            .arg(format!("--socket={}", socket().display()))
            .arg("--port=0")
            .arg(format!("--socks5-server=127.0.0.1:{port}"))
            .stdin(Stdio::null())
            .stdout(log.try_clone().map_err(|e| e.to_string())?)
            .stderr(log)
            .spawn()
            .map_err(|e| e.to_string())?;
        for _ in 0..50 {
            if status().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    if status().is_some_and(|s| s.state != "Running") {
        // `up` waits for the sign-in, so it runs on its own; the auth URL shows up in status().
        let t = tool("tailscale").ok_or("tailscale isn't next to macysync")?;
        Command::new(t).arg("--socket").arg(socket()).args(["up", "--accept-dns=false", "--accept-routes=false", &format!("--hostname=macysync-{id}")])
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Stops MacySync's node; its sign-in stays for next time.
pub fn stop() {
    let _ = Command::new("pkill").args(["-f", "--"]).arg(format!("--socket={}", socket().display())).status();
}

/// A connection to a tailnet address through this node's SOCKS5 port.
pub fn connect(addr: SocketAddr, timeout: Duration) -> std::io::Result<TcpStream> {
    let port = socks_port().ok_or_else(|| std::io::Error::other("MacySync's Tailscale isn't running"))?;
    let mut s = TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), timeout)?;
    s.set_read_timeout(Some(Duration::from_secs(15)))?;
    s.write_all(&[5, 1, 0])?;
    let mut r = [0u8; 2];
    s.read_exact(&mut r)?;
    let SocketAddr::V4(a) = addr else { return Err(std::io::Error::other("IPv4 only")) };
    let mut req = vec![5, 1, 0, 1];
    req.extend(a.ip().octets());
    req.extend(a.port().to_be_bytes());
    s.write_all(&req)?;
    let mut head = [0u8; 10];
    s.read_exact(&mut head)?;
    if r != [5, 0] || head[1] != 0 {
        return Err(std::io::Error::other(format!("Tailscale couldn't reach {addr} (SOCKS reply {})", head[1])));
    }
    s.set_read_timeout(None)?;
    Ok(s)
}
