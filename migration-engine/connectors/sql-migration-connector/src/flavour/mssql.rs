mod shadow_db;

use crate::{flavour::normalize_sql_schema, SqlFlavour};
use connection_string::JdbcString;
use indoc::formatdoc;
use migration_connector::{
    migrations_directory::MigrationDirectory, BoxFuture, ConnectorError, ConnectorParams, ConnectorResult,
};
use quaint::{
    connector::{Mssql as Connection, MssqlUrl},
    prelude::{Queryable, Table},
};
use sql_schema_describer::SqlSchema;
use std::{future, str::FromStr};
use user_facing_errors::{
    introspection_engine::DatabaseSchemaInconsistent, migration_engine::ApplyMigrationError, KnownError,
};

type State = super::State<Params, Connection>;

pub(crate) struct Params {
    pub(crate) connector_params: ConnectorParams,
    pub(crate) url: MssqlUrl,
}

impl Params {
    fn is_running_on_azure_sql(&self) -> bool {
        self.url.host().contains(".database.windows.net")
    }
}

pub(crate) struct MssqlFlavour {
    pub(crate) state: State,
}

impl Default for MssqlFlavour {
    fn default() -> Self {
        MssqlFlavour { state: State::Initial }
    }
}

impl std::fmt::Debug for MssqlFlavour {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MssqlFlavour").field("url", &"<REDACTED>").finish()
    }
}

impl MssqlFlavour {
    pub(crate) fn schema_name(&self) -> &str {
        self.state.params().map(|p| p.url.schema()).unwrap_or("dbo")
    }

    /// Get the url as a JDBC string, extract the database name, and re-encode the string.
    fn master_url(input: &str) -> ConnectorResult<(String, String)> {
        let mut conn = JdbcString::from_str(&format!("jdbc:{}", input))
            .map_err(|e| ConnectorError::from_source(e, "JDBC string parse error"))?;
        let params = conn.properties_mut();

        let db_name = params.remove("database").unwrap_or_else(|| String::from("master"));
        Ok((db_name, conn.to_string()))
    }
}

