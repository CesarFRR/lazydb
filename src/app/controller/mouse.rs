//! Handlers de MOUSE de `App` (extraído del monolito: la subcarpeta
//! `controller/` divide el event loop por responsabilidad — teclado en
//! `keys.rs`, mouse aquí, el resto en `mod.rs`).
//!
//! Un `impl App` puede repartirse en varios archivos del mismo módulo:
//! todos comparten acceso a los campos privados del struct.

use ratatui::prelude::Rect;

use super::{
    App, DetailTab, DragState, InputMode, PanelKind, SourceTab, h_scroll_thumb_geometry,
    list_index_from_click, now_millis, v_scroll_thumb_geometry,
};
use crate::ui::widgets::panel::MIN_COL_W;

impl App {
    // ── mouse ─────────────────────────────────────────────────────────

    pub fn on_scroll(&mut self, up: bool, mouse_x: u16, mouse_y: u16) {
        if self.show_row_inspector {
            if up {
                self.inspector_scroll.scroll_up();
            } else {
                self.inspector_scroll.scroll_down();
            }
            return;
        }

        if self.show_actions_menu {
            if up {
                self.actions_menu_idx = self.actions_menu_idx.saturating_sub(1);
            } else {
                let last = Self::ACTION_ITEMS.len().saturating_sub(1);
                self.actions_menu_idx = (self.actions_menu_idx + 1).min(last);
            }
            return;
        }

        // Buscar qué panel está bajo el mouse
        let hovered = self
            .layout
            .panels
            .iter()
            .find(|(_, rect)| {
                mouse_x >= rect.x
                    && mouse_x < rect.x.saturating_add(rect.width)
                    && mouse_y >= rect.y
                    && mouse_y < rect.y.saturating_add(rect.height)
            })
            .map(|(k, _)| *k);

        let Some(target) = hovered else {
            return; // mouse fuera de todos los paneles
        };

        if target == self.active_panel {
            // Panel enfocado: mover selección (comportamiento normal)
            if up {
                self.move_selection(-1);
            } else {
                self.move_selection(1);
            }
        } else {
            // Panel NO enfocado: mover la selección del panel hovered (sin
            // cambiar el foco). El scroll de la vista lo ajusta el render en
            // cada frame (panel.rs) para seguir a la selección — no se toca
            // aquí para que la vista y el cursor nunca se desincronicen
            // (antes el scroll manual solo bajaba en una dirección: la vista
            // se congelaba mientras el status avanzaba de fila, y al volver
            // el foco la vista "saltaba de página").
            let items_len = self.items_len_for(target);
            let old_idx = self.selected_idx(target);

            {
                let p = self.panel_mut(target);
                if up {
                    p.selected_idx =
                        p.selected_idx.saturating_sub(1).min(items_len.saturating_sub(1));
                } else {
                    p.selected_idx = (p.selected_idx + 1).min(items_len.saturating_sub(1));
                }
            } // ── drop `p` ──

            // Header bypass para Data tab no enfocado
            if target == PanelKind::Detail
                && self.data_view.detail_tab == DetailTab::Data
                && items_len > 1
            {
                let p = self.panel_mut(target);
                if p.selected_idx == 0 {
                    p.selected_idx = 1;
                }
            }

            // Scroll infinito en Data tab no enfocado
            if target == PanelKind::Detail
                && self.data_view.detail_tab == DetailTab::Data
                && items_len > 1
            {
                if !up && old_idx == items_len.saturating_sub(1) {
                    self.scroll_down_infinite();
                } else if up && old_idx == 1 && self.data_view.preview_loaded_offset > 0 {
                    self.scroll_up_infinite();
                }
            }
        }
    }

