use rusqlite::{Connection, OpenFlags, OptionalExtension};

pub fn list_objects_by_type(path: &str, object_type: &str) -> Result<Vec<String>, String> {
    let conn = open_read_only(path)?;
    let mut stmt = conn
        .prepare(
            "SELECT name FROM sqlite_master
             WHERE type = ?1
             AND name NOT LIKE 'sqlite_%'
             ORDER BY name",
        )
        .map_err(|err| err.to_string())?;

    let rows = stmt
        .query_map([object_type], |row| row.get::<_, String>(0))
        .map_err(|err| err.to_string())?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|err| err.to_string())?);
    }

    Ok(out)
}

pub fn list_advanced_objects(path: &str) -> Result<Vec<String>, String> {
    let conn = open_read_only(path)?;
    let mut stmt = conn
        .prepare(
            "SELECT type, name FROM sqlite_master
             WHERE type IN ('index','trigger')
             AND name NOT LIKE 'sqlite_%'
             ORDER BY type, name",
        )
        .map_err(|err| err.to_string())?;

    let rows = stmt
        .query_map([], |row| {
            let kind: String = row.get(0)?;
            let name: String = row.get(1)?;
            Ok(format!("{kind}:{name}"))
        })
        .map_err(|err| err.to_string())?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|err| err.to_string())?);
    }

    Ok(out)
}

pub fn object_sql(path: &str, object_name: &str) -> Result<String, String> {
    let conn = open_read_only(path)?;
    let escaped = object_name.replace('"', "\"\"");
    let sql = format!(
        "SELECT sql FROM sqlite_master WHERE name = \"{escaped}\" ORDER BY CASE type WHEN 'table' THEN 1 WHEN 'view' THEN 2 WHEN 'index' THEN 3 WHEN 'trigger' THEN 4 ELSE 9 END LIMIT 1"
    );
    let mut stmt = conn.prepare(&sql).map_err(|err| err.to_string())?;

    let ddl: Option<String> =
        stmt.query_row([], |row| row.get(0)).optional().map_err(|err| err.to_string())?;

    Ok(ddl.unwrap_or_else(|| "-- SQL no disponible para este objeto".to_string()))
}

#[allow(dead_code)]
pub fn list_objects(path: &str) -> Result<Vec<String>, String> {
    let conn = open_read_only(path)?;
    let mut stmt = conn
        .prepare(
            "SELECT name FROM sqlite_master
             WHERE type IN ('table','view')
             AND name NOT LIKE 'sqlite_%'
             ORDER BY name",
        )
        .map_err(|err| err.to_string())?;

    let rows = stmt.query_map([], |row| row.get::<_, String>(0)).map_err(|err| err.to_string())?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|err| err.to_string())?);
    }

    Ok(out)
}

#[allow(dead_code)]
pub fn table_columns(path: &str, table_name: &str) -> Result<Vec<String>, String> {
    let conn = open_read_only(path)?;
    let escaped = table_name.replace('"', "\"\"");
    let sql = format!("PRAGMA table_info(\"{escaped}\")");
    let mut stmt = conn.prepare(&sql).map_err(|err| err.to_string())?;

    let rows = stmt
        .query_map([], |row| {
            let cid: i64 = row.get(0)?;
            let name: String = row.get(1)?;
            let dtype: String = row.get(2)?;
            let notnull: i64 = row.get(3)?;
            let pk: i64 = row.get(5)?;

            let null_flag = if notnull == 1 { "NOT NULL" } else { "NULL" };
            let pk_flag = if pk == 1 { " PK" } else { "" };
            Ok(format!("{cid} | {name} | {dtype} | {null_flag}{pk_flag}"))
        })
        .map_err(|err| err.to_string())?;

    let mut out = vec!["cid | name | type | nullability".to_string()];
    for row in rows {
        out.push(row.map_err(|err| err.to_string())?);
    }

    Ok(out)
}

pub fn table_rows(
    path: &str,
    table_name: &str,
    limit: u32,
    offset: u32,
) -> Result<Vec<String>, String> {
    let conn = open_read_only(path)?;
    let escaped = table_name.replace('"', "\"\"");
    let sql = format!("SELECT * FROM \"{escaped}\" LIMIT {limit} OFFSET {offset}");
    let mut stmt = conn.prepare(&sql).map_err(|err| err.to_string())?;

    let mut out = Vec::new();

    // Get column names
    let col_names = stmt.column_names().iter().map(ToString::to_string).collect::<Vec<_>>();

    if col_names.is_empty() {
        return Ok(out);
    }

    // Add header row
    out.push(col_names.join(" | "));

    // Fetch rows
    let rows = stmt
        .query_map([], |row| {
            let mut values = Vec::new();
            for i in 0..col_names.len() {
                let val: String = row.get(i).unwrap_or_else(|_| "[NULL]".to_string());
                values.push(val);
            }
            Ok(values.join(" | "))
        })
        .map_err(|err| err.to_string())?;

    for row in rows {
        out.push(row.map_err(|err| err.to_string())?);
    }

    Ok(out)
}

pub fn table_row_count(path: &str, table_name: &str) -> Result<u32, String> {
    let conn = open_read_only(path)?;
    let escaped = table_name.replace('"', "\"\"");
    let sql = format!("SELECT COUNT(*) FROM \"{escaped}\"");
    let mut stmt = conn.prepare(&sql).map_err(|err| err.to_string())?;

    let count: u32 = stmt.query_row([], |row| row.get(0)).map_err(|err| err.to_string())?;

    Ok(count)
}

fn open_read_only(path: &str) -> Result<Connection, String> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|err| err.to_string())
}
