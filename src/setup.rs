//! The receiving side after the files land: put each item in place (keeping whatever was there), import
//! preference files, then write AGENTS.md and GOAL.md for the agent that finishes the job, and start it.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::scan::Node;
use crate::wire::{expand, safe};

/// (name, command). Plain shell commands the person can change; `{dir}` is the transfer folder.
pub const AGENTS: [(&str, &str); 5] = [
    ("Claude Code", "cd {dir} && claude \"Read AGENTS.md and GOAL.md in this folder and do what they say.\""),
    ("Codex", "cd {dir} && codex \"Read AGENTS.md and GOAL.md in this folder and do what they say.\""),
    ("OpenRouter", "cd {dir} && ANTHROPIC_BASE_URL=https://openrouter.ai/api ANTHROPIC_AUTH_TOKEN=$OPENROUTER_API_KEY claude \"Read AGENTS.md and GOAL.md in this folder and do what they say.\""),
    ("Ollama", "cd {dir} && ANTHROPIC_BASE_URL=http://localhost:11434 ANTHROPIC_AUTH_TOKEN=ollama claude --model qwen3-coder \"Read AGENTS.md and GOAL.md in this folder and do what they say.\""),
    ("Custom", "cd {dir} && "),
];

#[derive(Debug, Default)]
pub struct Placed {
    pub id: String,
    pub name: String,
    pub note: String,
    pub secret: bool,
    /// Where each path ended up.
    pub to: Vec<PathBuf>,
    /// What was already there, renamed aside.
    pub kept: Vec<PathBuf>,
    /// Preference domains imported with `defaults import`.
    pub imported: Vec<String>,
    /// Files made on the sender (a Brewfile…), now in the transfer folder.
    pub generated: Vec<PathBuf>,
    /// Apps' keys added to this Mac's login keychain, by service.
    pub keys: Vec<String>,
    pub problems: Vec<String>,
}

/// Moves every unpacked item from `staging` into place on this Mac. Paths under the old home folder go under
/// this one; something already at a destination is renamed to `<name>.before-macysync` first.
pub fn place(staging: &Path, dir: &Path, old_home: &str, home: &Path, items: &[Node]) -> Vec<Placed> {
    let old_home = Path::new(old_home);
    items.iter().map(|item| {
        let mut p = Placed { id: item.id.clone(), name: item.name.clone(), note: item.note.clone(), secret: item.secret, ..Placed::default() };
        let src_root = staging.join(safe(&item.id));
        if !item.command.is_empty() {
            let src = src_root.join("macysync-generated").join(&item.file);
            let dst = dir.join("generated").join(&item.file);
            let _ = std::fs::create_dir_all(dst.parent().unwrap());
            match std::fs::rename(&src, &dst) {
                Ok(()) if item.file.ends_with(".key.json") => match add_key(&dst, home) {
                    Ok(svc) => p.keys.push(svc),
                    Err(e) => p.problems.push(e),
                },
                Ok(()) => p.generated.push(dst),
                Err(e) => p.problems.push(format!("{}: {e}", item.file)),
            }
            return p;
        }
        for path in &item.paths {
            let src = src_root.join(expand(old_home, path).to_string_lossy().trim_start_matches('/'));
            if !src.exists() && !src.is_symlink() {
                continue;
            }
            let mut dst = expand(home, path);
            // The old login keychain sits next to this Mac's own, never over it.
            if item.id == "secrets/keychain" {
                dst = home.join("Library/Keychains/old-mac.keychain-db");
            }
            if dst.starts_with(home.join("Library/Preferences")) && dst.extension().is_some_and(|e| e == "plist") {
                let domain = dst.file_stem().unwrap().to_string_lossy().to_string();
                let mut c = Command::new("defaults");
                if domain == ".GlobalPreferences" { c.args(["import", "-g"]) } else { c.args(["import", &domain]) };
                match c.arg(&src).output() {
                    Ok(o) if o.status.success() => p.imported.push(domain),
                    Ok(o) => p.problems.push(format!("defaults import {domain}: {}", String::from_utf8_lossy(&o.stderr).trim())),
                    Err(e) => p.problems.push(format!("defaults import {domain}: {e}")),
                }
                continue;
            }
            if dst.exists() || dst.is_symlink() {
                let aside = PathBuf::from(format!("{}.before-macysync", dst.display()));
                let _ = std::fs::remove_dir_all(&aside);
                let _ = std::fs::remove_file(&aside);
                if let Err(e) = std::fs::rename(&dst, &aside) {
                    p.problems.push(format!("{}: couldn't move the existing one aside: {e}", dst.display()));
                    continue;
                }
                p.kept.push(aside);
            }
            let _ = std::fs::create_dir_all(dst.parent().unwrap());
            match std::fs::rename(&src, &dst) {
                Ok(()) => {
                    relink(&dst, old_home, home);
                    p.to.push(dst);
                }
                Err(e) => p.problems.push(format!("{}: {e}", dst.display())),
            }
        }
        p
    }).collect()
}

