//! What this Mac can offer, as a tree: categories, then apps or folders, then the parts of each. Leaves carry
//! the paths they move (`~/…` is the home folder) or a command whose output moves instead (a Brewfile).
//! Ticking a node ticks every leaf under it; a transfer moves leaves.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Node {
    pub id: String,
    pub name: String,
    /// Bytes, summed over the children for a parent.
    pub size: u64,
    pub paths: Vec<String>,
    /// For a generated leaf: the shell command whose output is sent as `file`.
    pub command: String,
    pub file: String,
    /// What the setup agent should know: goes into GOAL.md with the item.
    pub note: String,
    pub secret: bool,
    /// Left unticked at first: identity-bound apps, caches, things better reinstalled.
    pub off: bool,
    pub children: Vec<Node>,
}

impl Node {
    fn group(id: &str, name: &str, children: Vec<Node>) -> Node {
        Node { id: id.into(), name: name.into(), children, ..Node::default() }
    }

    fn leaf(id: &str, name: &str, paths: Vec<String>) -> Node {
        Node { id: id.into(), name: name.into(), paths, ..Node::default() }
    }

    fn with(mut self, f: impl FnOnce(&mut Node)) -> Node {
        f(&mut self);
        self
    }

    pub fn leaves(&self) -> Vec<&Node> {
        if self.children.is_empty() { vec![self] } else { self.children.iter().flat_map(|c| c.leaves()).collect() }
    }

    pub fn find(&self, id: &str) -> Option<&Node> {
        if self.id == id {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(id))
    }

    /// Sums sizes up from the leaves; drops groups left empty.
    fn settle(&mut self) {
        for c in &mut self.children {
            c.settle();
        }
        self.children.retain(|c| !c.children.is_empty() || !c.paths.is_empty() || !c.command.is_empty());
        if !self.children.is_empty() {
            self.size = self.children.iter().map(|c| c.size).sum();
        }
    }

    /// Only the leaves in `keep`, and the groups above them.
    pub fn pruned(&self, keep: &HashSet<String>) -> Option<Node> {
        if self.children.is_empty() {
            return keep.contains(&self.id).then(|| self.clone());
        }
        let children: Vec<Node> = self.children.iter().filter_map(|c| c.pruned(keep)).collect();
        (!children.is_empty()).then(|| Node { size: children.iter().map(|c| c.size).sum(), children, ..self.clone() })
    }
}

/// What the sender publishes (sealed to the receiver): its tree, and where its home folder was.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Offer {
    pub home: String,
    pub skip_rebuildable: bool,
    pub tree: Node,
}

/// (service, account) of each password item in the login keychain, read without the secrets (no dialog).
fn keychain_items() -> Vec<(String, String)> {
    let out = Command::new("security").arg("dump-keychain").output().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
    let field = |b: &str, k: &str| b.lines().find_map(|l| l.trim().strip_prefix(&format!("\"{k}\"<blob>=\"")).and_then(|v| v.strip_suffix('"')).map(str::to_string));
    out.split("keychain: ").filter(|b| b.contains("class: \"genp\"")).filter_map(|b| Some((field(b, "svce")?, field(b, "acct")?))).collect()
}

/// The keychain item an app encrypts its saved passwords, cookies and logins with: a Chromium browser's or an
/// Electron app's "<name> Safe Storage" (Google Chrome's "Chrome", Brave Browser's "Brave"), Helium's "Helium
/// Storage Key".
pub fn app_key(app: &str, items: &[(String, String)]) -> Option<(String, String)> {
    let short = app.strip_prefix("Google ").or_else(|| app.strip_suffix(" Browser")).unwrap_or(app);
    let names = [format!("{app} Safe Storage"), format!("{short} Safe Storage"), format!("{app} Storage Key")];
    items.iter().find(|(s, _)| names.contains(s)).cloned()
}

/// The command that sends an app's key: a JSON line with the item's service, account, the app's path and the key,
/// which the old Mac's user allows in a dialog (empty when nobody does in time).
pub fn key_command(svc: &str, acct: &str, app: &str) -> String {
    let sh = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
    let js = |s: &str| sh(&s.replace('\\', "\\\\").replace('"', "\\\""));
    // printf takes the fields as %s arguments: never read as its own escapes or formats
    format!("printf '{{\"service\":\"%s\",\"account\":\"%s\",\"app\":\"%s\",\"key\":\"%s\"}}' {} {} {} \"$(security find-generic-password -w -s {} -a {} 2>/dev/null)\"",
        js(svc), js(acct), js(app), sh(svc), sh(acct))
}

