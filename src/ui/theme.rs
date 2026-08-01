//! Tema centralizado: paleta semántica para toda la UI.
//!
//! Los widgets NUNCA eligen un color directamente; consultan este módulo
//! (patrón "una sola fuente de verdad", Fase 0 ítem 3). La paleta default
//! asume terminal oscura (Breeze-Dark): alto contraste y color con
//! propósito semántico (verde = ok, rojo = error, cyan = foco/selección).

use ratatui::style::Color;

/// Paleta semántica de la aplicación.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// Texto de primer plano base (lo que escribe/lee el usuario).
    pub text: Color,
    /// Fondo de la app (terminal oscura). Usado para inversión (bloque de
    /// selección: `fg=bg`, `bg=selection`) y para Clear sobre modales.
    pub bg: Color,
    /// Foco/selección activa (bordes, títulos, cursor, thumb de scroll).
    pub selection: Color,
    /// Panel presente pero sin foco (elementos atenuados no apagados).
    pub unfocused: Color,
    /// Elementos secundarios: placeholders, tracks, filas muertas.
    pub dim: Color,
    /// Bordes de paneles y modales.
    pub border: Color,
    /// Éxito / estado sano.
    pub ok: Color,
    /// Error / estado roto (fallo de probe, errores de query).
    pub error: Color,
    /// Marcas de favorito (`★`).
    pub favorite: Color,
    /// Marcas por tipo de fuente (▣ D M P ⊙).
    pub source: SourceColors,
}

/// Marcas de tipo de fuente: identidad visual por backend (Fase 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceColors {
    /// `▣` sqlite local.
    pub sqlite: Color,
    /// `D` duckdb.
    pub duckdb: Color,
    /// `M` mysql.
    pub mysql: Color,
    /// `P` postgres.
    pub postgres: Color,
    /// `⊙` fuente genérica/desconocida.
    pub generic: Color,
}

impl Default for Theme {
    /// Paleta Breeze-Dark (terminal oscura, alto contraste).
    fn default() -> Self {
        Self::breeze_dark()
    }
}

impl Theme {
    /// Paleta por defecto, asumiendo terminal oscura.
    ///
    /// Semántica de color:
    /// - cyan: selección/foco (el foco siempre es obvio)
    /// - verde: éxito · rojo: error · amarillo: favorito
    /// - gris: sin foco · gris oscuro: secundario
    pub const fn breeze_dark() -> Self {
        Self {
            text: Color::White,
            bg: Color::Black,
            selection: Color::Cyan,
            unfocused: Color::Gray,
            dim: Color::DarkGray,
            border: Color::Cyan,
            ok: Color::Green,
            error: Color::Red,
            favorite: Color::Yellow,
            source: SourceColors {
                sqlite: Color::Blue,
                duckdb: Color::Green,
                mysql: Color::Red,
                postgres: Color::Magenta,
                generic: Color::Magenta,
            },
        }
    }
}

/// Tema global de la aplicación (single source of truth, cero estado).
pub const THEME: Theme = Theme::breeze_dark();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paleta_por_defecto_tiene_colores_semanticos_diferenciados() {
        let t = Theme::default();
        // Estados opuestos nunca comparten color: el foco es cyan, el
        // error es rojo, el éxito es verde (alto contraste en oscuro).
        assert_ne!(t.selection, t.error);
        assert_ne!(t.selection, t.ok);
        assert_ne!(t.error, t.ok);
        assert_ne!(t.selection, t.unfocused);
        assert_ne!(t.dim, t.unfocused);
        assert_eq!(t, Theme::breeze_dark());
        assert_eq!(THEME, t);
    }

    #[test]
    fn marcas_de_fuente_tienen_identidad_propia() {
        // Cada tipo de backend distingue visualmente su marca.
        let s = THEME.source;
        assert_ne!(s.sqlite, s.duckdb);
        assert_ne!(s.sqlite, s.mysql);
        assert_ne!(s.sqlite, s.postgres);
        assert_ne!(s.duckdb, s.mysql);
        assert_ne!(s.duckdb, s.postgres);
        assert_ne!(s.mysql, s.postgres);
    }
}
