use std::process::ExitCode;

const USAGE: &str = "usage: lineworker [--version | --help]

lineworker keeps pull requests moving. It is at the design stage: see docs/design.md.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version" | "-V") => {
            println!("lineworker {}", lineworker::version());
            ExitCode::SUCCESS
        }
        None | Some("--help" | "-h") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("lineworker: unknown argument {other:?}\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}
