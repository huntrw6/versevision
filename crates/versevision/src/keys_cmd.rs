//! `versevision keys set|remove|status`: write-only access to the Bible API keys. A key comes from a hidden prompt (a terminal) or
//! from standard input (one line), never from an argument, and no code path here prints it, logs it or puts it in an error.

use std::io::{BufRead, IsTerminal};

use versevision_bible::KeyStore;
use versevision_pp::errors::VvError;

use crate::app::{echo, out, Rt};
use crate::cmds::{dumps, fail, is_json};
use crate::def::KeysCommand;
use crate::keys::{store, variable_for, PROVIDERS};
use crate::rich::{markup, Overflow, Style, Table, R};

fn unknown(provider: &str) -> VvError {
    let names: Vec<&str> = PROVIDERS.iter().map(|(n, _)| *n).collect();
    VvError::configuration(format!(
        "Unknown provider '{provider}': choose one of {}",
        names.join(", ")
    ))
}

/// The key text, from the hidden prompt or from one line of standard input. `None` when there was nothing to read.
fn read_key(provider: &str) -> std::io::Result<Option<String>> {
    if std::io::stdin().is_terminal() {
        let text = rpassword::prompt_password(format!("{provider} key (typing is hidden): "))?;
        return Ok(Some(text));
    }
    let mut line = String::new();
    let read = std::io::stdin().lock().read_line(&mut line)?;
    if read == 0 {
        return Ok(None);
    }
    Ok(Some(line.trim_end_matches(['\n', '\r']).to_string()))
}

pub fn keys(rt: &Rt, command: KeysCommand) -> i32 {
    let file_store = store(&rt.paths.keys_file);
    match command {
        KeysCommand::Set { provider, .. } => {
            let Some(variable) = variable_for(&provider) else {
                return fail(rt, &unknown(&provider));
            };
            let text = match read_key(&provider) {
                Ok(Some(t)) => t,
                Ok(None) => {
                    return fail(
                        rt,
                        &VvError::configuration("No key was given on standard input"),
                    )
                }
                Err(e) => {
                    return fail(
                        rt,
                        &VvError::configuration(format!("Could not read the key: {}", e.kind())),
                    )
                }
            };
            match file_store.set_key(variable, crate::keys::secret(text.trim())) {
                Ok(()) => {
                    out(&rt
                        .term
                        .out()
                        .print(&format!("Stored the key for {provider}.")));
                    0
                }
                Err(e) => fail(rt, &VvError::configuration(e.to_string())),
            }
        }
        KeysCommand::Remove { provider, .. } => {
            let Some(variable) = variable_for(&provider) else {
                return fail(rt, &unknown(&provider));
            };
            match file_store.remove_key(variable) {
                Ok(true) => out(&rt
                    .term
                    .out()
                    .print(&format!("Removed the stored key for {provider}."))),
                Ok(false) => out(&rt
                    .term
                    .out()
                    .print(&format!("There was no stored key for {provider}."))),
                Err(e) => return fail(rt, &VvError::configuration(e.to_string())),
            }
            if std::env::var_os(variable).is_some_and(|v| !v.is_empty()) {
                out(&rt.term.out().print(&format!(
                    "The environment variable {variable} is still set; it is not touched."
                )));
            }
            0
        }
        KeysCommand::Status(a) => {
            let rows: Vec<(&str, bool)> = PROVIDERS
                .iter()
                .map(|(n, v)| (*n, file_store.has_key(v)))
                .collect();
            if is_json(&a.output) {
                let items: Vec<serde_json::Value> = rows
                    .iter()
                    .map(|(n, s)| serde_json::json!({"provider": n, "set": s}))
                    .collect();
                echo(&dumps(&serde_json::Value::Array(items)));
            } else {
                let mut table = Table::new(&["Provider", "Key"]);
                for (name, set) in rows {
                    table.add_row(
                        [name, if set { "set" } else { "not set" }]
                            .iter()
                            .map(|c| R::Text {
                                cells: markup(c, Style::NONE),
                                overflow: Overflow::Ellipsis,
                                no_wrap: false,
                            })
                            .collect(),
                    );
                }
                out(&rt.term.out().print_r(&R::Table(Box::new(table))));
            }
            0
        }
    }
}