    /// Detecta qué tab del título de Detail fue clickeado.
    /// Formato del título: "[5] [ Datos - row 1/300 ] |  Esquema  |  SQL  |  Meta  | "
    fn detect_detail_tab_click(&self, cursor_x: u16, rect: Rect) -> Option<DetailTab> {
        let available = self.available_detail_tabs();
        let num = PanelKind::Detail.number();
        let prefix = format!("[{num}]");
        // El texto del título empieza en rect.x + 1 (después de la esquina ┌
        // del borde); las pestañas empiezan después del "[N]"
        #[allow(clippy::cast_possible_truncation)]
        let mut cursor = rect.x + 1 + prefix.len() as u16;

        for &tab in &available {
            let text_w = self.detail_tab_display_width(tab);
            if cursor_x >= cursor && cursor_x < cursor + text_w {
                return Some(tab);
            }
            // Separador REAL entre tabs: "|" (1 char, ver title_for → parts.join("|"))
            cursor += text_w + 1;
        }
        None
    }

    /// Detecta qué tab de Fuentes fue clickeado en el título del panel.
    /// Título: "[1]Fuentes (Todo [Local] Online)" — los corchetes marcan el
    /// tab activo. Busca la posición real de cada palabra dentro del string
    /// del título (que empieza en rect.x + 1, después de la esquina ┌).
    fn detect_source_tab_click(&self, cursor_x: u16, rect: Rect) -> Option<SourceTab> {
        let num = PanelKind::Sources.number();
        let tabs = match self.sources_state.source_tab {
            SourceTab::All => "[Todo] Local Online",
            SourceTab::Local => "Todo [Local] Online",
            SourceTab::Online => "Todo Local [Online]",
        };
        let title = format!("[{num}]Fuentes ({tabs})");
        let base = usize::from(rect.x) + 1;
        let cursor = usize::from(cursor_x);

        for (tab, word) in
            [(SourceTab::All, "Todo"), (SourceTab::Local, "Local"), (SourceTab::Online, "Online")]
        {
            if let Some(pos) = title.find(word) {
                let start = base + pos;
                let end = start + word.len();
                if cursor >= start && cursor < end {
                    return Some(tab);
                }
            }
        }
        None
    }

    /// Ancho en columnas del texto de un tab en el título.
    #[allow(clippy::cast_possible_truncation)]
    fn detail_tab_display_width(&self, tab: DetailTab) -> u16 {
        let label = tab.label();
        let inner = if tab == DetailTab::Data && self.data_view.total_rows > 0 {
            let current_row = self.data_view.preview_loaded_offset
                + self.selected_idx(PanelKind::Detail).saturating_sub(1) as u32
                + 1;
            let total = self.data_view.total_rows;
            format!("{label} - row {current_row}/{total}")
        } else {
            label.to_string()
        };
        let padded = if tab == self.data_view.detail_tab {
            format!(" [ {inner} ] ")
        } else {
            format!("  {inner}  ")
        };
        #[allow(clippy::cast_possible_truncation)]
        {
            padded.len() as u16
        }
    }

    /// Desplaza la ventana de columnas visibles del Data tab.
    /// `dir`: -1 = izquierda, 1 = derecha.
    pub fn on_h_scroll(&mut self, dir: i32) {
        let detail = self.panel(PanelKind::Detail);
        let current = detail.h_scroll.get();
        let max_cols = {
            let headers: Vec<&str> = self
                .data_view
                .preview_rows
                .first()
                .map_or_else(Vec::new, |r| r.split(" | ").collect());
            headers.len()
        };
        let inner_w = self
            .layout
            .panels
            .iter()
            .find(|(k, _)| *k == PanelKind::Detail)
            .map_or(0, |(_, r)| usize::from(r.width.saturating_sub(2)));
        if max_cols <= 1 || inner_w == 0 {
            return;
        }
        let total_min = max_cols.saturating_mul(MIN_COL_W);
        if total_min <= inner_w {
            return; // todas las columnas caben, no hay scroll horizontal
        }
        let max_visible = (inner_w / MIN_COL_W).max(1);
        let max_start = max_cols.saturating_sub(max_visible);
        let next = if dir < 0 { current.saturating_sub(1) } else { (current + 1).min(max_start) };
        if next != current {
            self.panel_mut(PanelKind::Detail).h_scroll.set(next);
        }
    }

