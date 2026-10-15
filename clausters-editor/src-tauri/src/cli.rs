//! The command line: what the binary is asked before it opens a window.
//!
//! The arguments are parsed first thing in `run`, ahead of Tauri, so an option
//! that only answers (`--help`, `--version`) or a mistake in them prints and
//! exits without a window ever appearing. What reaches the application is `Cli`; a new option
//! is a field here and, if the page needs it, one in what `cli_files` (or a
//! command beside it) hands over.

use std::path::{Path, PathBuf};

use clap::Parser;

/// A lightweight Python editor with an interactive session.
#[derive(Parser, Debug, Default)]
#[command(name = "clausters-editor", version)]
pub struct Cli {
    /// Files to open, each in a tab of its own. A file that does not exist yet
    /// opens empty and is created when it is saved.
    #[arg(value_name = "FILE", value_parser = file_arg)]
    pub files: Vec<PathBuf>,
}

/// A `FILE` argument, made absolute against the directory the command runs in
/// (the application's own working directory may be another). A directory is
/// refused: there is nothing to open in a tab.
fn file_arg(arg: &str) -> Result<PathBuf, String> {
    let path = std::path::absolute(Path::new(arg)).map_err(|e| e.to_string())?;
    if path.is_dir() {
        return Err("is a directory".into());
    }
    Ok(path)
}

/// A file named on the command line, as the page receives it.
#[derive(serde::Serialize)]
pub struct CliFile {
    path: String,
    /// Whether it exists: one that does not opens as an empty tab with its path.
    exists: bool,
}

/// The files named on the command line, in their order.
#[tauri::command]
pub fn cli_files(cli: tauri::State<Cli>) -> Vec<CliFile> {
    cli.files
        .iter()
        .map(|p| CliFile {
            path: p.to_string_lossy().into_owned(),
            exists: p.exists(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("clausters-editor").chain(args.iter().copied()))
    }

    #[test]
    fn files_are_absolute() {
        let cli = parse(&["a.py", "/tmp/b.py"]).unwrap();
        assert_eq!(cli.files.len(), 2);
        assert!(cli.files.iter().all(|p| p.is_absolute()));
        assert_eq!(cli.files[1], PathBuf::from("/tmp/b.py"));
    }

    #[test]
    fn answers_without_running() {
        use clap::error::ErrorKind;
        assert_eq!(
            parse(&["--help"]).unwrap_err().kind(),
            ErrorKind::DisplayHelp
        );
        assert_eq!(
            parse(&["-V"]).unwrap_err().kind(),
            ErrorKind::DisplayVersion
        );
        assert_eq!(
            parse(&["--nope"]).unwrap_err().kind(),
            ErrorKind::UnknownArgument
        );
    }

    #[test]
    fn a_directory_is_refused() {
        let dir = std::env::temp_dir();
        assert!(parse(&[dir.to_str().unwrap()]).is_err());
    }

    #[test]
    fn a_dash_file_after_the_separator() {
        let cli = parse(&["--", "--help"]).unwrap();
        assert!(cli.files[0].ends_with("--help"));
    }
}
