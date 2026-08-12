//! Estado del formulario de conexión y los modales de servidor (Fase 2 del
//! refactor del monolito). `App` delega aquí; los datos compartidos
//! (`db_path`, `active_adapter`) viven en App y se pasan POR PARÁMETRO.

use super::controller::{ConnField, ConnectionFormState, DbPickerState, PasswordPromptState};

/// Estado completo de la UI de conexión: formulario, prompt de contraseña
/// y picker de bases. Los handlers de teclado que orquestan con `App`
/// (connect_*) viven en el controller y operan sobre estos campos.
pub struct ConnectionState {
    pub connection_form: Option<ConnectionFormState>,
    pub password_prompt: Option<PasswordPromptState>,
    pub db_picker: Option<DbPickerState>,
}

impl ConnectionState {
    pub fn new() -> Self {
        Self {
            connection_form: Some(ConnectionFormState::default()),
            password_prompt: None,
            db_picker: None,
        }
    }

    /// Debounce del reparseo de la URL del formulario (1s tras el último
    /// keystroke): al pegar o escribir una URL, los campos individuales se
    /// rellenan solos. Se llama cada frame desde `compute_layout`.
    pub fn tick(&mut self) {
        let debounce_due = {
            let Some(form) = self.connection_form.as_ref() else { return };
            if !form.url_debounce_scheduled {
                return;
            }
            form.url_last_edit
                .is_some_and(|last| last.elapsed() >= std::time::Duration::from_secs(1))
        };
        if debounce_due {
            if let Some(form) = self.connection_form.as_mut() {
                form.url_debounce_scheduled = false;
                form.url_last_edit = None;
            }
            self.conn_parse_url_into_fields();
        }
    }

    /// Reconstruye la URL canónica desde los campos individuales (Host,
    /// Puerto, Usuario, Contraseña, Base). Es el inverso de
    /// `conn_parse_url_into_fields`.
    pub fn conn_rebuild_url_from_fields(&mut self) {
        let Some(form) = self.connection_form.as_mut() else { return };
        let kind = form.kind_override.unwrap_or(form.kind);
        let scheme = match kind {
            crate::db::connection::ConnectionType::Mysql => "mysql",
            crate::db::connection::ConnectionType::Postgres => "postgres",
            crate::db::connection::ConnectionType::Mongo => "mongodb",
            crate::db::connection::ConnectionType::Sqlite => "sqlite",
            crate::db::connection::ConnectionType::Duckdb => "duckdb",
            crate::db::connection::ConnectionType::File
            | crate::db::connection::ConnectionType::Unknown => {
                return; // archivos y desconocido: la URL es libre (ruta)
            }
        };
        let mut url = format!("{scheme}://");
        if !form.user.is_empty() || !form.pass.is_empty() {
            url.push_str(&form.user);
            if !form.pass.is_empty() {
                url.push(':');
                url.push_str(&form.pass);
            }
            url.push('@');
        }
        url.push_str(&form.host);
        if !form.port.is_empty() {
            url.push(':');
            url.push_str(&form.port);
        }
        if !form.db.is_empty() {
            url.push('/');
            url.push_str(&form.db);
        }
        form.url = url;
    }

    /// Parsea la URL del formulario a los campos individuales (Host,
    /// Puerto, Usuario, Contraseña, Base) usando `analyze_connection`.
    /// Devuelve `false` si la URL no define un tipo completo.
    pub fn conn_parse_url_into_fields(&mut self) -> bool {
        let Some(form) = self.connection_form.as_mut() else { return false };
        // Purga de la URL: quitar saltos de línea y espacios externos antes
        // de analizar (las URLs de CleverCloud pueden llegar partidas).
        form.url = form.url.trim().replace(['\n', '\r'], "");
        let spec = crate::db::connection::analyze_connection(&form.url);
        let complete = spec.kind != crate::db::connection::ConnectionType::Unknown;

        if complete {
            form.kind = spec.kind;
            // Solo sobreescribir campos si el usuario no los forzó
            if form.kind_override.is_none() {
                form.host = spec.host.clone().unwrap_or_default();
                form.port = spec.port.map_or_else(String::new, |p| p.to_string());
                form.user = spec.user.clone().unwrap_or_default();
                form.pass = spec.pass.clone().unwrap_or_default();
                form.db = spec.db_name.clone().unwrap_or_default();
            }
            form.detected_note = format!("✓ Detectado: {}", spec.display());
        } else if form.url.is_empty() {
            form.detected_note.clear();
        } else {
            form.detected_note = "⏳ escribe la URL o ruta…".to_string();
        }
        complete
    }

    /// Conecta con lo que haya en el formulario (URL canónica).
    /// Devuelve la URL a conectar; `App` la pasa a `connect_sqlite`.
    pub fn conn_submit(&mut self) -> Option<String> {
        // Si el foco está en un campo individual, reconstruir primero
        let rebuild = self.connection_form.as_ref().is_some_and(|form| {
            matches!(
                form.active,
                ConnField::Host
                    | ConnField::Port
                    | ConnField::User
                    | ConnField::Pass
                    | ConnField::Db
            )
        });
        if rebuild {
            self.conn_rebuild_url_from_fields();
        }
        let url = self.connection_form.as_ref().map(|f| f.url.clone()).unwrap_or_default();
        // Purga final antes de conectar (defensa en profundidad contra \n)
        let url = url.trim().replace(['\n', '\r'], "");
        if url.trim().is_empty() {
            return None;
        }
        tracing::debug!(url = %crate::security::strip_credentials(&url), rebuild, "conn_submit: url a conectar");
        if let Some(form) = self.connection_form.as_mut() {
            form.connecting = true;
        }
        Some(url)
    }

    /// Marca el formulario como "conectando terminado" (se llama al aplicar
    /// el resultado de la conexión o al fallar).
    // (clippy sugiere const fn pero el método muta `connecting`: no puede ser const.)
    #[allow(clippy::missing_const_for_fn)]
    pub fn finish_connecting(&mut self) {
        if let Some(form) = self.connection_form.as_mut() {
            form.connecting = false;
            form.url_debounce_scheduled = false;
        }
    }
}
