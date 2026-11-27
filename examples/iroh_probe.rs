//! A probe for moving MacySync's tunnel to iroh: one Mac listens and prints a ticket, the other sends a file to
//! it, and both print the speed and whether the connection went direct or through a relay.
//!
//!   cargo run --release --example iroh_probe -- listen [--local] [--out DIR]
//!   cargo run --release --example iroh_probe -- send TICKET FILE [--local]
//!
//! `--local` turns relays and address lookup off, so nothing leaves this network (for a test on one Mac).
//! Without it the endpoint uses n0's public relays and DNS lookup, as the real app would.

use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use iroh::endpoint::{Connection, PathEvent, presets};
use iroh::{Endpoint, EndpointAddr, RelayMode};
use n0_future::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const ALPN: &[u8] = b"macysync/probe/0";

type R<T> = Result<T, String>;

fn e<E: std::fmt::Display>(err: E) -> String {
    err.to_string()
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let local = args.iter().any(|a| a == "--local");
    let opt = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let pos: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).filter(|a| Some(*a) != opt("--out").as_ref()).collect();
    let res = match pos.first().map(|s| s.as_str()) {
        Some("listen") => listen(local, opt("--out").unwrap_or_else(|| ".".into())).await,
        Some("send") if pos.len() >= 3 => send(local, pos[1], pos[2]).await,
        _ => Err("usage: iroh_probe listen [--local] [--out DIR] | iroh_probe send TICKET FILE [--local]".into()),
    };
    if let Err(err) = res {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

async fn endpoint(local: bool, accept: bool) -> R<Endpoint> {
    let mut b = if local { Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled) } else { Endpoint::builder(presets::N0) };
    if accept {
        b = b.alpns(vec![ALPN.to_vec()]);
    }
    let ep = b.bind().await.map_err(e)?;
    if local {
        // No relay to wait for: give the sockets a moment to learn this Mac's addresses.
        tokio::time::sleep(Duration::from_millis(500)).await;
    } else {
        tokio::time::timeout(Duration::from_secs(20), ep.online()).await.map_err(|_| "no relay answered in 20 s".to_string())?;
    }
    Ok(ep)
}

/// Prints the path the connection uses now and each time it changes (relay first, then direct once a hole is punched).
fn watch_paths(conn: Connection, t0: Instant) {
    let show = move |conn: &Connection| {
        match conn.paths().iter().find(|p| p.is_selected()) {
            Some(p) => {
                let kind = if p.remote_addr().is_relay() { "RELAY" } else { "DIRECT" };
                println!("[{:>6.2}s] path {kind} {:?} rtt {:?}", t0.elapsed().as_secs_f32(), p.remote_addr(), p.rtt());
            }
            None => println!("[{:>6.2}s] path none selected", t0.elapsed().as_secs_f32()),
        }
    };
    tokio::spawn(async move {
        show(&conn);
        let mut events = conn.path_events();
        while let Some(ev) = events.next().await {
            if matches!(ev, PathEvent::Selected { .. }) {
                show(&conn);
            }
        }
    });
}

fn mbps(bytes: u64, d: Duration) -> String {
    format!("{:.1} MB/s", bytes as f64 / 1e6 / d.as_secs_f64().max(1e-6))
}

async fn listen(local: bool, out: String) -> R<()> {
    let ep = endpoint(local, true).await?;
    let ticket = B64.encode(serde_json::to_vec(&ep.addr()).map_err(e)?);
    println!("id {}", ep.id());
    println!("TICKET {ticket}");
    let incoming = ep.accept().await.ok_or("endpoint closed")?;
    let conn = incoming.await.map_err(e)?;
    let t0 = Instant::now();
    println!("connected from {}", conn.remote_id());
    watch_paths(conn.clone(), t0);
    let (mut send, mut recv) = conn.accept_bi().await.map_err(e)?;
    let len = recv.read_u64().await.map_err(e)?;
    let name_len = recv.read_u16().await.map_err(e)? as usize;
    let mut name = vec![0; name_len];
    recv.read_exact(&mut name).await.map_err(e)?;
    let name = String::from_utf8_lossy(&name).replace(['/', '\\'], "_");
    let path = std::path::Path::new(&out).join(format!("probe-{name}"));
    let mut file = tokio::fs::File::create(&path).await.map_err(e)?;
    let start = Instant::now();
    let got = tokio::io::copy(&mut (&mut recv).take(len), &mut file).await.map_err(e)?;
    file.flush().await.map_err(e)?;
    let took = start.elapsed();
    send.write_u64(got).await.map_err(e)?;
    send.finish().map_err(e)?;
    println!("received {} of {} bytes into {} in {:.1}s, {}", got, len, path.display(), took.as_secs_f32(), mbps(got, took));
    conn.closed().await;
    ep.close().await;
    Ok(())
}

async fn send(local: bool, ticket: &str, file: &str) -> R<()> {
    let addr: EndpointAddr = serde_json::from_slice(&B64.decode(ticket.trim()).map_err(e)?).map_err(e)?;
    let ep = endpoint(local, false).await?;
    let t0 = Instant::now();
    let conn = ep.connect(addr, ALPN).await.map_err(e)?;
    println!("connected in {:.2}s", t0.elapsed().as_secs_f32());
    watch_paths(conn.clone(), t0);
    let (mut send, mut recv) = conn.open_bi().await.map_err(e)?;
    let mut f = tokio::fs::File::open(file).await.map_err(e)?;
    let len = f.metadata().await.map_err(e)?.len();
    let name = std::path::Path::new(file).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    send.write_u64(len).await.map_err(e)?;
    send.write_u16(name.len() as u16).await.map_err(e)?;
    send.write_all(name.as_bytes()).await.map_err(e)?;
    let start = Instant::now();
    tokio::io::copy(&mut f, &mut send).await.map_err(e)?;
    send.finish().map_err(e)?;
    let acked = recv.read_u64().await.map_err(e)?;
    let took = start.elapsed();
    println!("sent {} bytes, receiver has {} ({}), in {:.1}s, {}", len, acked, if acked == len { "complete" } else { "SHORT" }, took.as_secs_f32(), mbps(len, took));
    conn.close(0u32.into(), b"done");
    ep.close().await;
    Ok(())
}
