//! The door: signing in with Discord, drawn with the app's own design system
//! so it is the same app from the very first frame.

use northstar::eframe::egui::{self, Color32, Pos2, Rect, Vec2};
use northstar::eframe;
use northstar::icons::{self, Icon};
use northstar::{anim, chrome, logo, theme, ui};

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
        theme::set_palette(theme::ThemeId::Bloodmoon, true);
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
                    Why::SignIn => 252.0,
                    Why::Trouble(_) => 274.0,
                    Why::Denied(_) => 340.0,
                };
                let card = Rect::from_center_size(screen.center(), Vec2::new(400.0_f32.min(screen.width() - 32.0), tall));
                theme::lift_shadow(ui.painter(), card, theme::R_ISLAND, 0.8);
                ui.painter().rect_filled(card, egui::Rounding::same(theme::R_ISLAND), p.solid);
                let mut c = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(card.shrink2(Vec2::new(30.0, 26.0)))
                        .layout(egui::Layout::top_down(egui::Align::Center)),
                );

                // the mark and the name, as the splash has them
                let (mark, _) = c.allocate_exact_size(Vec2::splat(46.0), egui::Sense::hover());
                logo::paint(c.painter(), mark, 0.0);
                c.add_space(10.0);
                let (name, _) = c.allocate_exact_size(Vec2::new(card.width() - 60.0, 18.0), egui::Sense::hover());
                let word = "NORTHSTAR";
                let galley = c.painter().layout_no_wrap(word.to_string(), theme::font_semi(15.0), p.text);
                let w = galley.rect.width() + 3.2 * (word.len() as f32 - 1.0);
                theme::tracked_text(c.painter(), Pos2::new(name.center().x - w * 0.5, name.center().y), word, theme::font_semi(15.0), p.text, 3.2);
                c.add_space(4.0);
                c.label(
                    egui::RichText::new("Screenwriting, in industry format")
                        .font(theme::font(theme::T_CAP))
                        .color(p.text_faint),
                );
                c.add_space(20.0);

                match &self.why {
                    Why::SignIn | Why::Trouble(_) => {
                        c.label(
                            egui::RichText::new("Sign in to your team's library.")
                                .font(theme::font_med(theme::T_SM))
                                .color(p.text_dim),
                        );
                        if let Why::Trouble(what) = self.why {
                            c.add_space(4.0);
                            c.label(egui::RichText::new(what).font(theme::font(theme::T_CAP)).color(p.sec_light));
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
                            let room = ui.available_width();
                            let id_w = 190.0;
                            ui.add_space(((room - id_w - 80.0) * 0.5).max(0.0));
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
                let text = "MARKEDEXILED SOFTWARE";
                let galley = ui.painter().layout_no_wrap(text.to_string(), theme::font_semi(theme::T_MICRO - 0.5), p.text_faint);
                let w = galley.rect.width() + 1.4 * (text.len() as f32 - 1.0);
                theme::tracked_text(
                    ui.painter(),
                    Pos2::new(foot.x - w * 0.5, foot.y),
                    text,
                    theme::font_semi(theme::T_MICRO - 0.5),
                    theme::wash(p.text_faint, 150),
                    1.4,
                );
            });
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }
}

/// Discord's own colour, as their brand asks — solid, no glow; it shifts a
/// shade on hover, as every button in the app does.
fn discord_button(ui: &mut egui::Ui, width: f32) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 40.0), egui::Sense::click());
    let hot = anim::ease(ui.ctx(), resp.id, resp.hovered(), anim::HOVER);
    let blurple = Color32::from_rgb(0x58, 0x65, 0xF2);
    let fill = theme::mix(blurple, Color32::from_rgb(0x47, 0x52, 0xC4), hot);
    ui.painter().rect_filled(rect, egui::Rounding::same(theme::R_CTRL), fill);
    let label = "Continue with Discord";
    let galley = ui.painter().layout_no_wrap(label.to_string(), theme::font_semi(theme::T_SM), Color32::WHITE);
    let total = 18.0 + 9.0 + galley.rect.width();
    let x = rect.center().x - total * 0.5;
    icons::draw(
        ui.painter(),
        Rect::from_center_size(Pos2::new(x + 9.0, rect.center().y), Vec2::splat(18.0)),
        Icon::Discord,
        Color32::WHITE,
    );
    ui.painter().galley(
        Pos2::new(x + 27.0, rect.center().y - galley.rect.height() * 0.5),
        galley,
        Color32::WHITE,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}