/// Folders rebuilt from source on the new Mac: skipped inside the person's own folders when asked to.
pub const REBUILDABLE: [&str; 8] = ["node_modules", "target", ".venv", "venv", "__pycache__", ".gradle", "DerivedData", ".next"];

/// Whether an item is the person's own folders, where REBUILDABLE is skipped. Never an app, its data or a tool's
/// settings: an app bundle's node_modules is part of the app, and macOS bins one whose seal it breaks.
pub fn rebuildable_in(id: &str) -> bool {
    id.starts_with("home/") || id.starts_with("files/")
}

fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

/// Apps that need more than their bundle, preferences and Application Support folder.
struct Known {
    app: &'static str,
    extra: &'static [(&'static str, &'static [&'static str])],
    note: &'static str,
    /// Tied to this Mac's identity: offered unticked, the person signs in again instead.
    identity: bool,
}

const KNOWN: &[Known] = &[
    Known { app: "Karabiner-Elements", extra: &[("Config", &["~/.config/karabiner"])], note: "Rules live in ~/.config/karabiner/karabiner.json. After placing it, open Karabiner-Elements once and approve its driver extension and Input Monitoring in System Settings > Privacy & Security; the person has to click those.", identity: false },
    Known { app: "Opensidian", extra: &[("Vault list", &["~/.opensidian.json"])], note: "~/.opensidian.json lists the vault folders by absolute path. The vault folders themselves travel with Documents or wherever they live; rewrite the paths if the user name changed, make sure each vault folder exists, then open the app and check the last vault opens.", identity: false },
    Known { app: "Obsidian", extra: &[], note: "~/Library/Application Support/obsidian/obsidian.json lists the vaults by absolute path; rewrite them if the user name changed and check each vault folder arrived.", identity: false },
    Known { app: "Visual Studio Code", extra: &[("Extensions", &["~/.vscode/extensions"])], note: "Settings, keybindings and snippets are in ~/Library/Application Support/Code/User; extensions copy as is. Run `code --list-extensions` to check they load.", identity: false },
    Known { app: "Wave", extra: &[("Config", &["~/.config/waveterm"]), ("Workspaces and layouts", &["~/Library/Application Support/waveterm"])], note: "Wave keeps settings in ~/.config/waveterm, its workspaces, tabs and layouts in ~/Library/Application Support/waveterm (a SQLite database, copied whole) and the window's web storage in ~/Library/Application Support/Wave. Quit Wave before placing them. Widget commands that name the old home folder need the new one.", identity: false },
    Known { app: "Helium", extra: &[], note: "Saved passwords and cookies are encrypted with the 'Helium Storage Key' item of the login keychain; its Password key part brings it, before the first start.", identity: false },
    Known { app: "Chromium", extra: &[], note: "Saved passwords and cookies need the 'Chromium Safe Storage' keychain key; its Password key part brings it, before the first start.", identity: false },
    Known { app: "Tailscale", extra: &[], note: "Sign in again on the new Mac; a copy would carry this Mac's node identity.", identity: true },
    Known { app: "RustDesk", extra: &[("Config", &["~/Library/Preferences/com.carriez.RustDesk"])], note: "Install fresh and set a new password; a copy would clone this Mac's RustDesk ID.", identity: true },
    Known { app: "AnyDesk", extra: &[("Config", &["~/.anydesk"])], note: "Install fresh; a copy would clone this Mac's AnyDesk ID.", identity: true },
    Known { app: "Cloudflare WARP", extra: &[], note: "Install and sign in again; registrations are per device.", identity: true },
    Known { app: "Chrome Remote Desktop Host Uninstaller", extra: &[], note: "Set Chrome Remote Desktop up again on the new Mac; hosts are registered per device.", identity: true },
    Known { app: "Xcode", extra: &[], note: "A copied Xcode works; run `sudo xcode-select -s /Applications/Xcode.app` and `xcodebuild -runFirstLaunch`, which need the person's password.", identity: false },
];