#[derive(serde::Deserialize)]
struct AppKey {
    service: String,
    account: String,
    app: String,
    key: String,
}

/// The `security -i` line that adds an app's key, the app (where it landed here) allowed to read it.
fn key_line(k: &AppKey, home: &Path) -> String {
    let q = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
    let hex: String = k.key.bytes().map(|b| format!("{b:02x}")).collect();
    format!("add-generic-password -U -s {} -a {} -T {} -X {hex}\n", q(&k.service), q(&k.account), q(&expand(home, &k.app).to_string_lossy()))
}

/// Adds the key in `file` (from the old Mac) to this Mac's login keychain, through stdin so it never shows in a
/// process list, then deletes the file. Returns the item's service.
fn add_key(file: &Path, home: &Path) -> Result<String, String> {
    let k: AppKey = serde_json::from_slice(&std::fs::read(file).map_err(|e| e.to_string())?).map_err(|e| format!("{}: {e}", file.display()))?;
    if k.key.is_empty() {
        let _ = std::fs::remove_file(file);
        return Err(format!("the old Mac didn't hand over '{}' (nobody allowed it there in time): add the old keychain (see Secrets) and unlock it with its password, or sign in to the app again", k.service));
    }
    let mut c = Command::new("security").arg("-i").stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    c.stdin.take().unwrap().write_all(key_line(&k, home).as_bytes()).map_err(|e| e.to_string())?;
    let o = c.wait_with_output().map_err(|e| e.to_string())?;
    let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
    if !o.status.success() || !err.is_empty() {
        return Err(format!("adding '{}' to the login keychain: {err}", k.service));
    }
    let _ = std::fs::remove_file(file);
    Ok(k.service)
}

/// Points links that named the old home folder (`~/.local/bin/claude -> /Users/old/.local/share/...`) at the
/// same place under this one, at `path` and anywhere under it.
fn relink(path: &Path, old_home: &Path, home: &Path) {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return };
    if meta.file_type().is_symlink() {
        if let Some(rest) = std::fs::read_link(path).ok().and_then(|t| t.strip_prefix(old_home).ok().map(Path::to_path_buf)) {
            let _ = std::fs::remove_file(path);
            let _ = std::os::unix::fs::symlink(home.join(rest), path);
        }
    } else if meta.is_dir() {
        for e in std::fs::read_dir(path).into_iter().flatten().flatten() {
            relink(&e.path(), old_home, home);
        }
    }
}

