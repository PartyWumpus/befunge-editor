use core::f32;
use std::fs;
use egui::StrokeKind;
use instant::Instant;
use std::future::Future;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use egui::{
    Color32, Frame, Pos2, Rect, Scene, Sense, Stroke, TextureHandle, Ui, Vec2, ahash::HashMap,
    pos2, vec2,
};

use crate::BefungeState;
use crate::befunge::Event;

#[derive(Default)]
enum Direction {
    North,
    South,
    #[default]
    East,
    West,
}

#[derive(Default, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct StoredState {}

#[derive(Default)]
pub struct CursorState {
    location: (i64, i64),
    direction: Direction,
    string_mode: bool,
}

/// We derive Deserialize/Serialize so we can persist app state on shutdown.
pub struct App {
    texture: TextureHandle,
    text_channel: (Sender<String>, Receiver<String>),
    time_since_step: Instant,
    paused: bool,
    speed: u8,

    bf_state: BefungeState,
    cursor_state: CursorState,
    scene_rect: Rect,
}

fn poss(pos: (i64, i64)) -> Rect {
    Rect::from_min_size(
        Pos2::new((pos.0 as f32) * 13.0, (pos.1 as f32) * 17.0),
        Vec2::new(12.0, 16.0),
    )
}

impl App {
    /// Called once before the first frame.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // This is also where you can customize the look and feel of egui using
        // `cc.egui_ctx.set_visuals` and `cc.egui_ctx.set_fonts`.

        // Load previous app state (if any).
        // Note that you must enable the `persistence` feature for this to work.
        //if let Some(storage) = cc.storage {
        //    return eframe::get_value(storage, eframe::APP_KEY).unwrap_or_default();
        //}

        /*
        use clap::Parser;
        #[derive(Parser, Debug)]
        #[command(about="", long_about = None)]
        struct Args {
            #[arg(short, long)]
            filename: Option<String>,
        }

        let args = Args::parse();
        let bf_state = if let Some(file) = args.filename {
            BefungeState::new_from_string(fs::read_to_string(file).unwrap())
        } else {
            BefungeState::new()
        }
        */

        Self {
            scene_rect: Rect::ZERO,
            text_channel: channel(),
            time_since_step: Instant::now(),
            cursor_state: CursorState::default(),
            paused: true,
            speed: 1,
            bf_state: BefungeState::new(),
            texture: cc.egui_ctx.load_texture(
                "noise",
                egui::ColorImage::example(),
                egui::TextureOptions::NEAREST,
            ),
        }
    }
}

impl eframe::App for App {
    /// Called by the frame work to save state before shutdown.
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        //eframe::set_value(storage, eframe::APP_KEY, self);
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // For inspiration and more examples, go to https://emilk.github.io/egui

        if let Ok(text) = self.text_channel.1.try_recv() {
            self.bf_state = BefungeState::new_from_string(text);
            self.cursor_state = CursorState::default();
        }

        if !self.paused {
            let elapsed = self.time_since_step.elapsed();
            let time_per_step = match self.speed {
                1 => Duration::from_millis((1000.0 / 1.0) as u64),
                2 => Duration::from_millis((1000.0 / 2.0) as u64),
                3 => Duration::from_millis((1000.0 / 4.0) as u64),
                4 => Duration::from_millis((1000.0 / 8.0) as u64),
                5 => Duration::from_millis((1000.0 / 16.0) as u64),
                5..=10 => Duration::from_millis((1000.0 / 32.0) as u64),
                _ => Duration::from_millis((1000.0 / 64.0) as u64),
            };
            if elapsed >= time_per_step {
                match self.speed {
                    ..6 => self.bf_state.step(),
                    6..=12 => {
                        for _ in 0..=self.speed {
                            self.bf_state.step();
                        }
                    }
                    13..=15 => {
                        for _ in 0..=2_usize.pow(self.speed as u32 - 8) {
                            self.bf_state.step();
                        }
                    }
                    16 => {
                        let now = Instant::now();
                        loop {
                            for _ in 0..=10000 {
                                self.bf_state.step();
                            }
                            if now.elapsed() > Duration::from_millis(30) {
                                break;
                            }
                        }
                    }
                    _ => unreachable!(),
                }

                self.time_since_step = Instant::now();
            }

            ctx.request_repaint_after(Duration::from_millis((1000.0 / 30.0) as u64));
        }

        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                let is_web = cfg!(target_arch = "wasm32");
                ui.menu_button("File", |ui| {
                    if !is_web && ui.button("Quit").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    if ui.button("New File").clicked() {
                        self.bf_state = BefungeState::new();
                        self.cursor_state = CursorState::default();
                    }
                    if ui.button("📂 Open text file").clicked() {
                        let sender = self.text_channel.0.clone();
                        let task = rfd::AsyncFileDialog::new().pick_file();
                        // Context is wrapped in an Arc so it's cheap to clone as per:
                        // > Context is cheap to clone, and any clones refers to the same mutable data (Context uses refcounting internally).
                        // Taken from https://docs.rs/egui/0.24.1/egui/struct.Context.html
                        let ctx = ui.ctx().clone();
                        execute(async move {
                            let file = task.await;
                            if let Some(file) = file {
                                let text = file.read().await;
                                let _ = sender.send(String::from_utf8_lossy(&text).to_string());
                                ctx.request_repaint();
                            }
                        });
                    }

                    if ui.button("💾 Save text to file").clicked() {
                        let task = rfd::AsyncFileDialog::new().save_file();
                        let contents = self.bf_state.map.serialize();
                        execute(async move {
                            let file = task.await;
                            if let Some(file) = file {
                                _ = file.write(contents.as_bytes()).await;
                            }
                        });
                    }

                    ui.menu_button("Presets", |ui| {
                        /*for key in PRESETS.keys() {
                            if ui.button(*key).clicked() {
                                match PRESETS.get(key) {
                                    None => unreachable!(),
                                    Some(data) => self.load(data),
                                }
                            }
                        }*/
                    });
                });