    /// Dado un click en el área del panel Detail (Data tab), calcula a qué columna
    /// corresponde según la posición X y las columnas parseadas del header.
    fn column_at_x(&self, x: u16, rect: Rect) -> Option<String> {
        if self.data_view.preview_rows.is_empty() {
            return None;
        }
        let headers: Vec<&str> = self.data_view.preview_rows[0].split(" | ").collect();
        let col_count = headers.len();
        if col_count <= 1 {
            return None;
        }
        let inner_w = usize::from(rect.width.saturating_sub(2));
        let h_scroll = self.panel(PanelKind::Detail).h_scroll.get();

        // Misma lógica de ventana que render_data_table
        let total_min = col_count.saturating_mul(MIN_COL_W);
        let (vis_start, cell_widths) = if total_min <= inner_w {
            let cell_base = inner_w / col_count;
            let widths: Vec<usize> = (0..col_count)
                .map(|i| {
                    if i == col_count.saturating_sub(1) {
                        inner_w.saturating_sub(cell_base * (col_count.saturating_sub(1)))
                    } else {
                        cell_base
                    }
                })
                .collect();
            (0, widths)
        } else {
            let max_visible = (inner_w / MIN_COL_W).max(1);
            let vis_start = h_scroll.min(col_count.saturating_sub(max_visible));
            let mut widths = vec![MIN_COL_W; max_visible];
            let rem = inner_w.saturating_sub(max_visible.saturating_mul(MIN_COL_W));
            if let Some(last) = widths.last_mut() {
                *last += rem;
            }
            (vis_start, widths)
        };

        let rel_x = usize::from(x.saturating_sub(rect.x + 1));
        let mut cumul = 0usize;
        for (i, _w) in cell_widths.iter().enumerate() {
            cumul += cell_widths[i];
            if rel_x < cumul {
                let real_idx = vis_start + i;
                if real_idx < headers.len() {
                    return Some(headers[real_idx].trim().to_string());
                }
                return None;
            }
        }
        None
    }

    /// Toggle orden por columna: si ya está ordenando por `col`, invierte ASC↔DESC;
    /// si no, ordena ASC. Si hay un filtro activo, se cancela (el click en header
    /// ordena y limpia el filtro de una vez).
    /// Ciclo de 3 estados al hacer click en el header de una columna:
    /// 1er click → ASC (▴), 2º click → DESC (▾), 3er click → desactivar el
    /// ordenamiento (vuelve al orden por defecto, sin indicador). Es el patrón
    /// estándar de tablas (VS Code, Excel, file managers).
    fn toggle_sort(&mut self, col: String) {
        if !self.filtered_items.is_empty() || self.input_mode == InputMode::Filtering {
            self.cancel_filter();
        }
        if self.data_view.sort_column.as_deref() == Some(col.as_str()) {
            if self.data_view.sort_asc {
                self.data_view.sort_asc = false;
            } else {
                // 3er click: desactivar ordenamiento
                self.data_view.sort_column = None;
                self.data_view.sort_asc = true;
            }
        } else {
            self.data_view.sort_column = Some(col);
            self.data_view.sort_asc = true;
        }
        // Recargar datos desde la página actual con el nuevo orden
        self.data_view.current_page = 0;
        self.data_view.preview_loaded_offset = 0;
        self.refresh_preview_from_selected_object();
    }