/// Installs what the moved items need before the agent starts: Apple's command line tools, Homebrew (for a
/// Brewfile; it runs in Terminal, so the installer can ask for the password) and Node (for npm globals and Claude
/// Code plugin hooks, which run `node`): with brew when it is here, else nodejs.org's LTS into ~/.local/node, no
/// sudo. Safe to run again: it only installs what is missing, and exits 1 when something is still missing.
const PREFLIGHT: &str = r#"#!/bin/sh
# Written by MacySync. Installs what the moved items need before the agent starts; safe to run again.
NEED_BREW=__BREW__
NEED_NODE=__NODE__
BREWS=${PREFLIGHT_BREWS-"/opt/homebrew/bin/brew /usr/local/bin/brew"}
ok=1
say() { printf '\033[1m[MacySync]\033[0m %s\n' "$1"; }
find_brew() { for b in $BREWS; do [ -x "$b" ] && { echo "$b"; return; }; done; }
add_line() { grep -qsF "$1" "$HOME/.zprofile" || { printf '\n%s\n' "$1" >> "$HOME/.zprofile"; say "Added to ~/.zprofile: $1"; }; }
# Node's LTS from nodejs.org into ~/.local/node, checked against its SHASUMS256, linked into ~/.local/bin. No sudo.
node_tarball() {
  arch=$(uname -m); [ "$arch" = x86_64 ] && arch=x64
  idx=$(curl -fsSL https://nodejs.org/dist/index.json) || return 1
  v=$(printf '%s' "$idx" | tr '{' '\n' | grep -v '"lts":false' | grep -m1 '"version"' | sed 's/.*"version":"\([^"]*\)".*/\1/')
  [ -n "$v" ] || return 1
  tar="node-$v-darwin-$arch.tar.gz"
  tmp=$(mktemp -d) || return 1
  curl -fsSL -o "$tmp/$tar" "https://nodejs.org/dist/$v/$tar" || return 1
  want=$(curl -fsSL "https://nodejs.org/dist/$v/SHASUMS256.txt" | awk -v f="$tar" '$2 == f { print $1 }')
  got=$(shasum -a 256 "$tmp/$tar" | awk '{ print $1 }')
  if [ -z "$want" ] || [ "$want" != "$got" ]; then say "Node's download did not match its checksum."; return 1; fi
  tar -xzf "$tmp/$tar" -C "$tmp" || return 1
  rm -rf "${HOME:?}/.local/node" && mkdir -p "$HOME/.local/bin" && mv "$tmp/node-$v-darwin-$arch" "$HOME/.local/node" || return 1
  for b in node npm npx; do ln -sf "$HOME/.local/node/bin/$b" "$HOME/.local/bin/$b"; done
  rm -rf "$tmp"
  add_line 'export PATH="$HOME/.local/bin:$PATH"'
  export PATH="$HOME/.local/bin:$PATH"
  say "Node $v is in ~/.local/node."
}
if ! xcode-select -p >/dev/null 2>&1; then
  say "Apple's command line tools are missing. A window opens to install them; run this again when it finishes:"
  say "  sh '$0'"
  xcode-select --install
  exit 1
fi
brew_bin=$(find_brew)
# Homebrew only for a Brewfile: Node alone comes from nodejs.org without a password.
if [ -z "$brew_bin" ] && [ "$NEED_BREW" = 1 ]; then
  say "Installing Homebrew (it asks for this Mac's password)."
  /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)" || say "Homebrew did not install."
  brew_bin=$(find_brew)
fi
if [ -n "$brew_bin" ]; then
  eval "$("$brew_bin" shellenv)"
  # Terminal shells read ~/.zprofile; without this line brew and what it installs are not on PATH.
  add_line "eval \"\$($brew_bin shellenv)\""
elif [ "$NEED_BREW" = 1 ]; then
  ok=0
fi
[ -x "$HOME/.local/node/bin/node" ] && export PATH="$HOME/.local/bin:$PATH"
if [ "$NEED_NODE" = 1 ] && ! command -v node >/dev/null 2>&1; then
  say "Installing Node."
  { [ -n "$brew_bin" ] && "$brew_bin" install node; } || node_tarball || { say "Node did not install; get it from nodejs.org."; ok=0; }
fi
if [ "$ok" = 1 ]; then say "Tools ready."; else say "Some tools are missing; the agent puts them in TODO-FOR-YOU.md."; exit 1; fi
"#;

