mod bus;
mod cli;
mod look;
mod me;
mod scan;
mod seal;
mod session;
mod tail;
mod setup;
mod ui;
mod wire;

const HELP: &str = "MacySync: move a Mac to a new Mac. Any Mac sends to or takes from another; the sender offers, the receiver picks, an agent sets it up.

  macysync              open the window
  macysync --demo       the window on made-up Macs; touches nothing
  macysync --detect     print what this Mac reports about itself and the look it gets
  macysync --scan [all] print what this Mac can offer, with sizes and ids (all: every part)
  macysync --give <id> [--only a,b]   without the window: ask Mac <id> to receive, then send
  macysync --accept [--only a,b]      without the window: approve the request waiting here, then do this side
  macysync --tailscale [down]         start MacySync's own Tailscale node (apart from any other Tailscale), or stop it";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let first = args.first().map(String::as_str).unwrap_or("");
    match first {
        "--help" | "-h" => println!("{HELP}"),
        "--demo" => ui::run(true),
        "--detect" => {
            let hw = me::detect();
            println!("{}", serde_json::to_string_pretty(&hw).unwrap());
            let l = look::guess(&hw);
            println!("look: {} / {}", look::model(&l.model).map(|m| m.marketing_name.as_str()).unwrap_or("?"), l.color);
        }
        "--tailscale" if args.get(1).is_some_and(|a| a == "down") => tail::stop(),
        "--tailscale" => {
            if let Err(e) = tail::start(&me::Config::load().id) {
                eprintln!("macysync: {e}");
                std::process::exit(1);
            }
            let mut shown = String::new();
            loop {
                let s = tail::status().unwrap_or_default();
                if s.state == "Running" && !s.ip.is_empty() {
                    println!("MacySync's Tailscale is on: {}", s.ip);
                    break;
                }
                if !s.auth_url.is_empty() && s.auth_url != shown {
                    println!("Sign in to Tailscale: {}", s.auth_url);
                    shown = s.auth_url;
                }
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
        }
        "--give" | "--accept" => {
            let only: Vec<String> = args.iter().position(|a| a == "--only").and_then(|i| args.get(i + 1)).map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()).unwrap_or_default();
            let r = if first == "--give" {
                match args.get(1).filter(|a| !a.starts_with("--")) {
                    Some(peer) => cli::give(peer.trim_start_matches('#'), only),
                    None => Err("--give needs the other Mac's id".into()),
                }
            } else {
                cli::accept(only)
            };
            if let Err(e) = r {
                eprintln!("macysync: {e}");
                std::process::exit(1);
            }
        }
        "--scan" => {
            let home = me::home_dir();
            let root = scan::scan(&home, &["/Applications".into(), home.join("Applications")], true);
            let limit = if args.get(1).is_some_and(|a| a == "all") { usize::MAX } else { 2 };
            fn show(n: &scan::Node, depth: usize, limit: usize) {
                if depth > 0 {
                    let flags = format!("{}{}", if n.secret { " [secret]" } else { "" }, if n.off { " [off]" } else { "" });
                    println!("{}{} {:.1} MB  {}{flags}", "  ".repeat(depth - 1), n.name, n.size as f64 / 1e6, n.id);
                }
                if depth < limit {
                    n.children.iter().for_each(|c| show(c, depth + 1, limit));
                }
            }
            show(&root, 0, limit);
            println!("total {:.1} GB in {} leaves", root.size as f64 / 1e9, root.leaves().len());
        }
        _ if first.starts_with("--") && !ui::is_tooling_arg(first) => {
            eprintln!("unknown option {first}\n\n{HELP}");
            std::process::exit(2);
        }
        _ => ui::run(false),
    }
}