    /// Punto de entrada para clicks de mouse (Down). Decide si el click cae
    /// sobre una barra de scroll (inicia drag) o se procesa como click normal.
    pub fn on_mouse_down(&mut self, x: u16, y: u16, width: u16, height: u16) {
        if self.try_start_inspector_scroll_drag(x, y, width, height) {
            return;
        }
        if self.try_start_h_scroll_drag(x, y, width, height) {
            return;
        }
        if self.try_start_v_scroll_drag(x, y, width, height) {
            return;
        }
        self.on_mouse_click(x, y, width, height);
    }

    /// Movimiento del mouse con botón presionado (drag): actualiza la barra
    /// arrastrada. No valida límites del eje para emular scroll de página web.
    pub fn on_mouse_drag(&mut self, x: u16, y: u16) {
        let Some(drag) = self.drag else {
            return;
        };
        match drag {
            DragState::InspectorScroll { rect, content_len } => {
                let viewport = usize::from(rect.height.saturating_sub(2));
                let max_scroll = content_len.saturating_sub(viewport);
                let (_, track) = v_scroll_thumb_geometry(rect.height, content_len, viewport);
                let rel = f32::from(y.saturating_sub(rect.y));
                self.apply_inspector_drag(rel, max_scroll, track);
            }
            DragState::HScroll => {
                if let Some(&(_, rect)) =
                    self.layout.panels.iter().find(|(k, _)| *k == PanelKind::Detail)
                {
                    let headers: Vec<&str> = self
                        .data_view
                        .preview_rows
                        .first()
                        .map_or_else(Vec::new, |r| r.split(" | ").collect());
                    let col_count = headers.len();
                    if col_count <= 1 {
                        return;
                    }
                    let inner_w = usize::from(rect.width.saturating_sub(2));
                    let max_visible = (inner_w / MIN_COL_W).max(1);
                    let max_start = col_count.saturating_sub(max_visible);
                    let (_, track) = h_scroll_thumb_geometry(inner_w, col_count, max_visible);
                    let rel = f32::from(x.saturating_sub(rect.x + 1));
                    self.apply_h_drag(rel, max_start, track);
                }
            }
            DragState::VScroll(kind) => {
                if let Some(&(_, rect)) = self.layout.panels.iter().find(|(k, _)| *k == kind) {
                    let items_len = self.items_len_for(kind);
                    let viewport = usize::from(rect.height.saturating_sub(2));
                    let max_scroll = items_len.saturating_sub(viewport);
                    let (_, track) = v_scroll_thumb_geometry(rect.height, items_len, viewport);
                    let rel = f32::from(y.saturating_sub(rect.y));
                    self.apply_v_drag(rel, kind, max_scroll, track);
                }
            }
        }
    }

    /// Suelta del botón: termina el arrastre.
    pub const fn on_mouse_up(&mut self) {
        self.drag = None;
    }

    /// ¿El click está sobre la barra de scroll horizontal del Data tab?
    /// Si sí, inicia el arrastre y mueve el thumb a la posición del click.
    #[allow(clippy::cast_precision_loss)]
    fn try_start_h_scroll_drag(&mut self, x: u16, y: u16, width: u16, height: u16) -> bool {
        if width < 40 || height < 10 {
            return false;
        }
        if self.show_row_inspector || self.show_actions_menu {
            return false;
        }
        if self.data_view.detail_tab != DetailTab::Data {
            return false;
        }
        let Some(&(_, rect)) = self.layout.panels.iter().find(|(k, _)| *k == PanelKind::Detail)
        else {
            return false;
        };
        // La barra está en la fila del espaciador (rect.y + 1), dentro del inner
        if y != rect.y + 1 || x <= rect.x || x >= rect.x + rect.width - 1 {
            return false;
        }
        let headers: Vec<&str> =
            self.data_view.preview_rows.first().map_or_else(Vec::new, |r| r.split(" | ").collect());
        let col_count = headers.len();
        if col_count <= 1 {
            return false;
        }
        let inner_w = usize::from(rect.width.saturating_sub(2));
        let max_visible = (inner_w / MIN_COL_W).max(1);
        if col_count.saturating_mul(MIN_COL_W) <= inner_w {
            return false; // sin scroll horizontal → sin barra
        }
        let max_start = col_count.saturating_sub(max_visible);

        // Jump-to-position: el thumb salta para quedar CENTRADO bajo el cursor.
        // Desde ahí el arrastre es 1:1 (cada celda de mouse = su proporción del
        // track), así el thumb recorre el 100% del recorrido disponible.
        let (thumb_w, track) = h_scroll_thumb_geometry(inner_w, col_count, max_visible);

        let rel = f32::from(x.saturating_sub(rect.x + 1));
        self.drag = Some(DragState::HScroll);
        self.apply_h_drag(rel - thumb_w as f32 / 2.0, max_start, track);
        true
    }

