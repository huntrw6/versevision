//! The VerseVision command line (`cli.py`, `cli_run.py`, `cli_explain.py`, `cli_themes.py`, `autostart.py` of the Python app).
//!
//! * [`def`]: the commands as clap sees them (names, options, help texts, defaults);
//! * [`help`], [`rich`]: typer's help pages and error boxes and the parts of Python's `rich` the output needs, so a table, a panel or a
//!   wrapped line comes out the same bytes when the output is not a terminal;
//! * [`app`]: parsing, errors and the commands themselves.

pub mod app;
pub mod autostart;
pub mod block;
pub mod cmds;
pub mod def;
pub mod difflib;
pub mod help;
pub mod keys;
pub mod keys_cmd;
pub mod legacy;
pub mod legacy_import;
pub mod paths;
pub mod render;
pub mod rich;
pub mod runcmds;
pub mod serve;
pub mod themes;

/// Python's `repr(float)`.
pub fn pyfloat(x: f64) -> String {
    versevision_pp::pyjson::float_repr(x)
}

/// Python's `repr(str)`.
pub fn rich_repr(text: &str) -> String {
    versevision_pp::pyrepr::py_repr(text)
}
