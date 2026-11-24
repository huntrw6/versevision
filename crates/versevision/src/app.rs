//! Parsing, errors and the dispatch of the command line (`typer`'s side of `cli.py`).

use std::io::{IsTerminal, Write};

use clap::error::{ContextKind, ContextValue, ErrorKind};
use clap::{ArgMatches, Command as ClapCommand, CommandFactory, FromArgMatches};

use crate::cmds;
use crate::def::{Cli, Command};
use crate::difflib::get_close_matches;
use crate::help::{render_error, render_help};
use crate::paths::Paths;
use crate::rich::Console;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The terminal facts the output depends on (`rich.console.Console`'s own detection, for the three streams).
#[derive(Debug, Clone)]
pub struct Term {
    pub width: usize,
    pub stdout_tty: bool,
    pub stderr_tty: bool,
}

impl Term {
    pub fn detect() -> Term {
        let stdout_tty = std::io::stdout().is_terminal();
        let stderr_tty = std::io::stderr().is_terminal();
        let from_env = std::env::var("COLUMNS")
            .ok()
            .filter(|c| !c.is_empty() && c.chars().all(|ch| ch.is_ascii_digit()))
            .and_then(|c| c.parse::<usize>().ok());
        let width = from_env
            .or_else(|| terminal_size::terminal_size().map(|(w, _)| w.0 as usize))
            .filter(|w| *w > 0)
            .unwrap_or(80);
        Term {
            width,
            stdout_tty,
            stderr_tty,
        }
    }

    fn color_for(tty: bool) -> bool {
        if std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
            return false;
        }
        let forced = std::env::var_os("FORCE_COLOR").is_some_and(|v| !v.is_empty());
        let dumb = std::env::var("TERM").is_ok_and(|t| t == "dumb");
        (tty || forced) && !dumb
    }

    pub fn out(&self) -> Console {
        Console::new(self.width, Term::color_for(self.stdout_tty))
    }

    pub fn err(&self) -> Console {
        Console::new(self.width, Term::color_for(self.stderr_tty))
    }
}

/// What every command works with.
pub struct Rt {
    pub paths: Paths,
    pub term: Term,
}

pub fn out(text: &str) {
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(text.as_bytes());
    let _ = stdout.flush();
}

pub fn err(text: &str) {
    let mut stderr = std::io::stderr().lock();
    let _ = stderr.write_all(text.as_bytes());
    let _ = stderr.flush();
}

/// `typer.echo(text)`.
pub fn echo(text: &str) {
    out(&format!("{text}\n"));
}

pub fn echo_err(text: &str) {
    err(&format!("{text}\n"));
}

/// The command (and its path, `versevision config set`) a list of arguments names, as far as it goes.
fn resolve<'a>(root: &'a ClapCommand, args: &[String]) -> (&'a ClapCommand, String) {
    let mut current = root;
    let mut path = vec!["versevision".to_string()];
    for arg in args {
        if arg.starts_with('-') {
            continue;
        }
        if let Some(sub) = current.get_subcommands().find(|c| c.get_name() == arg) {
            current = sub;
            path.push(arg.clone());
        }
    }
    (current, path.join(" "))
}

fn context_text(error: &clap::Error, kind: ContextKind) -> Option<String> {
    match error.get(kind) {
        Some(ContextValue::String(s)) => Some(s.clone()),
        Some(ContextValue::Strings(v)) => Some(v.join(" ")),
        _ => None,
    }
}

