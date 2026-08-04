//! Formateo "pretty" compartido para celdas de datos complejos.
//!
//! `DuckDB` (con tipos tipados) y `file.rs` ya renderizan compuestos con
//! indentación. Los demás backends reciben el dato como TEXTO crudo del
//! servidor (`jsonb` de postgres, JSON de mysql, `TEXT` con JSON de sqlite,
//! arrays de postgres `{a,b}`) — aquí está el formateo común que aplica
//! esas mismas reglas a cadenas ya renderizadas.

use crate::db::model::Row;

/// Texto que parece JSON (empieza por `{` o `[`) → pretty de `serde_json`.
/// Cualquier otro texto se devuelve tal cual.
pub fn pretty_json_or_plain(t: &str) -> String {
    let trimmed = t.trim();
    let looks_json = trimmed.starts_with('{') || trimmed.starts_with('[');
    if looks_json {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
            if let Ok(pretty) = serde_json::to_string_pretty(&value) {
                return pretty;
            }
        }
    }
    t.to_string()
}

/// Celda → texto bonito: JSON pretty si parsea; si no, array de postgres
/// (`{a,b}` / `{{1,2},{3,4}}`) → estilo numpy de duckdb; si no, tal cual.
pub fn pretty_cell_or_plain(t: &str) -> String {
    let trimmed = t.trim();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
            if let Ok(pretty) = serde_json::to_string_pretty(&value) {
                return pretty;
            }
        }
        // No era JSON válido: ¿array de postgres `{...}`?
        if trimmed.starts_with('{') {
            if let Some(PgElem::Arr(elems)) = parse_pg_array(trimmed) {
                return render_pg_array(&elems, 0);
            }
        }
    }
    t.to_string()
}

/// Aplica `pretty_cell_or_plain` a todas las celdas de las filas (en el
/// lugar). Para backends cuyas celdas ya son texto del servidor.
pub fn prettify_rows(rows: Vec<Row>) -> Vec<Row> {
    rows.into_iter()
        .map(|mut row| {
            row.cells.iter_mut().for_each(|cell| *cell = pretty_cell_or_plain(cell));
            row
        })
        .collect()
}

// ─── Arrays de PostgreSQL ──────────────────────────────────────────────
//
// El formato textual de un array en postgres es `{elem,elem,...}` con
// strings entre comillas dobles (`{"a,b","c"}`), escapes `\"`/`\\` y
// anidamiento `{{1,2},{3,4}}`. El literal `NULL` sin comillas es NULL.

#[derive(Debug, PartialEq)]
enum PgElem {
    Null,
    Str(String),
    Arr(Vec<Self>),
}

/// Parsea un array de postgres completo (`{...}`). Devuelve `None` si la
/// entrada no es un array bien formado.
fn parse_pg_array(s: &str) -> Option<PgElem> {
    let mut it = s.trim().chars().peekable();
    let elems = parse_pg_array_inner(&mut it)?;
    // El array raíz ya consumió su `}` de cierre: no debe quedar nada
    if it.next().is_none() { Some(PgElem::Arr(elems)) } else { None }
}

