//! `llmtodo schema`: every command and flag, plus the API route table.

use clap::CommandFactory;
use serde_json::{Value, json};

use crate::Cli;

fn describe(cmd: &clap::Command, path: &str) -> Vec<Value> {
    let mut out = Vec::new();
    let subs: Vec<&clap::Command> = cmd
        .get_subcommands()
        .filter(|c| c.get_name() != "help")
        .collect();
    if (subs.is_empty() || path.is_empty()) && !path.is_empty() {
        let args: Vec<Value> = cmd
            .get_arguments()
            .filter(|a| !a.is_global_set() && a.get_id() != "help" && a.get_id() != "version")
            .map(|a| {
                let possible: Vec<String> = a
                    .get_possible_values()
                    .iter()
                    .map(|v| v.get_name().to_string())
                    .collect();
                json!({
                    "name": a.get_id().as_str(),
                    "long": a.get_long().map(|l| format!("--{l}")),
                    "short": a.get_short().map(|s| format!("-{s}")),
                    "positional": a.is_positional(),
                    "required": a.is_required_set(),
                    "repeatable": matches!(a.get_action(), clap::ArgAction::Append),
                    "takes_value": a.get_action().takes_values(),
                    "values": if possible.is_empty() { Value::Null } else { json!(possible) },
                    "help": a.get_help().map(std::string::ToString::to_string),
                })
            })
            .collect();
        out.push(json!({ "command": path, "about": cmd.get_about().map(std::string::ToString::to_string), "args": args }));
    }
    for s in subs {
        let p = if path.is_empty() {
            s.get_name().to_string()
        } else {
            format!("{path} {}", s.get_name())
        };
        out.extend(describe(s, &p));
    }
    out
}

pub fn commands() -> Vec<Value> {
    describe(&Cli::command(), "")
}

pub fn global_flags() -> Vec<Value> {
    Cli::command()
        .get_arguments()
        .filter(|a| a.is_global_set())
        .map(|a| json!({ "long": a.get_long().map(|l| format!("--{l}")), "short": a.get_short().map(|s| format!("-{s}")), "help": a.get_help().map(std::string::ToString::to_string) }))
        .collect()
}

pub fn schema() -> Value {
    let exit_codes = json!({
        "0": "success", "1": "unknown error", "2": "bad flags or arguments", "3": "not found",
        "4": "claim held by another session", "5": "stale version on a description or document write",
        "6": "validation failed", "7": "server unreachable and could not start", "8": "input needed, but --no-input is set",
        "9": "this machine is not the primary", "10": "patch matched zero times or more than once", "11": "forbidden, such as prune by an agent"
    });
    let error_codes: serde_json::Map<String, Value> = llmtodo_api_types::ErrorCode::ALL
        .iter()
        .map(|c| (c.name(), json!(c.exit_code())))
        .collect();
    json!({
        "version": llmtodo_api_types::VERSION,
        "error_codes": error_codes,
        "global_flags": global_flags(),
        "commands": commands(),
        "exit_codes": exit_codes,
        "api": llmtodo_api_types::routes::describe(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every API route names a CLI command that exists.
    #[test]
    fn every_route_has_a_command() {
        let cmds: Vec<String> = commands()
            .iter()
            .map(|c| c["command"].as_str().unwrap().to_string())
            .collect();
        for r in llmtodo_api_types::routes::ROUTES {
            let words: Vec<&str> = r
                .cli
                .split_whitespace()
                .take_while(|w| !w.starts_with('-'))
                .collect();
            let name = words.join(" ");
            assert!(
                cmds.contains(&name),
                "route {} {} names `{}`, which is not a CLI command",
                r.method.as_str(),
                r.path,
                r.cli
            );
        }
    }

    #[test]
    fn clap_tree_is_valid() {
        Cli::command().debug_assert();
    }
}
