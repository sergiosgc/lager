use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use lager::inventory::Inventory;

/// A local web interface for editing an Ansible inventory database.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// The inventory database to edit. It must already exist.
    #[arg(long, default_value = "./database.sqlite")]
    database: PathBuf,

    /// The address to serve the web interface on.
    #[arg(long, default_value = "127.0.0.1:3000")]
    listen: SocketAddr,
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();

    let inventory = match Inventory::open(&args.database).await {
        Ok(inventory) => inventory,
        Err(e) => {
            eprintln!("lager: refusing to start: {e}");
            return ExitCode::FAILURE;
        }
    };

    let listener = match tokio::net::TcpListener::bind(args.listen).await {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("lager: cannot listen on {}: {e}", args.listen);
            return ExitCode::FAILURE;
        }
    };

    eprintln!(
        "lager: editing {} at http://{}",
        args.database.display(),
        args.listen
    );
    if let Err(e) = topcoat::serve(listener, lager::web::router(inventory)).await {
        eprintln!("lager: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
