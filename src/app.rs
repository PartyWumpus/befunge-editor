use coarsetime::{Duration, Instant};
use core::f32;
use egui::StrokeKind;
use std::future::Future;
use std::sync::mpsc::{Receiver, Sender, channel};

use egui::{Color32, Frame, Pos2, Rect, Scene, Sense, Stroke, TextureHandle, Ui, Vec2, pos2};

use crate::BefungeState;
use crate::befunge::{Event, FungeSpace, Graphics, get_color_of_bf_op};
#[derive(Default, Clone)]
enum Direction {
    North,
    South,
    #[default]
    East,
    West,
}

#[derive(Default, Clone)]
pub struct CursorState {
    location: (i64, i64),
    direction: Direction,
    string_mode: bool,
}

#[derive(Clone)]
enum Mode {
    Editing {
        cursor_state: CursorState,
        fungespace: FungeSpace,
    },
    Playing {
        snapshot: FungeSpace,
        time_since_step: Instant,
        bf_state: Box<BefungeState>,
        running: bool,
        follow: bool,
        speed: u8,
    },
}

#[derive(serde::Deserialize, serde::Serialize)]
pub struct Settings {
    pub put_history: bool,
    pub pos_history: bool,
    pub skip_spaces: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            pos_history: true,
            put_history: true,
            skip_spaces: false,
        }
    }
}

pub struct App {
    texture: TextureHandle,
    text_channel: (Sender<String>, Receiver<String>),
    settings: Settings,
    mode: Mode,
    scene_rect: Rect,
}

fn poss(pos: (f32, f32)) -> Pos2 {
    Pos2::new((pos.0) * 13.0, (pos.1) * 17.0)
}

fn poss_reverse(pos: Pos2) -> (i64, i64) {
    ((pos.x / 13.0) as i64, (pos.y / 17.0) as i64)
}

fn recter(pos: (i64, i64)) -> Rect {
    Rect::from_min_size(poss((pos.0 as f32, pos.1 as f32)), Vec2::new(13.0, 17.0))
}

impl CursorState {
    fn step(&mut self) {
        let (x, y) = self.location;
        match self.direction {
            Direction::North => self.location = (x, y - 1),
            Direction::South => self.location = (x, y + 1),
            Direction::East => self.location = (x + 1, y),
            Direction::West => self.location = (x - 1, y),
        }
        if self.location.0 < 0 {
            self.location.0 = 0
        }
        if self.location.1 < 0 {
            self.location.1 = 0
        }
    }

    fn step_cursor_back(&mut self) {
        let (x, y) = self.location;
        match self.direction {
            Direction::North => self.location = (x, y + 1),
            Direction::South => self.location = (x, y - 1),
            Direction::East => self.location = (x - 1, y),
            Direction::West => self.location = (x + 1, y),
        }
        if self.location.0 < 0 {
            self.location.0 = 0
        }
        if self.location.1 < 0 {
            self.location.1 = 0
        }
    }
}

impl Mode {
    fn swap_mode(&mut self) {
        *self = match self.clone() {
            Mode::Editing { fungespace, .. } => Mode::Playing {
                snapshot: fungespace.clone(),
                time_since_step: Instant::now(),
                bf_state: Box::new(BefungeState::new_from_fungespace(fungespace)),
                running: false,
                follow: false,
                speed: 5,
            },
            Mode::Playing { snapshot, .. } => Mode::Editing {
                cursor_state: CursorState::default(),
                fungespace: snapshot,
            },
        };
    }

    fn step_befunge_inner(
        bf_state: &mut BefungeState,
        running: &mut bool,
        settings: &Settings,
    ) -> bool {
        let breakpoint_reached = bf_state.step(settings);
        if breakpoint_reached {
            *running = false;
        };
        breakpoint_reached
    }

