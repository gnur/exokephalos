use clap::Parser;

#[derive(Debug, Parser)]
#[command(
    name = "xo-syncd",
    version,
    about = "Durable exokephalos replication peer"
)]
struct Cli {
    /// Directory containing local daemon state.
    #[arg(long, default_value = ".exo/syncd")]
    state_dir: std::path::PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let node = xo_core::iroh_node::IrohNode::persistent(&cli.state_dir).await?;
    println!("endpoint_id={}", node.endpoint_id());
    println!("author_id={}", node.author_id());
    println!("state={}", node.state_dir().display());
    tokio::signal::ctrl_c().await?;
    node.shutdown().await?;
    Ok(())
}
