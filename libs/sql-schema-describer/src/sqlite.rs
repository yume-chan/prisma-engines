//! SQLite description.

use crate::{
    getters::Getter, ids::*, parsers::Parser, Column, ColumnArity, ColumnType, ColumnTypeFamily, DefaultValue,
    DescriberResult, ForeignKey, ForeignKeyAction, Index, IndexColumn, IndexType, Lazy, PrimaryKey, PrimaryKeyColumn,
    PrismaValue, Regex, SQLSortOrder, SqlMetadata, SqlSchema, SqlSchemaDescriberBackend, Table, View,
};
use indexmap::IndexMap;
use quaint::{ast::Value, prelude::Queryable};
use std::{any::type_name, borrow::Cow, collections::BTreeMap, convert::TryInto, fmt::Debug, path::Path};
use tracing::trace;

pub struct SqlSchemaDescriber<'a> {
    conn: &'a dyn Queryable,
}

impl Debug for SqlSchemaDescriber<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(type_name::<SqlSchemaDescriber>()).finish()
    }
}

#[async_trait::async_trait]
impl SqlSchemaDescriberBackend for SqlSchemaDescriber<'_> {
    async fn list_databases(&self) -> DescriberResult<Vec<String>> {
        Ok(self.get_databases().await?)
    }

    async fn get_metadata(&self, _schema: &str) -> DescriberResult<SqlMetadata> {
        let mut sql_schema = SqlSchema::default();
        let table_count = self.get_table_names(&mut sql_schema).await?.len();
        let size_in_bytes = self.get_size().await?;

        Ok(SqlMetadata {
            table_count,
            size_in_bytes,
        })
    }

    async fn describe(&self, _schema: &str) -> DescriberResult<SqlSchema> {
        let mut schema = SqlSchema::default();
        let table_ids = self.get_table_names(&mut schema).await?;

        for (table_name, table_id) in &table_ids {
            self.get_table(table_name, *table_id, &table_ids, &mut schema).await?
        }

        // SQLite allows foreign key definitions without specifying the referenced columns, it then
        // assumes the pk is used.
        let foreign_keys_without_referenced_columns: Vec<(ForeignKeyId, Vec<String>)> = schema
            .walk_foreign_keys()
            .filter(|fk| fk.referenced_columns_count() == 0)
            .map(|fk| {
                (
                    fk.id,
                    fk.referenced_table()
                        .primary_key()
                        .unwrap()
                        .column_names()
                        .map(|name| name.to_owned())
                        .collect(),
                )
            })
            .collect();

        for (foreign_key_id, columns) in foreign_keys_without_referenced_columns {
            schema[foreign_key_id].1.referenced_columns = columns;
        }

        schema.views = self.get_views().await?;

        schema
            .foreign_keys
            .sort_by_cached_key(|(id, fk)| (*id, fk.columns.to_owned()));

        Ok(schema)
    }

    async fn version(&self, _schema: &str) -> DescriberResult<Option<String>> {
        Ok(self.conn.version().await?)
    }
}

impl Parser for SqlSchemaDescriber<'_> {}

