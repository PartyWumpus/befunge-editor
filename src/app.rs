use coarsetime::{Duration, Instant};
use egui::containers::menu::SubMenuButton;
use egui::scroll_area::ScrollBarVisibility;
use egui::style::ScrollStyle;
use egui::{FontId, Id, Mesh, Modal, RichText, ScrollArea, StrokeKind, TextStyle};
use include_dir::{Dir, include_dir};
use std::future::Future;
use std::ops::Range;
use std::sync::mpsc::{Receiver, Sender, channel};

use egui::{Color32, Frame, Pos2, Rect, Scene, Sense, Stroke, TextureHandle, Ui, Vec2, pos2};

use crate::befunge::{Event, FungeSpace, StepStatus, get_color_of_bf_op};
use crate::{BefungeState, befunge};

static PRESETS: Dir = include_dir!("./bf_programs");

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

impl CursorState {
    fn new(location: (i64, i64)) -> Self {
        Self {
            location,
            ..Default::default()
        }
    }
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
        error_state: Option<befunge::Error>,
    },
}

#[derive(serde::Deserialize, serde::Serialize, PartialEq)]
pub enum InvalidOperationBehaviour {
    Reflect,
    Halt,
    Ignore,
}

#[derive(serde::Deserialize, serde::Serialize)]
pub struct Settings {
    pub pos_history: (bool, [u8; 3]),
    pub get_history: (bool, [u8; 3]),
    pub put_history: (bool, [u8; 3]),
    pub skip_spaces: bool,
    pub render_unicode: bool,
    pub invalid_operation_behaviour: InvalidOperationBehaviour,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            pos_history: (true, [128, 0, 128]),
            get_history: (false, [255, 0, 0]),
            put_history: (true, [0, 255, 0]),
            skip_spaces: false,
            render_unicode: true,
            invalid_operation_behaviour: InvalidOperationBehaviour::Halt,
        }
    }
}

struct CharRenderer {
    glyph_uv_position: [Rect; 256],
    glyph_size: [Vec2; 256],
    glyph_offset: [Vec2; 256],
}

impl CharRenderer {
    // There is likely a better way to find these values
    fn new(ctx: &egui::Context) -> Self {
        puffin::profile_function!();
        let mut glyph_uv_position = vec![Rect::ZERO];
        let mut glyph_size = vec![Vec2::ZERO];
        let mut glyph_offset = vec![Vec2::ZERO];

        static DIVISOR: f32 = 2.0;

        ctx.fonts(|fonts| {
            let size = fonts.font_image_size();
            for val in 0u8..=255 {
                let galley = fonts.layout_no_wrap(
                    String::from(val as char),
                    egui::FontId::monospace(32.0),
                    egui::Color32::WHITE,
                );
                if let Some(row) = galley.rows.first() {
                    if let Some(g) = row.glyphs.first() {
                        glyph_uv_position.push(Rect::from_two_pos(
                            Pos2::new(
                                g.uv_rect.min[0] as f32 / size[0] as f32,
                                g.uv_rect.min[1] as f32 / size[1] as f32,
                            ),
                            Pos2::new(
                                g.uv_rect.max[0] as f32 / size[0] as f32,
                                g.uv_rect.max[1] as f32 / size[1] as f32,
                            ),
                        ));
                        glyph_size.push(g.uv_rect.size / DIVISOR);
                        glyph_offset.push(g.uv_rect.offset / DIVISOR);
                    }
                }
            }
        });

        Self {
            glyph_uv_position: glyph_uv_position.try_into().unwrap(),
            glyph_size: glyph_size.try_into().unwrap(),
            glyph_offset: glyph_offset.try_into().unwrap(),
        }
    }

