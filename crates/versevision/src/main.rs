//! `versevision`: see the library for the commands.

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = versevision_cli::app::run(args);
    std::process::ExitCode::from(code.clamp(0, 255) as u8)
}