    fn step_befunge(&mut self, ctx: &egui::Context, settings: &Settings) {
        match self {
            Mode::Editing { .. } => (),
            Mode::Playing {
                time_since_step,
                speed,
                bf_state,
                running,
                ..
            } => {
                let elapsed = time_since_step.elapsed();
                let time_per_step = match speed {
                    0 => unreachable!(),
                    1 => Duration::from_millis((1000.0 / 1.0) as u64),
                    2 => Duration::from_millis((1000.0 / 2.0) as u64),
                    3 => Duration::from_millis((1000.0 / 4.0) as u64),
                    4 => Duration::from_millis((1000.0 / 8.0) as u64),
                    5 => Duration::from_millis((1000.0 / 16.0) as u64),
                    _ => Duration::from_millis((1000.0 / 32.0) as u64),
                };
                if elapsed >= time_per_step {
                    match speed {
                        ..6 => {
                            Self::step_befunge_inner(bf_state, running, settings);
                        }
                        6..=9 => {
                            for _ in 0..=*speed - 6 {
                                if Self::step_befunge_inner(bf_state, running, settings) {
                                    return;
                                };
                            }
                        }
                        10..=15 => {
                            for _ in 0..=2_usize.pow(*speed as u32 - 8) {
                                if Self::step_befunge_inner(bf_state, running, settings) {
                                    return;
                                };
                            }
                        }
                        16..=19 => {
                            let now = Instant::now();
                            loop {
                                for _ in 0..=10000 {
                                    if Self::step_befunge_inner(bf_state, running, settings) {
                                        return;
                                    }
                                }
                                if now.elapsed()
                                    > Duration::from_millis(match *speed - 16 {
                                        0 => 4,
                                        1 => 8,
                                        2 => 16,
                                        3 => 32,
                                        _ => unreachable!(),
                                    })
                                {
                                    break;
                                }
                            }
                        }
                        _ => unreachable!(),
                    }

                    *time_since_step = Instant::now();
                }

                ctx.request_repaint_after(std::time::Duration::from_millis((1000.0 / 30.0) as u64));
            }
        }
    }
}

impl App {
    /// Called once before the first frame.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // This is also where you can customize the look and feel of egui using
        // `cc.egui_ctx.set_visuals` and `cc.egui_ctx.set_fonts`.

        // Load previous app state (if any).
        // Note that you must enable the `persistence` feature for this to work.
        let settings = if let Some(storage) = cc.storage {
            eframe::get_value(storage, eframe::APP_KEY).unwrap_or_default()
        } else {
            Settings::default()
        };

        Self {
            scene_rect: Rect::ZERO,
            text_channel: channel(),
            settings,
            mode: Mode::Editing {
                cursor_state: CursorState::default(),
                fungespace: FungeSpace::new(),
            },
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
        eframe::set_value(storage, eframe::APP_KEY, &self.settings);
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let dur = std::time::Duration::from_millis(5000);
        if let Mode::Playing {
            ref mut time_since_step,
            running,
            ..
        } = self.mode
        {
            if time_since_step.elapsed() > dur.into() {
                ctx.request_repaint_after(dur);
                *time_since_step = Instant::now();
            }

            if running {
                self.mode.step_befunge(ctx, &self.settings);
            }
        }

        if let Ok(text) = self.text_channel.1.try_recv() {
            self.mode = Mode::Editing {
                cursor_state: CursorState::default(),
                fungespace: FungeSpace::new_from_string(text),
            }
        }

        Instant::update();

        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            self.menu_bar(ui, ctx);
        });

        egui::TopBottomPanel::bottom("bottom_panel").show(ctx, |ui| {
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
                self.info_panel(ui);
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("befunge editor");

            let text = match self.mode {
                Mode::Editing { .. } => "To Interpreter Mode",
                Mode::Playing { .. } => "To Editor Mode",
            };
            if ui.button(text).clicked() {
                self.mode.swap_mode();
            }

            if let Mode::Playing {
                bf_state,
                running,
                follow,
                speed,
                ..
            } = &mut self.mode
            {
                ui.horizontal(|ui| {
                    if ui.button("step").clicked() {
                        bf_state.step(&self.settings);
                    }
                    ui.checkbox(running, "play");
                    ui.checkbox(follow, "follow");
                });

                ui.add(egui::Slider::new(speed, 1..=19).text("value"));
            }

            egui::Frame::group(ui.style())
                .inner_margin(0.0)
                .show(ui, |ui| {
                    ui.set_min_height(100.0);

                    self.befunge_scene(ui);
                    self.befunge_input(ui);
                });
        });
    }
}