    fn draw(&self, mesh: &mut Mesh, egui_pos: Rect, val: u8, color: Color32) {
        let uv_pos = self.glyph_uv_position[val as usize];
        let glyph_size = self.glyph_size[val as usize];
        let glyph_offset = self.glyph_offset[val as usize];

        static CENTERING_OFFSET: Vec2 = Vec2::new(1.5, -3.5); // eyeballed

        let egui_pos = Rect::from_min_size(
            egui_pos.left_bottom() + glyph_offset + CENTERING_OFFSET,
            glyph_size,
        );

        let idx = mesh.vertices.len() as u32;
        mesh.vertices.push(egui::epaint::Vertex {
            pos: egui_pos.left_top(),
            uv: uv_pos.left_top(),
            color,
        });
        mesh.vertices.push(egui::epaint::Vertex {
            pos: egui_pos.right_top(),
            uv: uv_pos.right_top(),
            color,
        });
        mesh.vertices.push(egui::epaint::Vertex {
            pos: egui_pos.right_bottom(),
            uv: uv_pos.right_bottom(),
            color,
        });
        mesh.vertices.push(egui::epaint::Vertex {
            pos: egui_pos.left_bottom(),
            uv: uv_pos.left_bottom(),
            color,
        });
        mesh.indices
            .extend_from_slice(&[idx, idx + 1, idx + 2, idx + 2, idx + 3, idx]);
    }
}

enum ModalState {
    Settings,
    SetPosition(i64, i64),
}

pub struct App {
    texture: TextureHandle,
    text_channel: (Sender<String>, Receiver<String>),
    settings: Settings,
    mode: Mode,
    scene_rect: Rect,
    open_modal: Option<ModalState>,
    scene_offset: (i64, i64),
    cursor_pos: (i64, i64),
}

fn poss(pos: (f32, f32)) -> Pos2 {
    Pos2::new((pos.0) * 13.0, (pos.1) * 17.0)
}

fn poss_reverse(pos: Pos2, offset: (i64, i64)) -> (i64, i64) {
    let x = pos.x / 13.0;
    let x = if x.is_sign_negative() { x - 1.0 } else { x };

    let y = pos.y / 17.0;
    let y = if y.is_sign_negative() { y - 1.0 } else { y };
    (x as i64 + offset.0, y as i64 + offset.1)
}