/// What the placed items need on this Mac: (Homebrew, Node).
fn needs(placed: &[Placed], home: &Path) -> (bool, bool) {
    let files: Vec<&PathBuf> = placed.iter().flat_map(|p| &p.generated).collect();
    let named = |n: &str| files.iter().find(|f| f.file_name().is_some_and(|x| x == n));
    let brewfile = named("Brewfile");
    let brew_node = brewfile.and_then(|f| std::fs::read_to_string(f).ok()).is_some_and(|b| b.lines().any(|l| l.trim_start().starts_with("brew \"node")));
    let node = named("npm-globals.txt").is_some() || brew_node || home.join(".claude/plugins").is_dir();
    (brewfile.is_some(), node)
}

/// Writes preflight.sh into `dir` when the items need Homebrew or Node; returns whether it did.
pub fn write_preflight(dir: &Path, placed: &[Placed], home: &Path) -> std::io::Result<bool> {
    let (brew, node) = needs(placed, home);
    if !brew && !node {
        return Ok(false);
    }
    let s = PREFLIGHT.replace("__BREW__", if brew { "1" } else { "0" }).replace("__NODE__", if node { "1" } else { "0" });
    std::fs::write(dir.join("preflight.sh"), s)?;
    Ok(true)
}

const AGENTS_MD: &str = "# You are finishing a Mac migration

MacySync moved files from the old Mac to this one and put them in place. GOAL.md lists every item, where it
went and what is left to make it work. Your job is the part a file copy can't do.

Rules:
- Work through GOAL.md item by item. For each one: check it arrived, finish its setup, check it works.
- Never delete a `*.before-macysync` copy; it is what this Mac had before. Never delete the transfer folder.
- Secrets (SSH keys, tokens, keychains, .zshrc exports) stay where they are; never print them, never send them
  anywhere, never paste them into a file outside their place.
- Fix paths that name the old home folder (see GOAL.md) in config files, plists, LaunchAgents, shell files.
- Things only the person can do (approve a driver or a privacy permission in System Settings, type a password,
  sign in to an account) go into TODO-FOR-YOU.md with exact steps, not into a loop of retries.
- Ask before anything with `sudo`, and before installing something GOAL.md doesn't mention.
- Keep a log in REPORT.md: per item, done / needs the person / failed, and why.
- Finish with a short summary of REPORT.md and TODO-FOR-YOU.md.
";

/// AGENTS.md and GOAL.md in `dir`; returns GOAL.md's text.
pub fn write_brief(dir: &Path, from: &str, old_home: &str, home: &Path, placed: &[Placed]) -> std::io::Result<String> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join("AGENTS.md"), AGENTS_MD)?;
    let mut g = format!("# GOAL: make this Mac work like {from}\n\n");
    if write_preflight(dir, placed, home)? {
        g += "`preflight.sh` in this folder ran before you to install Apple's command line tools, Homebrew and Node where these items need them. If `brew` or `node` is still missing, run `sh preflight.sh` again; what it can't do goes into TODO-FOR-YOU.md.\n\n";
    }
    let new_home = home.display().to_string();
    if old_home != new_home {
        g += &format!("The old home folder was `{old_home}`, this one is `{new_home}`. Every config that names `{old_home}` must name `{new_home}` instead (`grep -rl '{old_home}' ~/.config ~/.zshrc ~/Library/LaunchAgents ~/.claude*` and the like).\n\n");
    } else {
        g += &format!("The home folder is `{new_home}` on both Macs, so absolute paths still hold.\n\n");
    }
    g += "## Items\n\n";
    for p in placed {
        g += &format!("### {}{}\n\n", p.name, if p.secret { " (secret)" } else { "" });
        g += &format!("- id: `{}`\n", p.id);
        for t in &p.to {
            g += &format!("- placed: `{}`\n", t.display());
        }
        for d in &p.imported {
            g += &format!("- imported preferences: `{d}` (run `killall cfprefsd` and restart the app or `killall Dock Finder SystemUIServer` for system ones)\n");
        }
        for f in &p.generated {
            g += &format!("- file from the old Mac: `{}`\n", f.display());
        }
        for s in &p.keys {
            g += &format!("- added to this Mac's login keychain: `{s}` (done; start the app to check its saved logins)\n");
        }
        for k in &p.kept {
            g += &format!("- what was here before: `{}`\n", k.display());
        }
        for e in &p.problems {
            g += &format!("- PROBLEM: {e}\n");
        }
        if !p.note.is_empty() {
            g += &format!("- to finish: {}\n", p.note);
        }
        g += "\n";
    }
    g += "## Done means\n\n- Every item above is checked and works, or is in TODO-FOR-YOU.md with steps.\n- REPORT.md says what was done per item.\n";
    std::fs::write(dir.join("GOAL.md"), &g)?;
    Ok(g)
}

