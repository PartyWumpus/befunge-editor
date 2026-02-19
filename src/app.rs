use coarsetime::{Duration, Instant};
use egui::containers::menu::SubMenuButton;
use egui::emath::TSTransform;
use egui::scroll_area::ScrollBarVisibility;
use egui::style::ScrollStyle;
use egui::{
    FontId, Id, Label, LayerId, Mesh, Modal, RichText, ScrollArea, Shape, StrokeKind, TextStyle,
};
use include_dir::{Dir, include_dir};
use std::future::Future;
use std::ops::Range;
use std::sync::mpsc::{Receiver, Sender, channel};

use egui::{Color32, Pos2, Rect, Scene, Sense, Stroke, TextureHandle, Ui, Vec2, pos2};

use crate::befunge::{Event, FungeSpace, StepStatus, get_color_of_bf_op};
use crate::{BefungeState, befunge};

static PRESETS: Dir = include_dir!("./bf_programs");
static CURSOR_COLOR: Color32 = Color32::from_rgb(110, 200, 255);

#[derive(Default, Clone, Copy)]
enum Direction {
    North,
    South,
    #[default]
    East,
    West,
}

#[derive(Default, Clone, Copy)]
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
        time_since_avg: Instant,
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
    glyph_uv_position: [Rect; Self::LENGTH],
    glyph_size: [Vec2; Self::LENGTH],
    glyph_offset: [Vec2; Self::LENGTH],
    /// jank 'is atlas dirty' check
    /// would be nice if i could use the built in delta
    /// but it's one time use and eframe uses it :(
    prev_fill_ratio: f32,
}

impl CharRenderer {
    const MAX: u8 = 255;
    const MIN: u8 = b' ';
    const LENGTH: usize = (Self::MAX - Self::MIN + 1) as usize;

    fn empty() -> Self {
        Self {
            glyph_uv_position: [Rect::ZERO; Self::LENGTH],
            glyph_size: [Vec2::ZERO; Self::LENGTH],
            glyph_offset: [Vec2::ZERO; Self::LENGTH],
            prev_fill_ratio: 0.0,
        }
    }

    // There is likely a better way to find these values
    fn update(&mut self, ctx: &egui::Context) {
        puffin::profile_function!();

        static DIVISOR: f32 = 2.0;

        ctx.fonts(|fonts| {
            let size = fonts.font_image_size();

            let fill_ratio = fonts.font_atlas_fill_ratio();
            if fill_ratio == self.prev_fill_ratio {
                return;
            }
            self.prev_fill_ratio = fill_ratio;

            for val in Self::MIN..=Self::MAX {
                let str = match val {
                    Self::MIN => {
                        // special case used for rendering rectangles
                        String::from("⬜")
                    }
                    b'^' => {
                        // used so "^" is the same size as "v"
                        String::from("v")
                    }
                    val => String::from(val as char),
                };

                let galley =
                    fonts.layout_no_wrap(str, egui::FontId::monospace(32.0), Color32::WHITE);
                let row = galley.rows.first().unwrap();
                let g = row.glyphs.first().unwrap();
                self.glyph_uv_position[(val - Self::MIN) as usize] = Rect::from_two_pos(
                    Pos2::new(
                        g.uv_rect.min[0] as f32 / size[0] as f32,
                        g.uv_rect.min[1] as f32 / size[1] as f32,
                    ),
                    Pos2::new(
                        g.uv_rect.max[0] as f32 / size[0] as f32,
                        g.uv_rect.max[1] as f32 / size[1] as f32,
                    ),
                );
                self.glyph_size[(val - Self::MIN) as usize] = g.uv_rect.size / DIVISOR;
                self.glyph_offset[(val - Self::MIN) as usize] = g.uv_rect.offset / DIVISOR;
            }
        });
    }