fn intersects(a: ((i64, i64), (i64, i64)), b: (i64, i64)) -> bool {
    a.0.0 <= b.0 && b.0 <= a.1.0 && a.0.1 <= b.1 && b.1 <= a.1.1
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
                error_state: None,
            },
            Mode::Playing {
                snapshot, bf_state, ..
            } => Mode::Editing {
                cursor_state: CursorState::new(bf_state.position),
                fungespace: snapshot,
            },
        };
    }

    fn step_befunge_inner(
        bf_state: &mut BefungeState,
        running: &mut bool,
        error_state: &mut Option<befunge::Error>,
        settings: &Settings,
    ) -> bool {
        let step_state = bf_state.step(settings);
        match step_state {
            StepStatus::Normal => false,
            StepStatus::Breakpoint => {
                *running = false;
                true
            }
            StepStatus::Error(error) => {
                use InvalidOperationBehaviour as IOpBehav;
                match settings.invalid_operation_behaviour {
                    IOpBehav::Reflect => {
                        bf_state.direction = bf_state.direction.reverse();
                        bf_state.step_position(settings);
                        false
                    }
                    IOpBehav::Halt => {
                        *error_state = Some(error);
                        true
                    }
                    IOpBehav::Ignore => {
                        bf_state.step_position(settings);
                        false
                    }
                }
            }
        }
    }

    fn step_befunge(&mut self, ctx: &egui::Context, settings: &Settings) {
        puffin::profile_function!();
        match self {
            Mode::Editing { .. } => (),
            Mode::Playing {
                time_since_step,
                speed,
                bf_state,
                running,
                error_state,
                ..
            } => {
                let elapsed = time_since_step.elapsed();
                let time_per_step = match speed {
                    0 => unreachable!(),
                    1 => elapsed >= Duration::from_millis((1000.0 / 1.0) as u64),
                    2 => elapsed >= Duration::from_millis((1000.0 / 2.0) as u64),
                    3 => elapsed >= Duration::from_millis((1000.0 / 4.0) as u64),
                    4 => elapsed >= Duration::from_millis((1000.0 / 8.0) as u64),
                    5 => elapsed >= Duration::from_millis((1000.0 / 16.0) as u64),
                    6..=19 => elapsed >= Duration::from_millis((1000.0 / 32.0) as u64),
                    _ => true,
                };
                if time_per_step {
                    bf_state.instruction_count = 0;
                    match speed {
                        ..6 => {
                            Self::step_befunge_inner(bf_state, running, error_state, settings);
                        }
                        6..=9 => {
                            for _ in 0..=*speed - 6 {
                                if Self::step_befunge_inner(
                                    bf_state,
                                    running,
                                    error_state,
                                    settings,
                                ) {
                                    return;
                                };
                            }
                        }
                        10..=15 => {
                            for _ in 0..=2_usize.pow(*speed as u32 - 8) {
                                if Self::step_befunge_inner(
                                    bf_state,
                                    running,
                                    error_state,
                                    settings,
                                ) {
                                    return;
                                };
                            }
                        }
                        16..=20 => {
                            let now = Instant::now();
                            loop {
                                for _ in 0..10000 {
                                    if Self::step_befunge_inner(
                                        bf_state,
                                        running,
                                        error_state,
                                        settings,
                                    ) {
                                        return;
                                    }
                                }
                                if now.elapsed()
                                    > Duration::from_millis(match *speed - 16 {
                                        0 => 4,
                                        1 => 8,
                                        2 => 16,
                                        _ => 32,
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
            scene_offset: (0, 0),
            cursor_pos: (0, 0),
            open_modal: None,
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

fn recter(pos: (i64, i64), offset: (i64, i64)) -> Rect {
    Rect::from_min_size(
        poss(((pos.0 - offset.0) as f32, (pos.1 - offset.1) as f32)),
        Vec2::new(13.0, 17.0),
    )
}

impl eframe::App for App {
    /// Called by the frame work to save state before shutdown.
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, &self.settings);
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        puffin::profile_function!();

        if !cfg!(target_arch = "wasm32") {
            puffin::GlobalProfiler::lock().new_frame();
            puffin_egui::show_viewport_if_enabled(ctx);
        }

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
                fungespace: FungeSpace::new_from_string(&text),
            }
        }

        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            self.menu_bar(ui, ctx);
        });

        egui::TopBottomPanel::bottom("bottom_panel").show(ctx, |ui| {
            puffin::profile_scope!("bottom panel");
            egui::MenuBar::new().ui(ui, |ui| {
                powered_by_egui_and_eframe(ui);
                ui.add(egui::github_link_file!(
                    "https://github.com/PartyWumpus/befunge-editor/blob/main/",
                    "Source code."
                ));

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    egui::warn_if_debug_build(ui);

                    ui.label(self.cursor_pos.0.to_string());
                    ui.label(self.cursor_pos.1.to_string());
                    if let Mode::Playing { bf_state, .. } = &self.mode {
                        ui.label(bf_state.instruction_count.to_string());
                    };
                });
            });
        });

        egui::SidePanel::left("left_panel")
            .resizable(false)
            .show(ctx, |ui| {
                self.info_panel(ui);
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            puffin::profile_scope!("central panel");
            ui.horizontal(|ui| {
                ui.heading("befunge editor");
                if let Mode::Playing {
                    error_state: Some(error),
                    ..
                } = &self.mode
                {
                    ui.label(RichText::new(error.to_string()).color(Color32::RED));
                };
            });

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
                error_state,
                ..
            } = &mut self.mode
            {
                ui.horizontal(|ui| {
                    if error_state.is_some() {
                        ui.disable();
                    }
                    ui.horizontal(|ui| {
                        if ui.button("step").clicked() {
                            Mode::step_befunge_inner(
                                bf_state,
                                running,
                                error_state,
                                &self.settings,
                            );
                        }
                        ui.checkbox(running, "play");
                        ui.checkbox(follow, "follow");
                    });

                    ui.horizontal(|ui| {
                        ui.add(egui::Slider::new(speed, 1..=20).text("speed"));
                    });
                });
            }

            egui::Frame::group(ui.style())
                .inner_margin(0.0)
                .show(ui, |ui| {
                    ui.set_min_height(100.0);

                    self.befunge_scene(ui);
                    if self.open_modal.is_none() {
                        self.befunge_input(ui);
                    }
                });
        });
    }
}