fn expand(home: &Path, p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => PathBuf::from(p),
    }
}

/// What in ~/.claude is conversation history: sessions, transcripts, file snapshots. Gigabytes on a Mac used
/// for long, and not needed to work: everything else (settings, skills, plugins, agents, memory) is.
const CLAUDE_HISTORY: [&str; 12] = ["projects", "file-history", "transcripts", "history.jsonl", "shell-snapshots", "session-env", "sessions", "paste-cache", "todos", "debug", "backups", "jobs"];

/// Claude Code in three parts, so its history can stay behind: `paths` are ~/.claude, ~/.claude.json and the
/// `claude` link with the version it points at.
fn claude_code(home: &Path, paths: Vec<String>, note: &str) -> Node {
    let (app, files): (Vec<String>, Vec<String>) = paths.into_iter().partition(|p| p.starts_with("~/.local/"));
    let dot = home.join(".claude");
    let entries: Vec<String> = std::fs::read_dir(&dot).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
    let mut settings: Vec<String> = files.iter().filter(|p| p.as_str() != "~/.claude").cloned().collect();
    settings.extend(entries.iter().filter(|e| !CLAUDE_HISTORY.contains(&e.as_str())).map(|e| format!("~/.claude/{e}")));
    // Memory lives under projects/<folder>/memory: it goes with the settings, the rest of projects/ is history.
    for d in std::fs::read_dir(dot.join("projects")).into_iter().flatten().flatten() {
        if d.path().join("memory").is_dir() {
            settings.push(tilde(home, &d.path().join("memory")));
        }
    }
    let history = entries.iter().filter(|e| CLAUDE_HISTORY.contains(&e.as_str())).map(|e| format!("~/.claude/{e}")).collect();
    Node::group("ai/claude-code", "Claude Code", vec![
        Node::leaf("ai/claude-code/app", "App", app),
        Node::leaf("ai/claude-code/settings", "Settings, skills, plugins, memory", settings).with(|n| n.note = note.into()),
        Node::leaf("ai/claude-code/history", "Conversation history", history).with(|n| n.note = "Past sessions, transcripts and file snapshots; `claude --resume` lists them. Folder names under projects/ carry the old home path: rename them for the new one.".into()),
    ])
}

fn tilde(home: &Path, p: &Path) -> String {
    match p.strip_prefix(home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => p.display().to_string(),
    }
}

fn list(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir).map(|r| r.flatten().map(|e| e.path()).filter(|p| !p.ends_with(".DS_Store")).collect()).unwrap_or_default();
    v.sort();
    v
}

fn bundle_id(app: &Path) -> String {
    Command::new("plutil").args(["-extract", "CFBundleIdentifier", "raw"]).arg(app.join("Contents/Info.plist")).output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
}

