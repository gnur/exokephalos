use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "xo-admin", version, about = "Workspace administration")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Validate every Markdown file in an existing workspace without modifying it.
    AuditWorkspace { path: std::path::PathBuf },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    match Cli::parse().command {
        Command::AuditWorkspace { path } => {
            let mut valid = 0_u64;
            let mut invalid = 0_u64;
            audit(&path, &mut valid, &mut invalid)?;
            println!("valid={valid} invalid={invalid}");
            if invalid > 0 {
                std::process::exit(1);
            }
        }
    }
    Ok(())
}

fn audit(path: &std::path::Path, valid: &mut u64, invalid: &mut u64) -> std::io::Result<()> {
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            if entry.file_name() != ".exo" {
                audit(&path, valid, invalid)?;
            }
        } else if path.extension().is_some_and(|extension| extension == "md") {
            match std::fs::read_to_string(&path)
                .map_err(|error| error.to_string())
                .and_then(|content| {
                    xo_core::markdown::parse(&content)
                        .map(|_| ())
                        .map_err(|error| error.to_string())
                }) {
                Ok(()) => *valid += 1,
                Err(error) => {
                    *invalid += 1;
                    eprintln!("{}: {error}", path.display());
                }
            }
        }
    }
    Ok(())
}
