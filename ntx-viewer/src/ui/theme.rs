use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, RichText, Stroke, TextStyle, Ui, Vec2};

/// Semantic color palette adapted for both Dark ("Cockpit Slate") and Light ("Porcelain") modes.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub is_dark: bool,
    pub bg_app: Color32,
    pub bg_sidebar: Color32,
    pub bg_card: Color32,
    pub bg_card_hover: Color32,
    pub bg_card_selected: Color32,
    pub bg_subtle: Color32,
    pub border: Color32,
    pub border_strong: Color32,
    pub bar_track: Color32,
    pub text_primary: Color32,
    pub text_secondary: Color32,
    pub text_muted: Color32,
    pub accent: Color32,
    pub accent_bg: Color32,
    pub accent_border: Color32,
    pub primary_btn_bg: Color32,
    pub primary_btn_fg: Color32,
    pub success: Color32,
    pub success_bg: Color32,
    pub success_border: Color32,
    pub warning: Color32,
    pub warning_bg: Color32,
    pub warning_border: Color32,
    pub danger: Color32,
    pub danger_bg: Color32,
    pub danger_border: Color32,
    pub purple: Color32,
    pub orange: Color32,
}

impl Palette {
    pub fn dark() -> Self {
        Self {
            is_dark: true,
            bg_app: Color32::from_rgb(13, 17, 23),
            bg_sidebar: Color32::from_rgb(17, 22, 31),
            bg_card: Color32::from_rgb(22, 28, 38),
            bg_card_hover: Color32::from_rgb(29, 37, 50),
            bg_card_selected: Color32::from_rgb(20, 42, 60),
            bg_subtle: Color32::from_rgb(27, 34, 46),
            border: Color32::from_rgb(41, 51, 69),
            border_strong: Color32::from_rgb(60, 74, 98),
            bar_track: Color32::from_rgb(38, 48, 64),
            text_primary: Color32::from_rgb(232, 238, 246),
            text_secondary: Color32::from_rgb(154, 166, 184),
            text_muted: Color32::from_rgb(110, 123, 143),
            accent: Color32::from_rgb(34, 211, 238),
            accent_bg: Color32::from_rgb(18, 48, 66),
            accent_border: Color32::from_rgb(34, 139, 166),
            primary_btn_bg: Color32::from_rgb(8, 145, 178),
            primary_btn_fg: Color32::WHITE,
            success: Color32::from_rgb(52, 211, 153),
            success_bg: Color32::from_rgb(19, 49, 41),
            success_border: Color32::from_rgb(36, 106, 83),
            warning: Color32::from_rgb(251, 191, 36),
            warning_bg: Color32::from_rgb(55, 41, 20),
            warning_border: Color32::from_rgb(138, 94, 27),
            danger: Color32::from_rgb(248, 113, 113),
            danger_bg: Color32::from_rgb(61, 28, 32),
            danger_border: Color32::from_rgb(153, 56, 65),
            purple: Color32::from_rgb(192, 132, 252),
            orange: Color32::from_rgb(251, 146, 60),
        }
    }

    pub fn light() -> Self {
        Self {
            is_dark: false,
            bg_app: Color32::from_rgb(244, 246, 249),
            bg_sidebar: Color32::from_rgb(235, 240, 245),
            bg_card: Color32::WHITE,
            bg_card_hover: Color32::from_rgb(248, 250, 252),
            bg_card_selected: Color32::from_rgb(226, 243, 250),
            bg_subtle: Color32::from_rgb(236, 241, 247),
            border: Color32::from_rgb(214, 222, 233),
            border_strong: Color32::from_rgb(180, 194, 212),
            bar_track: Color32::from_rgb(221, 228, 238),
            text_primary: Color32::from_rgb(15, 23, 42),
            text_secondary: Color32::from_rgb(71, 85, 105),
            text_muted: Color32::from_rgb(100, 116, 139),
            accent: Color32::from_rgb(8, 145, 178),
            accent_bg: Color32::from_rgb(224, 242, 254),
            accent_border: Color32::from_rgb(56, 189, 248),
            primary_btn_bg: Color32::from_rgb(2, 132, 199),
            primary_btn_fg: Color32::WHITE,
            success: Color32::from_rgb(5, 150, 105),
            success_bg: Color32::from_rgb(220, 252, 231),
            success_border: Color32::from_rgb(110, 231, 183),
            warning: Color32::from_rgb(217, 119, 6),
            warning_bg: Color32::from_rgb(254, 243, 199),
            warning_border: Color32::from_rgb(252, 211, 77),
            danger: Color32::from_rgb(220, 38, 38),
            danger_bg: Color32::from_rgb(254, 226, 226),
            danger_border: Color32::from_rgb(252, 165, 165),
            purple: Color32::from_rgb(147, 51, 234),
            orange: Color32::from_rgb(234, 88, 12),
        }
    }