    /// Convierte la X del mouse en posición de `h_scroll`.
    /// Mapeo 1:1: cada celda del mouse sobre el track equivale a su proporción
    /// del scroll total (`track` = recorrido efectivo del thumb), así el thumb
    /// recorre el 100% del recorrido disponible.
    fn apply_h_drag(&mut self, rel: f32, max_start: usize, track: f32) {
        let pct = (rel / track.max(1.0)).clamp(0.0, 1.0);
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let new = (pct * max_start as f32).round() as usize;
        self.panel_mut(PanelKind::Detail).h_scroll.set(new.min(max_start));
    }

    /// ¿El click está sobre el scrollbar INTERIOR del modal del inspector?
    /// El scrollbar vive en la última columna del inner (dentro del modal):
    /// un click ahí solo puede significar scroll del modal, sin ambigüedad
    /// con los scrollbars de los paneles de detrás.
    #[allow(clippy::cast_precision_loss)]
    fn try_start_inspector_scroll_drag(&mut self, x: u16, y: u16, width: u16, height: u16) -> bool {
        if !self.show_row_inspector || width < 40 || height < 10 {
            return false;
        }
        let rect = crate::ui::widgets::modal::geometry(Rect::new(0, 0, width, height), 70, 70);
        // Última columna del inner (el modal se dibuja con borde: el scrollbar
        // interior está en rect.x + rect.width - 2).
        let sb_x = rect.x.saturating_add(rect.width).saturating_sub(2);
        if x != sb_x || y <= rect.y || y >= rect.y.saturating_add(rect.height).saturating_sub(1) {
            return false;
        }

        let inner = crate::ui::widgets::modal::inner_area(rect);
        let (key_w, val_w) = crate::ui::widgets::modal::table_geometry(inner);
        let expanded = crate::ui::widgets::modal::expand_pairs(
            &self.row_inspector_pairs,
            key_w as usize,
            val_w as usize,
        );
        let content_len = expanded.len().saturating_add(1); // +1 header
        let viewport = usize::from(inner.height.max(1));
        if content_len <= viewport {
            return false; // sin scrollbar visible
        }
        let max_scroll = content_len.saturating_sub(viewport);
        let (thumb_h, track) = v_scroll_thumb_geometry(rect.height, content_len, viewport);

        // Jump-to-position: thumb centrado bajo el cursor, luego 1:1
        let rel = f32::from(y.saturating_sub(rect.y));
        self.drag = Some(DragState::InspectorScroll { rect, content_len });
        self.apply_inspector_drag(rel - thumb_h as f32 / 2.0, max_scroll, track);
        true
    }

    /// Convierte la Y del mouse en offset del scroll del inspector.
    /// Mapeo 1:1 (ver `apply_v_drag`).
    fn apply_inspector_drag(&mut self, rel: f32, max_scroll: usize, track: f32) {
        let pct = (rel / track.max(1.0)).clamp(0.0, 1.0);
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let new = (pct * max_scroll as f32).round() as usize;
        self.inspector_scroll.offset = new.min(max_scroll);
    }

