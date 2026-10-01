//! Compatibility executable for the former server command.
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let result: std::io::Result<std::process::ExitStatus> =
        std::env::current_exe().and_then(|exe| {
            let name = if cfg!(windows) {
                "nio-de.exe"
            } else {
                "nio-de"
            };
            let mut command = Command::new(exe.with_file_name(name));
            command.args(std::env::args_os().skip(1));
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                Err(command.exec())
            }
            #[cfg(not(unix))]
            {
                command.status()
            }
        });
    match result {
        Ok(status) => ExitCode::from(status.code().unwrap_or(1) as u8),
        Err(error) => {
            eprintln!("noide-server: could not launch nio-de: {error}");
            ExitCode::FAILURE
        }
    }
}