    fn draw(&self, mesh: &mut Mesh, egui_pos: Rect, val: u8, color: Color32) {
        assert!(val >= Self::MIN);
        let char_index = val - Self::MIN;
        let uv_pos = self.glyph_uv_position[char_index as usize];
        let glyph_size = self.glyph_size[char_index as usize];
        let glyph_offset = self.glyph_offset[char_index as usize];

        static CENTERING_OFFSET: Vec2 = Vec2::new(1.5, -3.0); // eyeballed

        let egui_pos = if char_index == 0 {
            // special case, draws a box
            egui_pos
        } else {
            Rect::from_min_size(
                egui_pos.left_bottom() + glyph_offset + CENTERING_OFFSET,
                glyph_size,
            )
        };

        let mesh_index = mesh.vertices.len() as u32;
        if val == b'^' {
            mesh.vertices.push(egui::epaint::Vertex {
                pos: egui_pos.left_top(),
                uv: uv_pos.left_bottom(),
                color,
            });
            mesh.vertices.push(egui::epaint::Vertex {
                pos: egui_pos.right_top(),
                uv: uv_pos.right_bottom(),
                color,
            });
            mesh.vertices.push(egui::epaint::Vertex {
                pos: egui_pos.right_bottom(),
                uv: uv_pos.right_top(),
                color,
            });
            mesh.vertices.push(egui::epaint::Vertex {
                pos: egui_pos.left_bottom(),
                uv: uv_pos.left_top(),
                color,
            });
        } else {
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
        }
        mesh.indices.extend_from_slice(&[
            mesh_index,
            mesh_index + 1,
            mesh_index + 2,
            mesh_index + 2,
            mesh_index + 3,
            mesh_index,
        ]);
    }
}

enum ModalState {
    Settings,
    SetPosition(i64, i64),
}