/// The message click/typer would print for a parse failure.
fn click_message(error: &clap::Error, cmd: &ClapCommand) -> String {
    match error.kind() {
        ErrorKind::InvalidSubcommand => {
            let name = context_text(error, ContextKind::InvalidSubcommand).unwrap_or_default();
            let names: Vec<String> = cmd
                .get_subcommands()
                .filter(|c| !c.is_hide_set())
                .map(|c| c.get_name().to_string())
                .collect();
            let matches = get_close_matches(&name, &names, 3, 0.6);
            let mut message = format!("No such command {}.", crate::rich_repr(&name));
            if !matches.is_empty() {
                let suggestions: Vec<String> =
                    matches.iter().map(|m| crate::rich_repr(m)).collect();
                message = format!(
                    "{}. Did you mean {}?",
                    message.trim_end_matches('.'),
                    suggestions.join(", ")
                );
            }
            message
        }
        ErrorKind::UnknownArgument => {
            let arg = context_text(error, ContextKind::InvalidArg).unwrap_or_default();
            let arg = arg.split_whitespace().next().unwrap_or("").to_string();
            if arg.starts_with('-') {
                let name = arg.split('=').next().unwrap_or(&arg).to_string();
                let longs: Vec<String> = cmd
                    .get_arguments()
                    .filter(|a| !a.is_hide_set() || a.get_long().is_some())
                    .filter_map(|a| a.get_long().map(|l| format!("--{l}")))
                    .collect();
                let possibilities = get_close_matches(&name, &longs, 3, 0.6);
                if possibilities.is_empty() {
                    format!("No such option: {name}")
                } else {
                    let mut sorted = possibilities;
                    sorted.sort();
                    format!(
                        "No such option: {name} (Possible options: {})",
                        sorted.join(", ")
                    )
                }
            } else {
                let mut rest: Vec<String> = vec![arg];
                if let Some(extra) = context_text(error, ContextKind::InvalidArg) {
                    let parts: Vec<&str> = extra.split_whitespace().collect();
                    if parts.len() > 1 {
                        rest = parts.iter().map(|s| s.to_string()).collect();
                    }
                }
                format!("Got unexpected extra argument(s) ({})", rest.join(" "))
            }
        }
        ErrorKind::MissingRequiredArgument => {
            let names = context_text(error, ContextKind::InvalidArg).unwrap_or_default();
            let first = names.split_whitespace().next().unwrap_or("").to_string();
            let cleaned = first.trim_matches(|c| c == '<' || c == '>').to_lowercase();
            format!("Missing argument '{cleaned}'.")
        }
        ErrorKind::ValueValidation | ErrorKind::InvalidValue => {
            let arg = context_text(error, ContextKind::InvalidArg).unwrap_or_default();
            let value = context_text(error, ContextKind::InvalidValue).unwrap_or_default();
            let text = error.to_string();
            if text.contains("a value is required") {
                let option = arg.split_whitespace().next().unwrap_or("").to_string();
                return format!("Option '{option}' requires an argument.");
            }
            let option = arg.split_whitespace().next().unwrap_or("").to_string();
            // our value parsers answer "'abc' is not a valid int."
            let reason = text
                .split(": ")
                .last()
                .unwrap_or("")
                .lines()
                .next()
                .unwrap_or("")
                .to_string();
            if reason.contains("is not a valid") {
                format!("Invalid value for '{option}': {reason}")
            } else {
                format!("Invalid value for '{option}': {value}")
            }
        }
        _ => error
            .to_string()
            .lines()
            .next()
            .unwrap_or("")
            .trim_start_matches("error: ")
            .to_string(),
    }
}

/// Run the command line. Returns the exit code.
pub fn run(args: Vec<String>) -> i32 {
    let term = Term::detect();
    let rt = Rt {
        paths: Paths::from_process(),
        term,
    };
    let root = Cli::command();

    // `--version` is eager: it answers before anything else is looked at
    if args
        .iter()
        .take_while(|a| *a != "--")
        .any(|a| a == "--version")
    {
        echo(&format!("VerseVision {VERSION}"));
        return 0;
    }
    if args.is_empty() {
        out(&render_help(&root, "versevision", &rt.term.out()));
        return 2;
    }
    let matches: ArgMatches = match root.clone().try_get_matches_from(
        std::iter::once("versevision".to_string()).chain(args.iter().cloned()),
    ) {
        Ok(m) => m,
        Err(error) => {
            let (cmd, path) = resolve(&root, &args);
            return match error.kind() {
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
                    out(&format!("{}\n", render_help(cmd, &path, &rt.term.out())));
                    0
                }
                _ => {
                    let message = click_message(&error, cmd);
                    // click blames the group when the subcommand is unknown, the command otherwise
                    err(&render_error(cmd, &path, &message, &rt.term.err()));
                    2
                }
            };
        }
    };
    let cli = match Cli::from_arg_matches(&matches) {
        Ok(c) => c,
        Err(error) => {
            let (cmd, path) = resolve(&root, &args);
            let message = click_message(&error, cmd);
            err(&render_error(cmd, &path, &message, &rt.term.err()));
            return 2;
        }
    };
    if cli.install_completion || cli.show_completion {
        err("Shell completion is not available in this build of VerseVision.\n");
        return 1;
    }
    let Some(command) = cli.command else {
        out(&render_help(&root, "versevision", &rt.term.out()));
        return 2;
    };
    match cmds::dispatch(&rt, &root, command) {
        Ok(code) => code,
        Err(Usage { path, message }) => {
            let (cmd, _) = resolve(
                &root,
                &path
                    .split_whitespace()
                    .skip(1)
                    .map(str::to_string)
                    .collect::<Vec<_>>(),
            );
            err(&render_error(cmd, &path, &message, &rt.term.err()));
            2
        }
    }
}

/// A usage error found after parsing (`typer.BadParameter`, a missing sub-command): printed in the red box, exit 2.
pub struct Usage {
    pub path: String,
    pub message: String,
}

pub fn group_help_needed(command: &Command) -> bool {
    matches!(
        command,
        Command::Config { command: None, .. }
            | Command::Propresenter { command: None, .. }
            | Command::Versions { command: None, .. }
            | Command::Providers { command: None, .. }
            | Command::Cache { command: None, .. }
            | Command::Template { command: None, .. }
            | Command::Autostart { command: None, .. }
            | Command::Keys { command: None, .. }
    )
}