/// Parsea el contenido tras el `{` de apertura; consume hasta el `}` de
/// cierre INCLUSIVE y devuelve los elementos. `None` si mal formado.
fn parse_pg_array_inner(it: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<Vec<PgElem>> {
    if it.next() != Some('{') {
        return None;
    }
    let mut elems = Vec::new();
    loop {
        match it.peek().copied() {
            None => return None, // nunca cerró
            Some('}') => {
                it.next();
                return Some(elems);
            }
            Some('{') => {
                elems.push(PgElem::Arr(parse_pg_array_inner(it)?));
            }
            Some('"') => {
                it.next();
                elems.push(PgElem::Str(parse_pg_quoted(it)?));
            }
            Some(_) => {
                let mut tok = String::new();
                while let Some(&c) = it.peek() {
                    if c == ',' || c == '}' {
                        break;
                    }
                    tok.push(c);
                    it.next();
                }
                let trimmed = tok.trim();
                elems.push(if trimmed == "NULL" {
                    PgElem::Null
                } else {
                    PgElem::Str(trimmed.to_string())
                });
            }
        }
        match it.peek().copied() {
            Some(',') => {
                it.next();
            }
            Some('}') => {}
            _ => return None, // esperábamos `,` o `}`
        }
    }
}

/// String entre comillas con escapes `\"` y `\\`. Consume hasta la comilla
/// de cierre (exclusive).
fn parse_pg_quoted(it: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<String> {
    let mut out = String::new();
    loop {
        match it.next() {
            None => return None,
            Some('"') => return Some(out),
            Some('\\') => {
                let c = it.next()?;
                out.push(c);
            }
            Some(c) => out.push(c),
        }
    }
}

/// Render estilo duckdb (`list_to_pretty`): si todos los elementos son
/// escalares → una línea `[a, b]`; si hay anidados → cada elemento en su
/// línea y los sub-arrays compactos (numpy style).
fn render_pg_array(elems: &[PgElem], indent: usize) -> String {
    if elems.is_empty() {
        return "[]".to_string();
    }
    if elems.iter().all(|e| !matches!(e, PgElem::Arr(_))) {
        let inner: Vec<String> = elems.iter().map(render_pg_scalar).collect();
        return format!("[{}]", inner.join(", "));
    }
    let pad = "  ".repeat(indent + 1);
    let mut out = String::from("[\n");
    for (i, elem) in elems.iter().enumerate() {
        out.push_str(&pad);
        match elem {
            PgElem::Arr(inner) => out.push_str(&render_pg_compact(inner)),
            scalar => out.push_str(&render_pg_scalar(scalar)),
        }
        if i + 1 < elems.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str(&"  ".repeat(indent));
    out.push(']');
    out
}

/// Sub-array compacto (una línea) para filas de matrices.
fn render_pg_compact(elems: &[PgElem]) -> String {
    let inner: Vec<String> = elems.iter().map(render_pg_scalar).collect();
    format!("[{}]", inner.join(", "))
}

fn render_pg_scalar(elem: &PgElem) -> String {
    match elem {
        PgElem::Null => "[NULL]".to_string(),
        PgElem::Str(s) => s.clone(),
        PgElem::Arr(inner) => render_pg_compact(inner),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_pretty_inline() {
        assert_eq!(pretty_json_or_plain("hola"), "hola");
        assert_eq!(pretty_json_or_plain("NULL"), "NULL");
        assert_eq!(pretty_json_or_plain("{\"a\": 1}"), "{\n  \"a\": 1\n}");
    }

    #[test]
    fn json_invalido_queda_tal_cual() {
        assert_eq!(pretty_cell_or_plain("no json"), "no json");
        assert_eq!(pretty_cell_or_plain("hola"), "hola");
    }

    #[test]
    fn array_postgres_plano() {
        assert_eq!(pretty_cell_or_plain("{rust}"), "[rust]");
        assert_eq!(pretty_cell_or_plain("{a,b,c}"), "[a, b, c]");
        assert_eq!(pretty_cell_or_plain("{1,2,3}"), "[1, 2, 3]");
        // `{}` es JSON válido (objeto vacío) → gana el camino JSON
        assert_eq!(pretty_cell_or_plain("{}"), "{}");
    }

    #[test]
    fn array_postgres_matriz_2d() {
        assert_eq!(pretty_cell_or_plain("{{1,2},{3,4}}"), "[\n  [1, 2],\n  [3, 4]\n]");
    }

    #[test]
    fn array_postgres_matriz_3d_compacta_interna() {
        // numpy style: solo el primer nivel va por línea
        assert_eq!(
            pretty_cell_or_plain("{{{1,2},{3,4}},{{5,6},{7,8}}}"),
            "[\n  [[1, 2], [3, 4]],\n  [[5, 6], [7, 8]]\n]"
        );
    }

    #[test]
    fn array_postgres_strings_con_coma_y_escapados() {
        // Los strings desescapan y pierden las comillas (formato de array)
        assert_eq!(pretty_cell_or_plain("{\"a,b\",c}"), "[a,b, c]");
        assert_eq!(pretty_cell_or_plain("{\"a\\\"b\",\"c\\\\d\"}"), "[a\"b, c\\d]");
    }

    #[test]
    fn array_postgres_null() {
        assert_eq!(pretty_cell_or_plain("{NULL,hola}"), "[[NULL], hola]");
    }

    #[test]
    fn json_gana_sobre_array() {
        // Un jsonb object es JSON válido → pretty de serde_json
        assert_eq!(
            pretty_cell_or_plain("{\"os\":\"linux\",\"ver\":\"6.12\"}"),
            "{\n  \"os\": \"linux\",\n  \"ver\": \"6.12\"\n}"
        );
    }
}