impl SqlFlavour for MssqlFlavour {
    fn acquire_lock(&mut self) -> BoxFuture<'_, ConnectorResult<()>> {
        // see
        // https://docs.microsoft.com/en-us/sql/relational-databases/system-stored-procedures/sp-getapplock-transact-sql?view=sql-server-ver15
        // We don't set an explicit timeout because we want to respect the
        // server-set default.
        Box::pin(
            self.raw_cmd("sp_getapplock @Resource = 'prisma_migrate', @LockMode = 'Exclusive', @LockOwner = 'Session'"),
        )
    }

    fn apply_migration_script<'a>(
        &'a mut self,
        migration_name: &'a str,
        script: &'a str,
    ) -> BoxFuture<'a, ConnectorResult<()>> {
        with_connection(&mut self.state, move |_, connection| {
            generic_apply_migration_script(migration_name, script, connection)
        })
    }

    fn datamodel_connector(&self) -> &'static dyn datamodel::datamodel_connector::Connector {
        sql_datamodel_connector::MSSQL
    }

    fn describe_schema(&mut self) -> BoxFuture<'_, ConnectorResult<SqlSchema>> {
        use sql_schema_describer::{mssql as describer, DescriberErrorKind, SqlSchemaDescriberBackend};
        with_connection(&mut self.state, |params, connection| async move {
            let mut schema = describer::SqlSchemaDescriber::new(connection)
                .describe(params.url.schema())
                .await
                .map_err(|err| match err.into_kind() {
                    DescriberErrorKind::QuaintError(err) => quaint_err_url(&params.url)(err),
                    e @ DescriberErrorKind::CrossSchemaReference { .. } => {
                        let err = KnownError::new(DatabaseSchemaInconsistent {
                            explanation: e.to_string(),
                        });

                        ConnectorError::from(err)
                    }
                })?;

            normalize_sql_schema(&mut schema, params.connector_params.preview_features);

            Ok(schema)
        })
    }

    fn migrations_table(&self) -> Table<'static> {
        (self.schema_name().to_owned(), self.migrations_table_name().to_owned()).into()
    }

    fn connection_string(&self) -> Option<&str> {
        self.state
            .params()
            .map(|p| p.connector_params.connection_string.as_str())
    }

    fn connector_type(&self) -> &'static str {
        "mssql"
    }

    fn create_database(&mut self) -> BoxFuture<'_, ConnectorResult<String>> {
        Box::pin(async {
            let params = self.state.get_unwrapped_params();
            let connection_string = &params.connector_params.connection_string;
            let (db_name, master_uri) = Self::master_url(connection_string)?;
            let conn = connect(&master_uri).await?;

            let query = format!("CREATE DATABASE [{}]", db_name);
            raw_cmd(&query, &conn, &MssqlUrl::new(&master_uri).unwrap()).await?;

            let conn = connect(&params.connector_params.connection_string).await?;

            // dbo is created automatically
            if params.url.schema() != "dbo" {
                let query = format!("CREATE SCHEMA {}", params.url.schema());
                raw_cmd(&query, &conn, &params.url).await?;
            }

            Ok(db_name)
        })
    }

    fn create_migrations_table(&mut self) -> BoxFuture<'_, ConnectorResult<()>> {
        let sql = formatdoc! { r#"
            CREATE TABLE [{}].[{}] (
                id                      VARCHAR(36) PRIMARY KEY NOT NULL,
                checksum                VARCHAR(64) NOT NULL,
                finished_at             DATETIMEOFFSET,
                migration_name          NVARCHAR(250) NOT NULL,
                logs                    NVARCHAR(MAX) NULL,
                rolled_back_at          DATETIMEOFFSET,
                started_at              DATETIMEOFFSET NOT NULL DEFAULT CURRENT_TIMESTAMP,
                applied_steps_count     INT NOT NULL DEFAULT 0
            );
        "#, self.schema_name(), self.migrations_table_name()};

        Box::pin(async move { self.raw_cmd(&sql).await })
    }

    fn drop_database(&mut self) -> BoxFuture<'_, ConnectorResult<()>> {
        Box::pin(async {
            let params = self.state.get_unwrapped_params();
            let connection_string = &params.connector_params.connection_string;
            {
                let conn_str: JdbcString = format!("jdbc:{}", connection_string)
                    .parse()
                    .map_err(ConnectorError::url_parse_error)?;

                let db_name = conn_str
                    .properties()
                    .get("database")
                    .map(|s| s.to_owned())
                    .unwrap_or_else(|| "master".to_owned());

                assert!(db_name != "master", "Cannot drop the `master` database.");
            }

            let (db_name, master_uri) = Self::master_url(&params.connector_params.connection_string)?;
            let conn = connect(&master_uri.to_string()).await?;

            let query = format!("DROP DATABASE IF EXISTS [{}]", db_name);
            raw_cmd(&query, &conn, &MssqlUrl::new(&master_uri).unwrap()).await?;

            Ok(())
        })
    }

    fn drop_migrations_table(&mut self) -> BoxFuture<'_, ConnectorResult<()>> {
        let sql = format!("DROP TABLE [{}].[{}]", self.schema_name(), self.migrations_table_name());
        Box::pin(async move { self.raw_cmd(&sql).await })
    }

    fn query<'a>(
        &'a mut self,
        query: quaint::ast::Query<'a>,
    ) -> BoxFuture<'a, ConnectorResult<quaint::prelude::ResultSet>> {
        with_connection(&mut self.state, move |params, conn| async move {
            conn.query(query).await.map_err(quaint_err(params))
        })
    }

    fn query_raw<'a>(
        &'a mut self,
        sql: &'a str,
        params: &'a [quaint::Value<'a>],
    ) -> BoxFuture<'a, ConnectorResult<quaint::prelude::ResultSet>> {
        with_connection(&mut self.state, move |conn_params, conn| async move {
            conn.query_raw(sql, params).await.map_err(quaint_err(conn_params))
        })
    }

    fn run_query_script<'a>(&'a mut self, sql: &'a str) -> BoxFuture<'a, ConnectorResult<()>> {
        self.raw_cmd(sql)
    }

    fn reset(&mut self) -> BoxFuture<'_, ConnectorResult<()>> {
        with_connection(&mut self.state, move |params, connection| async move {
            let schema_name = params.url.schema();

            let drop_procedures = format!(
                r#"
                DECLARE @stmt NVARCHAR(max)
                DECLARE @n CHAR(1)

                SET @n = CHAR(10)

                SELECT @stmt = ISNULL(@stmt + @n, '') +
                    'DROP PROCEDURE [' + SCHEMA_NAME(schema_id) + '].[' + OBJECT_NAME(object_id) + ']'
                FROM sys.objects
                WHERE SCHEMA_NAME(schema_id) = '{0}' AND type = 'P'

                EXEC SP_EXECUTESQL @stmt
                "#,
                schema_name
            );

            let drop_shared_defaults = format!(
                r#"
                DECLARE @stmt NVARCHAR(max)
                DECLARE @n CHAR(1)

                SET @n = CHAR(10)

                SELECT @stmt = ISNULL(@stmt + @n, '') +
                    'DROP DEFAULT [' + SCHEMA_NAME(schema_id) + '].[' + OBJECT_NAME(object_id) + ']'
                FROM sys.objects
                WHERE SCHEMA_NAME(schema_id) = '{0}' AND type = 'D' AND parent_object_id = 0

                EXEC SP_EXECUTESQL @stmt
                "#,
                schema_name
            );

            let drop_views = format!(
                r#"
                DECLARE @stmt NVARCHAR(max)
                DECLARE @n CHAR(1)

                SET @n = CHAR(10)

                SELECT @stmt = ISNULL(@stmt + @n, '') +
                    'DROP VIEW [' + SCHEMA_NAME(schema_id) + '].[' + name + ']'
                FROM sys.views
                WHERE SCHEMA_NAME(schema_id) = '{0}'

                EXEC SP_EXECUTESQL @stmt
                "#,
                schema_name
            );

            let drop_fks = format!(
                r#"
                DECLARE @stmt NVARCHAR(max)
                DECLARE @n CHAR(1)

                SET @n = CHAR(10)

                SELECT @stmt = ISNULL(@stmt + @n, '') +
                    'ALTER TABLE [' + SCHEMA_NAME(schema_id) + '].[' + OBJECT_NAME(parent_object_id) + '] DROP CONSTRAINT [' + name + ']'
                FROM sys.foreign_keys
                WHERE SCHEMA_NAME(schema_id) = '{0}'

                EXEC SP_EXECUTESQL @stmt
                "#,
                schema_name
            );

            let drop_tables = format!(
                r#"
                DECLARE @stmt NVARCHAR(max)
                DECLARE @n CHAR(1)

                SET @n = CHAR(10)

                SELECT @stmt = ISNULL(@stmt + @n, '') +
                    'DROP TABLE [' + SCHEMA_NAME(schema_id) + '].[' + name + ']'
                FROM sys.tables
                WHERE SCHEMA_NAME(schema_id) = '{0}'

                EXEC SP_EXECUTESQL @stmt
                "#,
                schema_name
            );

            let drop_types = format!(
                r#"
                DECLARE @stmt NVARCHAR(max)
                DECLARE @n CHAR(1)

                SET @n = CHAR(10)

                SELECT @stmt = ISNULL(@stmt + @n, '') +
                    'DROP TYPE [' + SCHEMA_NAME(schema_id) + '].[' + name + ']'
                FROM sys.types
                WHERE SCHEMA_NAME(schema_id) = '{0}'
                AND is_user_defined = 1

                EXEC SP_EXECUTESQL @stmt
                "#,
                schema_name
            );

            raw_cmd(&drop_procedures, connection, &params.url).await?;
            raw_cmd(&drop_views, connection, &params.url).await?;
            raw_cmd(&drop_shared_defaults, connection, &params.url).await?;
            raw_cmd(&drop_fks, connection, &params.url).await?;
            raw_cmd(&drop_tables, connection, &params.url).await?;
            raw_cmd(&drop_types, connection, &params.url).await?;

            Ok(())
        })
    }

    fn ensure_connection_validity(&mut self) -> BoxFuture<'_, ConnectorResult<()>> {
        self.raw_cmd("SELECT 1")
    }

    fn raw_cmd<'a>(&'a mut self, sql: &'a str) -> BoxFuture<'a, ConnectorResult<()>> {
        with_connection(&mut self.state, move |params, conn| raw_cmd(sql, conn, &params.url))
    }

    fn set_params(&mut self, connector_params: ConnectorParams) -> ConnectorResult<()> {
        let url =
            MssqlUrl::new(&connector_params.connection_string).map_err(|err| ConnectorError::url_parse_error(err))?;
        let params = Params { connector_params, url };
        self.state.set_params(params);
        Ok(())
    }

    fn sql_schema_from_migration_history<'a>(
        &'a mut self,
        migrations: &'a [MigrationDirectory],
        shadow_database_connection_string: Option<String>,
    ) -> BoxFuture<'a, ConnectorResult<SqlSchema>> {
        let shadow_database_connection_string = shadow_database_connection_string.or_else(|| {
            self.state
                .params()
                .and_then(|p| p.connector_params.shadow_database_connection_string.clone())
        });
        let mut shadow_database = MssqlFlavour::default();

        if let Some(shadow_database_connection_string) = shadow_database_connection_string {
            Box::pin(async move {
                if let Some(params) = self.state.params() {
                    super::validate_connection_infos_do_not_match(
                        &shadow_database_connection_string,
                        &params.connector_params.connection_string,
                    )?;
                }

                let shadow_db_params = ConnectorParams {
                    connection_string: shadow_database_connection_string,
                    preview_features: self
                        .state
                        .params()
                        .map(|cp| cp.connector_params.preview_features)
                        .unwrap_or_default(),
                    shadow_database_connection_string: None,
                };
                shadow_database.set_params(shadow_db_params)?;
                shadow_database.ensure_connection_validity().await?;

                if shadow_database.reset().await.is_err() {
                    crate::best_effort_reset(&mut shadow_database).await?;
                }

                match self.state.params().map(|p| p.url.schema()) {
                    Some("dbo") | None => (),
                    Some(other) => {
                        let create_schema = format!("CREATE SCHEMA [{schema}]", schema = other);
                        shadow_database.raw_cmd(&create_schema).await?;
                    }
                }

                shadow_db::sql_schema_from_migrations_history(migrations, shadow_database).await
            })
        } else {
            with_connection(&mut self.state, move |params, main_connection| async move {
                let shadow_database_name = crate::new_shadow_database_name();
                // See https://github.com/prisma/prisma/issues/6371 for the rationale on
                // this conditional.
                if params.is_running_on_azure_sql() {
                    return Err(ConnectorError::user_facing(
                        user_facing_errors::migration_engine::AzureMssqlShadowDb,
                    ));
                }

                let create_database = format!("CREATE DATABASE [{}]", shadow_database_name);

                raw_cmd(&create_database, main_connection, &params.url)
                    .await
                    .map_err(ConnectorError::from)
                    .map_err(|err| err.into_shadow_db_creation_error())?;

                let connection_string = format!("jdbc:{}", params.connector_params.connection_string);
                let mut jdbc_string: JdbcString = connection_string.parse().unwrap();
                jdbc_string
                    .properties_mut()
                    .insert("database".into(), shadow_database_name.to_owned());
                let host = jdbc_string.server_name();

                let jdbc_string = jdbc_string.to_string();

                tracing::debug!("Connecting to shadow database at {}", host.unwrap_or("localhost"));

                let shadow_db_params = ConnectorParams {
                    connection_string: jdbc_string,
                    preview_features: params.connector_params.preview_features,
                    shadow_database_connection_string: None,
                };
                shadow_database.set_params(shadow_db_params)?;

                if params.url.schema() != "dbo" {
                    let create_schema = format!("CREATE SCHEMA [{schema}]", schema = params.url.schema());
                    shadow_database.raw_cmd(&create_schema).await?;
                }

                // We go through the whole process without early return, then clean up
                // the shadow database, and only then return the result. This avoids
                // leaving shadow databases behind in case of e.g. faulty
                // migrations.
                let ret = shadow_db::sql_schema_from_migrations_history(migrations, shadow_database).await;
                clean_up_shadow_database(&shadow_database_name, main_connection, params).await?;
                ret
            })
        }
    }

    fn version(&mut self) -> BoxFuture<'_, ConnectorResult<Option<String>>> {
        with_connection(&mut self.state, |params, connection| async {
            connection.version().await.map_err(quaint_err(params))
        })
    }
}

