mod api;
mod db;
mod indexer;
mod models;
mod parser;
mod rpc;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Parser)]
#[command(name = "dash-indexer")]
#[command(about = "Dash blockchain indexer for Ordinals and DAR-20 tokens", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the indexer and API server
    Start {
        /// Start block height (default: 0 for genesis, or last indexed)
        #[arg(short, long)]
        start_block: Option<u64>,

        /// API server port
        #[arg(short, long, default_value = "3939")]
        port: u16,

        /// Only run API server (no indexing)
        #[arg(long)]
        api_only: bool,
    },
    /// Reindex from a specific block
    Reindex {
        /// Block height to start reindexing from
        #[arg(short, long, default_value = "0")]
        from_block: u64,
    },
    /// Show indexer status
    Status,
    /// Parse a single transaction for debugging
    ParseTx {
        /// Transaction ID to parse
        txid: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "dash_indexer=info,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Load environment variables
    dotenvy::dotenv().ok();

    let cli = Cli::parse();

    match cli.command {
        Commands::Start {
            start_block,
            port,
            api_only,
        } => {
            info!("Starting Dash Indexer...");

            // Initialize database
            let db = db::Database::new().await?;
            db.run_migrations().await?;

            // Initialize RPC client
            let rpc = rpc::DashRpc::from_env()?;

            // Test connection
            let blockchain_info = rpc.get_blockchain_info().await?;
            info!(
                "Connected to Dash node. Chain: {}, Blocks: {}",
                blockchain_info.chain, blockchain_info.blocks
            );

            if !api_only {
                // Start indexer in background
                let indexer = indexer::Indexer::new(db.clone(), rpc.clone());
                let start = start_block.unwrap_or_else(|| db.get_last_indexed_block_sync());
                
                tokio::spawn(async move {
                    if let Err(e) = indexer.start(start).await {
                        tracing::error!("Indexer error: {}", e);
                    }
                });
            }

            // Start API server
            info!("Starting API server on port {}", port);
            api::start_server(db, port).await?;
        }
        Commands::Reindex { from_block } => {
            info!("Reindexing from block {}...", from_block);

            let db = db::Database::new().await?;
            db.run_migrations().await?;
            let rpc = rpc::DashRpc::from_env()?;

            // Clear data from specified block
            db.clear_from_block(from_block).await?;

            // Start indexer
            let indexer = indexer::Indexer::new(db, rpc);
            indexer.start(from_block).await?;
        }
        Commands::Status => {
            let db = db::Database::new().await?;
            db.run_migrations().await?;
            let rpc = rpc::DashRpc::from_env()?;

            let last_indexed = db.get_last_indexed_block().await?;
            let blockchain_info = rpc.get_blockchain_info().await?;

            println!("=== Dash Indexer Status ===");
            println!("Last indexed block: {}", last_indexed);
            println!("Current chain height: {}", blockchain_info.blocks);
            println!(
                "Blocks behind: {}",
                blockchain_info.blocks.saturating_sub(last_indexed)
            );

            let stats = db.get_stats().await?;
            println!("\n=== Index Statistics ===");
            println!("Total inscriptions: {}", stats.total_inscriptions);
            println!("Total DAR-20 tokens: {}", stats.total_tokens);
            println!("Total DAR-20 holders: {}", stats.total_holders);
        }
        Commands::ParseTx { txid } => {
            info!("Parsing transaction: {}", txid);

            let rpc = rpc::DashRpc::from_env()?;
            let tx = rpc.get_raw_transaction(&txid).await?;

            println!("=== Transaction {} ===", txid);
            println!("Version: {}", tx.version);
            println!("Inputs: {}", tx.vin.len());
            println!("Outputs: {}", tx.vout.len());

            // Parse for inscriptions
            if let Some(inscription) = parser::parse_inscription(&tx) {
                println!("\n=== Inscription Found ===");
                println!("Content Type: {}", inscription.content_type);
                println!("Content Size: {} bytes", inscription.content.len() / 2);

                // Check for DAR-20
                if let Some(dar20) = parser::parse_dar20(&inscription) {
                    println!("\n=== DAR-20 Operation ===");
                    println!("Protocol: {}", dar20.protocol);
                    println!("Operation: {:?}", dar20.operation);
                    println!("Tick: {}", dar20.tick);
                    if let Some(max) = &dar20.max {
                        println!("Max Supply: {}", max);
                    }
                    if let Some(lim) = &dar20.lim {
                        println!("Mint Limit: {}", lim);
                    }
                    if let Some(amt) = &dar20.amt {
                        println!("Amount: {}", amt);
                    }
                }
            } else {
                println!("\nNo inscription found in this transaction.");
            }
        }
    }

    Ok(())
}

