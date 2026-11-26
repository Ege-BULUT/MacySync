//! This Mac: its id, name, look and keys, kept in ~/.config/macysync (MACYSYNC_HOME for tests and demos).

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::seal;

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

pub fn dir() -> PathBuf {
    std::env::var_os("MACYSYNC_HOME").map(PathBuf::from).unwrap_or_else(|| home_dir().join(".config/macysync"))
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Look {
    /// A model id from the catalog (`look::catalog()`), and one of that model's colour names.
    pub model: String,
    pub color: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    pub id: String,
    pub name: String,
    pub look: Look,
    /// owner/repo of the private bus repo.
    pub bus: String,
    /// The command the receiver runs to set things up; `{dir}` is the transfer folder with AGENTS.md and GOAL.md.
    pub agent: String,
    pub registered: bool,
    /// "en" (the default) or "tr".
    pub lang: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            id: String::new(),
            name: String::new(),
            look: Look::default(),
            bus: String::new(),
            agent: crate::setup::AGENTS[0].1.into(),
            registered: false,
            lang: "en".into(),
        }
    }
}

impl Config {
    /// The saved config; a first run makes the id, takes the computer name and the detected look.
    pub fn load() -> Config {
        let mut c: Config = fs::read(dir().join("config.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        if c.id.is_empty() {
            c.id = new_id();
            c.name = computer_name();
            let hw = detect();
            c.look = crate::look::guess(&hw);
            c.save();
        }
        c
    }

    pub fn save(&self) {
        let _ = fs::create_dir_all(dir());
        let _ = fs::write(dir().join("config.json"), serde_json::to_vec_pretty(self).unwrap());
    }
}

/// Eight hex digits, random; it never changes after the first run.
pub fn new_id() -> String {
    seal::random::<4>().iter().map(|b| format!("{b:02x}")).collect()
}

fn run(cmd: &str, args: &[&str]) -> String {
    Command::new(cmd).args(args).output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
}

pub fn computer_name() -> String {
    let n = run("scutil", &["--get", "ComputerName"]);
    if n.is_empty() { run("hostname", &["-s"]) } else { n }
}

/// What the hardware says about itself: `hw.model`, the part number's first five characters (they name the
/// colour), the chip, memory and the startup disk's size.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Hardware {
    pub model_id: String,
    pub part: String,
    pub chip: String,
    pub ram_gb: u64,
    pub disk_gb: u64,
}

pub fn detect() -> Hardware {
    let io = run("ioreg", &["-c", "IOPlatformExpertDevice", "-d", "2"]);
    Hardware {
        model_id: ioreg_field(&io, "model").unwrap_or_else(|| run("sysctl", &["-n", "hw.model"])),
        part: ioreg_field(&io, "model-number").unwrap_or_default(),
        chip: run("sysctl", &["-n", "machdep.cpu.brand_string"]),
        ram_gb: run("sysctl", &["-n", "hw.memsize"]).parse::<u64>().unwrap_or(0) >> 30,
        disk_gb: run("df", &["-k", "/"]).lines().nth(1).and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok()).unwrap_or(0) / 1_000_000,
    }
}

/// `"model" = <"MacBookPro17,1">` gives the text, `"model-number" = <4d59443832000000…>` the hex bytes up to the first 0.
fn ioreg_field(io: &str, key: &str) -> Option<String> {
    let line = io.lines().find(|l| l.trim_start_matches([' ', '|']).starts_with(&format!("\"{key}\" = <")))?;
    let v = line.split_once("= <")?.1.trim_end_matches('>');
    if let Some(t) = v.strip_prefix('"') {
        return Some(t.trim_end_matches('"').to_string());
    }
    let bytes: Vec<u8> = (0..v.len() / 2).filter_map(|i| u8::from_str_radix(&v[2 * i..2 * i + 2], 16).ok()).take_while(|b| *b != 0).collect();
    String::from_utf8(bytes).ok().filter(|s| !s.is_empty())
}

/// The device key pair, made on first use. The secret stays in a file only this user can read.
pub fn keys() -> ([u8; 32], [u8; 32]) {
    let path = dir().join("device.key");
    let secret: [u8; 32] = match fs::read(&path).ok().and_then(|b| b.try_into().ok()) {
        Some(s) => s,
        None => {
            let s = seal::random::<32>();
            let _ = fs::create_dir_all(dir());
            let _ = fs::write(&path, s);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
            }
            s
        }
    };
    (secret, seal::public(&secret))
}

const KEYRING: &str = "MacySync";

/// MACYSYNC_TOKEN, then the token saved at sign-in, then the GitHub CLI's.
pub fn token() -> Option<String> {
    if let Ok(t) = std::env::var("MACYSYNC_TOKEN") {
        return Some(t).filter(|t| !t.is_empty());
    }
    keyring::Entry::new(KEYRING, "github").ok().and_then(|e| e.get_password().ok()).or_else(|| {
        let t = run("gh", &["auth", "token"]);
        (!t.is_empty()).then_some(t)
    })
}

pub fn save_token(t: &str) -> Result<(), String> {
    keyring::Entry::new(KEYRING, "github").and_then(|e| e.set_password(t)).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ioreg_text_and_hex_fields() {
        let io = "    |   \"model\" = <\"MacBookPro17,1\">\n    |   \"model-number\" = <4d59443832000000000000>\n";
        assert_eq!(ioreg_field(io, "model").as_deref(), Some("MacBookPro17,1"));
        assert_eq!(ioreg_field(io, "model-number").as_deref(), Some("MYD82"));
        assert_eq!(ioreg_field(io, "target-type"), None);
    }
}
