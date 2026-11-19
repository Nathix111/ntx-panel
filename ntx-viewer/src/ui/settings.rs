use crate::state::AppSettings;
use egui::{Color32, RichText, Ui};

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
) -> Option<SettingsAction> {
    let mut action = None;

    // Header
    ui.horizontal(|ui| {
        if ui.button("⬅ Retour").clicked() {
            action = Some(SettingsAction::BackToPanel);
        }

        ui.separator();
        ui.heading("⚙ Paramètres de l'application");

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("💾 Enregistrer").clicked() {
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

    if let Some(warn_msg) = settings_warning {
        ui.add_space(6.0);
        egui::Frame::new()
            .fill(Color32::from_rgb(65, 42, 20))
            .stroke(egui::Stroke::new(1.0_f32, Color32::from_rgb(230, 126, 34)))
            .corner_radius(4.0)
            .inner_margin(egui::Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.label(
                    RichText::new(format!("⚠ {warn_msg}"))
                        .color(Color32::from_rgb(255, 200, 120))
                        .size(12.0),
                );
            });
    }

    if let Some((msg, is_error)) = &state.status_message {
        ui.add_space(4.0);
        let color = if *is_error {
            Color32::from_rgb(231, 76, 60)
        } else {
            Color32::from_rgb(46, 204, 113)
        };
        ui.label(RichText::new(msg).color(color));
    }

    ui.separator();

    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.add_space(8.0);

        // Security / PSK
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.heading("🔑 Sécurité & Clé Pré-Partagée (PSK)");
            ui.add_space(4.0);
            ui.label(
                RichText::new(
                    "Définissez la clé PSK par défaut utilisée lors de la connexion aux agents ntx-server.",
                )
                .weak(),
            );

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Clé PSK par défaut :");
                if state.show_psk {
                    ui.text_edit_singleline(&mut settings.default_psk);
                } else {
                    ui.add(
                        egui::TextEdit::singleline(&mut settings.default_psk)
                            .password(true)
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

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Identifiant Viewer :");
                ui.label(RichText::new(&settings.viewer_id).monospace());
                if ui.button("🔄 Régénérer").clicked() {
                    settings.viewer_id = crate::auth::generate_viewer_id();
                }
            });
        });

        ui.add_space(12.0);

        // Appearance & Monitoring Configuration
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.heading("📊 Apparence & Monitoring");
            ui.add_space(4.0);

            ui.horizontal(|ui| {
                ui.label("Thème de l'interface :");
                let prev = settings.dark_mode;
                ui.selectable_value(&mut settings.dark_mode, true, "🌙 Mode sombre");
                ui.selectable_value(&mut settings.dark_mode, false, "☀ Mode clair");
                if settings.dark_mode != prev {
                    action = Some(SettingsAction::ThemeChanged);
                }
            });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Fréquence de rafraîchissement des métriques :");
                egui::ComboBox::from_id_salt("refresh_rate_combo")
                    .selected_text(format!("{} ms", settings.refresh_rate_ms))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut settings.refresh_rate_ms, 250, "250 ms (Rapide)");
                        ui.selectable_value(&mut settings.refresh_rate_ms, 500, "500 ms");
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

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Rétention de l'historique des graphiques (points) :");
                ui.add(egui::Slider::new(&mut settings.history_capacity, 30..=300));
            });
        });

        ui.add_space(12.0);

        // Saved Device PSK overrides
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("💻 Clés PSK spécifiques par serveur");
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
                .weak(),
            );

            let mut to_remove = None;
            if settings.device_psks.is_empty() {
                ui.label(RichText::new("Aucune clé spécifique configurée.").weak());
            } else {
                egui::Grid::new("device_psks_grid")
                    .striped(true)
                    .min_col_width(120.0)
                    .show(ui, |ui| {
                        ui.label(RichText::new("Identifiant serveur").strong());
                        ui.label(RichText::new("Clé PSK").strong());
                        ui.label(RichText::new("Action").strong());
                        ui.end_row();

                        for (dev_id, psk) in &mut settings.device_psks {
                            ui.label(dev_id.as_str());
                            ui.add(
                                egui::TextEdit::singleline(psk)
                                    .password(!state.show_device_psks)
                                    .desired_width(220.0),
                            );
                            if ui.button("🗑 Supprimer").clicked() {
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

        // About
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.heading("ℹ À propos de NTX Panel");
            ui.label("NTX Panel — Supervision et administration sécurisée de serveurs locaux");
            ui.label(format!("Version du client : {}", env!("CARGO_PKG_VERSION")));
            ui.label("Transport chiffré TLS (TOFU SHA-256), authentification challenge-response HMAC, terminal VT100.");
        });
    });

    action
}
