mod bus;
mod look;
mod me;
mod seal;
mod setup;
mod ui;

const HELP: &str = "MacySync: move a Mac to a new Mac. The old one offers, the new one picks, an agent sets it up.

  macysync              open the window
  macysync --demo       the window on made-up Macs; touches nothing
  macysync --detect     print what this Mac reports about itself and the look it gets";

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
        _ if first.starts_with("--") && !ui::is_tooling_arg(first) => {
            eprintln!("unknown option {first}\n\n{HELP}");
            std::process::exit(2);
        }
        _ => ui::run(false),
    }
}