                ui.menu_button("Settings", |ui| {
                    //ui.checkbox(&mut self.extra, "extra info");
                });
                ui.add_space(8.0);

                egui::widgets::global_theme_preference_buttons(ui);
            });
        });

        egui::TopBottomPanel::bottom("bottom_panel").show(ctx, |ui| {
            // The top panel is often a good place for a menu bar:

            egui::menu::bar(ui, |ui| {
                powered_by_egui_and_eframe(ui);
                ui.add(egui::github_link_file!(
                    "https://github.com/PartyWumpus/befunge-editor/blob/main/",
                    "Source code."
                ));

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    egui::warn_if_debug_build(ui);
                });
                ui.rect_contains_pointer(ui.max_rect());
            });
        });

        egui::SidePanel::left("left_panel")
            .resizable(false)
            .show(ctx, |ui| {
                if let Some(graphics) = &mut self.bf_state.graphics {
                    ui.label("Graphics:");
                    self.texture.set(
                        egui::ColorImage {
                            size: [graphics.size.0, graphics.size.1],
                            pixels: graphics.texture.clone(),
                        },
                        egui::TextureOptions::NEAREST,
                    );

                    let size = self.texture.size_vec2() * 2.0;
                    let sized_texture = egui::load::SizedTexture::new(&self.texture, size);
                    let canvas = ui.add(egui::Image::new(sized_texture).fit_to_exact_size(size));
                    let canvas = canvas.interact(Sense::click());
                    if canvas.clicked()
                        && let Some(pos) = canvas.interact_pointer_pos()
                    {
                        let container = canvas.interact_rect;
                        let pos = pos.clamp(container.min, container.max);
                        let pos = pos2(
                            (pos.x - container.left()) / container.width(),
                            (pos.y - container.top()) / container.height(),
                        );
                        let pixel_pos = (
                            ((graphics.size.0 - 1) as f32 * pos.x).round() as i64,
                            ((graphics.size.1 - 1) as f32 * pos.y).round() as i64,
                        );
                        graphics
                            .event_queue
                            .push_back(Event::MouseClick(pixel_pos.0, pixel_pos.1));
                    }
                    ui.horizontal(|ui| {
                        ui.label("Color:");
                        let size = Vec2::splat(16.0);
                        let (response, painter) = ui.allocate_painter(size, Sense::hover());
                        let color = graphics.current_color;
                        let response = response.on_hover_text(format!(
                            "#{:02X}{:02X}{:02X}",
                            color.r(),
                            color.g(),
                            color.b()
                        ));
                        let rect = response.rect;
                        let c = rect.center();
                        let r = rect.width() / 2.0 - 1.0;
                        let color = Color32::from_gray(128);
                        let stroke = Stroke::new(1.0, color);
                        painter.circle(c, r, graphics.current_color, stroke);
                    });
                }

                ui.label("Stack:");
                for value in &self.bf_state.stack {
                    ui.label(value.to_string());
                }
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            // The central panel the region left after adding TopPanel's and SidePanel's
            ui.heading("befunge editor");

            ui.label("hiii!!!");

            ui.horizontal(|ui| {
                if ui.button("step").clicked() {
                    self.bf_state.step();
                }
                ui.checkbox(&mut self.paused, "paused");
            });

            ui.add(egui::Slider::new(&mut self.speed, 1..=16).text("value"));

            let reset_view = ui.button("Reset view").clicked();
            egui::Frame::group(ui.style())
                .inner_margin(0.0)
                .show(ui, |ui| {
                    ui.set_min_height(100.0);
                    let scene = Scene::new()
                        .max_inner_size([450.0, 1000.0])
                        .zoom_range(0.01..=5.0);

                    ui.input(|e| {
                        if let Some(direction) = if e.key_pressed(egui::Key::ArrowDown) {
                            Some(Direction::South)
                        } else if e.key_pressed(egui::Key::ArrowUp) {
                            Some(Direction::North)
                        } else if e.key_pressed(egui::Key::ArrowLeft) {
                            Some(Direction::West)
                        } else if e.key_pressed(egui::Key::ArrowRight) {
                            Some(Direction::East)
                        } else {
                            None
                        } {
                            self.cursor_state.direction = direction;
                            self.step_cursor();
                        };

                        for event in e.filtered_events(&egui::EventFilter {
                            tab: true,
                            escape: false,
                            horizontal_arrows: true,
                            vertical_arrows: true,
                        }) {
                            if let egui::Event::Text(text) | egui::Event::Paste(text) = event {
                                for char in text.chars() {
                                    self.bf_state
                                        .map
                                        .set(self.cursor_state.location, char as i64);

                                    if char == '"' {
                                        self.cursor_state.string_mode =
                                            !self.cursor_state.string_mode;
                                    };

                                    if !self.cursor_state.string_mode {
                                        match char {
                                            '>' => self.cursor_state.direction = Direction::East,
                                            'v' => self.cursor_state.direction = Direction::South,
                                            '<' => self.cursor_state.direction = Direction::West,
                                            '^' => self.cursor_state.direction = Direction::North,
                                            _ => (),
                                        }
                                    }

                                    self.step_cursor();
                                }
                            }
                        }
                    });

                    let mut inner_rect = Rect::NAN;
                    let response = scene
                        .show(ui, &mut self.scene_rect, |ui| {
                            self.bf_state
                                .pos_history
                                .retain(|_, v| v.elapsed() < Duration::from_millis(5000));

                            {
                                let painter = ui.painter();
                                painter.rect(
                                    poss(self.cursor_state.location),
                                    0.0,
                                    if self.cursor_state.string_mode {
                                        Color32::LIGHT_GREEN
                                    } else {
                                        Color32::LIGHT_BLUE
                                    },
                                    Stroke::NONE,
                                    StrokeKind::Outside,
                                );
                                painter.rect(
                                    poss(self.bf_state.position),
                                    0.0,
                                    Color32::PURPLE,
                                    Stroke::NONE,
                                    StrokeKind::Outside,
                                );
                                for (pos, instant) in &self.bf_state.pos_history {
                                    let rect = poss(*pos);
                                    let time = instant.elapsed().as_secs_f32() as f32;
                                    let mut mult = f32::log2(5.0 - time) - 1.322;
                                    if mult < 0.0 {
                                        mult = 0.0
                                    }

                                    painter.rect(
                                        rect,
                                        0.0,
                                        Color32::PURPLE.gamma_multiply(mult),
                                        Stroke::NONE,
                                        StrokeKind::Outside,
                                    );
                                }
                            }

                            for (pos, val) in self.bf_state.map.entries() {
                                let pos = poss(pos);

                                if let Ok(val) = TryInto::<u8>::try_into(val)
                                    && val <= b'~'
                                {
                                    if val < b' ' {
                                        ui.put(pos, |ui: &mut Ui| {
                                            Frame::default()
                                                .stroke(Stroke::new(0.5, Color32::GRAY))
                                                .show(ui, |ui| {
                                                    ui.add(
                                                        egui::Label::new(String::from(match val {
                                                            0..=9 => val + b'0',
                                                            10.. => val - 10 + b'A',
                                                        }
                                                            as char))
                                                        .selectable(false),
                                                    )
                                                })
                                                .response
                                        })
                                    } else {
                                        ui.put(
                                            pos,
                                            egui::Label::new(String::from(val as char))
                                                .selectable(false),
                                        )
                                    }
                                } else {
                                    ui.put(pos, |ui: &mut Ui| {
                                        Frame::default()
                                            .stroke(Stroke::new(0.5, Color32::GRAY))
                                            .show(ui, |ui| {
                                                ui.add(egui::Label::new("X").selectable(false))
                                            })
                                            .response
                                    })
                                }
                                .on_hover_text(val.to_string());
                            }

                            inner_rect = ui.min_rect();
                        })
                        .response;

                    if reset_view || response.double_clicked() {
                        self.scene_rect = inner_rect;
                    }
                });
        });
    }
}

