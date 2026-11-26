mod bus;
mod look;
mod me;
mod scan;
mod seal;
mod setup;
mod ui;
mod wire;

const HELP: &str = "MacySync: move a Mac to a new Mac. Any Mac sends to or takes from another; the sender offers, the receiver picks, an agent sets it up.

  macysync              open the window
  macysync --demo       the window on made-up Macs; touches nothing
  macysync --detect     print what this Mac reports about itself and the look it gets
  macysync --scan       print what this Mac can offer, with sizes";

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
        "--scan" => {
            let home = me::home_dir();
            let root = scan::scan(&home, &["/Applications".into(), home.join("Applications")], true);
            fn show(n: &scan::Node, depth: usize) {
                if depth > 0 {
                    let flags = format!("{}{}", if n.secret { " [secret]" } else { "" }, if n.off { " [off]" } else { "" });
                    println!("{}{} {:.1} MB{flags}", "  ".repeat(depth - 1), n.name, n.size as f64 / 1e6);
                }
                if depth < 2 {
                    n.children.iter().for_each(|c| show(c, depth + 1));
                }
            }
            show(&root, 0);
            println!("total {:.1} GB in {} leaves", root.size as f64 / 1e9, root.leaves().len());
        }
        _ if first.starts_with("--") && !ui::is_tooling_arg(first) => {
            eprintln!("unknown option {first}\n\n{HELP}");
            std::process::exit(2);
        }
        _ => ui::run(false),
    }
}
