use std::process::ExitCode;

fn main() -> ExitCode {
    match orcalens::cli::run(std::env::args_os()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("orcalens: {error}");
            ExitCode::FAILURE
        }
    }
}