impl<'a> SqlSchemaDescriber<'a> {
    /// Constructor.
    pub fn new(conn: &'a dyn Queryable) -> SqlSchemaDescriber<'a> {
        SqlSchemaDescriber { conn }
    }

    async fn get_databases(&self) -> DescriberResult<Vec<String>> {
        let sql = "PRAGMA database_list;";
        let rows = self.conn.query_raw(sql, &[]).await?;
        let names = rows
            .into_iter()
            .map(|row| {
                row.get("file")
                    .and_then(|x| x.to_string())
                    .and_then(|x| {
                        Path::new(&x)
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                    })
                    .expect("convert schema names")
            })
            .collect();

        trace!("Found schema names: {:?}", names);

        Ok(names)
    }

    async fn get_table_names(&self, schema: &mut SqlSchema) -> DescriberResult<IndexMap<String, TableId>> {
        let sql = r#"SELECT name FROM sqlite_master WHERE type='table' ORDER BY name ASC"#;

        let result_set = self.conn.query_raw(sql, &[]).await?;

        let names = result_set
            .into_iter()
            .map(|row| row.get("name").and_then(|x| x.to_string()).unwrap())
            .filter(|table_name| !is_system_table(table_name));

        let mut map = IndexMap::default();

        for name in names {
            let cloned_name = name.clone();
            let id = schema.push_table(name);
            map.insert(cloned_name, id);
        }

        Ok(map)
    }

    async fn get_size(&self) -> DescriberResult<usize> {
        let sql = r#"SELECT page_count * page_size as size FROM pragma_page_count(), pragma_page_size();"#;
        let result = self.conn.query_raw(sql, &[]).await?;
        let size: i64 = result
            .first()
            .map(|row| row.get("size").and_then(|x| x.as_integer()).unwrap_or(0))
            .unwrap();

        Ok(size.try_into().unwrap())
    }

    async fn get_table(
        &self,
        name: &str,
        table_id: TableId,
        table_ids: &IndexMap<String, TableId>,
        schema: &mut SqlSchema,
    ) -> DescriberResult<()> {
        let (table_columns, primary_key) = self.get_columns(name).await?;
        let indices = self.get_indices(name).await?;

        schema[table_id] = Table {
            name: name.to_owned(),
            indices,
            primary_key,
        };

        for col in table_columns {
            schema.columns.push((table_id, col));
        }

        self.push_foreign_keys(name, table_id, table_ids, schema).await?;

        Ok(())
    }

    async fn get_views(&self) -> DescriberResult<Vec<View>> {
        let sql = "SELECT name AS view_name, sql AS view_sql FROM sqlite_master WHERE type = 'view'";
        let result_set = self.conn.query_raw(sql, &[]).await?;
        let mut views = Vec::with_capacity(result_set.len());

        for row in result_set.into_iter() {
            views.push(View {
                name: row.get_expect_string("view_name"),
                definition: row.get_string("view_sql"),
            })
        }

        Ok(views)
    }

    async fn get_columns(&self, table: &str) -> DescriberResult<(Vec<Column>, Option<PrimaryKey>)> {
        let sql = format!(r#"PRAGMA table_info ("{}")"#, table);
        let result_set = self.conn.query_raw(&sql, &[]).await?;
        let mut pk_cols: BTreeMap<i64, String> = BTreeMap::new();
        let mut cols: Vec<Column> = result_set
            .into_iter()
            .map(|row| {
                trace!("Got column row {:?}", row);
                let is_required = row.get("notnull").and_then(|x| x.as_bool()).expect("notnull");

                let arity = if is_required {
                    ColumnArity::Required
                } else {
                    ColumnArity::Nullable
                };

                let tpe = get_column_type(&row.get("type").and_then(|x| x.to_string()).expect("type"), arity);

                let default = match row.get("dflt_value") {
                    None => None,
                    Some(val) if val.is_null() => None,
                    Some(Value::Text(Some(cow_string))) => {
                        let default_string = cow_string.to_string();

                        if default_string.to_lowercase() == "null" {
                            None
                        } else {
                            Some(match &tpe.family {
                                ColumnTypeFamily::Int => match Self::parse_int(&default_string) {
                                    Some(int_value) => DefaultValue::value(int_value),
                                    None => DefaultValue::db_generated(default_string),
                                },
                                ColumnTypeFamily::BigInt => match Self::parse_big_int(&default_string) {
                                    Some(int_value) => DefaultValue::value(int_value),
                                    None => DefaultValue::db_generated(default_string),
                                },
                                ColumnTypeFamily::Float => match Self::parse_float(&default_string) {
                                    Some(float_value) => DefaultValue::value(float_value),
                                    None => DefaultValue::db_generated(default_string),
                                },
                                ColumnTypeFamily::Decimal => match Self::parse_float(&default_string) {
                                    Some(float_value) => DefaultValue::value(float_value),
                                    None => DefaultValue::db_generated(default_string),
                                },
                                ColumnTypeFamily::Boolean => match Self::parse_int(&default_string) {
                                    Some(PrismaValue::Int(1)) => DefaultValue::value(true),
                                    Some(PrismaValue::Int(0)) => DefaultValue::value(false),
                                    _ => match Self::parse_bool(&default_string) {
                                        Some(bool_value) => DefaultValue::value(bool_value),
                                        None => DefaultValue::db_generated(default_string),
                                    },
                                },
                                ColumnTypeFamily::String => {
                                    DefaultValue::value(unquote_sqlite_string_default(&default_string).into_owned())
                                }
                                ColumnTypeFamily::DateTime => match default_string.to_lowercase().as_str() {
                                    "current_timestamp" | "datetime(\'now\')" | "datetime(\'now\', \'localtime\')" => {
                                        DefaultValue::now()
                                    }
                                    _ => DefaultValue::db_generated(default_string),
                                },
                                ColumnTypeFamily::Binary => DefaultValue::db_generated(default_string),
                                ColumnTypeFamily::Json => DefaultValue::db_generated(default_string),
                                ColumnTypeFamily::Uuid => DefaultValue::db_generated(default_string),
                                ColumnTypeFamily::Enum(_) => DefaultValue::value(PrismaValue::Enum(default_string)),
                                ColumnTypeFamily::Unsupported(_) => DefaultValue::db_generated(default_string),
                            })
                        }
                    }
                    Some(_) => None,
                };

                let pk_col = row.get("pk").and_then(|x| x.as_integer()).expect("primary key");

                let col = Column {
                    name: row.get("name").and_then(|x| x.to_string()).expect("name"),
                    tpe,
                    default,
                    auto_increment: false,
                };

                if pk_col > 0 {
                    pk_cols.insert(pk_col, col.name.clone());
                }

                trace!(
                    "Found column '{}', type: '{:?}', default: {:?}, primary key: {}",
                    col.name,
                    col.tpe,
                    col.default,
                    pk_col > 0
                );

                col
            })
            .collect();

        let primary_key = if pk_cols.is_empty() {
            trace!("Determined that table has no primary key");
            None
        } else {
            let mut columns: Vec<PrimaryKeyColumn> = vec![];
            let mut col_idxs: Vec<&i64> = pk_cols.keys().collect();

            col_idxs.sort_unstable();

            for i in col_idxs {
                columns.push(PrimaryKeyColumn::new(pk_cols[i].clone()));
            }

            //Integer Id columns are always implemented with either row id or autoincrement
            if pk_cols.len() == 1 {
                let pk_col = &columns[0];
                for col in cols.iter_mut() {
                    if col.name == pk_col.name() && &col.tpe.full_data_type.to_lowercase() == "integer" {
                        trace!(
                            "Detected that the primary key column corresponds to rowid and \
                                 is auto incrementing"
                        );
                        col.auto_increment = true;
                        // It is impossible to write a null value to an
                        // autoincrementing primary key column.
                        col.tpe.arity = ColumnArity::Required;
                    }
                }
            }

            trace!("Determined that table has primary key with columns {:?}", columns);
            Some(PrimaryKey {
                columns,
                constraint_name: None,
            })
        };

        Ok((cols, primary_key))
    }

    async fn push_foreign_keys(
        &self,
        table_name: &str,
        table_id: TableId,
        table_ids: &IndexMap<String, TableId>,
        schema: &mut SqlSchema,
    ) -> DescriberResult<()> {
        struct IntermediateForeignKey {
            pub columns: BTreeMap<i64, String>,
            pub referenced_table: TableId,
            pub referenced_columns: BTreeMap<i64, String>,
            pub on_delete_action: ForeignKeyAction,
            pub on_update_action: ForeignKeyAction,
        }

        let sql = format!(r#"PRAGMA foreign_key_list("{}");"#, table_name);
        trace!("describing table foreign keys, SQL: '{}'", sql);
        let result_set = self.conn.query_raw(&sql, &[]).await.expect("querying for foreign keys");

        // Since one foreign key with multiple columns will be represented here as several
        // rows with the same ID, we have to use an intermediate representation that gets
        // translated into the real foreign keys in another pass
        let mut intermediate_fks: BTreeMap<i64, IntermediateForeignKey> = BTreeMap::new();
        for row in result_set.into_iter() {
            trace!("got FK description row {:?}", row);
            let id = row.get("id").and_then(|x| x.as_integer()).expect("id");
            let seq = row.get("seq").and_then(|x| x.as_integer()).expect("seq");
            let column = row.get("from").and_then(|x| x.to_string()).expect("from");
            // this can be null if the primary key and shortened fk syntax was used
            let referenced_column = row.get("to").and_then(|x| x.to_string());
            let referenced_table = row.get("table").and_then(|x| x.to_string()).expect("table");
            let referenced_table_id = if let Some(id) = table_ids.get(&referenced_table) {
                *id
            } else {
                continue;
            };
            match intermediate_fks.get_mut(&id) {
                Some(fk) => {
                    fk.columns.insert(seq, column);
                    if let Some(column) = referenced_column {
                        fk.referenced_columns.insert(seq, column);
                    };
                }
                None => {
                    let mut columns: BTreeMap<i64, String> = BTreeMap::new();
                    columns.insert(seq, column);
                    let mut referenced_columns: BTreeMap<i64, String> = BTreeMap::new();

                    if let Some(column) = referenced_column {
                        referenced_columns.insert(seq, column);
                    };
                    let on_delete_action = match row
                        .get("on_delete")
                        .and_then(|x| x.to_string())
                        .expect("on_delete")
                        .to_lowercase()
                        .as_str()
                    {
                        "no action" => ForeignKeyAction::NoAction,
                        "restrict" => ForeignKeyAction::Restrict,
                        "set null" => ForeignKeyAction::SetNull,
                        "set default" => ForeignKeyAction::SetDefault,
                        "cascade" => ForeignKeyAction::Cascade,
                        s => panic!("Unrecognized on delete action '{}'", s),
                    };
                    let on_update_action = match row
                        .get("on_update")
                        .and_then(|x| x.to_string())
                        .expect("on_update")
                        .to_lowercase()
                        .as_str()
                    {
                        "no action" => ForeignKeyAction::NoAction,
                        "restrict" => ForeignKeyAction::Restrict,
                        "set null" => ForeignKeyAction::SetNull,
                        "set default" => ForeignKeyAction::SetDefault,
                        "cascade" => ForeignKeyAction::Cascade,
                        s => panic!("Unrecognized on update action '{}'", s),
                    };
                    let fk = IntermediateForeignKey {
                        columns,
                        referenced_table: referenced_table_id,
                        referenced_columns,
                        on_delete_action,
                        on_update_action,
                    };
                    intermediate_fks.insert(id, fk);
                }
            };
        }

        for (_, intermediate_fk) in intermediate_fks {
            let mut column_keys: Vec<&i64> = intermediate_fk.columns.keys().collect();
            column_keys.sort();
            let mut columns: Vec<String> = vec![];
            columns.reserve(column_keys.len());
            for i in column_keys {
                columns.push(intermediate_fk.columns[i].to_owned());
            }

            let mut referenced_column_keys: Vec<&i64> = intermediate_fk.referenced_columns.keys().collect();
            referenced_column_keys.sort();
            let mut referenced_columns: Vec<String> = vec![];
            referenced_columns.reserve(referenced_column_keys.len());
            for i in referenced_column_keys {
                referenced_columns.push(intermediate_fk.referenced_columns[i].clone());
            }

            let fk = ForeignKey {
                columns,
                referenced_table: intermediate_fk.referenced_table,
                referenced_columns,
                on_delete_action: intermediate_fk.on_delete_action,
                on_update_action: intermediate_fk.on_update_action,

                // Not relevant in SQLite since we cannot ALTER or DROP foreign keys by
                // constraint name.
                constraint_name: None,
            };
            schema.foreign_keys.push((table_id, fk));
        }

        Ok(())
    }

    async fn get_indices(&self, table: &str) -> DescriberResult<Vec<Index>> {
        let sql = format!(r#"PRAGMA index_list("{}");"#, table);
        let result_set = self.conn.query_raw(&sql, &[]).await?;
        trace!("Got indices description results: {:?}", result_set);

        let mut indices = Vec::new();
        let filtered_rows = result_set
            .into_iter()
            // Exclude primary keys, they are inferred separately.
            .filter(|row| row.get("origin").and_then(|origin| origin.as_str()).unwrap() != "pk")
            // Exclude partial indices
            .filter(|row| !row.get("partial").and_then(|partial| partial.as_bool()).unwrap());

        for row in filtered_rows {
            let mut valid_index = true;

            let is_unique = row.get("unique").and_then(|x| x.as_bool()).expect("get unique");
            let name = row.get("name").and_then(|x| x.to_string()).expect("get name");
            let mut index = Index {
                name: name.clone(),
                tpe: match is_unique {
                    true => IndexType::Unique,
                    false => IndexType::Normal,
                },
                columns: vec![],
            };

            let sql = format!(r#"PRAGMA index_info("{}");"#, name);
            let result_set = self.conn.query_raw(&sql, &[]).await.expect("querying for index info");
            trace!("Got index description results: {:?}", result_set);

            for row in result_set.into_iter() {
                //if the index is on a rowid or expression, the name of the column will be null, we ignore these for now
                match row.get("name").and_then(|x| x.to_string()) {
                    Some(name) => {
                        let pos = row.get("seqno").and_then(|x| x.as_integer()).expect("get seqno") as usize;
                        if index.columns.len() <= pos {
                            index.columns.resize(pos + 1, IndexColumn::default());
                        }
                        index.columns[pos] = IndexColumn::new(name);
                    }
                    None => valid_index = false,
                }
            }

            let sql = format!(r#"PRAGMA index_xinfo("{}");"#, name);
            let result_set = self.conn.query_raw(&sql, &[]).await.expect("querying for index info");
            trace!("Got index description results: {:?}", result_set);

            for row in result_set.into_iter() {
                //if the index is on a rowid or expression, the name of the column will be null, we ignore these for now
                if row.get("name").and_then(|x| x.to_string()).is_some() {
                    let pos = row.get("seqno").and_then(|x| x.as_integer()).expect("get seqno") as usize;

                    let sort_order = row.get("desc").and_then(|r| r.as_integer()).map(|v| match v {
                        0 => SQLSortOrder::Asc,
                        _ => SQLSortOrder::Desc,
                    });

                    index.columns[pos].sort_order = sort_order;
                }
            }

            if valid_index {
                indices.push(index)
            }
        }

        Ok(indices)
    }
}

fn get_column_type(tpe: &str, arity: ColumnArity) -> ColumnType {
    let tpe_lower = tpe.to_lowercase();

    let family = match tpe_lower.as_ref() {
        // SQLite only has a few native data types: https://www.sqlite.org/datatype3.html
        // It's tolerant though, and you can assign any data type you like to columns
        "int" => ColumnTypeFamily::Int,
        "integer" => ColumnTypeFamily::Int,
        "bigint" => ColumnTypeFamily::BigInt,
        "real" => ColumnTypeFamily::Float,
        "float" => ColumnTypeFamily::Float,
        "serial" => ColumnTypeFamily::Int,
        "boolean" => ColumnTypeFamily::Boolean,
        "text" => ColumnTypeFamily::String,
        s if s.contains("char") => ColumnTypeFamily::String,
        s if s.contains("numeric") => ColumnTypeFamily::Decimal,
        s if s.contains("decimal") => ColumnTypeFamily::Decimal,
        "date" => ColumnTypeFamily::DateTime,
        "datetime" => ColumnTypeFamily::DateTime,
        "timestamp" => ColumnTypeFamily::DateTime,
        "binary" | "blob" => ColumnTypeFamily::Binary,
        "double" => ColumnTypeFamily::Float,
        "binary[]" => ColumnTypeFamily::Binary,
        "boolean[]" => ColumnTypeFamily::Boolean,
        "date[]" => ColumnTypeFamily::DateTime,
        "datetime[]" => ColumnTypeFamily::DateTime,
        "timestamp[]" => ColumnTypeFamily::DateTime,
        "double[]" => ColumnTypeFamily::Float,
        "float[]" => ColumnTypeFamily::Float,
        "int[]" => ColumnTypeFamily::Int,
        "integer[]" => ColumnTypeFamily::Int,
        "text[]" => ColumnTypeFamily::String,
        // NUMERIC type affinity
        data_type if data_type.starts_with("decimal") => ColumnTypeFamily::Decimal,
        data_type => ColumnTypeFamily::Unsupported(data_type.into()),
    };
    ColumnType {
        full_data_type: tpe.to_string(),
        family,
        arity,
        native_type: None,
    }
}

// "A string constant is formed by enclosing the string in single quotes ('). A single quote within
// the string can be encoded by putting two single quotes in a row - as in Pascal. C-style escapes
// using the backslash character are not supported because they are not standard SQL."
//
// - https://www.sqlite.org/lang_expr.html
fn unquote_sqlite_string_default(s: &str) -> Cow<'_, str> {
    static SQLITE_STRING_DEFAULT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"(?ms)^'(.*)'$|^"(.*)"$"#).unwrap());
    static SQLITE_ESCAPED_CHARACTER_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"''"#).unwrap());

    match SQLITE_STRING_DEFAULT_RE.replace(s, "$1$2") {
        Cow::Borrowed(s) => SQLITE_ESCAPED_CHARACTER_RE.replace_all(s, "'"),
        Cow::Owned(s) => SQLITE_ESCAPED_CHARACTER_RE.replace_all(&s, "'").into_owned().into(),
    }
}

/// Returns whether a table is one of the SQLite system tables.
fn is_system_table(table_name: &str) -> bool {
    SQLITE_SYSTEM_TABLES
        .iter()
        .any(|system_table| table_name == *system_table)
}

/// See https://www.sqlite.org/fileformat2.html
const SQLITE_SYSTEM_TABLES: &[&str] = &[
    "sqlite_sequence",
    "sqlite_stat1",
    "sqlite_stat2",
    "sqlite_stat3",
    "sqlite_stat4",
];