pub struct App {
    texture: TextureHandle,
    text_channel: (
        Sender<(String, Option<String>)>,
        Receiver<(String, Option<String>)>,
    ),
    settings: Settings,
    mode: Mode,
    scene_rect: Rect,
    open_modal: Option<ModalState>,
    scene_offset: (i64, i64),
    cursor_pos: (i64, i64),
    popup_pos: Option<(i64, i64)>,
    char_renderer: CharRenderer,
    filename: Option<String>,
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
                time_since_avg: Instant::now(),
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
                let now = Instant::now();
                if time_per_step {
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
                        16..=20 => loop {
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
                        },
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
            popup_pos: None,
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
            char_renderer: CharRenderer::empty(),
            filename: None,
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

        self.char_renderer.update(ctx);

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

        if let Ok((filename, text)) = self.text_channel.1.try_recv() {
            self.filename = Some(filename);
            if let Some(text) = text {
                self.mode = Mode::Editing {
                    cursor_state: CursorState::default(),
                    fungespace: FungeSpace::new_from_string(&text),
                }
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
                    if cfg!(debug_assertions) {
                        egui::warn_if_debug_build(ui);
                        ui.separator();
                    }
                    ui.add(egui::Label::new(
                        RichText::new(format!(
                            "({:03}, {:03})",
                            self.cursor_pos.0, self.cursor_pos.1
                        ))
                        .text_style(TextStyle::Monospace),
                    ));
                    ui.label("Position: ");

                    ui.separator();

                    let fungespace = match &self.mode {
                        Mode::Playing { bf_state, .. } => &bf_state.map,
                        Mode::Editing { fungespace, .. } => fungespace,
                    };
                    ui.add(egui::Label::new(
                        RichText::new(format!("{:04}", fungespace.get_wrapped(self.cursor_pos)))
                            .text_style(TextStyle::Monospace),
                    ));
                    ui.label("Value: ");

                    if let Mode::Playing {
                        bf_state,
                        time_since_avg,
                        speed,
                        ..
                    } = &mut self.mode
                        && *speed == 20
                    {
                        ui.separator();
                        let now = Instant::now();
                        let time_since = now.duration_since(*time_since_avg).as_micros();
                        let hz =
                            (bf_state.instruction_count as f64 * 1000000.0) / time_since as f64;
                        let hz = match hz {
                            -0.0..1_000.0 => {
                                format!("~{:4}Hz", hz.round())
                            }
                            1_000.0..1_000_000.0 => {
                                format!("~{:.1}KHz", hz / 1_000.0)
                            }
                            1_000_000.0.. => {
                                format!("~{:.2}MHz", hz / 1_000_000.0)
                            }
                            _ => "? Hz".to_string(),
                        };
                        ui.add(egui::Label::new(
                            RichText::new(hz).text_style(TextStyle::Monospace),
                        ))
                        .on_hover_text("Estimate is only vaguely accurate, calculated per frame.");
                        ui.label("Speed: ");
                        bf_state.instruction_count = 0;
                        *time_since_avg = now;
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
                puffin::profile_scope!("control bar");
                match &mut self.mode {
                    Mode::Playing {
                        bf_state,
                        running,
                        follow,
                        speed,
                        error_state,
                        snapshot,
                        ..
                    } => {
                        if error_state.is_some() {
                            ui.disable();
                        }
                        if ui.button("⏵ Step").clicked() {
                            *running = false;
                            Mode::step_befunge_inner(
                                bf_state,
                                running,
                                error_state,
                                &self.settings,
                            );
                        }
                        if ui
                            .button(if *running { "⏸ Pause" } else { "▶ Play" })
                            .clicked()
                        {
                            *running = !(*running);
                        };
                        if ui.button("Reset").clicked() {
                            *running = false;
                            *error_state = None;
                            // teeny bit wasteful
                            let breakpoints = bf_state.breakpoints.clone();
                            **bf_state = BefungeState::new_from_fungespace(snapshot.clone());
                            bf_state.breakpoints = breakpoints;
                        };
                        ui.checkbox(follow, "follow");

                        ui.add(egui::Slider::new(speed, 1..=20).text("speed"));
                    }
                    Mode::Editing { cursor_state, .. } => {
                        ui.label("Cursor direction:");
                        ui.label(match cursor_state.direction {
                            Direction::North => "⬆",
                            Direction::South => "⬇",
                            Direction::East => "➡",
                            Direction::West => "⬅",
                        });

                        ui.label("Cursor mode:");
                        ui.label(if cursor_state.string_mode {
                            "String"
                        } else {
                            "Normal"
                        });
                    }
                }
            });

            ui.add_space(3.0);

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
        ui.input(|e| {
            if e.modifiers.command && e.key_pressed(egui::Key::Enter) {
                self.mode.swap_mode();
            }
            match &mut self.mode {
                Mode::Playing {
                    bf_state,
                    running,
                    snapshot,
                    error_state,
                    follow,
                    ..
                } => {
                    if e.key_pressed(egui::Key::R) {
                        *running = false;
                        *error_state = None;
                        // teeny bit wasteful
                        let breakpoints = bf_state.breakpoints.clone();
                        **bf_state = BefungeState::new_from_fungespace(snapshot.clone());
                        bf_state.breakpoints = breakpoints;
                    }

                    if e.key_pressed(egui::Key::F) {
                        *follow = !(*follow);
                    }

                    if e.key_pressed(egui::Key::Space) {
                        *running = !(*running);
                    }

                    if e.key_pressed(egui::Key::ArrowRight) {
                        *running = false;
                        Mode::step_befunge_inner(bf_state, running, error_state, &self.settings);
                    }
                }
                Mode::Editing {
                    cursor_state,
                    fungespace,
                } => {
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
                }
            }
        });
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

                    const SHOW_OFFSET: bool = false;
                    if SHOW_OFFSET {
                        painter.line_segment(
                            [
                                Pos2::new(clip_rect.left(), 0.0),
                                Pos2::new(clip_rect.right(), 0.0),
                            ],
                            Stroke::new(1.0, Color32::from_gray(50)),
                        );

                        painter.line_segment(
                            [
                                Pos2::new(0.0, clip_rect.top()),
                                Pos2::new(0.0, clip_rect.bottom()),
                            ],
                            Stroke::new(1.0, Color32::from_gray(50)),
                        );
                    }
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
                                let time = (instant.elapsed().as_millis() as f32) / 1000.0;

                                if let Some(mult) = calculate_decay(time) {
                                    let rect = recter(*pos, self.scene_offset);

                                    let [r, g, b] = self.settings.pos_history.1;
                                    painter.rect(
                                        rect,
                                        0.0,
                                        Color32::from_rgb(r, g, b).gamma_multiply(mult),
                                        Stroke::NONE,
                                        StrokeKind::Outside,
                                    );
                                }
                            }

                            for (pos, instant) in &bf_state.put_history {
                                let time = (instant.elapsed().as_millis() as f32) / 1000.0;
                                if let Some(mult) = calculate_decay(time) {
                                    let rect = recter(*pos, self.scene_offset);

                                    let [r, g, b] = self.settings.put_history.1;
                                    painter.rect(
                                        rect,
                                        0.0,
                                        Color32::from_rgb(r, g, b).gamma_multiply(mult),
                                        Stroke::NONE,
                                        StrokeKind::Outside,
                                    );
                                }
                            }

                            for (pos, instant) in &bf_state.get_history {
                                let time = (instant.elapsed().as_millis() as f32) / 1000.0;
                                if let Some(mult) = calculate_decay(time) {
                                    let rect = recter(*pos, self.scene_offset);

                                    let [r, g, b] = self.settings.get_history.1;
                                    painter.rect(
                                        rect,
                                        0.0,
                                        Color32::from_rgb(r, g, b).gamma_multiply(mult),
                                        Stroke::NONE,
                                        StrokeKind::Outside,
                                    );
                                }
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
                                    CURSOR_COLOR
                                },
                                Stroke::new(0.25, Color32::from_gray(90)),
                                StrokeKind::Inside,
                            );

