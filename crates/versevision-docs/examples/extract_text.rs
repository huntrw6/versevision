//! `cargo run -p versevision-docs --example extract_text -- <file>`: the text of a .txt, .md, .html, .docx or .pdf file as VerseVision reads it.
use std::process::ExitCode;

fn main() -> ExitCode {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: extract_text <file>");
        return ExitCode::from(2);
    };
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("{path}: {error}");
            return ExitCode::from(1);
        }
    };
    match versevision_docs::extract_text(&bytes, &path) {
        Ok((text, info)) => {
            print!("{text}");
            eprintln!("{}", info.to_json());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{}", error.message());
            ExitCode::from(1)
        }
    }
}
