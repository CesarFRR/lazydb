use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct AppState {
    pub recents: Vec<String>,
    pub favorites: HashMap<String, String>,
}

impl AppState {
    pub fn new() -> Self {
        Self { recents: Vec::new(), favorites: HashMap::new() }
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

        Self { recents, favorites }
    }
    /// Guarda el estado a ~/.config/lazydb/recents.json
    pub fn save(&self) -> Result<(), String> {
        let config_file = config_file_path();

        // Crear directorio si no existe
        if let Some(parent) = config_file.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("Error creando config dir: {e}"))?;
        }

        let json = json!({
            "recents": self.recents,
            "favorites": self.favorites,
        });

        let content = serde_json::to_string_pretty(&json).map_err(|e| format!("Error: {e}"))?;
        fs::write(&config_file, content).map_err(|e| format!("Error guardando config: {e}"))?;

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

    /// Remueve un favorito
    #[allow(dead_code)]
    pub fn remove_favorite(&mut self, name: &str) {
        self.favorites.remove(name);
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