    /// ¿El click está sobre el scrollbar vertical (última columna) de un panel?
    /// Si sí, inicia el arrastre y mueve el thumb a la posición del click.
    #[allow(clippy::cast_precision_loss)]
    fn try_start_v_scroll_drag(&mut self, x: u16, y: u16, width: u16, height: u16) -> bool {
        if width < 40 || height < 10 {
            return false;
        }
        if self.show_row_inspector || self.show_actions_menu {
            return false;
        }
        for &(kind, rect) in &self.layout.panels {
            if x != rect.x + rect.width - 1 || y < rect.y || y >= rect.y + rect.height {
                continue;
            }
            // Detail + Data tab no tiene scrollbar vertical (usa el horizontal)
            if kind == PanelKind::Detail && self.data_view.detail_tab == DetailTab::Data {
                continue;
            }
            let items_len = self.items_len_for(kind);
            let viewport = usize::from(rect.height.saturating_sub(2));
            if items_len <= 1 || items_len <= viewport {
                continue; // sin scrollbar visible
            }
            let max_scroll = items_len.saturating_sub(viewport);
            let (thumb_h, track) = v_scroll_thumb_geometry(rect.height, items_len, viewport);

            // Jump-to-position: thumb centrado bajo el cursor, luego 1:1
            let rel = f32::from(y.saturating_sub(rect.y));
            self.drag = Some(DragState::VScroll(kind));
            self.apply_v_drag(rel - thumb_h as f32 / 2.0, kind, max_scroll, track);
            return true;
        }
        false
    }

    /// Convierte la Y del mouse en `scroll_offset` del panel.
    /// Mapeo 1:1 (ver `apply_h_drag`).
    fn apply_v_drag(&mut self, rel: f32, kind: PanelKind, max_scroll: usize, track: f32) {
        let pct = (rel / track.max(1.0)).clamp(0.0, 1.0);
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let new = (pct * max_scroll as f32).round() as usize;
        let new = new.min(max_scroll);
        let p = self.panel_mut(kind);
        p.scroll_offset.set(new);
        // La selección sigue al scroll para que el scrollbar (posicionado por
        // selected_idx) muestre el thumb en el lugar correcto
        p.selected_idx = new;
    }