    pub fn for_ui(ui: &Ui) -> Self {
        if ui.visuals().dark_mode {
            Self::dark()
        } else {
            Self::light()
        }
    }
}

/// Apply the Cockpit Slate / Porcelain design system onto `egui::Context`.
pub fn apply_theme(ctx: &egui::Context, dark_mode: bool) {
    let p = if dark_mode {
        Palette::dark()
    } else {
        Palette::light()
    };

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = Vec2::new(8.0, 6.0);
    style.spacing.button_padding = Vec2::new(10.0, 5.0);
    style.spacing.window_margin = Margin::same(14);
    style.spacing.menu_margin = Margin::same(8);
    style.spacing.indent = 16.0;

    style.text_styles.insert(
        TextStyle::Heading,
        FontId::new(18.0, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Body,
        FontId::new(13.5, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Button,
        FontId::new(13.0, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Small,
        FontId::new(11.5, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Monospace,
        FontId::new(13.0, FontFamily::Monospace),
    );

    let mut visuals = if dark_mode {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };

    visuals.window_corner_radius = CornerRadius::same(10);
    visuals.menu_corner_radius = CornerRadius::same(8);
    visuals.panel_fill = p.bg_app;
    visuals.window_fill = p.bg_card;
    visuals.window_stroke = Stroke::new(1.0_f32, p.border_strong);
    visuals.faint_bg_color = p.bg_subtle;
    visuals.extreme_bg_color = if dark_mode {
        Color32::from_rgb(11, 15, 20)
    } else {
        Color32::from_rgb(248, 250, 252)
    };
    visuals.code_bg_color = p.bg_subtle;
    visuals.hyperlink_color = p.accent;
    visuals.selection.bg_fill = p.accent_bg;
    visuals.selection.stroke = Stroke::new(1.0_f32, p.accent);

    let r_widget = CornerRadius::same(6);
    visuals.widgets.noninteractive.corner_radius = r_widget;
    visuals.widgets.noninteractive.bg_fill = p.bg_card;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, p.border);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, p.text_secondary);

    visuals.widgets.inactive.corner_radius = r_widget;
    visuals.widgets.inactive.bg_fill = p.bg_subtle;
    visuals.widgets.inactive.weak_bg_fill = p.bg_subtle;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, p.border);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, p.text_primary);

    visuals.widgets.hovered.corner_radius = r_widget;
    visuals.widgets.hovered.bg_fill = p.bg_card_hover;
    visuals.widgets.hovered.weak_bg_fill = p.bg_card_hover;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, p.border_strong);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.2_f32, p.text_primary);

    visuals.widgets.active.corner_radius = r_widget;
    visuals.widgets.active.bg_fill = p.accent_bg;
    visuals.widgets.active.weak_bg_fill = p.accent_bg;
    visuals.widgets.active.bg_stroke = Stroke::new(1.2_f32, p.accent_border);
    visuals.widgets.active.fg_stroke = Stroke::new(1.5_f32, p.text_primary);

    visuals.widgets.open.corner_radius = r_widget;
    visuals.widgets.open.bg_fill = p.bg_card_hover;
    visuals.widgets.open.weak_bg_fill = p.bg_card_hover;
    visuals.widgets.open.bg_stroke = Stroke::new(1.0_f32, p.accent_border);

    style.visuals = visuals;
    ctx.set_style(style);
}

pub fn card_frame(p: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(p.bg_card)
        .stroke(Stroke::new(1.0_f32, p.border))
        .corner_radius(8)
        .inner_margin(Margin::symmetric(14, 12))
}

pub fn subtle_bar_frame(p: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(p.bg_subtle)
        .stroke(Stroke::new(1.0_f32, p.border))
        .corner_radius(6)
        .inner_margin(Margin::symmetric(12, 8))
}

pub fn pill_badge(
    ui: &mut Ui,
    text: &str,
    bg: Color32,
    fg: Color32,
    border: Option<Color32>,
) {
    let mut frame = egui::Frame::new()
        .fill(bg)
        .corner_radius(10)
        .inner_margin(Margin::symmetric(8, 2));
    if let Some(b) = border {
        frame = frame.stroke(Stroke::new(1.0_f32, b));
    }
    frame.show(ui, |ui| {
        ui.label(RichText::new(text).color(fg).size(11.0).strong());
    });
}

pub fn primary_button(ui: &mut Ui, p: &Palette, text: &str) -> egui::Response {
    let btn = egui::Button::new(RichText::new(text).color(p.primary_btn_fg).strong())
        .fill(p.primary_btn_bg)
        .stroke(Stroke::new(1.0_f32, p.accent_border))
        .corner_radius(6);
    ui.add(btn)
}

