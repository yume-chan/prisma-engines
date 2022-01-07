#![deny(rust_2018_idioms, unsafe_code)]

mod commands;
mod logger;

use crate::logger::log_error_and_exit;
use migration_core::rpc_api;

const HELPTEXT: &str = r#"
When no subcommand is specified, the migration engine will default to starting as a JSON-RPC server over stdio

USAGE:
    migration-engine [OPTIONS] [SUBCOMMAND]

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

OPTIONS:
    -d, --datamodel <FILE>    Path to the datamodel

SUBCOMMANDS:
    cli     Doesn't start a server, but allows running specific commands against Prisma
    help    Prints this message or the help of the given subcommand(s)
"#;

#[tokio::main]
async fn main() -> Result<(), pico_args::Error> {
    set_panic_hook();
    logger::init_logger();

    let mut args = pico_args::Arguments::from_env();

    if args.contains("-V") || args.contains("--version") {
        eprintln!("{}", env!("GIT_HASH"));
        return Ok(());
    }

    match args.subcommand()?.as_deref() {
        None => {
            if args.contains("-h") || args.contains("--help") {
                eprintln!("{}", HELPTEXT);
                return Ok(());
            }

            let datamodel_location = match (
                args.opt_value_from_str::<_, String>("--datamodel")?,
                args.opt_value_from_str("-d")?,
            ) {
                (Some(arg), None) | (None, Some(arg)) => arg,
                (Some(_), Some(_)) => {
                    eprintln!(
                        "Both -d and --datamodel were provided. Please provide only one.\n\n{}",
                        HELPTEXT
                    );
                    std::process::exit(1);
                }
                (None, None) => {
                    eprintln!("The required --datamodel argument is missing.\n\n{}", HELPTEXT);
                    std::process::exit(1);
                }
            };

            start_engine(&datamodel_location).await
        }
        Some("cli") => {
            tracing::info!(git_hash = env!("GIT_HASH"), "Starting migration engine CLI");
            commands::run_cli(&mut args).await?;
        }
        Some(other) => {
            eprintln!("Unknown subcommand: {}\n\n{}", other, HELPTEXT);
            std::process::exit(1);
        }
    };

    let remaining_args = args.finish();

    if let Some(arg) = remaining_args.get(0) {
        eprintln!("Unknown argument: {}", arg.to_string_lossy());
        std::process::exit(1);
    }

    Ok(())
}

fn set_panic_hook() {
    std::panic::set_hook(Box::new(move |panic_info| {
        let message = panic_info
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| panic_info.payload().downcast_ref::<String>().map(|s| s.as_str()))
            .unwrap_or("<unknown panic>");

        let location = panic_info
            .location()
            .map(|loc| loc.to_string())
            .unwrap_or_else(|| "<unknown location>".to_owned());

        tracing::error!(
            is_panic = true,
            backtrace = ?backtrace::Backtrace::new(),
            location = %location,
            "[{}] {}",
            location,
            message
        );
    }));
}

struct JsonRpcHost;

#[async_trait::async_trait]
impl migration_connector::ConnectorHost for JsonRpcHost {
    async fn print(&self, text: &str) -> migration_connector::ConnectorResult<()> {
        tracing::info!(migrate_action = "log", "{}", text);
        Ok(())
    }
}

async fn start_engine(datamodel_location: &str) {
    use std::io::Read as _;

    tracing::info!(git_hash = env!("GIT_HASH"), "Starting migration engine RPC server",);
    let mut file = std::fs::File::open(datamodel_location).expect("error opening datamodel file");

    let mut datamodel = String::new();
    file.read_to_string(&mut datamodel).unwrap();

    match rpc_api(&datamodel, Box::new(JsonRpcHost)).await {
        // Block the thread and handle IO in async until EOF.
        Ok(api) => json_rpc_stdio::run(&api).await.unwrap(),
        Err(err) => {
            log_error_and_exit(err);
        }
    }
}
