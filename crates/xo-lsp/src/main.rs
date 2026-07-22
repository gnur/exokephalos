use clap::Parser;

#[derive(Debug, Parser)]
#[command(name = "xo-lsp", version, about = "Editor integration for exokephalos")]
struct Cli {}

fn main() {
    let _ = Cli::parse();
    eprintln!("xo-lsp protocol support will be introduced in implementation stage 8");
}