impl App {
    fn step_cursor(&mut self) {
        let (x, y) = self.cursor_state.location;
        match self.cursor_state.direction {
            Direction::North => self.cursor_state.location = (x, y - 1),
            Direction::South => self.cursor_state.location = (x, y + 1),
            Direction::East => self.cursor_state.location = (x + 1, y),
            Direction::West => self.cursor_state.location = (x - 1, y),
        }
        if self.cursor_state.location.0 < 0 {
            self.cursor_state.location.0 = 0
        }
        if self.cursor_state.location.1 < 0 {
            self.cursor_state.location.1 = 0
        }
    }
}

fn powered_by_egui_and_eframe(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.label("Powered by ");
        ui.hyperlink_to("egui", "https://github.com/emilk/egui");
        ui.label(" and ");
        ui.hyperlink_to(
            "eframe",
            "https://github.com/emilk/egui/tree/master/crates/eframe",
        );
        ui.label(".");
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn execute<F: Future<Output = ()> + Send + 'static>(f: F) {
    // this is stupid... use any executor of your choice instead
    std::thread::spawn(move || futures::executor::block_on(f));
}

#[cfg(target_arch = "wasm32")]
fn execute<F: Future<Output = ()> + 'static>(f: F) {
    wasm_bindgen_futures::spawn_local(f);
}