impl App {
    fn befunge_input(&mut self, ui: &mut egui::Ui) {
        puffin::profile_function!();
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
        puffin::profile_function!();
        let mut scene = Scene::new()
            .max_inner_size([f32::INFINITY, f32::INFINITY])
            .zoom_range(0.01..=5.0);

        if let Mode::Playing {
            follow: true,
            bf_state,
            ..
        } = &self.mode
        {
            self.scene_offset = bf_state.position;
            self.scene_rect.set_center(poss((0.5, 0.5)));
            // disables panning, TODO: disable scrolling
            scene = scene.sense(Sense::HOVER);
        } else {
            if self.scene_rect.left() >= 130.0 {
                *self.scene_rect.left_mut() -= 130.0;
                *self.scene_rect.right_mut() -= 130.0;
                self.scene_offset.0 += 10;
            };

            if self.scene_rect.left() <= -130.0 {
                *self.scene_rect.left_mut() += 130.0;
                *self.scene_rect.right_mut() += 130.0;
                self.scene_offset.0 -= 10;
            };

            if self.scene_rect.top() >= 170.0 {
                *self.scene_rect.top_mut() -= 170.0;
                *self.scene_rect.bottom_mut() -= 170.0;
                self.scene_offset.1 += 10;
            };

            if self.scene_rect.top() <= -170.0 {
                *self.scene_rect.top_mut() += 170.0;
                *self.scene_rect.bottom_mut() += 170.0;
                self.scene_offset.1 -= 10;
            };
        }

        let response = scene
            .show(ui, &mut self.scene_rect, |ui| {
                let painter = ui.painter();
                let clip_rect = painter.clip_rect();

                // Grid dots
                {
                    puffin::profile_scope!("grid dots");
                    if clip_rect.height() < 2500.0 {
                        let mut y = f32::max(
                            (clip_rect.top() / 17.0).round() * 17.0,
                            17.0 - (self.scene_offset.1 as f32 * 17.0),
                        );
                        loop {
                            let mut x = f32::max(
                                (clip_rect.left() / 13.0).round() * 13.0,
                                13.0 - (self.scene_offset.0 as f32 * 13.0),
                            );
                            loop {
                                painter.circle_filled(Pos2::new(x, y), 0.5, Color32::from_gray(90));
                                //painter.rect_filled(Rect::from_min_max(Pos2::new(x, y), Pos2::new(x+0.5, y+0.5)), 0.0, Color32::from_gray(90));
                                if x > f32::min(
                                    clip_rect.right(),
                                    (i64::MAX - i64::max(self.scene_offset.0, 0) - 1) as f32 * 13.0,
                                ) {
                                    break;
                                };
                                x += 13.0;
                            }
                            if y > f32::min(
                                clip_rect.bottom(),
                                (i64::MAX - i64::max(self.scene_offset.1, 0) - 1) as f32 * 17.0,
                            ) {
                                break;
                            };
                            y += 17.0;
                        }
                    }
                }

                // Border lines
                {
                    puffin::profile_scope!("border");
                    // Top line
                    painter.line_segment(
                        [
                            Pos2::new(
                                f32::max(
                                    clip_rect.left(),
                                    -1.0 - (self.scene_offset.0 as f32) * 13.0,
                                ),
                                -0.5 - (self.scene_offset.1 as f32) * 17.0,
                            ),
                            Pos2::new(
                                f32::min(
                                    clip_rect.right(),
                                    ((i64::MAX - i64::max(self.scene_offset.0, 0)) as f32 + 1.0)
                                        * 13.0,
                                ),
                                -0.5 - (self.scene_offset.1 as f32) * 17.0,
                            ),
                        ],
                        Stroke::new(1.0, Color32::from_gray(50)),
                    );

                    // Bottom line
                    painter.line_segment(
                        [
                            Pos2::new(
                                f32::max(
                                    clip_rect.left(),
                                    -1.0 - (self.scene_offset.0 as f32) * 13.0,
                                ),
                                0.5 - ((self.scene_offset.1 - i64::MAX - 1) as f32) * 17.0,
                            ),
                            Pos2::new(
                                f32::min(
                                    clip_rect.right(),
                                    ((i64::MAX - i64::max(self.scene_offset.0, 0)) as f32 + 1.0)
                                        * 13.0,
                                ),
                                0.5 - ((self.scene_offset.1 - i64::MAX - 1) as f32) * 17.0,
                            ),
                        ],
                        Stroke::new(1.0, Color32::from_gray(50)),
                    );

                    // Left line
                    painter.line_segment(
                        [
                            Pos2::new(
                                -0.5 - (self.scene_offset.0 as f32) * 13.0,
                                f32::max(
                                    clip_rect.top(),
                                    -1.0 - (self.scene_offset.1 as f32) * 17.0,
                                ),
                            ),
                            Pos2::new(
                                -0.5 - (self.scene_offset.0 as f32) * 13.0,
                                f32::min(
                                    clip_rect.bottom(),
                                    ((i64::MAX - i64::max(self.scene_offset.1, 0)) as f32 + 1.0)
                                        * 17.0,
                                ),
                            ),
                        ],
                        Stroke::new(1.0, Color32::from_gray(50)),
                    );

                    // Right line
                    painter.line_segment(
                        [
                            Pos2::new(
                                0.5 - ((self.scene_offset.0 - i64::MAX - 1) as f32) * 13.0,
                                f32::max(
                                    clip_rect.top(),
                                    -1.0 - (self.scene_offset.1 as f32) * 17.0,
                                ),
                            ),
                            Pos2::new(
                                0.5 - ((self.scene_offset.0 - i64::MAX - 1) as f32) * 13.0,
                                f32::min(
                                    clip_rect.bottom(),
                                    ((i64::MAX - i64::max(self.scene_offset.1, 0)) as f32 + 1.0)
                                        * 17.0,
                                ),
                            ),
                        ],
                        Stroke::new(1.0, Color32::from_gray(50)),
                    );
                }

                {
                    puffin::profile_scope!("history heatmap");
                    match &mut self.mode {
                        Mode::Playing { bf_state, .. } => {
                            // TODO: move this somewhere more sensible
                            bf_state
                                .pos_history
                                .retain(|_, v| v.elapsed() < Duration::from_millis(5000));

                            bf_state
                                .put_history
                                .retain(|_, v| v.elapsed() < Duration::from_millis(5000));

                            bf_state
                                .get_history
                                .retain(|_, v| v.elapsed() < Duration::from_millis(5000));

                            painter.rect(
                                recter(bf_state.position, self.scene_offset),
                                0.0,
                                Color32::PURPLE,
                                Stroke::NONE,
                                StrokeKind::Outside,
                            );
                            for (pos, instant) in &bf_state.pos_history {
                                let rect = recter(*pos, self.scene_offset);
                                let time = (instant.elapsed().as_millis() as f32) / 1000.0;
                                let mut mult = f32::log2(5.0 - time) - 1.322 - 0.3;
                                if mult < 0.0 {
                                    mult = 0.0
                                }

                                let [r, g, b] = self.settings.pos_history.1;
                                painter.rect(
                                    rect,
                                    0.0,
                                    Color32::from_rgb(r, g, b).gamma_multiply(mult),
                                    Stroke::NONE,
                                    StrokeKind::Outside,
                                );
                            }

                            for (pos, instant) in &bf_state.put_history {
                                let rect = recter(*pos, self.scene_offset);
                                let time = (instant.elapsed().as_millis() as f32) / 1000.0;
                                let mut mult = f32::log2(5.0 - time) - 1.322 - 0.5;
                                if mult < 0.0 {
                                    mult = 0.0
                                }

                                let [r, g, b] = self.settings.put_history.1;
                                painter.rect(
                                    rect,
                                    0.0,
                                    Color32::from_rgb(r, g, b).gamma_multiply(mult),
                                    Stroke::NONE,
                                    StrokeKind::Outside,
                                );
                            }

                            for (pos, instant) in &bf_state.get_history {
                                let rect = recter(*pos, self.scene_offset);
                                let time = (instant.elapsed().as_millis() as f32) / 1000.0;
                                let mut mult = f32::log2(5.0 - time) - 1.322 - 0.5;
                                if mult < 0.0 {
                                    mult = 0.0
                                }

                                let [r, g, b] = self.settings.get_history.1;
                                painter.rect(
                                    rect,
                                    0.0,
                                    Color32::from_rgb(r, g, b).gamma_multiply(mult),
                                    Stroke::NONE,
                                    StrokeKind::Outside,
                                );
                            }

                            for pos in &bf_state.breakpoints {
                                let rect = recter(*pos, self.scene_offset);

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
                                recter(cursor_state.location, self.scene_offset),
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
                }

                static PROFILE_EACH_CHAR: bool = false;

                {
                    puffin::profile_scope!("chars");

                    let mut mesh = egui::Mesh::with_texture(egui::TextureId::default());
                    // TODO: cache this and only remake when atlas updates (which it prolly doesn't)
                    let char_renderer = CharRenderer::new(ui.ctx());

                    let map = match &mut self.mode {
                        Mode::Playing { bf_state, .. } => &mut bf_state.map,
                        Mode::Editing { fungespace, .. } => fungespace,
                    };

                    let integer_clip_rect = (
                        poss_reverse(clip_rect.left_top(), self.scene_offset),
                        poss_reverse(clip_rect.right_bottom(), self.scene_offset),
                    );

                    for (pos, val) in map.entries() {
                        if !intersects(integer_clip_rect, pos) || val == ' ' as i64 {
                            continue;
                        }

                        let pos = recter(pos, self.scene_offset);

                        //puffin::profile_scope!("char");

                        if let Ok(val) = TryInto::<u8>::try_into(val) {
                            if val < b' ' {
                                puffin::profile_scope_if!(PROFILE_EACH_CHAR, "char boxed");
                                ui.place(pos, |ui: &mut Ui| {
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
                                });
                            } else if let Some(color) = get_color_of_bf_op(val) {
                                puffin::profile_scope_if!(PROFILE_EACH_CHAR, "char colored");
                                char_renderer.draw(&mut mesh, pos, val, color);
                            } else {
                                puffin::profile_scope_if!(PROFILE_EACH_CHAR, "char simple");
                                char_renderer.draw(&mut mesh, pos, val, Color32::GRAY);
                            }
                        } else if self.settings.render_unicode
                            && let Ok(val) = val.try_into()
                            && let Some(val) = char::from_u32(val)
                            && ui.fonts(|fonts| fonts.has_glyph(&egui::FontId::monospace(1.0), val))
                        {
                            puffin::profile_scope_if!(PROFILE_EACH_CHAR, "char unicode");
                            ui.place(pos, egui::Label::new(String::from(val)).selectable(false));
                        } else {
                            puffin::profile_scope_if!(PROFILE_EACH_CHAR, "char unknown");
                            ui.place(pos, |ui: &mut Ui| {
                                // this is not great
                                // i'm not really sure what the best way to do this would be
                                let str = format!("{val:X}");
                                let n = str.len();
                                let font_size = match n {
                                    ..=1 => 16.0,
                                    2 => 8.0,
                                    3..=6 => 6.0,
                                    7..16 => 4.0,
                                    16.. => 3.2,
                                };

                                let ranges: &[Range<usize>] = match n {
                                    0..4 => &[0..n],
                                    4 => &[0..2, 2..n],
                                    5 | 6 => &[0..3, 3..n],
                                    7 | 8 | 9 => &[0..3, 3..6, 6..n],
                                    10 => &[0..4, 4..7, 7..n],
                                    11 | 12 => &[0..4, 4..8, 8..n],
                                    13 => &[0..5, 5..9, 9..n],
                                    14 | 15 => &[0..5, 5..10, 10..n],
                                    16.. => &[0..4, 4..8, 8..12, 12..n],
                                };

                                let str = ranges
                                    .iter()
                                    .map(|range| &str[range.clone()])
                                    .collect::<Vec<_>>()
                                    .join("\n");

                                Frame::default()
                                    .stroke(Stroke::new(0.5, Color32::GRAY))
                                    .show(ui, |ui| {
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(str)
                                                    .font(egui::FontId::monospace(font_size)),
                                            )
                                            .selectable(false),
                                        )
                                    })
                                    .response
                            });
                        };
                    }
                    ui.painter().add(egui::Shape::Mesh(mesh.into()));
                }
            })
            .response;

        if response.contains_pointer()
            && let Some(pos) = response.hover_pos()
        {
            let (x, y) = poss_reverse(pos, self.scene_offset);
            if x >= 0 && y >= 0 {
                self.cursor_pos = (x, y)
            }
        };

        if response.clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            let pos = poss_reverse(pos, self.scene_offset);
            match &mut self.mode {
                Mode::Playing { bf_state, .. } => {
                    if pos.0 >= 0 && pos.1 >= 0 {
                        if bf_state.breakpoints.contains(&pos) {
                            bf_state.breakpoints.remove(&pos)
                        } else {
                            bf_state.breakpoints.insert(pos)
                        };
                    }
                }
                Mode::Editing { cursor_state, .. } => {
                    if pos.1 >= 0 && pos.0 >= 0 {
                        cursor_state.location = pos;
                    }
                }
            }
        };

        if let Mode::Playing { .. } = self.mode
            && response.secondary_clicked()
            && let Some(_pos) = response.interact_pointer_pos()
        {
            // TODO, a "run through to click" feature?
        };
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        puffin::profile_function!();
        egui::MenuBar::new().ui(ui, |ui| {
            let is_web = cfg!(target_arch = "wasm32");
            ui.menu_button("File", |ui| {
                if ui.button("📄 New").clicked() {
                    self.mode = Mode::Editing {
                        cursor_state: CursorState::default(),
                        fungespace: FungeSpace::new(),
                    }
                }
                if ui.button("📂 Open").clicked() {
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

                if ui.button("💾 Save").clicked() {
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

                ui.menu_button("👕 Load Preset", |ui| {
                    for file in PRESETS.files() {
                        if ui
                            .button(file.path().file_stem().unwrap().to_string_lossy())
                            .clicked()
                        {
                            self.mode = Mode::Editing {
                                cursor_state: CursorState::default(),
                                fungespace: FungeSpace::new_from_string(
                                    file.contents_utf8().unwrap(),
                                ),
                            }
                        }
                    }
                });

                if !is_web {
                    ui.separator();
                    if ui.add(egui::Button::new("Quit").right_text("❌")).clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
            });

            if self.open_modal.is_some() {
                let modal = Modal::new(Id::new("Settings modal")).show(ui.ctx(), |ui| {
                    ui.set_width(300.0);

                    match self.open_modal.as_mut().unwrap() {
                        ModalState::Settings => self.settings_modal(ui),
                        ModalState::SetPosition(x, y) => Self::set_position_modal(ui, x, y),
                    }

                    ui.add_space(32.0);

                    egui::Sides::new().show(
                        ui,
                        |_ui| {},
                        |ui| {
                            if ui.button("Close").clicked() {
                                ui.close();
                            }
                        },
                    );
                });

                if modal.should_close() {
                    let prev_modal = self.open_modal.take();
                    match prev_modal.unwrap() {
                        ModalState::Settings => (),
                        ModalState::SetPosition(x, y) => {
                            self.scene_offset = (x, y);
                            self.scene_rect.set_center(poss((0.5, 0.5)));
                        }
                    }
                }
            }

            ui.menu_button("Settings", |ui| {
                ui.checkbox(&mut self.settings.pos_history.0, "Track position history");
                ui.checkbox(&mut self.settings.skip_spaces, "Skip spaces");

                ui.menu_button("Invalid operation behaviour", |ui| {
                    ui.radio_value(
                        &mut self.settings.invalid_operation_behaviour,
                        InvalidOperationBehaviour::Halt,
                        "Halt",
                    );
                    ui.radio_value(
                        &mut self.settings.invalid_operation_behaviour,
                        InvalidOperationBehaviour::Reflect,
                        "Reflect",
                    );
                    ui.radio_value(
                        &mut self.settings.invalid_operation_behaviour,
                        InvalidOperationBehaviour::Ignore,
                        "Ignore",
                    );
                });
                ui.checkbox(
                    &mut self.settings.render_unicode,
                    "Display non-ascii characters",
                );

                if !is_web {
                    let mut profile = puffin::are_scopes_on();
                    ui.checkbox(&mut profile, "Enable UI profiling");
                    puffin::set_scopes_on(profile);
                }

                let settings_button =
                    egui::Button::new("Advanced settings").right_text(SubMenuButton::RIGHT_ARROW);

                if ui.add(settings_button).clicked() {
                    self.open_modal = Some(ModalState::Settings);
                };
            });

            ui.menu_button("Tools", |ui| {
                if ui.button("Set viewport position").clicked() {
                    self.open_modal = Some(ModalState::SetPosition(0, 0));
                };
            });
            ui.add_space(8.0);

            egui::widgets::global_theme_preference_buttons(ui);
        });
    }

    fn info_panel(&mut self, ui: &mut egui::Ui) {
        puffin::profile_function!();
        if let Mode::Playing {
            bf_state, running, ..
        } = &mut self.mode
        {
            if let Some(graphics) = &mut bf_state.graphics {
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

                egui::Window::new("Graphics")
                    .min_size((1.0, 1.0))
                    .show(ui.ctx(), |ui| {
                        self.texture.set(
                            egui::ColorImage {
                                size: [graphics.size.0, graphics.size.1],
                                source_size: Vec2::new(
                                    graphics.size.0 as f32,
                                    graphics.size.1 as f32,
                                ),
                                pixels: graphics.texture.clone(),
                            },
                            egui::TextureOptions::NEAREST,
                        );

                        let ppp = ui.ctx().pixels_per_point();
                        let tex_size = self.texture.size_vec2();
                        let available = ui.available_size();

                        let scale = ((available * ppp) / tex_size).floor().min_elem().max(1.0);
                        let display_size = tex_size * scale / ppp;

                        let (rect, canvas) =
                            ui.allocate_exact_size(display_size, egui::Sense::click());

                        let snapped_min = (rect.min * ppp).round() / ppp;
                        let snapped_rect = egui::Rect::from_min_size(snapped_min, display_size);

                        let sized_texture =
                            egui::load::SizedTexture::new(&self.texture, display_size);
                        ui.painter().image(
                            sized_texture.id,
                            snapped_rect,
                            Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0)),
                            Color32::WHITE,
                        );
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
                    });
            };

            ui.label("Stack:");
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
                let resp = ui.text_edit_singleline(&mut bf_state.input_buffer);
                if resp.changed() {
                    *running = true
                }
                ui.label("Input:");

                ui.add_space(2.0);
                ui.label(&bf_state.output);
                ui.label("Output:");
                ui.add_space(2.0);

                ui.vertical(|ui| {
                    let text_style = TextStyle::Body;

                    ui.style_mut().spacing.scroll = ScrollStyle {
                        floating: true,
                        bar_width: 8.0,
                        floating_width: 8.0,
                        floating_allocated_width: 6.0,
                        foreground_color: false,

                        dormant_background_opacity: 0.4,
                        dormant_handle_opacity: 0.4,

                        active_background_opacity: 0.6,
                        active_handle_opacity: 0.6,

                        interact_background_opacity: 0.8,
                        interact_handle_opacity: 0.8,
                        ..ScrollStyle::solid()
                    };
                    ScrollArea::vertical()
                        .auto_shrink([false; 2])
                        .scroll_bar_visibility(ScrollBarVisibility::AlwaysVisible)
                        .show_rows(
                            ui,
                            ui.text_style_height(&text_style),
                            bf_state.stack.len(),
                            |ui, row_range| {
                                let painter = ui.painter();
                                painter.rect_filled(
                                    ui.clip_rect(),
                                    5.0,
                                    ui.visuals().faint_bg_color,
                                );
                                for value in row_range {
                                    ui.label(bf_state.stack[value].to_string());
                                }
                            },
                        );
                });
            });
        }
    }

    fn settings_modal(&mut self, ui: &mut egui::Ui) {
        ui.heading("Advanced settings");
        ui.separator();
        ui.label(RichText::new("Track position history").font(FontId::proportional(14.0)));
        ui.horizontal(|ui| {
            ui.color_edit_button_srgb(&mut self.settings.pos_history.1);
            ui.label("Color");
        });
        ui.horizontal(|ui| ui.checkbox(&mut self.settings.pos_history.0, "Enabled"));

        ui.separator();
        ui.label(RichText::new("Track put history").font(FontId::proportional(14.0)));
        ui.horizontal(|ui| {
            ui.color_edit_button_srgb(&mut self.settings.put_history.1);
            ui.label("Color");
        });
        ui.checkbox(&mut self.settings.put_history.0, "Enabled");

        ui.separator();
        ui.label(RichText::new("Track get history").font(FontId::proportional(14.0)));
        ui.horizontal(|ui| {
            ui.color_edit_button_srgb(&mut self.settings.get_history.1);
            ui.label("Color");
        });
        ui.horizontal(|ui| ui.checkbox(&mut self.settings.get_history.0, "Enabled"));

        ui.separator();
        if ui.button("Reset all settings").clicked() {
            self.settings = Settings::default();
        };
    }

    fn set_position_modal(ui: &mut egui::Ui, x: &mut i64, y: &mut i64) {
        ui.heading("Set position");
        ui.add(egui::DragValue::new(x).speed(0.1));
        ui.add(egui::DragValue::new(y).speed(0.1));
    }
}

fn powered_by_egui_and_eframe(ui: &mut egui::Ui) {
    puffin::profile_function!();
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
