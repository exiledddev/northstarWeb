//! The door: signing in with Discord, drawn with the app's own design system
//! — NativeCompute's — so it is the same app from the very first frame.

use blackline::eframe::egui::{self, Color32, Pos2, Rect, Vec2};
use blackline::eframe;
use blackline::icons::{self, Icon};
use blackline::{anim, chrome, logo, theme, ui};

use crate::browser;

/// Why the door is shown.
pub enum Why {
    /// Not signed in.
    SignIn,
    /// Discord said who they are, and they are not on the team's list.
    Denied(String),
    /// Something went wrong on the way back from Discord.
    Trouble(&'static str),
}

pub struct SignIn {
    why: Why,
    copied: bool,
}

impl SignIn {
    pub fn new(cc: &eframe::CreationContext<'_>, why: Why) -> SignIn {
        theme::set_palette(true);
        theme::apply(&cc.egui_ctx);
        theme::install_fonts(&cc.egui_ctx);
        SignIn { why, copied: false }
    }
}

impl eframe::App for SignIn {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let p = theme::pal();
        chrome::backdrop(ctx, p.backdrop);
        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                let screen = ui.max_rect();
                // the card is as tall as what it has to say
                let tall = match self.why {
                    Why::SignIn => 240.0,
                    Why::Trouble(_) => 262.0,
                    Why::Denied(_) => 330.0,
                };
                let card = Rect::from_center_size(screen.center(), Vec2::new(420.0_f32.min(screen.width() - 32.0), tall));
                // the site's panel: opaque, a strong hairline, square corners
                ui.painter().rect_filled(card, egui::Rounding::same(theme::R_ISLAND), p.solid);
                ui.painter().rect_stroke(
                    card.shrink(0.5),
                    egui::Rounding::same(theme::R_ISLAND),
                    egui::Stroke::new(1.0_f32, p.line_strong),
                );
                theme::top_edge(ui.painter(), card);
                let inner = card.shrink2(Vec2::new(30.0, 28.0));
                let mut c = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(inner)
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );

                // the eyebrow: the mark and a HUD line, then the name set
                // giant across the card, as the splash has them
                c.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    let (mark, _) = ui.allocate_exact_size(Vec2::splat(20.0), egui::Sense::hover());
                    logo::paint(ui.painter(), mark, 0.0);
                    ui::hud(ui, "Team edition / sign in", Some(p.text_dim));
                });
                c.add_space(14.0);
                let word = "BLACKLINE";
                let probe = c.painter().layout_no_wrap(word.to_string(), theme::font_display(100.0), p.text);
                // a hair under the full measure, so rounding never wraps it
                let size = 99.0 * inner.width() / probe.rect.width().max(1.0);
                c.add(egui::Label::new(egui::RichText::new(word).font(theme::font_display(size)).color(p.text)).extend());
                c.add_space(18.0);

                match &self.why {
                    Why::SignIn | Why::Trouble(_) => {
                        c.label(
                            egui::RichText::new("Sign in to your team's library.")
                                .font(theme::font_med(theme::T_SM))
                                .color(p.text_dim),
                        );
                        if let Why::Trouble(what) = self.why {
                            c.add_space(4.0);
                            c.label(egui::RichText::new(what).font(theme::font(theme::T_CAP)).color(p.danger_light));
                        }
                        c.add_space(14.0);
                        if discord_button(&mut c, card.width() - 60.0) {
                            browser::go("/auth/discord");
                        }
                    }
                    Why::Denied(id) => {
                        c.label(
                            egui::RichText::new("You're not on this team's list yet.")
                                .font(theme::font_semi(theme::T_SM))
                                .color(p.text),
                        );
                        c.add_space(4.0);
                        c.label(
                            egui::RichText::new("Send your Discord ID to one of the team's owners. Once they add you, sign in again.")
                                .font(theme::font(theme::T_CAP))
                                .color(p.text_dim),
                        );
                        c.add_space(10.0);
                        let id = id.clone();
                        c.horizontal(|ui| {
                            ui.label(egui::RichText::new(&id).font(theme::font_mono(theme::T_SM)).color(p.text));
                            if ui::button(ui, if self.copied { "Copied" } else { "Copy" }, Some(Icon::Copy), false) {
                                ui.ctx().copy_text(id.clone());
                                self.copied = true;
                            }
                        });
                        c.add_space(12.0);
                        if discord_button(&mut c, card.width() - 60.0) {
                            browser::go("/auth/discord");
                        }
                    }
                }

                // the studio, at the foot of the window
                let foot = Pos2::new(screen.center().x, screen.bottom() - 26.0);
                let text = "BLACKLINE BY NATIVECOMPUTE";
                let font = theme::font_hud(theme::T_MICRO);
                let w = theme::tracked_width(ui.ctx(), text, font.clone(), 0.8);
                theme::tracked_text(ui.painter(), Pos2::new(foot.x - w * 0.5, foot.y), text, font, p.text_faint, 0.8);
            });
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }
}

/// Discord's own colour, as their brand asks, in the site's primary button:
/// white fills it from the left on hover and the label goes black with it.
fn discord_button(ui: &mut egui::Ui, width: f32) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 42.0), egui::Sense::click());
    let hot = anim::ease(ui.ctx(), resp.id, resp.hovered(), 0.30);
    let blurple = Color32::from_rgb(0x58, 0x65, 0xF2);
    ui.painter().rect_filled(rect, egui::Rounding::same(theme::R_SM), blurple);
    ui::wipe(ui.painter(), rect, hot, Color32::WHITE);
    let ink = theme::mix(Color32::WHITE, Color32::BLACK, hot);
    let label = "Continue with Discord";
    let galley = ui.painter().layout_no_wrap(label.to_string(), theme::font_semi(theme::T_SM), ink);
    let total = 18.0 + 9.0 + galley.rect.width();
    let x = rect.center().x - total * 0.5;
    icons::draw(
        ui.painter(),
        Rect::from_center_size(Pos2::new(x + 9.0, rect.center().y), Vec2::splat(18.0)),
        Icon::Discord,
        ink,
    );
    ui.painter().galley(
        Pos2::new(x + 27.0, rect.center().y - galley.rect.height() * 0.5),
        galley,
        ink,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}
