use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use lineworker::decide::Config;
use lineworker::github::GitHub;

const USAGE: &str = "usage: lineworker scan --github OWNER/REPO
       lineworker [--version | --help]

scan    What lineworker would do about each open pull request of a repository: which are
        stuck, why and for how long, and what it would do. It changes nothing.
        The token is read from GITHUB_TOKEN or GH_TOKEN; without one only public
        repositories can be read (60 requests an hour). GITHUB_API_URL sets another API.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["--version" | "-V"] => {
            println!("lineworker {}", lineworker::version());
            ExitCode::SUCCESS
        }
        [] | ["--help" | "-h"] => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        ["scan", "--github", repo] => scan(repo),
        _ => {
            eprintln!("lineworker: unknown arguments {args:?}\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn scan(repo: &str) -> ExitCode {
    if repo.split('/').count() != 2 || repo.split('/').any(str::is_empty) {
        eprintln!("lineworker: the repository is OWNER/REPO, not {repo:?}");
        return ExitCode::from(2);
    }
    let token = std::env::var("GITHUB_TOKEN")
        .ok()
        .or_else(|| std::env::var("GH_TOKEN").ok());
    let api = std::env::var("GITHUB_API_URL").ok();
    let github = GitHub::new(api.as_deref(), token);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    match github.snapshot(repo) {
        Ok(snapshot) => {
            print!(
                "{}",
                lineworker::scan::report(&snapshot, now, &Config::default())
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("lineworker: {e}");
            ExitCode::FAILURE
        }
    }
}
