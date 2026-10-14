//! `uucc`: Unofficial Uniwill Control Center from a terminal. It asks the running app over its pipe; the app does the
//! work, as if its window had been clicked.

// Shared with the app, which uses other parts of it.
#[allow(dead_code)]
#[path = "../../cli/protocol.rs"]
mod protocol;

mod args;
mod connect;
mod output;

use protocol::{Envelope, Reply, VERSION};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(run(&args));
}

/// Does what the command line says and returns the exit code.
fn run(args: &[String]) -> i32 {
    let command = match args::parse(args) {
        Ok(command) => command,
        Err(args::Usage(message)) => {
            eprintln!("uucc: {message}");
            return 1;
        }
    };
    if command.version {
        println!("uucc {}", env!("CARGO_PKG_VERSION"));
        return 0;
    }
    if let Some(topic) = &command.help {
        println!("{}", args::help(topic));
        return 0;
    }
    let Some(request) = command.request else { return 1 };
    let demo = std::env::var("UUCC_DEMO").is_ok_and(|v| v == "1");
    let envelope = Envelope { version: VERSION, request: request.clone() };
    let (code, message) = match connect::send(&envelope, demo, command.start) {
        Ok(Reply::Ok(result)) => {
            if command.json {
                println!("{}", output::ascii_json(&serde_json::to_string_pretty(&result).unwrap_or_default()));
            } else {
                println!("{}", output::text(&request, &result));
            }
            return 0;
        }
        Ok(Reply::Error { code, message }) => (code as i32, message),
        Err(failure) => (failure.code(), failure.message()),
    };
    if command.json {
        println!("{}", output::ascii_json(&serde_json::json!({ "error": { "code": code, "message": message } }).to_string()));
    } else {
        eprintln!("uucc: {message}");
    }
    code
}