/// Opens Terminal on this Mac with the chosen agent command in the transfer folder.
pub fn start_agent(command: &str, dir: &Path) -> Result<(), String> {
    let mut cmd = command.replace("{dir}", &format!("'{}'", dir.display()));
    if dir.join("preflight.sh").exists() {
        cmd = format!("/bin/sh '{}'; {cmd}", dir.join("preflight.sh").display());
    }
    let script = format!("tell application \"Terminal\"\n activate\n do script \"{}\"\nend tell", cmd.replace('\\', "\\\\").replace('"', "\\\""));
    let o = Command::new("osascript").arg("-e").arg(script).output().map_err(|e| e.to_string())?;
    if o.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&o.stderr).trim().to_string()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_land_under_the_new_home_and_old_files_are_kept() {
        let base = std::env::temp_dir().join(format!("macysync-place-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let old_home = "/Users/olduser";
        let home = base.join("newhome");
        let staging = base.join("staging");
        let dir = base.join("transfer");
        let put = |rel: &str, body: &str| {
            let f = staging.join(rel);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, body).unwrap();
        };
        put("apps__k__config/Users/olduser/.config/karabiner/karabiner.json", "new");
        put("tools__brew/macysync-generated/Brewfile", "brew \"jq\"");
        put("secrets__keychain/Users/olduser/Library/Keychains/login.keychain-db", "old keys");
        put("ai__c/Users/olduser/.local/share/c/1.0", "binary");
        std::fs::create_dir_all(staging.join("ai__c/Users/olduser/.local/bin")).unwrap();
        std::os::unix::fs::symlink("/Users/olduser/.local/share/c/1.0", staging.join("ai__c/Users/olduser/.local/bin/c")).unwrap();
        std::os::unix::fs::symlink("../share/c/1.0", staging.join("ai__c/Users/olduser/.local/bin/rel")).unwrap();
        std::fs::create_dir_all(home.join(".config/karabiner")).unwrap();
        std::fs::write(home.join(".config/karabiner/karabiner.json"), "mine").unwrap();
        std::fs::create_dir_all(home.join("Library/Keychains")).unwrap();
        std::fs::write(home.join("Library/Keychains/login.keychain-db"), "this mac").unwrap();
        let items = vec![
            Node { id: "apps/k/config".into(), name: "Karabiner config".into(), paths: vec!["~/.config/karabiner".into()], note: "approve the driver".into(), ..Node::default() },
            Node { id: "tools/brew".into(), name: "Brewfile".into(), command: "x".into(), file: "Brewfile".into(), ..Node::default() },
            Node { id: "secrets/keychain".into(), name: "Keychain".into(), paths: vec!["~/Library/Keychains/login.keychain-db".into()], secret: true, ..Node::default() },
            Node { id: "ai/c".into(), name: "C".into(), paths: vec!["~/.local/bin".into(), "~/.local/share/c/1.0".into()], ..Node::default() },
        ];
        let placed = place(&staging, &dir, old_home, &home, &items);
        assert!(placed.iter().all(|p| p.problems.is_empty()), "{placed:?}");
        assert_eq!(std::fs::read_to_string(home.join(".config/karabiner/karabiner.json")).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(home.join(".config/karabiner.before-macysync/karabiner.json")).unwrap(), "mine");
        assert_eq!(std::fs::read_to_string(home.join("Library/Keychains/login.keychain-db")).unwrap(), "this mac", "never over this Mac's keychain");
        assert_eq!(std::fs::read_to_string(home.join("Library/Keychains/old-mac.keychain-db")).unwrap(), "old keys");
        assert!(dir.join("generated/Brewfile").exists());
        assert_eq!(std::fs::read_link(home.join(".local/bin/c")).unwrap(), home.join(".local/share/c/1.0"), "a link into the old home follows to the new one");
        assert_eq!(std::fs::read_to_string(home.join(".local/bin/c")).unwrap(), "binary");
        assert_eq!(std::fs::read_link(home.join(".local/bin/rel")).unwrap(), Path::new("../share/c/1.0"), "a relative link stays as it was");

        let goal = write_brief(&dir, "Old Mac", old_home, &home, &placed).unwrap();
        assert!(goal.contains("`/Users/olduser`"), "the path rewrite is spelled out");
        assert!(goal.contains("approve the driver"));
        assert!(goal.contains("Keychain (secret)"));
        assert!(dir.join("AGENTS.md").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A key's line quotes names with spaces and quotes, puts the app where it landed here, and carries the key
    /// only as hex; an empty key is a problem, never an empty item.
    #[test]
    fn app_keys_go_in_by_hex_and_never_empty() {
        let home = Path::new("/Users/new");
        let k = AppKey { service: "Helium \"Storage\" Key".into(), account: "Helium".into(), app: "~/Applications/Helium.app".into(), key: "K3y+/=".into() };
        assert_eq!(key_line(&k, home), "add-generic-password -U -s \"Helium \\\"Storage\\\" Key\" -a \"Helium\" -T \"/Users/new/Applications/Helium.app\" -X 4b33792b2f3d\n");
        let f = std::env::temp_dir().join(format!("macysync-empty-key-{}.key.json", std::process::id()));
        std::fs::write(&f, r#"{"service":"Helium Storage Key","account":"Helium","app":"/Applications/Helium.app","key":""}"#).unwrap();
        assert!(add_key(&f, home).unwrap_err().contains("didn't hand over 'Helium Storage Key'"));
        assert!(!f.exists(), "the file goes either way");
    }

    #[test]
    fn preflight_is_written_only_when_tools_are_needed_and_runs_clean() {
        let base = std::env::temp_dir().join(format!("macysync-preflight-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (dir, home) = (base.join("transfer"), base.join("home"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let plain = vec![Placed { id: "docs".into(), ..Placed::default() }];
        assert!(!write_preflight(&dir, &plain, &home).unwrap(), "nothing needs a tool");
        let brewfile = dir.join("Brewfile");
        std::fs::write(&brewfile, "brew \"git\"\nbrew \"node@22\"\n").unwrap();
        let tools = vec![Placed { id: "tools/brewfile".into(), generated: vec![brewfile], ..Placed::default() }];
        assert_eq!(needs(&tools, &home), (true, true), "a Brewfile with node needs both");
        assert!(write_preflight(&dir, &tools, &home).unwrap());
        let script = dir.join("preflight.sh");
        let s = std::fs::read_to_string(&script).unwrap();
        assert!(s.contains("NEED_BREW=1") && s.contains("NEED_NODE=1"));
        assert!(Command::new("/bin/sh").arg("-n").arg(&script).status().unwrap().success(), "valid sh");
        // On a Mac that already has Homebrew and Node it installs nothing and only adds the PATH line, twice-safe.
        let has = |c: &str| Command::new("/bin/sh").args(["-c", &format!("command -v {c}")]).status().is_ok_and(|s| s.success());
        if Path::new("/opt/homebrew/bin/brew").exists() && has("node") && has("xcode-select") {
            for _ in 0..2 {
                let o = Command::new("/bin/sh").arg(&script).env("HOME", &home).output().unwrap();
                assert!(String::from_utf8_lossy(&o.stdout).contains("Tools ready."), "{}", String::from_utf8_lossy(&o.stdout));
            }
            let zp = std::fs::read_to_string(home.join(".zprofile")).unwrap();
            assert_eq!(zp.matches("brew shellenv").count(), 1, "the line is added once");
        }
        let _ = std::fs::remove_dir_all(base);
    }
}