/// The whole tree for `home`, with apps from `app_dirs`. Sizes come last, from one `du` run.
pub fn scan(home: &Path, app_dirs: &[PathBuf], skip_rebuildable: bool) -> Node {
    let lib = home.join("Library");
    let mut claimed: HashSet<String> = HashSet::new();
    let exists = |p: &str| expand(home, p).exists();

    // Apps, each with its bundle, preferences, data, known extras and the keychain key its saved logins need.
    let keys = keychain_items();
    let mut apps = Vec::new();
    let support: Vec<PathBuf> = list(&lib.join("Application Support"));
    for dir in app_dirs {
        for app in list(dir).into_iter().filter(|p| p.extension().is_some_and(|e| e == "app")) {
            let name = app.file_stem().unwrap().to_string_lossy().to_string();
            let id = format!("apps/{}", slug(&name));
            let bid = bundle_id(&app);
            // macOS's own apps come with the system; hidden helpers come with the app that installed them.
            if name.starts_with('.') || (bid.starts_with("com.apple.") && bid != "com.apple.dt.Xcode") {
                continue;
            }
            let mut parts = vec![Node::leaf(&format!("{id}/app"), "App", vec![tilde(home, &app)])];
            if !bid.is_empty() {
                let prefs: Vec<String> = list(&lib.join("Preferences")).into_iter().filter(|p| p.file_name().is_some_and(|f| f.to_string_lossy().starts_with(&bid))).map(|p| tilde(home, &p)).collect();
                parts.push(Node::leaf(&format!("{id}/settings"), "Settings", prefs));
            }
            let lname = name.to_lowercase();
            let mut data: Vec<String> = support.iter().filter(|p| {
                let f = p.file_name().unwrap().to_string_lossy().to_lowercase();
                f == lname || (!bid.is_empty() && f == bid.to_lowercase()) || (lname == "visual studio code" && f == "code")
            }).map(|p| tilde(home, p)).collect();
            if !bid.is_empty() && lib.join("Containers").join(&bid).exists() {
                data.push(format!("~/Library/Containers/{bid}"));
            }
            parts.push(Node::leaf(&format!("{id}/data"), "Data", data));
            let known = KNOWN.iter().find(|k| k.app == name);
            for (part, paths) in known.map(|k| k.extra).unwrap_or(&[]) {
                let paths: Vec<String> = paths.iter().filter(|p| exists(p)).map(|p| p.to_string()).collect();
                parts.push(Node::leaf(&format!("{id}/{}", slug(part)), part, paths));
            }
            for p in parts.iter().flat_map(|p| p.paths.iter()) {
                claimed.insert(p.clone());
            }
            if let Some((svc, acct)) = app_key(&name, &keys) {
                // Ticked with the app: without it the app makes a new key and its saved logins can't be read.
                parts.push(Node { id: format!("{id}/key"), name: "Password key".into(), command: key_command(&svc, &acct, &tilde(home, &app)),
                    file: format!("{}.key.json", slug(&name)), secret: true,
                    note: format!("The '{svc}' item of the old Mac's login keychain; macOS asks the old Mac's user to allow reading it once. MacySync adds it to this Mac's login keychain before the app first starts."),
                    ..Node::default() });
            }
            let note = known.map(|k| k.note).unwrap_or("");
            let identity = known.is_some_and(|k| k.identity);
            let mut node = Node::group(&id, &name, parts).with(|n| {
                n.note = note.into();
                n.off = identity;
            });
            if identity {
                node.children.iter_mut().for_each(|c| c.off = true);
            }
            apps.push(node);
        }
    }
    apps.sort_by_key(|a| a.name.to_lowercase());

    // Command line tools: lists the agent reinstalls from, made when the transfer starts.
    let made = |id: &str, name: &str, cmd: &str, file: &str, note: &str| Node { id: id.into(), name: name.into(), command: cmd.into(), file: file.into(), note: note.into(), ..Node::default() };
    let mut tools = vec![];
    if which("brew") {
        tools.push(made("tools/brewfile", "Homebrew (Brewfile)", "brew bundle dump --file=- --describe", "Brewfile",
            "Install Homebrew if it's missing, then `brew bundle install --file Brewfile`. Casks whose app was copied already: use `brew install --cask --adopt <name>` so Homebrew takes them over instead of failing."));
    }
    if which("npm") {
        tools.push(made("tools/npm", "npm global packages", "npm ls -g --depth=0", "npm-globals.txt", "Reinstall these with `npm install -g` once Node is installed."));
    }
    if which("cargo") {
        tools.push(made("tools/cargo", "Rust tools (cargo install)", "cargo install --list", "cargo-installs.txt", "Install rustup, then `cargo install` each crate listed (git ones with --git)."));
    }
    if which("uv") {
        tools.push(made("tools/uv", "uv tools", "uv tool list", "uv-tools.txt", "Reinstall with `uv tool install <name>`."));
    }
    if exists("~/.local") {
        claimed.insert("~/.local".into());
        tools.push(Node::leaf("tools/local", "~/.local (bin, share, state)", vec!["~/.local".into()]).with(|n| n.note = "Personal scripts in ~/.local/bin and tool data in ~/.local/share (uv's Pythons, pipx). Check scripts that point into other folders still resolve; reinstall uv-managed Pythons if they fail to start.".into()));
    }

    // Secrets and keys: sealed like everything else, marked so the receiver sees what they are.
    let secret = |id: &str, name: &str, paths: &[&str], note: &str| {
        Node::leaf(id, name, paths.iter().filter(|p| exists(p)).map(|p| p.to_string()).collect()).with(|n| {
            n.secret = true;
            n.note = note.into();
        })
    };
    let mut secrets = vec![
        secret("secrets/ssh", "SSH keys and config", &["~/.ssh"], "Keep ~/.ssh at 700 and private keys at 600."),
        secret("secrets/gpg", "GnuPG", &["~/.gnupg"], "Keep ~/.gnupg at 700."),
        secret("secrets/gh", "GitHub CLI login", &["~/.config/gh"], "gh may keep its token in the keychain instead; run `gh auth status`, and `gh auth login` if it says so."),
        secret("secrets/tokens", "Token files (.npmrc, .netrc, cloud CLIs)", &["~/.npmrc", "~/.netrc", "~/.aws", "~/.docker/config.json", "~/.kube"], ""),
        secret("secrets/keychain", "Login keychain (as a second keychain)", &["~/Library/Keychains/login.keychain-db"],
            "Do not replace the new Mac's login keychain. Add the copy as a second keychain: `security create-keychain` is not needed, copy it to ~/Library/Keychains/old-mac.keychain-db and run `security list-keychains -d user -s login.keychain-db old-mac.keychain-db`; it opens with the old Mac's login password, or with an older one if the keychain kept it when the login password changed (seen on a real move). Apps' own keys come with each app's Password key part, so this copy is for the rest. The person can then move items in Keychain Access."),
    ];
    for p in ["~/.ssh", "~/.gnupg", "~/.config/gh", "~/.npmrc", "~/.netrc", "~/.aws", "~/.docker", "~/.kube"] {
        claimed.insert(p.into());
    }

    // AI agents' own folders.
    let ai_paths = [("Claude Code", &["~/.claude", "~/.claude.json"][..], "The login is in the keychain: run `claude` once and sign in. Paths in settings, hooks and memory files that name the old home folder need rewriting."),
        ("Codex", &["~/.codex"][..], "Run `codex login` if it asks."),
        ("Ollama models", &["~/.ollama"][..], "Large; Ollama can pull models again instead."),
        ("OpenCode", &["~/.opencode", "~/.config/opencode"][..], ""),
        ("OpenCommunicate", &["~/.opencommunicate", "~/.opencommunicate-apps"][..], "Each folder is one device identity; moving it moves that identity, so don't run both Macs as the same one.")];
    let mut ai = Vec::new();
    for (name, paths, note) in ai_paths {
        let mut paths: Vec<String> = paths.iter().filter(|p| exists(p)).map(|p| p.to_string()).collect();
        // The native installer's `claude` is a link to one version under ~/.local/share/claude: take both, not all versions.
        if name == "Claude Code" {
            let link = home.join(".local/bin/claude");
            if let Ok(target) = std::fs::read_link(&link) {
                paths.push(tilde(home, &link));
                paths.push(tilde(home, &link.parent().unwrap().join(target)));
            }
        }
        claimed.extend(paths.iter().cloned());
        if name == "Claude Code" && exists("~/.claude") {
            ai.push(claude_code(home, paths, note));
            continue;
        }
        ai.push(Node::leaf(&format!("ai/{}", slug(name)), name, paths).with(|n| {
            n.note = note.into();
            n.off = name == "Ollama models";
        }));
    }

    // System settings: the preference files behind them; the agent imports them with `defaults import`.
    let prefs = |id: &str, name: &str, files: &[&str]| Node::leaf(&format!("system/{id}"), name, files.iter().map(|f| format!("~/Library/Preferences/{f}.plist")).filter(|p| exists(p)).collect());
    let system = vec![
        prefs("dock", "Dock", &["com.apple.dock"]),
        prefs("finder", "Finder", &["com.apple.finder"]),
        prefs("global", "Keyboard, appearance, global", &[".GlobalPreferences"]),
        prefs("trackpad", "Trackpad and mouse", &["com.apple.AppleMultitouchTrackpad", "com.apple.driver.AppleBluetoothMultitouch.trackpad", "com.apple.AppleMultitouchMouse"]),
        prefs("screenshots", "Screenshots", &["com.apple.screencapture"]),
        prefs("menubar", "Menu bar and Control Centre", &["com.apple.controlcenter"]),
        Node::leaf("system/launchagents", "Login agents (LaunchAgents)", vec!["~/Library/LaunchAgents".into()]).with(|n| n.note = "Check each plist's program path exists on the new Mac before `launchctl bootstrap gui/$(id -u) <plist>`; drop the ones for apps that weren't moved.".into()),
        Node::leaf("system/fonts", "Fonts", vec!["~/Library/Fonts".into()]),
        Node::leaf("system/services", "Services and Quick Actions", vec!["~/Library/Services".into()]),
        Node::leaf("system/keyboard-layouts", "Keyboard layouts", vec!["~/Library/Keyboard Layouts".into()]),
    ]
    .into_iter()
    .map(|n| n.with(|n| {
        n.paths.retain(|p| exists(p));
        if n.id.starts_with("system/") && n.note.is_empty() {
            n.note = "Import with `defaults import <domain> <file>`, then `killall Dock Finder SystemUIServer cfprefsd` so it takes effect.".into();
        }
    }))
    .collect();

    // Shell, git and the rest of ~/.config.
    let mut shell: Vec<Node> = [".zshrc", ".zprofile", ".zshenv", ".profile", ".bashrc", ".bash_profile", ".tcshrc", ".gitconfig", ".vimrc", ".tmux.conf", ".zsh_history"]
        .iter()
        .map(|f| format!("~/{f}"))
        .filter(|p| exists(p))
        .map(|p| Node::leaf(&format!("shell/{}", slug(&p)), p.trim_start_matches("~/"), vec![p.clone()]).with(|n| {
            if n.name.starts_with(".zsh") || n.name.ends_with("profile") || n.name == ".bashrc" {
                n.note = "May hold secrets (exported tokens); they travel sealed. Fix paths that name the old home folder.".into();
                n.secret = true;
            }
        }))
        .collect();
    for c in list(&home.join(".config")) {
        let t = tilde(home, &c);
        if !claimed.contains(&t) {
            shell.push(Node::leaf(&format!("shell/config-{}", slug(&c.file_name().unwrap().to_string_lossy())), &format!("~/.config/{}", c.file_name().unwrap().to_string_lossy()), vec![t]));
        }
    }

    // Personal files: each top-level item of these folders.
    let mut files = Vec::new();
    for f in ["Desktop", "Documents", "Downloads", "Pictures", "Movies", "Music"] {
        let kids: Vec<Node> = list(&home.join(f)).iter().filter(|p| !p.file_name().unwrap().to_string_lossy().starts_with('.')).map(|p| {
            let n = p.file_name().unwrap().to_string_lossy().to_string();
            Node::leaf(&format!("files/{}/{}", slug(f), slug(&n)), &n, vec![tilde(home, p)])
        }).collect();
        files.push(Node::group(&format!("files/{}", slug(f)), f, kids));
    }

    // Everything else in the home folder.
    let mut skip: HashSet<String> = ["Library", "Applications", "Desktop", "Documents", "Downloads", "Pictures", "Movies", "Music", "Public", ".Trash", ".DS_Store", ".CFUserTextEncoding", ".config", ".zsh_sessions"].iter().map(|s| s.to_string()).collect();
    skip.extend(claimed.iter().filter_map(|p| p.strip_prefix("~/").map(|s| s.split('/').next().unwrap().to_string())).filter(|s| !s.is_empty() && s != "Library" && s != ".config"));
    let caches: HashSet<&str> = [".cache", ".npm", ".gradle", ".cargo", ".rustup", ".pub-cache", ".gem", ".nuget", ".yarn", ".cocoapods", ".dartServer", ".dart-tool", ".swiftpm", ".android", ".dotnet", ".gradle-migrator", ".dspy_cache"].into();
    let mut folders = Vec::new();
    let mut hidden = Vec::new();
    for p in list(home) {
        let n = p.file_name().unwrap().to_string_lossy().to_string();
        if skip.contains(&n) || shell.iter().any(|s| s.paths.first().is_some_and(|x| x == &format!("~/{n}"))) || p.is_symlink() {
            continue;
        }
        let leaf = Node::leaf(&format!("{}/{}", if n.starts_with('.') { "hidden" } else { "home" }, slug(&n)), &n, vec![tilde(home, &p)]);
        let kids = if p.is_dir() && !n.starts_with('.') { list(&p) } else { vec![] };
        if !kids.is_empty() && kids.len() <= 300 {
            // A folder of projects: each one can be picked on its own.
            let base = leaf.id.clone();
            folders.push(Node::group(&base, &n, kids.iter().map(|k| {
                let kn = k.file_name().unwrap().to_string_lossy().to_string();
                Node::leaf(&format!("{base}/{}", slug(&kn)), &kn, vec![tilde(home, k)])
            }).collect()));
            continue;
        }
        if n.starts_with('.') {
            hidden.push(leaf.with(|l| if caches.contains(n.as_str()) {
                l.off = true;
                l.note = "A cache or toolchain the tools rebuild or reinstall; copy it only to save the download.".into();
            }));
        } else {
            folders.push(leaf);
        }
    }

    // Application Support folders of apps that aren't installed any more, minus Apple's own.
    let apple = ["AddressBook", "Animoji", "AskPermission", "CallHistoryDB", "CallHistoryTransactions", "CloudDocs", "CrashReporter", "DifferentialPrivacy", "DiskImages", "Dock", "FaceTime", "FileProvider", "Knowledge", "Music", "default.store", "familycircled", "Xcode"];
    let leftovers: Vec<Node> = support.iter().filter(|p| {
        let f = p.file_name().unwrap().to_string_lossy().to_string();
        !f.starts_with("com.apple.") && !f.starts_with("default.store") && !apple.contains(&f.as_str()) && !claimed.contains(&tilde(home, p))
    }).map(|p| Node::leaf(&format!("leftover/{}", slug(&p.file_name().unwrap().to_string_lossy())), &p.file_name().unwrap().to_string_lossy(), vec![tilde(home, p)]).with(|n| n.off = true)).collect();

    let mut root = Node::group("", "This Mac", vec![
        Node::group("apps", "Apps", apps),
        Node::group("tools", "Command line tools", tools),
        Node::group("secrets", "Secrets and keys", secrets),
        Node::group("shell", "Shell, git and ~/.config", shell),
        Node::group("ai", "AI agents", ai),
        Node::group("system", "System settings", system),
        Node::group("files", "Personal files", files),
        Node::group("home", "Other folders in home", folders),
        Node::group("hidden", "Other hidden folders", hidden),
        Node::group("leftover", "Data of apps no longer installed", leftovers),
    ]);
    root.settle();
    sizes(&mut root, home, skip_rebuildable);
    root.settle();
    root
}

