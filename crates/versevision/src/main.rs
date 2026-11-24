//! VerseVision command line. S1 only knows `--version`; the commands arrive with the later stages.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [flag] if flag == "--version" || flag == "-V" => {
            println!("versevision {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("usage: versevision --version");
            ExitCode::from(2)
        }
    }
}