fn with_connection<'a, O, F, C>(state: &'a mut State, f: C) -> BoxFuture<'a, ConnectorResult<O>>
where
    O: 'a,
    F: future::Future<Output = ConnectorResult<O>> + Send + 'a,
    C: (FnOnce(&'a mut Params, &'a mut Connection) -> F) + Send + 'a,
{
    match state {
        super::State::Initial => panic!("logic error: Initial"),
        super::State::Connected(p, c) => Box::pin(f(p, c)),
        state @ super::State::WithParams(_) => Box::pin(async move {
            state
                .try_connect(|params| Box::pin(connect(&params.connector_params.connection_string)))
                .await?;
            with_connection(state, f).await
        }),
    }
}

fn quaint_err(params: &Params) -> impl (Fn(quaint::error::Error) -> ConnectorError) + '_ {
    quaint_err_url(&params.url)
}

fn quaint_err_url(url: &MssqlUrl) -> impl (Fn(quaint::error::Error) -> ConnectorError) + '_ {
    use quaint::prelude::ConnectionInfo;
    |err| super::quaint_error_to_connector_error(err, &ConnectionInfo::Mssql(url.clone()))
}

/// Call this on the _main_ database when you are done with a shadow database.
async fn clean_up_shadow_database(
    database_name: &str,
    connection: &Connection,
    params: &Params,
) -> ConnectorResult<()> {
    let drop_database = format!("DROP DATABASE [{}]", database = database_name);
    raw_cmd(&drop_database, connection, &params.url).await
}

