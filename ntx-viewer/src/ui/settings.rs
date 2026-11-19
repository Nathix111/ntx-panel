use crate::state::AppSettings;
use crate::ui::theme::{self, Palette};
use egui::{RichText, Stroke, Ui};
use std::collections::HashMap;

pub enum SettingsAction {
    BackToPanel,
    Saved,
    ThemeChanged,
}

#[derive(Default)]
pub struct SettingsState {
    pub show_psk: bool,
    pub show_device_psks: bool,
    pub status_message: Option<(String, bool)>,
}

pub fn render(
    ui: &mut Ui,
    settings: &mut AppSettings,
    state: &mut SettingsState,
    settings_warning: Option<&str>,
    device_names: &HashMap<String, String>,
) -> Option<SettingsAction> {
    let p = Palette::for_ui(ui);
    let mut action = None;

    // 1. Header Bar
    theme::card_frame(&p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            if ui.button("⬅ Retour").clicked() {
                action = Some(SettingsAction::BackToPanel);
            }

            ui.separator();
            ui.label(
                RichText::new("⚙ Paramètres de l'application")
                    .size(18.0)
                    .strong()
                    .color(p.text_primary),
            );

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::primary_button(ui, &p, "💾 Enregistrer").clicked() {
                    match settings.save() {
                        Ok(()) => {
                            state.status_message = Some((
                                "Paramètres enregistrés avec succès !".to_string(),
                                false,
                            ));
                            action = Some(SettingsAction::Saved);
                        }
                        Err(e) => {
                            state.status_message = Some((
                                format!("Erreur lors de l'enregistrement : {e}"),
                                true,
                            ));
                        }
                    }
                }
            });
        });
    });

    if let Some(warn_msg) = settings_warning {
        ui.add_space(8.0);
        egui::Frame::new()
            .fill(p.warning_bg)
            .stroke(Stroke::new(1.0_f32, p.warning_border))
            .corner_radius(6)
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new(format!("⚠ {warn_msg}"))
                        .color(p.warning)
                        .size(12.5),
                );
            });
    }

    if let Some((msg, is_error)) = &state.status_message {
        ui.add_space(8.0);
        let (bg, border, fg) = if *is_error {
            (p.danger_bg, p.danger_border, p.danger)
        } else {
            (p.success_bg, p.success_border, p.success)
        };
        egui::Frame::new()
            .fill(bg)
            .stroke(Stroke::new(1.0_f32, border))
            .corner_radius(6)
            .inner_margin(egui::Margin::symmetric(12, 7))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(msg).color(fg).size(12.5));
            });
    }

    ui.add_space(10.0);

    // 2. Centered Scrollable Settings Column
    let max_col_w = 780.0_f32;
    let avail_w = ui.available_width();
    let col_w = avail_w.min(max_col_w);
    let side_pad = ((avail_w - col_w) * 0.5).max(0.0);

    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.horizontal(|ui| {
            if side_pad > 0.0 {
                ui.add_space(side_pad);
            }
            ui.vertical(|ui| {
                ui.set_width((col_w - 8.0).max(320.0));

                // Card 1: Security & Default PSK
                theme::card_frame(&p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(
                        RichText::new("🔑 Sécurité & Clé Pré-Partagée (PSK)")
                            .size(15.0)
                            .strong()
                            .color(p.text_primary),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(
                            "Clé PSK utilisée par défaut lorsqu'un serveur n'a pas de clé spécifique.",
                        )
                        .color(p.text_secondary)
                        .size(12.0),
                    );

                    ui.add_space(8.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Clé PSK par défaut :");
                        if state.show_psk {
                            ui.add(
                                egui::TextEdit::singleline(&mut settings.default_psk)
                                    .desired_width(280.0),
                            );
                        } else {
                            ui.add(
                                egui::TextEdit::singleline(&mut settings.default_psk)
                                    .password(true)
                                    .desired_width(280.0)
                                    .hint_text("Saisissez la clé PSK réseau..."),
                            );
                        }

                        if ui
                            .button(if state.show_psk {
                                "🙈 Masquer"
                            } else {
                                "👁 Afficher"
                            })
                            .clicked()
                        {
                            state.show_psk = !state.show_psk;
                        }
                    });

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.label("Identifiant Viewer :");
                        ui.label(
                            RichText::new(&settings.viewer_id)
                                .monospace()
                                .color(p.text_secondary),
                        );
                        if ui.button("🔄 Régénérer").clicked() {
                            settings.viewer_id = crate::auth::generate_viewer_id();
                        }
                    });
                });

                ui.add_space(12.0);

                // Card 2: Appearance & Comfort
                theme::card_frame(&p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(
                        RichText::new("✨ Apparence & Ergonomie")
                            .size(15.0)
                            .strong()
                            .color(p.text_primary),
                    );
                    ui.add_space(8.0);

                    ui.horizontal(|ui| {
                        ui.label("Thème de l'interface :");
                        let prev = settings.dark_mode;
                        if theme::tab_button(ui, &p, settings.dark_mode, "🌙 Mode sombre").clicked()
                        {
                            settings.dark_mode = true;
                        }
                        if theme::tab_button(ui, &p, !settings.dark_mode, "☀ Mode clair").clicked()
                        {
                            settings.dark_mode = false;
                        }
                        if settings.dark_mode != prev {
                            action = Some(SettingsAction::ThemeChanged);
                        }
                    });

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.label("Taille de police du terminal VT100 :");
                        ui.add(
                            egui::Slider::new(&mut settings.terminal_font_size, 10.0..=20.0)
                                .step_by(1.0)
                                .suffix(" px"),
                        );
                    });

                    ui.add_space(6.0);
                    ui.checkbox(
                        &mut settings.hide_virtual_disks,
                        "Masquer les disques virtuels par défaut (tmpfs, squashfs, overlay, boot, snap)",
                    );
                    ui.checkbox(
                        &mut settings.hide_virtual_interfaces,
                        "Masquer les interfaces réseau virtuelles par défaut (lo, docker, veth, br-)",
                    );
                });

                ui.add_space(12.0);

                // Card 3: Monitoring & History
                theme::card_frame(&p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(
                        RichText::new("📊 Monitoring & Rétention")
                            .size(15.0)
                            .strong()
                            .color(p.text_primary),
                    );
                    ui.add_space(8.0);

                    ui.horizontal(|ui| {
                        ui.label("Fréquence de rafraîchissement des métriques :");
                        egui::ComboBox::from_id_salt("refresh_rate_combo")
                            .selected_text(format!("{} ms", settings.refresh_rate_ms))
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut settings.refresh_rate_ms,
                                    250,
                                    "250 ms (Rapide)",
                                );
                                ui.selectable_value(
                                    &mut settings.refresh_rate_ms,
                                    500,
                                    "500 ms",
                                );
                                ui.selectable_value(
                                    &mut settings.refresh_rate_ms,
                                    1000,
                                    "1000 ms (Recommandé)",
                                );
                                ui.selectable_value(
                                    &mut settings.refresh_rate_ms,
                                    2000,
                                    "2000 ms (Économie réseau)",
                                );
                            });
                    });

                    ui.add_space(8.0);
                    let window_secs = (settings.history_capacity as f64)
                        * (settings.refresh_rate_ms.max(250) as f64 / 1000.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Rétention des graphiques :");
                        ui.add(
                            egui::Slider::new(&mut settings.history_capacity, 30..=300)
                                .suffix(" pts"),
                        );
                        theme::pill_badge(
                            ui,
                            &format!("Fenêtre : ~{window_secs:.0} s"),
                            p.accent_bg,
                            p.accent,
                            None,
                        );
                    });
                });

                ui.add_space(12.0);

                // Card 4: Saved Device PSK overrides
                theme::card_frame(&p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("💻 Clés PSK spécifiques par serveur")
                                .size(15.0)
                                .strong()
                                .color(p.text_primary),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if !settings.device_psks.is_empty()
                                && ui
                                    .small_button(if state.show_device_psks {
                                        "🙈 Masquer les clés"
                                    } else {
                                        "👁 Afficher les clés"
                                    })
                                    .clicked()
                            {
                                state.show_device_psks = !state.show_device_psks;
                            }
                        });
                    });
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(
                            "Personnalisez la clé PSK d'un serveur précis (prioritaire sur la clé par défaut).",
                        )
                        .color(p.text_secondary)
                        .size(12.0),
                    );
                    ui.add_space(8.0);

                    let mut to_remove = None;
                    if settings.device_psks.is_empty() {
                        ui.label(
                            RichText::new("Aucune clé spécifique configurée.").color(p.text_muted),
                        );
                    } else {
                        egui::Grid::new("device_psks_grid")
                            .striped(true)
                            .min_col_width(140.0)
                            .spacing([14.0, 8.0])
                            .show(ui, |ui| {
                                ui.label(RichText::new("Serveur").strong());
                                ui.label(RichText::new("Clé PSK").strong());
                                ui.label(RichText::new("Action").strong());
                                ui.end_row();

                                for (dev_id, psk) in &mut settings.device_psks {
                                    ui.vertical(|ui| {
                                        if let Some(friendly) = device_names.get(dev_id) {
                                            ui.label(
                                                RichText::new(friendly)
                                                    .strong()
                                                    .color(p.text_primary),
                                            );
                                            ui.label(
                                                RichText::new(dev_id.as_str())
                                                    .monospace()
                                                    .size(10.5)
                                                    .color(p.text_muted),
                                            );
                                        } else {
                                            ui.label(
                                                RichText::new(dev_id.as_str()).monospace(),
                                            );
                                        }
                                    });
                                    ui.add(
                                        egui::TextEdit::singleline(psk)
                                            .password(!state.show_device_psks)
                                            .desired_width(240.0),
                                    );
                                    if theme::danger_button(ui, &p, "🗑 Supprimer").clicked() {
                                        to_remove = Some(dev_id.clone());
                                    }
                                    ui.end_row();
                                }
                            });
                    }

                    if let Some(key) = to_remove {
                        settings.device_psks.remove(&key);
                    }
                });

                ui.add_space(12.0);

                // Card 5: About & Config Path
                theme::card_frame(&p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(
                        RichText::new("ℹ À propos & Stockage local")
                            .size(15.0)
                            .strong()
                            .color(p.text_primary),
                    );
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(
                            "NTX Panel — Supervision et administration sécurisée de serveurs locaux",
                        )
                        .color(p.text_primary),
                    );
                    ui.label(
                        RichText::new(format!(
                            "Version du client : {}  •  Transport TLS (TOFU SHA-256), challenge HMAC, VT100",
                            env!("CARGO_PKG_VERSION")
                        ))
                        .size(12.0)
                        .color(p.text_secondary),
                    );
                    ui.add_space(8.0);

                    let cfg_path = AppSettings::config_path_display();
                    theme::subtle_bar_frame(&p).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                RichText::new("Fichier de configuration :")
                                    .size(11.5)
                                    .color(p.text_muted),
                            );
                            ui.label(
                                RichText::new(&cfg_path)
                                    .monospace()
                                    .size(11.5)
                                    .color(p.text_primary),
                            );
                            if ui
                                .small_button("📋 Copier le chemin")
                                .on_hover_text("Copier le chemin de settings.toml")
                                .clicked()
                            {
                                ui.ctx().copy_text(cfg_path);
                            }
                        });
                    });
                });

                ui.add_space(16.0);
            });
        });
    });

    action
}