fn which(cmd: &str) -> bool {
    Command::new("/bin/sh").args(["-c", &format!("command -v {cmd}")]).output().is_ok_and(|o| o.status.success())
}

/// Every leaf's size in one `du` run (in kilobytes, files counted once even when two leaves share them).
fn sizes(root: &mut Node, home: &Path, skip_rebuildable: bool) {
    let leaves = root.leaves();
    let paths = |own: bool| -> Vec<String> {
        let set: std::collections::BTreeSet<String> = leaves.iter().filter(|l| rebuildable_in(&l.id) == own).flat_map(|l| l.paths.iter().cloned()).collect();
        set.into_iter().filter(|p| expand(home, p).exists()).collect()
    };
    let mut by_path: BTreeMap<String, u64> = BTreeMap::new();
    for (own, chunk) in [true, false].into_iter().flat_map(|own| paths(own).chunks(200).map(|c| (own, c.to_vec())).collect::<Vec<_>>()) {
        let mut c = Command::new("du");
        c.arg("-sk");
        if skip_rebuildable && own {
            for r in REBUILDABLE {
                c.args(["-I", r]);
            }
        }
        let out = c.args(chunk.iter().map(|p| expand(home, p))).output().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
        for line in out.lines() {
            if let Some((k, p)) = line.split_once('\t') {
                by_path.insert(tilde(home, Path::new(p)), k.trim().parse::<u64>().unwrap_or(0) * 1024);
            }
        }
    }
    fn walk(n: &mut Node, by: &BTreeMap<String, u64>) {
        if n.children.is_empty() {
            n.size = n.paths.iter().map(|p| by.get(p).copied().unwrap_or(0)).sum();
        }
        n.children.iter_mut().for_each(|c| walk(c, by));
    }
    walk(root, &by_path);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fake_home_becomes_a_tree_with_sizes() {
        let home = std::env::temp_dir().join(format!("macysync-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let mk = |p: &str, bytes: usize| {
            let f = home.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, vec![b'x'; bytes]).unwrap();
        };
        mk(".zshrc", 100);
        mk(".config/karabiner/karabiner.json", 5000);
        mk(".config/micro/settings.json", 10);
        mk(".ssh/id_ed25519", 400);
        mk("Documents/notes/a.md", 9000);
        mk("projects/app/src/main.rs", 3000);
        mk("projects/app/node_modules/big.js", 200_000);
        mk("Applications/Karabiner-Elements.app/Contents/Info.plist", 10);
        mk("Applications/Karabiner-Elements.app/Contents/Resources/node_modules/addon.node", 100_000);
        let root = scan(&home, &[home.join("Applications")], true);

        let k = root.find("apps/karabiner-elements").expect("the app");
        assert!(k.find("apps/karabiner-elements/config").unwrap().paths == vec!["~/.config/karabiner".to_string()]);
        assert!(k.note.contains("karabiner.json"));
        // ~/.config/karabiner belongs to the app, not to the plain config list.
        assert!(root.find("shell/config-karabiner").is_none());
        assert!(root.find("shell/config-micro").is_some());
        assert!(root.find("secrets/ssh").unwrap().secret);
        assert!(root.find("shell/zshrc").unwrap().secret);
        // Rebuildable folders don't count: the project is ~4 KB of source, not 200 KB of node_modules.
        let p = root.find("home/projects/app").unwrap();
        assert!(p.size < 50_000, "{}", p.size);
        // An app's own node_modules does count: it is copied with the app.
        assert!(root.find("apps/karabiner-elements/app").unwrap().size >= 100_000);
        assert_eq!(root.size, root.children.iter().map(|c| c.size).sum::<u64>());
        // Empty groups are gone.
        assert!(root.find("leftover").is_none());

        // An app's key: found by the app's name, Chrome's and Brave's short ones too, and not some other app's.
        let items: Vec<(String, String)> = [("Chrome Safe Storage", "Chrome"), ("Brave Safe Storage", "Brave"), ("Helium Storage Key", "Helium"), ("Wave (Dev) Safe Storage", "Wave")]
            .iter().map(|(s, a)| (s.to_string(), a.to_string())).collect();
        assert_eq!(app_key("Google Chrome", &items).unwrap().0, "Chrome Safe Storage");
        assert_eq!(app_key("Brave Browser", &items).unwrap().0, "Brave Safe Storage");
        assert_eq!(app_key("Helium", &items).unwrap().1, "Helium");
        assert!(app_key("Wave", &items).is_none());

        let keep: HashSet<String> = ["secrets/ssh".to_string()].into();
        let only = root.pruned(&keep).unwrap();
        assert_eq!(only.leaves().len(), 1);
        assert_eq!(only.children[0].id, "secrets");
        let _ = std::fs::remove_dir_all(&home);
    }

    /// The key command's JSON survives quotes and backslashes in the names, and asks `security` for that item.
    #[test]
    fn key_command_writes_one_json_line() {
        let dir = std::env::temp_dir().join(format!("macysync-key-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A stand-in `security`: the key only for the right item, so a wrong -s or -a shows.
        std::fs::write(dir.join("security"), "#!/bin/sh\n[ \"$4\" = \"Tom's \\\"Safe\\\" Storage\" ] && [ \"$6\" = 'a\\b' ] && printf 'K3y+/='\n").unwrap();
        Command::new("chmod").arg("+x").arg(dir.join("security")).status().unwrap();
        let cmd = key_command("Tom's \"Safe\" Storage", "a\\b", "~/Applications/X Y.app");
        let out = Command::new("/bin/sh").args(["-c", &cmd]).env("PATH", format!("{}:/usr/bin:/bin", dir.display())).output().unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stdout)));
        assert_eq!(v["service"], "Tom's \"Safe\" Storage");
        assert_eq!(v["account"], "a\\b");
        assert_eq!(v["app"], "~/Applications/X Y.app");
        assert_eq!(v["key"], "K3y+/=");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
