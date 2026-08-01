//! Configuración de la aplicación: global + por proyecto.
//!
//! - **Global:** `~/.config/lazydb/config.toml`
//! - **Por proyecto:** `lazydb.toml` buscado desde el CWD hacia arriba (se
//!   detiene en la raíz del repo: el directorio con `.git`, o la raíz del
//!   filesystem). Los valores del proyecto SOBREESCRIBEN a los globales
//!   (fusión a nivel de tablas).
//!
//! Todo campo tiene default (`#[serde(default)]`): una config mínima es
//! válida — nada de monstruos de 597 líneas de los prototipos.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub ui: UiConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    /// Filas por página del Data tab (clamped 1..=500).
    pub rows_per_page: u32,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self { rows_per_page: 10 }
    }
}

impl Config {
    /// Carga global + proyecto (el proyecto gana). Nunca falla: sin archivos,
    /// devuelve los defaults.
    pub fn load() -> Self {
        let mut merged = parse_file_or_default(&config_file_path());

        if let Some(project_path) = find_project_config() {
            if let Ok(content) = fs::read_to_string(&project_path)
                && let Ok(overlay) = content.parse::<toml::Value>()
            {
                merge_toml(&mut merged, overlay);
            }
        }

        let mut cfg: Self = merged.try_into().unwrap_or_default();
        cfg.ui.rows_per_page = cfg.ui.rows_per_page.clamp(1, 500);
        cfg
    }
}

/// Ruta de la config global (`~/.config/lazydb/config.toml`).
pub fn config_file_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config").join("lazydb").join("config.toml")
}

/// Busca `lazydb.toml` desde `start` hacia la raíz. Se detiene en la raíz
/// del repo (el directorio que contiene `.git`): no se sale del proyecto.
pub fn find_project_config_from(start: &Path) -> Option<PathBuf> {
    let mut dir = start.to_path_buf();
    loop {
        let candidate = dir.join("lazydb.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
        // Última oportunidad en la raíz del repo; fuera de un repo, sube
        // hasta la raíz del filesystem.
        if dir.join(".git").exists() || !dir.pop() {
            return None;
        }
    }
}

/// Busca `lazydb.toml` desde el directorio de trabajo actual.
pub fn find_project_config() -> Option<PathBuf> {
    std::env::current_dir().ok().and_then(|cwd| find_project_config_from(&cwd))
}

fn parse_file_or_default(path: &Path) -> toml::Value {
    fs::read_to_string(path)
        .ok()
        .and_then(|content| content.parse::<toml::Value>().ok())
        .unwrap_or_else(|| toml::Value::Table(toml::Table::new()))
}

/// Fusión recursiva: las tablas se combinan campo a campo, los valores del
/// overlay reemplazan a los de la base (overlay gana).
fn merge_toml(base: &mut toml::Value, overlay: toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(base_table), toml::Value::Table(overlay_table)) => {
            for (key, value) in overlay_table {
                match base_table.get_mut(&key) {
                    Some(existing) => merge_toml(existing, value),
                    None => {
                        base_table.insert(key, value);
                    }
                }
            }
        }
        (base_slot, overlay_value) => *base_slot = overlay_value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mini temp-dir sin dependencia extra: elimina el árbol al dropear.
    struct TempDir {
        path: PathBuf,
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn tmp_tree() -> (TempDir, PathBuf) {
        // Crea: root/a (repo con .git) / b / c
        let dir = std::env::temp_dir().join(format!("lazydb_cfg_{}", std::process::id()));
        let root = dir.join("root");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(root.join("a/b/c")).expect("árbol");
        fs::write(root.join("a/.git"), "").expect(".git");
        (TempDir { path: dir }, root)
    }

    #[test]
    fn find_project_config_subiendo_desde_el_fondo() {
        let (t, root) = tmp_tree();
        fs::write(root.join("a/b/lazydb.toml"), "[ui]\nrows_per_page = 25\n").expect("cfg");

        let found = find_project_config_from(&root.join("a/b/c")).expect("encontrar config");
        assert_eq!(found, root.join("a/b/lazydb.toml"));
        let _ = t;
    }

    #[test]
    fn no_escapa_de_la_raiz_del_repo() {
        let (t, root) = tmp_tree();
        // La config está FUERA del repo (root/lazydb.toml): no debe verse
        fs::write(root.join("lazydb.toml"), "").expect("cfg");
        assert_eq!(find_project_config_from(&root.join("a/b/c")), None);
        let _ = t;
    }

    #[test]
    fn la_config_del_proyecto_sobreescribe_a_la_global() {
        // global: rows_per_page = 10 (default) · proyecto: 25
        let mut merged = parse_file_or_default(Path::new("/no/existe.toml"));
        let overlay: toml::Value = "[ui]\nrows_per_page = 25\n".parse().expect("toml valido");
        merge_toml(&mut merged, overlay);

        let cfg: Config = merged.try_into().expect("config valida");
        assert_eq!(cfg.ui.rows_per_page, 25);
    }

    #[test]
    fn config_vacia_usa_defaults() {
        let empty = toml::Value::Table(toml::Table::new());
        let cfg: Config = empty.try_into().expect("defaults");
        assert_eq!(cfg.ui.rows_per_page, 10);
    }

    #[test]
    fn merge_es_recursivo_y_el_overlay_gana_por_campo() {
        let mut base: toml::Value = "[ui]\nrows_per_page = 3\n".parse().expect("base");
        let overlay: toml::Value = "[ui]\nrows_per_page = 9\n".parse().expect("overlay");
        merge_toml(&mut base, overlay);
        let cfg: Config = base.try_into().expect("valida");
        assert_eq!(cfg.ui.rows_per_page, 9);
    }
}