impl App {
    fn befunge_input(&mut self, ui: &mut egui::Ui) {
        if let Mode::Editing {
            cursor_state,
            fungespace,
        } = &mut self.mode
        {
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
                    cursor_state.direction = direction;
                    cursor_state.step();
                };

                if e.key_pressed(egui::Key::Backspace) {
                    cursor_state.step_cursor_back();
                }

                for event in e.filtered_events(&egui::EventFilter {
                    tab: true,
                    escape: false,
                    horizontal_arrows: true,
                    vertical_arrows: true,
                }) {
                    match event {
                        egui::Event::Text(text) => {
                            for char in text.chars() {
                                fungespace.set(cursor_state.location, char as i64);

                                if char == '"' {
                                    cursor_state.string_mode = !cursor_state.string_mode;
                                };

                                if !cursor_state.string_mode {
                                    match char {
                                        '>' => cursor_state.direction = Direction::East,
                                        'v' => cursor_state.direction = Direction::South,
                                        '<' => cursor_state.direction = Direction::West,
                                        '^' => cursor_state.direction = Direction::North,
                                        _ => (),
                                    }
                                }

                                cursor_state.step();
                            }
                        }
                        egui::Event::Paste(text) => {
                            let (mut x, mut y) = cursor_state.location;
                            for char in text.chars() {
                                if char == '\n' {
                                    y += 1;
                                    x = cursor_state.location.0;
                                    continue;
                                };
                                fungespace.set((x, y), char as i64);
                                x += 1
                            }
                        }
                        _ => (),
                    }
                }
            });
        }
    }

    fn befunge_scene(&mut self, ui: &mut egui::Ui) {
        let scene = Scene::new()
            .max_inner_size([f32::INFINITY, f32::INFINITY])
            .zoom_range(0.01..=5.0);

        if let Mode::Playing {
            follow: true,
            bf_state,
            ..
        } = &self.mode
        {
            self.scene_rect.set_center(poss((
                bf_state.position.0 as f32 + 0.5,
                bf_state.position.1 as f32 + 0.5,
            )));
        }

        let response = scene
            .show(ui, &mut self.scene_rect, |ui| {
                let painter = ui.painter();
                let clip_rect = painter.clip_rect();

                // Grid dots
                let mut y = f32::max((clip_rect.top() / 17.0).round() * 17.0, 17.0);
                loop {
                    let mut x = f32::max((clip_rect.left() / 13.0).round() * 13.0, 13.0);
                    loop {
                        painter.circle_filled(Pos2::new(x, y), 0.5, Color32::from_gray(90));
                        if x > clip_rect.right() {
                            break;
                        };
                        x += 13.0;
                    }
                    if y > clip_rect.bottom() {
                        break;
                    };
                    y += 17.0;
                }

                // Border lines
                painter.line_segment(
                    [
                        Pos2::new(f32::max(clip_rect.left(), -0.5), -0.5),
                        Pos2::new(clip_rect.right(), -0.5),
                    ],
                    Stroke::new(1.0, Color32::from_gray(50)),
                );

                painter.line_segment(
                    [
                        Pos2::new(-0.5, f32::max(clip_rect.top(), -0.5)),
                        Pos2::new(-0.5, clip_rect.bottom()),
                    ],
                    Stroke::new(1.0, Color32::from_gray(50)),
                );

                match &mut self.mode {
                    Mode::Playing { bf_state, .. } => {
                        // TODO: move this somewhere more sensible
                        bf_state
                            .pos_history
                            .retain(|_, v| v.elapsed() < Duration::from_millis(5000));

                        bf_state
                            .put_history
                            .retain(|_, v| v.elapsed() < Duration::from_millis(5000));

                        painter.rect(
                            recter(bf_state.position),
                            0.0,
                            Color32::PURPLE,
                            Stroke::NONE,
                            StrokeKind::Outside,
                        );
                        for (pos, instant) in &bf_state.pos_history {
                            let rect = recter(*pos);
                            let time = (instant.elapsed().as_millis() as f32) / 1000.0;
                            let mut mult = f32::log2(5.0 - time) - 1.322 - 0.3;
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

                        for (pos, instant) in &bf_state.put_history {
                            let rect = recter(*pos);
                            let time = (instant.elapsed().as_millis() as f32) / 1000.0;
                            let mut mult = f32::log2(5.0 - time) - 1.322 - 0.5;
                            if mult < 0.0 {
                                mult = 0.0
                            }

                            painter.rect(
                                rect,
                                0.0,
                                Color32::GREEN.gamma_multiply(mult),
                                Stroke::NONE,
                                StrokeKind::Outside,
                            );
                        }

                        for pos in &bf_state.breakpoints {
                            let rect = recter(*pos);

                            painter.rect(
                                rect,
                                0.0,
                                Color32::TRANSPARENT,
                                Stroke::new(2.0, Color32::GREEN),
                                StrokeKind::Inside,
                            );
                        }
                    }
                    Mode::Editing { cursor_state, .. } => {
                        painter.rect(
                            recter(cursor_state.location),
                            0.0,
                            if cursor_state.string_mode {
                                Color32::LIGHT_GREEN
                            } else {
                                Color32::LIGHT_BLUE
                            },
                            Stroke::new(0.25, Color32::from_gray(90)),
                            StrokeKind::Inside,
                        );
                    }
                };

                let map = match &mut self.mode {
                    Mode::Playing { bf_state, .. } => &mut bf_state.map,
                    Mode::Editing { fungespace, .. } => fungespace,
                };
                for (pos, val) in map.entries() {
                    let pos = recter(pos);

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
                        } else if let Some(color) = get_color_of_bf_op(val) {
                            ui.put(
                                pos,
                                egui::Label::new(egui::RichText::new(val as char).color(color))
                                    .selectable(false),
                            )
                        } else {
                            ui.put(
                                pos,
                                egui::Label::new(String::from(val as char)).selectable(false),
                            )
                        }
                    } else {
                        ui.put(pos, |ui: &mut Ui| {
                            Frame::default()
                                .stroke(Stroke::new(0.5, Color32::GRAY))
                                .show(ui, |ui| ui.add(egui::Label::new("X").selectable(false)))
                                .response
                        })
                    }
                    .on_hover_text(val.to_string());
                }
            })
            .response;

        if response.clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            match &mut self.mode {
                Mode::Playing { bf_state, .. } => {
                    if pos.x > 0.0 && pos.y > 0.0 {
                        let pos = poss_reverse(pos);
                        if bf_state.breakpoints.contains(&pos) {
                            bf_state.breakpoints.remove(&pos)
                        } else {
                            bf_state.breakpoints.insert(pos)
                        };
                    }
                }
                Mode::Editing { cursor_state, .. } => {
                    if pos.x > 0.0 && pos.y > 0.0 {
                        cursor_state.location = poss_reverse(pos);
                    }
                }
            }
        };

        if let Mode::Playing { .. } = self.mode
            && response.secondary_clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            // TODO, a "run through to click" feature
        };
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::menu::bar(ui, |ui| {
            let is_web = cfg!(target_arch = "wasm32");
            ui.menu_button("File", |ui| {
                if ui.button("New File").clicked() {
                    self.mode = Mode::Editing {
                        cursor_state: CursorState::default(),
                        fungespace: FungeSpace::new(),
                    }
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
                    let contents = match &mut self.mode {
                        Mode::Playing { bf_state, .. } => bf_state.map.serialize(),
                        Mode::Editing { fungespace, .. } => fungespace.serialize(),
                    };

                    execute(async move {
                        let file = task.await;
                        if let Some(file) = file {
                            _ = file.write(contents.as_bytes()).await;
                        }
                    });
                }

                ui.separator();

                if !is_web && ui.button("Quit").clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
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
                ui.checkbox(&mut self.settings.put_history, "Track puts");
                ui.checkbox(&mut self.settings.pos_history, "Track position history");
                ui.checkbox(&mut self.settings.skip_spaces, "Skip spaces");
            });
            ui.add_space(8.0);

            egui::widgets::global_theme_preference_buttons(ui);
        });
    }

    fn info_panel(&mut self, ui: &mut egui::Ui) {
        if let Mode::Playing { bf_state, .. } = &mut self.mode {
            if let Some(graphics) = &mut bf_state.graphics {
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
            egui::Frame::new()
                .fill(ui.visuals().faint_bg_color)
                .show(ui, |ui| {
                    for value in &bf_state.stack {
                        ui.label(value.to_string());
                    }
                });

            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                ui.add_space(2.0);
                // TODO: as part of making this use a vecdeque<char>
                // make it unpause if the cursor is currently on a `~`
                // (not `&`, as that can be multichar)
                // and the text was empty before
                // teechnically this logic would be wrong if a user
                // paused ontop of a `~` while there was input
                // and then deleted the input, and then started typing
                // but like ¯\_(ツ)_/¯ there aren't any users
                ui.text_edit_singleline(&mut bf_state.input_buffer);
                ui.label("Input:");

                ui.add_space(2.0);
                ui.label(&bf_state.output);
                ui.label("Output:");
            });
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