pub fn danger_button(ui: &mut Ui, p: &Palette, text: &str) -> egui::Response {
    let btn = egui::Button::new(RichText::new(text).color(p.danger).strong())
        .fill(p.danger_bg)
        .stroke(Stroke::new(1.0_f32, p.danger_border))
        .corner_radius(6);
    ui.add(btn)
}

pub fn tab_button(ui: &mut Ui, p: &Palette, selected: bool, text: &str) -> egui::Response {
    let (fill, stroke, fg) = if selected {
        (
            p.accent_bg,
            Stroke::new(1.0_f32, p.accent_border),
            p.text_primary,
        )
    } else {
        (
            Color32::TRANSPARENT,
            Stroke::new(1.0_f32, Color32::TRANSPARENT),
            p.text_secondary,
        )
    };
    let mut rich = RichText::new(text).color(fg).size(13.0);
    if selected {
        rich = rich.strong();
    }
    let btn = egui::Button::new(rich)
        .fill(fill)
        .stroke(stroke)
        .corner_radius(6);
    ui.add(btn)
}

pub fn mini_progress_bar(
    ui: &mut Ui,
    p: &Palette,
    fraction: f32,
    color: Color32,
    width: f32,
    height: f32,
) -> egui::Response {
    let clamped = fraction.clamp(0.0, 1.0);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), egui::Sense::hover());
    let rounding = (height * 0.5).min(4.0);
    ui.painter().rect_filled(rect, rounding, p.bar_track);
    if clamped > 0.005 {
        let filled_w = (rect.width() * clamped).max(3.0);
        let filled_rect = egui::Rect::from_min_size(rect.min, Vec2::new(filled_w, rect.height()));
        ui.painter().rect_filled(filled_rect, rounding, color);
    }
    response
}

pub fn usage_color(p: &Palette, pct: f64) -> Color32 {
    if pct >= 85.0 {
        p.danger
    } else if pct >= 60.0 {
        p.warning
    } else {
        p.success
    }
}

pub fn latency_color(p: &Palette, ms: u32) -> Color32 {
    if ms >= 150 {
        p.danger
    } else if ms >= 60 {
        p.warning
    } else {
        p.accent
    }
}

/// Draw a compact inline sparkline with subtle gradient fill and latest-sample marker.
#[allow(clippy::too_many_arguments)]
pub fn sparkline(
    ui: &mut Ui,
    p: &Palette,
    values: &[f64],
    min_val: f64,
    max_val: f64,
    color: Color32,
    width: f32,
    height: f32,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), egui::Sense::hover());
    if !ui.is_rect_visible(rect) {
        return response;
    }

    let painter = ui.painter();
    let rounding = 4.0_f32;
    painter.rect(
        rect,
        rounding,
        p.bg_subtle,
        Stroke::new(0.5_f32, p.border),
        egui::StrokeKind::Inside,
    );

    let inner = rect.shrink(2.0);
    if inner.width() <= 1.0 || inner.height() <= 1.0 {
        return response;
    }

    let mid_y = inner.center().y;
    painter.line_segment(
        [egui::pos2(inner.left(), mid_y), egui::pos2(inner.right(), mid_y)],
        Stroke::new(0.5_f32, p.border.gamma_multiply(0.45)),
    );

    if values.is_empty() {
        painter.line_segment(
            [
                egui::pos2(inner.left(), inner.bottom()),
                egui::pos2(inner.right(), inner.bottom()),
            ],
            Stroke::new(1.0_f32, p.border),
        );
        return response;
    }

    let lo = if min_val.is_finite() { min_val } else { 0.0 };
    let hi = if max_val.is_finite() && max_val > lo {
        max_val
    } else {
        lo + 1.0
    };
    let span = hi - lo;

    let map_y = |v: f64| -> f32 {
        let safe = if v.is_finite() { v } else { lo };
        let norm = ((safe.clamp(lo, hi) - lo) / span) as f32;
        inner.bottom() - norm * inner.height()
    };

    let points: Vec<egui::Pos2> = if values.len() == 1 {
        let y = map_y(values[0]);
        vec![egui::pos2(inner.left(), y), egui::pos2(inner.right(), y)]
    } else {
        let last_idx = (values.len() - 1) as f32;
        values
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                let x = inner.left() + (i as f32 / last_idx) * inner.width();
                egui::pos2(x, map_y(v))
            })
            .collect()
    };

    let top_fill = color.gamma_multiply(0.18);
    let bot_fill = color.gamma_multiply(0.03);
    let mut mesh = egui::Mesh::default();
    for pt in &points {
        mesh.colored_vertex(*pt, top_fill);
        mesh.colored_vertex(egui::pos2(pt.x, inner.bottom()), bot_fill);
    }
    for i in 0..points.len().saturating_sub(1) {
        let a = (i * 2) as u32;
        let b = ((i + 1) * 2) as u32;
        let c = (i * 2 + 1) as u32;
        let d = ((i + 1) * 2 + 1) as u32;
        mesh.add_triangle(a, b, c);
        mesh.add_triangle(b, d, c);
    }
    painter.add(egui::Shape::mesh(mesh));
    painter.line(points.clone(), Stroke::new(1.5_f32, color));

    if let Some(&last_pt) = points.last() {
        painter.circle_filled(last_pt, 2.2, color);
    }

    response
}