async fn generic_apply_migration_script(migration_name: &str, script: &str, conn: &Connection) -> ConnectorResult<()> {
    conn.raw_cmd(script).await.map_err(|sql_error| {
        ConnectorError::user_facing(ApplyMigrationError {
            migration_name: migration_name.to_owned(),
            database_error_code: String::from(sql_error.original_code().unwrap_or("none")),
            database_error: sql_error
                .original_message()
                .map(String::from)
                .unwrap_or_else(|| sql_error.to_string()),
        })
    })
}

async fn raw_cmd(sql: &str, conn: &Connection, url: &MssqlUrl) -> ConnectorResult<()> {
    conn.raw_cmd(sql).await.map_err(quaint_err_url(url))
}

async fn connect(connection_str: &str) -> ConnectorResult<Connection> {
    let url = MssqlUrl::new(connection_str).map_err(|err| {
        ConnectorError::user_facing(user_facing_errors::common::InvalidConnectionString {
            details: err.to_string(),
        })
    })?;
    Connection::new(url.clone()).await.map_err(quaint_err_url(&url))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_impl_does_not_leak_connection_info() {
        let url = "sqlserver://myserver:8765;database=master;schema=mydbname;user=SA;password=<mypassword>;trustServerCertificate=true;socket_timeout=60;isolationLevel=READ UNCOMMITTED";

        let params = ConnectorParams {
            connection_string: url.to_owned(),
            preview_features: Default::default(),
            shadow_database_connection_string: None,
        };

        let mut flavour = MssqlFlavour::default();
        flavour.set_params(params).unwrap();
        let debugged = format!("{:?}", flavour);

        let words = &["myname", "mypassword", "myserver", "8765", "mydbname"];

        for word in words {
            assert!(!debugged.contains(word));
        }
    }
}