    #[allow(clippy::too_many_lines)]
    pub fn on_mouse_click(&mut self, x: u16, y: u16, width: u16, height: u16) {
        if width < 40 || height < 10 {
            return;
        }

        // Formulario de nueva conexión (sin db): click en el botón
        // "[Conectar]" conecta; click en cualquier otra zona del Detail
        // enfoca el panel para que el teclado alimente los campos.
        if self.db_path.is_none() && self.connection.connection_form.is_some() {
            if let Some(&(_, rect)) =
                self.layout.panels.iter().find(|(k, _)| *k == PanelKind::Detail)
            {
                let inside = x >= rect.x
                    && x < rect.x.saturating_add(rect.width)
                    && y >= rect.y
                    && y < rect.y.saturating_add(rect.height);
                if inside {
                    // El botón vive en la fila inner.y + 11 del formulario
                    // (URL + nota + separador + Tipo + 5 campos + hueco).
                    let inner_y = rect.y.saturating_add(1);
                    let connect_row = inner_y.saturating_add(11);
                    if y == connect_row && x > rect.x {
                        self.conn_submit();
                        return;
                    }
                    // Cualquier otra zona: enfocar Detail (captura teclas)
                    self.active_panel = PanelKind::Detail;
                    return;
                }
            }
        }

        // Click fuera del modal de inspector de fila → cerrarlo y continuar
        // con el procesamiento normal del click (seleccionar el ítem clickeado).
        if self.show_row_inspector {
            let mw = width.saturating_mul(70) / 100;
            let mh = height.saturating_mul(70) / 100;
            let mx = width.saturating_sub(mw) / 2;
            let my = height.saturating_sub(mh) / 2;
            let inside =
                x >= mx && x < mx.saturating_add(mw) && y >= my && y < my.saturating_add(mh);
            if inside {
                // Botón del modo NoSQL (`[J: json]` / `[J: pares]`): vive en el
                // título (fila superior del borde del modal), lado derecho.
                if self.is_nosql && y == my {
                    self.toggle_inspector_json_mode();
                }
                return;
            }
            self.close_row_inspector();
        }

        // Encontrar qué panel fue clickeado usando el layout computado
        // El layout ya se calculó en el loop principal antes de renderizar
        for &(kind, rect) in &self.layout.panels {
            if x < rect.x
                || x >= rect.x.saturating_add(rect.width)
                || y < rect.y
                || y >= rect.y.saturating_add(rect.height)
            {
                continue;
            }

            // Click dentro de este panel
            let rel_y = y.saturating_sub(rect.y);

            // ¿Click en el título (primera línea)?
            if rel_y == 0 {
                if kind == PanelKind::Detail {
                    // Detectar click en tabs del título
                    if let Some(tab) = self.detect_detail_tab_click(x, rect) {
                        self.set_detail_tab(tab);
                    }
                } else if kind == PanelKind::Sources {
                    // Detectar click en tabs de Fuentes (Todo/Local/Online)
                    if let Some(tab) = self.detect_source_tab_click(x, rect) {
                        self.set_source_tab(tab);
                    }
                }
                self.set_focus(kind);
                return;
            }

            // Click en contenido
            self.set_focus(kind);

            // Click en header de Data tab → ordenar por columna
            if kind == PanelKind::Detail
                && self.data_view.detail_tab == DetailTab::Data
                && rel_y == 2
            {
                if let Some(col_name) = self.column_at_x(x, rect) {
                    self.toggle_sort(col_name);
                }
                return;
            }

            // Click en un ítem de la lista
            // Para Data tab, las filas de datos empiezan en rel_y=4 (spacer+header+separator)
            let top_reserved =
                if kind == PanelKind::Detail && self.data_view.detail_tab == DetailTab::Data {
                    3
                } else {
                    0
                };
            if let Some(mut index) = list_index_from_click(rel_y, rect.height, top_reserved) {
                if kind == PanelKind::Detail && self.data_view.detail_tab == DetailTab::Data {
                    // +1 porque selected_idx=0 salta el header (primera fila de datos es idx=1)
                    index = index.saturating_add(1);
                }
                let max_idx = self.items_len_for(kind).saturating_sub(1);
                let scroll = self.panel(kind).scroll_offset.get();
                let mut index = (index + scroll).min(max_idx);
                // Click sobre una sección de Fuentes → aterrizar en el primer entry
                if kind == PanelKind::Sources {
                    let shown = self.items_for(kind);
                    index = Self::skip_section_idx(shown, index, 1);
                }
                let p = self.panel_mut(kind);
                p.selected_idx = index;

                // Doble-click: detectar 2 clicks en < 400ms sobre el mismo panel+ítem
                let now = now_millis();
                let is_double = self.last_click_kind == Some(kind)
                    && self.last_click_idx == index
                    && now.saturating_sub(self.last_click_time) < 400;
                self.last_click_time = now;
                self.last_click_kind = Some(kind);
                self.last_click_idx = index;

                if is_double && kind == PanelKind::Detail {
                    self.open_row_inspector();
                    return;
                }

                // Click simple: ejecutar acción del panel sin saltar a Detail
                if kind == PanelKind::Sources {
                    self.connect_selected_source();
                } else if kind == PanelKind::Tables
                    || kind == PanelKind::Views
                    || kind == PanelKind::Advanced
                {
                    self.data_view.current_page = 0;
                    self.refresh_preview_from_selected_object();
                }
                // Detail: doble-click ya manejado arriba, click simple no hace nada extra
            }

            return;
        }

        // Click fuera de cualquier panel → ignorar
    }
}
