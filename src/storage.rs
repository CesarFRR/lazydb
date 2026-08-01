use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct AppState {
    pub recents: Vec<String>,
    pub favorites: HashMap<String, String>,
    /// Historial de queries ejecutadas (navegable con ↑/↓ en el query input).
    /// Más reciente al inicio; máx. `QUERY_HISTORY_MAX` entradas.
    pub query_history: Vec<String>,
}

/// Tope del historial de queries persistente (como los 10 recents, acotado).
pub const QUERY_HISTORY_MAX: usize = 50;

impl AppState {
    pub fn new() -> Self {
        Self { recents: Vec::new(), favorites: HashMap::new(), query_history: Vec::new() }
    }

    /// Carga el estado desde ~/.config/lazydb/recents.json
    pub fn load() -> Self {
        let config_file = config_file_path();

        // Intentamos leer el archivo y parsearlo en una sola cadena de eventos
        let result = fs::read_to_string(&config_file)
            .ok() // Convertimos Result a Option
            .and_then(|content| serde_json::from_str::<Value>(&content).ok());

        // Si algo falló arriba (archivo no existe o JSON mal formado), result será None
        let Some(json) = result else {
            return Self::new();
        };

        let recents = json["recents"]
            .as_array()
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default(); // Más limpio que un unwrap_or con un vec vacío

        let favorites = json["favorites"]
            .as_object()
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default();

        let query_history = json["query_history"]
            .as_array()
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();

        Self { recents, favorites, query_history }
    }
    /// Guarda el estado a ~/.config/lazydb/recents.json
    pub fn save(&self) -> Result<(), crate::db::DbError> {
        let config_file = config_file_path();

        // Crear directorio si no existe
        if let Some(parent) = config_file.parent() {
            fs::create_dir_all(parent)?;
        }

        let json = json!({
            "recents": self.recents,
            "favorites": self.favorites,
            "query_history": self.query_history,
        });

        let content = serde_json::to_string_pretty(&json)
            .map_err(|e| crate::db::DbError::Io(format!("serializando config: {e}")))?;
        fs::write(&config_file, content)?;

        Ok(())
    }

    /// Agrega un path a recents (evita duplicados, mantiene últimos 10)
    pub fn add_recent(&mut self, path: String) {
        // Remover si ya existe
        self.recents.retain(|p| p != &path);

        // Agregar al inicio
        self.recents.insert(0, path);

        // Mantener solo últimos 10
        self.recents.truncate(10);
    }

    /// Agrega/actualiza un favorito
    #[allow(dead_code)]
    pub fn add_favorite(&mut self, name: String, path: String) {
        self.favorites.insert(name, path);
    }

    /// Registra una query ejecutada: evita la duplicada consecutiva,
    /// reposiciona al frente las queries ya existentes y mantiene
    /// `QUERY_HISTORY_MAX` entradas (la más reciente al inicio).
    pub fn add_query_history(&mut self, sql: &str) {
        let sql = sql.trim().to_string();
        if sql.is_empty() {
            return;
        }
        // Si ya es la más reciente, NO ensuciar el historial
        if self.query_history.first().is_some_and(|h| h == &sql) {
            return;
        }
        // Remover cualquier aparición anterior (para reposicionarla al frente,
        // sin duplicados)
        self.query_history.retain(|h| h != &sql);
        self.query_history.insert(0, sql);
        self.query_history.truncate(QUERY_HISTORY_MAX);
    }

    /// Remueve un favorito
    #[allow(dead_code)]
    pub fn remove_favorite(&mut self, name: &str) {
        self.favorites.remove(name);
    }

    /// Quita un path de recientes (para olvidar fuentes con `d`)
    pub fn remove_recent(&mut self, path: &str) {
        self.recents.retain(|p| p != path);
    }

    /// Nombre del favorito cuyo path coincide, si existe.
    pub fn favorite_name_for_path(&self, path: &str) -> Option<String> {
        self.favorites.iter().find(|(_, v)| v.as_str() == path).map(|(name, _)| name.clone())
    }

    /// Quita el favorito que apunta al path dado (si existe).
    pub fn remove_favorite_by_path(&mut self, path: &str) {
        self.favorites.retain(|_, v| v != path);
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

fn config_file_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config").join("lazydb").join("recents.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_query_history_guarda_al_inicio_y_deduplica_consecutivas() {
        let mut s = AppState::new();
        s.add_query_history("SELECT 1");
        s.add_query_history("SELECT 2");
        // La misma query repetida justo después NO ensucia el historial
        s.add_query_history("SELECT 2");
        assert_eq!(s.query_history, vec!["SELECT 2".to_string(), "SELECT 1".to_string(),]);
    }

    #[test]
    fn add_query_history_ignora_espacios_y_vacias() {
        let mut s = AppState::new();
        s.add_query_history("   SELECT 1   ");
        // Hace trim: queda sin espacios
        assert_eq!(s.query_history, vec!["SELECT 1".to_string()]);
        s.add_query_history("    ");
        assert_eq!(s.query_history.len(), 1, "query de espacios no se guarda");
    }

    #[test]
    fn add_query_history_acota_en_query_history_max() {
        let mut s = AppState::new();
        for i in 0..(QUERY_HISTORY_MAX + 5) {
            s.add_query_history(&format!("SELECT {i}"));
        }
        assert_eq!(s.query_history.len(), QUERY_HISTORY_MAX);
        // Las más recientes sobreviven al inicio
        assert!(s.query_history[0].contains(&(QUERY_HISTORY_MAX + 4).to_string()));
        // Las más viejas se cayeron
        assert!(!s.query_history.iter().any(|q| q == "SELECT 0"));
    }

    #[test]
    fn add_query_history_permite_repetir_una_query_mas_tarde() {
        let mut s = AppState::new();
        s.add_query_history("SELECT 1");
        s.add_query_history("SELECT 2");
        // Re-ejecutar la primera Cuenta como nueva: sube al frente
        s.add_query_history("SELECT 1");
        assert_eq!(s.query_history[0], "SELECT 1");
        assert_eq!(s.query_history.len(), 2, "la duplicada NO consecutiva se reposiciona");
    }
}
