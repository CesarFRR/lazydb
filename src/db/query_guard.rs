//! Construcción segura del SQL de queries libres con tope de filas.
//!
//! Antes, cada backend concatenaba `LIMIT n` al final del SQL del usuario,
//! lo que tenía dos fugas:
//! 1. Un comentario `--` al final descartaba el LIMIT silenciosamente
//!    (query sin cota → riesgo de materializar millones de filas).
//! 2. Un `;` final producía `SELECT...; LIMIT n` (SQL inválido).
//!
//! La solución es envolver SIEMPRE en una subquery: el motor corta antes de
//! mandar filas por el wire, y el wrapper es inmune a comentarios, `;` y a
//! un LIMIT preexistente (el interno gana). Solo aplica a SELECT/WITH; el
//! resto (INSERT/UPDATE...) se devuelve tal cual.

/// Devuelve el SQL listo para ejecutar con tope `limit`.
///
/// - SQL de lectura (`SELECT`/`WITH`): envuelto en
///   `SELECT * FROM (<sql>) AS _lazydb_q LIMIT <limit>`.
/// - Cualquier otra cosa: el SQL original (sin tope; las queries libres de
///   la app son read-only por diseño).
// Solo lo consumen los backends remotos (mysql/postgres); sin esas features
// la función es dead code por diseño.
#[cfg_attr(
    not(any(feature = "mysql", feature = "postgres")),
    allow(dead_code)
)]
pub fn bounded_select_sql(sql: &str, limit: u32) -> String {
    let trimmed = sql.trim().trim_end_matches(';').trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("select") || lower.starts_with("with") {
        format!("SELECT * FROM ({trimmed}) AS _lazydb_q LIMIT {limit}")
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::bounded_select_sql;

    #[test]
    fn select_simple_se_envuelve_en_subquery() {
        let sql = bounded_select_sql("SELECT * FROM users", 500);
        assert_eq!(sql, "SELECT * FROM (SELECT * FROM users) AS _lazydb_q LIMIT 500");
    }

    #[test]
    fn comentario_final_no_anula_el_limite() {
        // Regresión: `SELECT * FROM t -- debug` + ` LIMIT 500` dejaba el
        // LIMIT dentro del comentario → query sin cota.
        let sql = bounded_select_sql("SELECT * FROM users -- debug", 500);
        assert_eq!(sql, "SELECT * FROM (SELECT * FROM users -- debug) AS _lazydb_q LIMIT 500");
    }

    #[test]
    fn punto_y_coma_final_se_recorta() {
        let sql = bounded_select_sql("SELECT * FROM users;", 500);
        assert_eq!(sql, "SELECT * FROM (SELECT * FROM users) AS _lazydb_q LIMIT 500");
    }

    #[test]
    fn limit_preexistente_el_interno_gana() {
        let sql = bounded_select_sql("SELECT * FROM users LIMIT 10", 500);
        assert_eq!(sql, "SELECT * FROM (SELECT * FROM users LIMIT 10) AS _lazydb_q LIMIT 500");
    }

    #[test]
    fn with_cte_se_envuelve() {
        let sql = bounded_select_sql("WITH x AS (SELECT 1) SELECT * FROM x", 500);
        assert_eq!(
            sql,
            "SELECT * FROM (WITH x AS (SELECT 1) SELECT * FROM x) AS _lazydb_q LIMIT 500"
        );
    }

    #[test]
    fn no_select_se_devuelve_tal_cual() {
        // Las queries libres son read-only por diseño; por seguridad no
        // tocamos SQL que no sea de lectura.
        assert_eq!(bounded_select_sql("EXPLAIN SELECT 1", 500), "EXPLAIN SELECT 1");
    }
}
