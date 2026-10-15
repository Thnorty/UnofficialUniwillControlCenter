//! `uucc`: Unofficial Uniwill Control Center from a terminal. It asks the running app over its pipe; the app does the
//! work, as if its window had been clicked.

// Shared with the app, which uses other parts of it.
#[allow(dead_code)]
#[path = "../../cli/protocol.rs"]
mod protocol;

mod args;
#[cfg(windows)]
mod connect;
#[cfg(target_os = "linux")]
#[path = "connect_linux.rs"]
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
            let (out, err) = usage_output(args, &message);
            if let Some(out) = out {
                println!("{out}");
            }
            if let Some(err) = err {
                eprintln!("{err}");
            }
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
        println!("{}", error_json(code, &message));
    } else {
        eprintln!("uucc: {message}");
    }
    code
}

/// What a mistake in the command line prints: on stdout and on stderr.
fn usage_output(args: &[String], message: &str) -> (Option<String>, Option<String>) {
    // Read before the line is parsed: a script that asked for JSON gets it whatever went wrong.
    if args.iter().any(|a| a == "--json") {
        (Some(error_json(1, message)), None)
    } else {
        (None, Some(format!("uucc: {message}")))
    }
}

fn error_json(code: i32, message: &str) -> String {
    output::ascii_json(&serde_json::json!({ "error": { "code": code, "message": message } }).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn a_mistake_with_json_is_json() {
        let (out, err) = usage_output(&line(&["mode", "nosuch", "--json"]), "unknown mode \"nosuch\"");
        let out: serde_json::Value = serde_json::from_str(&out.expect("nothing on stdout")).unwrap();
        assert_eq!(out["error"]["code"], 1);
        assert_eq!(out["error"]["message"], "unknown mode \"nosuch\"");
        assert_eq!(err, None);
    }

    #[test]
    fn a_mistake_without_json_is_text() {
        let (out, err) = usage_output(&line(&["mode", "nosuch"]), "unknown mode");
        assert_eq!(out, None);
        assert_eq!(err.as_deref(), Some("uucc: unknown mode"));
    }
}
