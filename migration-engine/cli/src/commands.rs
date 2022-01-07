use crate::logger::log_error_and_exit;
use migration_connector::ConnectorError;
use migration_core::migration_api;
use user_facing_errors::common::{InvalidConnectionString, SchemaParserError};

const CLI_HELPTEXT: &str = r#"
Doesn't start a server, but allows running specific commands against Prisma

USAGE:
    migration-engine cli --datasource <datasource> <SUBCOMMAND>

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

OPTIONS:
    -d, --datasource <datasource>    The connection string to the database

SUBCOMMANDS:
    can-connect-to-database    Does the database connection string work?
    create-database            Create an empty database defined in the configuration string
    drop-database              Drop the database
    help                       Prints this message or the help of the given subcommand(s)
"#;

pub(crate) async fn run_cli(args: &mut pico_args::Arguments) -> Result<(), pico_args::Error> {
    let print_helptext = || {
        eprintln!("{}", CLI_HELPTEXT);
        Ok(())
    };

    if args.contains("-h") || args.contains("--help") {
        return print_helptext();
    }

    let datasource = match (
        args.opt_value_from_fn("--datasource", parse_base64_string)?,
        args.opt_value_from_fn("-d", parse_base64_string)?,
    ) {
        (Some(arg), None) | (None, Some(arg)) => arg,
        (Some(_), Some(_)) => {
            eprintln!(
                "Both -d and --datasource were provided. Please provide only one.\n\n{}",
                CLI_HELPTEXT
            );
            std::process::exit(1);
        }
        _ => return print_helptext(),
    };

    match args.subcommand()? {
        Some(cmd) => match run_inner(&cmd, &datasource).await {
            Ok(msg) => {
                tracing::info!("{}", msg);
                Ok(())
            }
            Err(err) => {
                log_error_and_exit(err);
            }
        },
        None => print_helptext(),
    }
}

pub(crate) async fn run_inner(cmd: &str, datasource: &str) -> Result<String, ConnectorError> {
    let datamodel = datasource_from_database_str(&datasource)?;
    let api = migration_api(&datamodel)?;

    match cmd {
        "create-database" => {
            let db_name = api.create_database().await?;
            Ok(format!("Database '{}' was successfully created.", db_name))
        }
        "can-connect-to-database" => {
            api.ensure_connection_validity().await?;
            Ok("Connection successful".to_owned())
        }
        "drop-database" => {
            api.drop_database().await?;
            Ok("The database was successfully dropped.".to_owned())
        }
        other => {
            eprintln!("Unknown subcommand: {}\n\n{}", other, CLI_HELPTEXT);
            std::process::exit(1);
        }
    }
}

fn parse_base64_string(s: &str) -> Result<String, ConnectorError> {
    match base64::decode(s) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(s) => Ok(s),
            Err(e) => Err(ConnectorError::user_facing(SchemaParserError {
                full_error: e.to_string(),
            })),
        },
        Err(_) => Ok(String::from(s)),
    }
}

fn datasource_from_database_str(database_str: &str) -> Result<String, ConnectorError> {
    let provider = match database_str.split(':').next() {
        Some("postgres") => "postgresql",
        Some("file") => "sqlite",
        Some("mongodb+srv") => "mongodb",
        Some(other) => other,
        None => {
            return Err(ConnectorError::user_facing(InvalidConnectionString {
                details: String::new(),
            }))
        }
    };

    let schema = format!(
        r#"
            datasource db {{
                provider = "{provider}"
                url = "{url}"
            }}
        "#,
        provider = provider,
        url = database_str,
    );

    Ok(schema)
}