                            let mut cursor_copy = *cursor_state;
                            cursor_copy.step();

                            painter.rect(
                                recter(cursor_copy.location, self.scene_offset),
                                0.0,
                                if cursor_state.string_mode {
                                    Color32::LIGHT_GREEN
                                } else {
                                    CURSOR_COLOR
                                }
                                .gamma_multiply_u8(80),
                                Stroke::NONE,
                                StrokeKind::Inside,
                            );
                        }
                    };
                }

                static PROFILE_EACH_CHAR: bool = false;

                {
                    puffin::profile_scope!("chars");

                    let mut mesh = egui::Mesh::with_texture(egui::TextureId::default());
                    let map = match &mut self.mode {
                        Mode::Playing { bf_state, .. } => &mut bf_state.map,
                        Mode::Editing { fungespace, .. } => fungespace,
                    };

                    let mut integer_clip_rect = (
                        poss_reverse(clip_rect.left_top(), self.scene_offset),
                        poss_reverse(clip_rect.right_bottom(), self.scene_offset),
                    );

                    // fixes bug near edges of screen
                    if integer_clip_rect.1.0 < i64::MIN + 100000 {
                        integer_clip_rect.1.0 = i64::MAX;
                    }

                    if integer_clip_rect.1.1 < i64::MIN + 100000 {
                        integer_clip_rect.1.1 = i64::MAX;
                    }

                    for (pos, val) in map.entries() {
                        if !intersects(integer_clip_rect, pos) || val == ' ' as i64 {
                            continue;
                        }

                        let pos = recter(pos, self.scene_offset);

                        //puffin::profile_scope!("char");

                        if let Ok(val) = TryInto::<u8>::try_into(val) {
                            if val < b' ' {
                                puffin::profile_scope_if!(PROFILE_EACH_CHAR, "char boxed");
                                self.char_renderer.draw(
                                    &mut mesh,
                                    pos,
                                    match val {
                                        0..=9 => val + b'0',
                                        10.. => val - 10 + b'A',
                                    },
                                    Color32::GRAY,
                                );
                                self.char_renderer.draw(&mut mesh, pos, b' ', Color32::GRAY);
                            } else if let Some(color) = get_color_of_bf_op(val) {
                                puffin::profile_scope_if!(PROFILE_EACH_CHAR, "char colored");
                                self.char_renderer.draw(&mut mesh, pos, val, color);
                            } else {
                                puffin::profile_scope_if!(PROFILE_EACH_CHAR, "char simple");
                                self.char_renderer.draw(&mut mesh, pos, val, Color32::GRAY);
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
                            self.char_renderer.draw(&mut mesh, pos, b' ', Color32::GRAY);
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

                                ui.add(
                                    egui::Label::new(
                                        RichText::new(str).font(egui::FontId::monospace(font_size)),
                                    )
                                    .selectable(false),
                                )
                            });
                        };
                    }

                    ui.painter().add(egui::Shape::Mesh(mesh.into()));
                }

                if let Some(popup_pos) = self.popup_pos {
                    puffin::profile_scope!("popup");
                    let transform = ui
                        .ctx()
                        .layer_transform_to_global(ui.layer_id())
                        .unwrap_or(TSTransform::IDENTITY);

                    let rect = recter(popup_pos, self.scene_offset);

                    ui.painter().add(Shape::dashed_line(
                        &[
                            rect.left_top(),
                            rect.right_top(),
                            rect.right_bottom(),
                            rect.left_bottom(),
                            rect.left_top(),
                        ],
                        Stroke::new(0.5, Color32::ORANGE),
                        1.0,
                        1.0,
                    ));

                    // TODO: figure out sizing
                    let popup = egui::Popup::new(
                        Id::new("info context menu"),
                        ui.ctx().clone(),
                        transform.mul_pos(poss((
                            (popup_pos.0 - self.scene_offset.0) as f32 + 0.75,
                            (popup_pos.1 - self.scene_offset.1) as f32 + 0.75,
                        ))),
                        LayerId::background(),
                    )
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                    .show(|ui| {
                        ui.label(format!("Pos: {}, {}", popup_pos.0, popup_pos.1));
                        match &mut self.mode {
                            Mode::Playing { bf_state, .. } => {
                                Self::dual_char_and_numeric_input(ui, &mut bf_state.map, popup_pos);

                                let mut breakpoint = bf_state.breakpoints.contains(&popup_pos);
                                // FIXME: don't do hotkey if char/numeric input is selected
                                ui.input(|e| {
                                    if e.key_pressed(egui::Key::B) {
                                        if breakpoint {
                                            bf_state.breakpoints.remove(&popup_pos);
                                        } else {
                                            bf_state.breakpoints.insert(popup_pos);
                                        }
                                    }
                                });
                                if ui.checkbox(&mut breakpoint, "Breakpoint").clicked() {
                                    if breakpoint {
                                        bf_state.breakpoints.insert(popup_pos);
                                    } else {
                                        bf_state.breakpoints.remove(&popup_pos);
                                    }
                                };
                            }
                            Mode::Editing { fungespace, .. } => {
                                Self::dual_char_and_numeric_input(ui, fungespace, popup_pos);
                            }
                        };
                    });

                    if let Some(popup) = popup
                        && popup.response.should_close()
                    {
                        self.popup_pos = None;
                    }
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
                Mode::Playing { .. } => (),
                Mode::Editing { cursor_state, .. } => {
                    if pos.1 >= 0 && pos.0 >= 0 {
                        cursor_state.location = pos;
                    }
                }
            }
        };

        if response.secondary_clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            let pos = poss_reverse(pos, self.scene_offset);
            if pos.0 >= 0 && pos.1 >= 0 {
                self.popup_pos = Some(pos);
            }
        };
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        puffin::profile_function!();
        egui::MenuBar::new().ui(ui, |ui| {
            let is_web = cfg!(target_arch = "wasm32");
            ui.menu_button("File", |ui| {
                if ui.button("📄 New").clicked() {
                    self.filename = None;
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
                            let _ = sender.send((
                                file.file_name(),
                                Some(String::from_utf8_lossy(&text).to_string()),
                            ));
                            ctx.request_repaint();
                        }
                    });
                }

                if ui.button("💾 Save").clicked() {
                    let sender = self.text_channel.0.clone();
                    let mut task = rfd::AsyncFileDialog::new();
                    if let Some(filename) = &self.filename {
                        task = task.set_file_name(filename);
                    }

                    let task = task.save_file();
                    let contents = match &mut self.mode {
                        Mode::Playing { bf_state, .. } => bf_state.map.serialize(),
                        Mode::Editing { fungespace, .. } => fungespace.serialize(),
                    };

                    execute(async move {
                        let file = task.await;
                        if let Some(file) = file {
                            _ = file.write(contents.as_bytes()).await;
                            let _ = sender.send((file.file_name(), None));
                        }
                    });
                }

                ui.menu_button("👕 Load Preset", |ui| {
                    for file in PRESETS.files() {
                        if ui
                            .button(file.path().file_stem().unwrap().to_string_lossy())
                            .clicked()
                        {
                            self.filename = Some(
                                file.path()
                                    .file_name()
                                    .unwrap()
                                    .to_string_lossy()
                                    .to_string(),
                            );
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

            if let Some(open_modal) = &mut self.open_modal {
                let modal = Modal::new(Id::new("Settings modal")).show(ui.ctx(), |ui| {
                    ui.set_width(300.0);

                    match open_modal {
                        ModalState::Settings => Self::settings_modal(ui, &mut self.settings),
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

                let settings_button =
                    egui::Button::new("Advanced settings").right_text(SubMenuButton::RIGHT_ARROW);

                if ui.add(settings_button).clicked() {
                    self.open_modal = Some(ModalState::Settings);
                };
            });

            ui.menu_button("View", |ui| {
                if ui.button("Show whole program").clicked() {
                    self.scene_offset = (0, 0);
                    let fungespace = match &self.mode {
                        Mode::Playing { bf_state, .. } => &bf_state.map,
                        Mode::Editing { fungespace, .. } => fungespace,
                    };
                    self.scene_rect = Rect::from_min_max(
                        poss((-1.0, -1.0)),
                        poss((
                            (fungespace.max_size.0 + 2) as f32,
                            (fungespace.max_size.1 + 2) as f32,
                        )),
                    );
                };
            });

            ui.menu_button("Tools", |ui| {
                if !is_web {
                    let mut profile = puffin::are_scopes_on();
                    ui.checkbox(&mut profile, "Enable UI profiling");
                    puffin::set_scopes_on(profile);
                }

                if ui.button("Set viewport position").clicked() {
                    self.open_modal = Some(ModalState::SetPosition(0, 0));
                };
            });

            egui::widgets::global_theme_preference_switch(ui);

            ui.separator();

            let mode = match self.mode {
                Mode::Editing { .. } => false,
                Mode::Playing { .. } => true,
            };

            if ui.add(egui::Button::selectable(!mode, "Edit")).clicked() && mode {
                self.mode.swap_mode();
            };

            if ui.add(egui::Button::selectable(mode, "Run")).clicked() && !mode {
                self.mode.swap_mode();
            };

            if let Mode::Playing {
                error_state: Some(error),
                ..
            } = &self.mode
            {
                ui.label(RichText::new(error.to_string()).color(Color32::RED));
            }

            if let Some(filename) = &self.filename {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(filename);
                });
            }
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

                let resp = ui.text_edit_multiline(&mut bf_state.input_buffer);
                if resp.changed() && bf_state.map.get_wrapped(bf_state.position) == b'~' as i64 {
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

    fn settings_modal(ui: &mut egui::Ui, settings: &mut Settings) {
        ui.heading("Advanced settings");
        ui.separator();
        ui.label(RichText::new("Track position history").font(FontId::proportional(14.0)));
        ui.horizontal(|ui| {
            ui.color_edit_button_srgb(&mut settings.pos_history.1);
            ui.label("Color");
        });
        ui.horizontal(|ui| ui.checkbox(&mut settings.pos_history.0, "Enabled"));

        ui.separator();
        ui.label(RichText::new("Track put history").font(FontId::proportional(14.0)));
        ui.horizontal(|ui| {
            ui.color_edit_button_srgb(&mut settings.put_history.1);
            ui.label("Color");
        });
        ui.checkbox(&mut settings.put_history.0, "Enabled");

        ui.separator();
        ui.label(RichText::new("Track get history").font(FontId::proportional(14.0)));
        ui.horizontal(|ui| {
            ui.color_edit_button_srgb(&mut settings.get_history.1);
            ui.label("Color");
        });
        ui.horizontal(|ui| ui.checkbox(&mut settings.get_history.0, "Enabled"));

        ui.separator();
        if ui.button("Reset all settings").clicked() {
            *settings = Settings::default();
        };
    }

    fn set_position_modal(ui: &mut egui::Ui, x: &mut i64, y: &mut i64) {
        ui.heading("Set position");
        ui.add(egui::DragValue::new(x).speed(0.1));
        ui.add(egui::DragValue::new(y).speed(0.1));
    }

    fn dual_char_and_numeric_input(
        ui: &mut egui::Ui,
        fungespace: &mut FungeSpace,
        pos: (i64, i64),
    ) {
        // TODO: clean this up & make the text input box a lil wider
        let chr = fungespace.get_wrapped(pos);
        ui.horizontal(|ui| {
            let str = if let Ok(chr) = chr.try_into() {
                match chr {
                    0..7 => format!("\\{chr}"),
                    7 => r"\a".to_string(),
                    8 => r"\b".to_string(),
                    9 => r"\t".to_string(),
                    10 => r"\n".to_string(),
                    11 => r"\v".to_string(),
                    12 => r"\f".to_string(),
                    13 => r"\r".to_string(),
                    14..b' ' | 127.. => "❎".to_string(),
                    b' ' => "⚫".to_string(),
                    other => String::from(other as char),
                }
            } else {
                String::from("❎")
            };

            let where_to_put_background = ui.painter().add(Shape::Noop);
            let background_color = ui.visuals().text_edit_bg_color();
            let label = Label::new(&str).sense(Sense::click()).selectable(false);
            let output = ui.add(label);

            if output.clicked() {
                output.request_focus()
            }

            if output.has_focus() {
                ui.input(|e| {
                    if e.key_pressed(egui::Key::Backspace) {
                        fungespace.set(pos, b' ' as i64);
                    }

                    for event in e.filtered_events(&egui::EventFilter {
                        tab: false,
                        escape: false,
                        horizontal_arrows: false,
                        vertical_arrows: false,
                    }) {
                        match event {
                            egui::Event::Text(text) | egui::Event::Paste(text) => {
                                if let Some(chr) = text.chars().last() {
                                    fungespace.set(pos, chr as i64);
                                }
                            }
                            _ => (),
                        }
                    }
                })
            }

            // Most of this is taken straight from the egui code for
            // TextEdit
            let visuals = ui.style().interact(&output);
            let frame_rect = output.rect.expand(visuals.expansion);
            let shape = if output.has_focus() {
                egui::epaint::RectShape::new(
                    frame_rect,
                    visuals.corner_radius,
                    background_color,
                    ui.visuals().selection.stroke,
                    StrokeKind::Inside,
                )
            } else {
                egui::epaint::RectShape::new(
                    frame_rect,
                    visuals.corner_radius,
                    background_color,
                    visuals.bg_stroke,
                    StrokeKind::Inside,
                )
            };

            ui.painter().set(where_to_put_background, shape);

            let mut chr = chr;
            if ui.add(egui::DragValue::new(&mut chr).speed(1.0)).changed() {
                fungespace.set(pos, chr);
            };
        });
    }
}

// could optimize by caching within a frame cuz there's likely to be a lot of identical timestamps
fn calculate_decay(time: f32) -> Option<f32> {
    if time >= 5.0 {
        return None;
    }
    let mult = f32::log2(5.0 - time) - 1.322 - 0.3;
    if mult <= 0.0 { None } else { Some(mult) }
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