pub fn format_uptime(secs: u64) -> String {
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let mins = (secs % 3_600) / 60;
    if days > 0 {
        format!("{days}j {hours}h")
    } else if hours > 0 {
        format!("{hours}h {mins:02}m")
    } else if mins > 0 {
        format!("{mins}m")
    } else {
        format!("{secs}s")
    }
}

pub fn format_bytes_rate(bytes_per_sec: f64) -> String {
    if bytes_per_sec < 1024.0 {
        format!("{:.0} o/s", bytes_per_sec)
    } else if bytes_per_sec < 1024.0 * 1024.0 {
        format!("{:.1} Ko/s", bytes_per_sec / 1024.0)
    } else if bytes_per_sec < 1024.0 * 1024.0 * 1024.0 {
        format!("{:.2} Mo/s", bytes_per_sec / (1024.0 * 1024.0))
    } else {
        format!("{:.2} Go/s", bytes_per_sec / (1024.0 * 1024.0 * 1024.0))
    }
}

pub fn format_ram(bytes: u64) -> String {
    let mb = bytes as f64 / (1024.0 * 1024.0);
    if mb >= 1024.0 {
        format!("{:.2} Go", mb / 1024.0)
    } else {
        format!("{:.1} Mo", mb)
    }
}

pub fn format_storage(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * KB;
    const GB: f64 = 1024.0 * MB;
    const TB: f64 = 1024.0 * GB;
    let b = bytes as f64;
    if bytes < 1024 {
        format!("{bytes} o")
    } else if b < MB {
        format!("{:.0} Ko", b / KB)
    } else if b < GB {
        format!("{:.0} Mo", b / MB)
    } else if b < TB {
        format!("{:.1} Go", b / GB)
    } else {
        format!("{:.2} To", b / TB)
    }
}

pub fn os_icon(os_name: Option<&str>) -> &'static str {
    let Some(raw) = os_name else {
        return "🖥";
    };
    let os = raw.to_lowercase();
    if os.contains("raspbian")
        || os.contains("debian")
        || os.contains("ubuntu")
        || os.contains("arch")
        || os.contains("fedora")
        || os.contains("linux")
    {
        "🐧"
    } else if os.contains("mac") || os.contains("darwin") {
        "🍎"
    } else {
        "🖥"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_uptime() {
        assert_eq!(format_uptime(45), "45s");
        assert_eq!(format_uptime(125), "2m");
        assert_eq!(format_uptime(3660), "1h 01m");
        assert_eq!(format_uptime(90060), "1j 1h");
    }

    #[test]
    fn test_format_bytes_rate_and_ram() {
        assert_eq!(format_bytes_rate(512.0), "512 o/s");
        assert_eq!(format_bytes_rate(2048.0), "2.0 Ko/s");
        assert_eq!(format_bytes_rate(5.0 * 1024.0 * 1024.0), "5.00 Mo/s");
        assert_eq!(format_ram(512 * 1024 * 1024), "512.0 Mo");
        assert_eq!(format_ram(2 * 1024 * 1024 * 1024), "2.00 Go");
    }

    #[test]
    fn test_os_icon_and_usage_color() {
        assert_eq!(os_icon(Some("Debian GNU/Linux")), "🐧");
        assert_eq!(os_icon(Some("Darwin")), "🍎");
        assert_eq!(os_icon(Some("Windows")), "🖥");
        assert_eq!(os_icon(None), "🖥");

        let dark = Palette::dark();
        assert_eq!(usage_color(&dark, 25.0), dark.success);
        assert_eq!(usage_color(&dark, 70.0), dark.warning);
        assert_eq!(usage_color(&dark, 92.0), dark.danger);
    }

    #[test]
    fn test_format_storage_and_latency_color() {
        assert_eq!(format_storage(512), "512 o");
        assert_eq!(format_storage(2048), "2 Ko");
        assert_eq!(format_storage(50 * 1024 * 1024), "50 Mo");
        assert_eq!(format_storage(128 * 1024 * 1024 * 1024), "128.0 Go");
        assert_eq!(format_storage(2 * 1024 * 1024 * 1024 * 1024), "2.00 To");

        let dark = Palette::dark();
        assert_eq!(latency_color(&dark, 15), dark.accent);
        assert_eq!(latency_color(&dark, 60), dark.warning);
        assert_eq!(latency_color(&dark, 150), dark.danger);
    }
}
