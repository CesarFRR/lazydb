//! Normalización de rutas de fuentes de bases de datos.
//!
//! El panel Fuentes mezcla rutas locales (relativas o absolutas) con URLs de
//! servidores. Sin normalizar, la misma base aparece duplicada (p.ej.
//! `sakila.db` vs `/home/.../sakila.db`) y la marca `●` de DB conectada nunca
//! coincide. Este módulo unifica el criterio: toda comparación de fuentes
//! pasa por [`normalize_path`].

use std::path::{Component, Path, PathBuf};

/// Normaliza una ruta de archivo local a una forma canónica consistente:
///
/// 1. Expande `~/` contra `$HOME`.
/// 2. Convierte rutas relativas a absolutas contra el directorio actual.
/// 3. Si el archivo existe, resuelve `./`, `..` y symlinks (`canonicalize`).
/// 4. Si no existe, limpia los componentes `.` y `..` léxicamente (fallback).
///
/// Las URLs (`mysql://`, `postgres://`, `http(s)://`, `ssh://`, `sqlite://`)
/// se devuelven **sin tocar**: son identificadores de conexión, no rutas.
pub fn normalize_path(input: &str) -> String {
    if is_url(input) || input.is_empty() {
        return input.to_string();
    }

    let expanded = expand_tilde(input);
    let path = PathBuf::from(&expanded);

    let absolute = if path.is_absolute() {
        path
    } else {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join(path)
    };

    // El archivo existe: resolver symlinks y componentes reales.
    if let Ok(canonical) = absolute.canonicalize() {
        return canonical.to_string_lossy().into_owned();
    }

    // Fallback para rutas que aún no existen: limpieza léxica determinista.
    clean_components(&absolute).to_string_lossy().into_owned()
}

/// ¿Es un identificador de conexión (URL) y no una ruta de archivo?
pub fn is_url(input: &str) -> bool {
    let lower = input.to_ascii_lowercase();
    lower.starts_with("mysql://")
        || lower.starts_with("postgres://")
        || lower.starts_with("sqlite://")
        || lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("ssh://")
}

/// Expande `~/` al home del usuario; el resto se devuelve intacto.
fn expand_tilde(input: &str) -> String {
    if let Some(rest) = input.strip_prefix("~/")
        && let Ok(home) = std::env::var("HOME")
    {
        return Path::new(&home).join(rest).to_string_lossy().into_owned();
    }
    input.to_string()
}

/// Elimina componentes `.` y resuelve `..` léxicamente, sin tocar el FS.
fn clean_components(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_no_se_tocan() {
        for url in [
            "mysql://127.0.0.1:3306/lazy",
            "postgres://user@db.azure.com:5432/prod",
            "https://api.example.com/db",
            "ssh://host/db",
            "sqlite:///tmp/x.db",
        ] {
            assert_eq!(normalize_path(url), url, "URL debe quedar intacta: {url}");
        }
    }

    #[test]
    fn vacio_no_explota() {
        assert_eq!(normalize_path(""), "");
    }

    #[test]
    fn ruta_absoluta_existente_es_canonica() {
        let cwd = std::env::current_dir().expect("cwd en tests");
        let me = cwd.join("src/paths.rs");
        assert_eq!(normalize_path("src/paths.rs"), me.to_string_lossy());
    }

    #[test]
    fn ruta_con_doble_punto_se_resuelve() {
        let cwd = std::env::current_dir().expect("cwd en tests");
        // "src/../src/paths.rs" apunta al mismo archivo → canonicaliza
        let normalizada = normalize_path("src/../src/paths.rs");
        assert_eq!(normalizada, cwd.join("src/paths.rs").to_string_lossy());
    }

    #[test]
    fn ruta_inexistente_queda_absoluta_y_limpia() {
        let cwd = std::env::current_dir().expect("cwd en tests");
        let normalizada = normalize_path("./no/existe/../x.db");
        assert_eq!(normalizada, cwd.join("no/x.db").to_string_lossy());
    }

    #[test]
    fn tilde_se_expande() {
        let Ok(home) = std::env::var("HOME") else {
            return; // entorno sin HOME: no hay nada que verificar
        };
        let normalizada = normalize_path("~/x.db");
        assert!(normalizada.starts_with(&home), "debe empezar por HOME: {normalizada}");
        assert!(normalizada.ends_with("/x.db"));
    }

    #[test]
    fn relativa_y_absoluta_son_la_misma() {
        let cwd = std::env::current_dir().expect("cwd en tests");
        let abs = normalize_path(&cwd.join("src/paths.rs").to_string_lossy());
        let rel = normalize_path("src/paths.rs");
        assert_eq!(abs, rel, "relativa y absoluta deben normalizar igual");
    }
}
