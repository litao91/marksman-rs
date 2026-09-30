//! Marksman is a language server for Markdown.

use clap::{Parser, Subcommand};
use log::info;

/// CLI shape mirrors the original: `marksman` with no subcommand starts the
/// server with default verbosity.
#[derive(Parser, Debug)]
#[command(name = "marksman", about = "Marksman is a language server for Markdown", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Start LSP server on stdin/stdout
    Server {
        /// Set logging verbosity level
        #[arg(short, long, default_value_t = 2)]
        verbose: u8,
    },
}

fn configure_logging(verbosity: u8) {
    use std::io::Write;

    let level = match verbosity {
        0 => log::LevelFilter::Error,
        1 => log::LevelFilter::Warn,
        2 => log::LevelFilter::Info,
        3 => log::LevelFilter::Debug,
        _ => log::LevelFilter::Trace,
    };

    // Everything goes to stderr: stdout carries the LSP framing.
    env_logger::Builder::new()
        .filter_level(level)
        .format(|buf, record| {
            writeln!(
                buf,
                "[{} {:<5}] <{}> {}",
                buf.timestamp(),
                record.level(),
                record.target(),
                record.args()
            )
        })
        .init();
}

fn start_lsp(verbosity: u8) -> i32 {
    configure_logging(verbosity);

    let version = env!("CARGO_PKG_VERSION");
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    info!("Starting Marksman LSP server: version={version}, os={os}, arch={arch}");

    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(err) => {
            log::error!("Couldn't start the async runtime: {err}");
            return 1;
        }
    };

    match runtime.block_on(marksman::server::start()) {
        Ok(()) => {
            log::trace!("Stopped Marksman LSP server");
            0
        }
        Err(err) => {
            log::error!("Marksman LSP server failed: {err:?}");
            1
        }
    }
}

fn main() {
    let cli = Cli::parse();

    let code = match cli.command {
        Some(Command::Server { verbose }) => start_lsp(verbose),
        None => start_lsp(2),
    };

    std::process::exit(code);
}
