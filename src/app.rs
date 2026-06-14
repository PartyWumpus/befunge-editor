use coarsetime::{Duration, Instant};
use egui::ahash::{HashMap, HashSet};
use egui::emath::TSTransform;
use egui::scroll_area::ScrollBarVisibility;
use egui::style::ScrollStyle;
use egui::{
    FontId, Id, Key, KeyboardShortcut, Label, LayerId, Mesh, Modal, ModifierNames, Modifiers,
    Response, RichText, ScrollArea, Shape, StrokeKind, TextStyle, UiBuilder,
};
use egui_material_icons::icons;
use include_dir::{Dir, include_dir};
use rfd::FileHandle;
use std::future::Future;
use std::mem;
use std::ops::Range;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use egui::{Color32, Pos2, Rect, Scene, Sense, Stroke, TextureHandle, Ui, Vec2, pos2};

use crate::befunge::{
    self, Befunge, BefungeVersion, BefungeVersionDiscriminants, Direction, FungeSpaceTrait,
    GraphicalEvent, Position, SerializationError, StepStatus, Value, bf93_op_info, bf98_op_info,
};
use crate::{befunge93, befunge93mini, befunge98};

static BF93_PRESETS: Dir = include_dir!("./bf_programs/befunge93");
static BF98_PRESETS: Dir = include_dir!("./bf_programs/befunge98");
static BF_PRESETS: Dir = include_dir!("./bf_programs/generic");

static CURSOR_COLOR: Color32 = Color32::from_rgb(110, 200, 255);
static PROFILE_EACH_CHAR: bool = false;
macro_rules! icon {
    ($icon:expr, $text:expr) => {
        const_format::concatcp!($icon, "\u{2009}", $text)
    };
}

const SHORTCUT_SWAP_MODE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Enter);

const SHORTCUT_SAVE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);
const SHORTCUT_SAVE_AS: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::S);
const SHORTCUT_RELOAD_FILE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::R);

const SHORTCUT_UNDO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Z);
const SHORTCUT_REDO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Y);
const SHORTCUT_REDO_ALT: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Z);

const SYMBOLS: ModifierNames = ModifierNames {
    is_short: true,
    alt: icons::ICON_KEYBOARD_OPTION_KEY,
    ctrl: icons::ICON_KEYBOARD_CONTROL_KEY,
    shift: icons::ICON_SHIFT,
    mac_cmd: icons::ICON_KEYBOARD_COMMAND_KEY,
    mac_alt: icons::ICON_KEYBOARD_OPTION_KEY,
    concat: "",
};

const NAMES: ModifierNames = ModifierNames {
    is_short: false,
    alt: "Alt",
    ctrl: "Ctrl",
    shift: "Shift",
    mac_cmd: "Cmd",
    mac_alt: "Option",
    concat: "+",
};

#[derive(Default)]
pub struct StartConfig {
    pub contents: Option<String>,
    pub start_in_run_mode: bool,
    pub run_and_exit: bool,
    pub hide_toolbar: bool,
    pub hide_extra: bool,
    pub fix_camera_to_program: bool,
    pub starting_position: Option<(i64, i64)>,
    pub settings: Option<Settings>,
}

#[derive(Default, Clone, Copy)]
pub struct CursorCaret {
    location: (i64, i64),
    direction: Direction,
    string_mode: bool,
}

#[derive(Clone, Default)]
pub struct FungeSpace {
    pub map: HashMap<Position, Value>,
    dirty: bool,
}

#[derive(Clone)]
enum Undo {
    Normal(Box<[((i64, i64), i64)]>),
    Dedupable((i64, i64), i64),
}

type UndoList = Vec<Undo>;
type RedoList = Vec<Box<[((i64, i64), i64)]>>;

impl FungeSpaceTrait for FungeSpace {
    fn set(&mut self, pos: Position, val: Value) {
        if pos.0 < 0 || pos.1 < 0 {
            return;
        };

        self.dirty = true;

        if val == b' ' as Value {
            self.map.remove(&pos);
        } else {
            self.map.insert(pos, val);
        }
    }

    fn get(&self, pos: Position) -> Value {
        if pos.0 < 0 || pos.1 < 0 {
            return b' ' as i64;
        }
        *self.map.get(&pos).unwrap_or(&(b' ' as Value))
    }

    fn program_size(&self) -> (i64, i64) {
        let (mut width, mut height) = (10, 10);
        for (x, y) in self.map.keys() {
            if *y > height {
                height = *y
            }
            if *x > width {
                width = *x
            }
        }
        (width, height)
    }

    fn entries(&self) -> impl Iterator<Item = (Position, Value)> {
        self.map.iter().map(|(k, v)| (*k, *v))
    }
}

impl FungeSpace {
    pub fn new_from_string(input: &str) -> FungeSpace {
        let mut map = FungeSpace::default();
        for (y, line) in input.lines().enumerate() {
            for (x, char) in line.chars().enumerate() {
                map.map.insert(
                    (x.try_into().unwrap(), y.try_into().unwrap()),
                    char as Value,
                );
            }
        }
        map
    }
}

#[derive(Clone)]
enum CursorSpan {
    Partial((i64, i64)),
    Full {
        original_start: (i64, i64),
        start: (i64, i64),
        end: (i64, i64),
        data: Vec<i64>,
    },
}

#[derive(Clone)]
enum CursorMode {
    Span(CursorSpan),
    Caret(CursorCaret),
}

impl CursorMode {
    fn caret(location: (i64, i64)) -> Self {
        Self::Caret(CursorCaret {
            location,
            ..Default::default()
        })
    }
}

impl Default for CursorMode {
    fn default() -> Self {
        Self::Caret(CursorCaret::default())
    }
}

#[derive(Clone)]
enum Mode {
    Editing {
        undos: UndoList,
        redos: RedoList,
        cursor_mode: CursorMode,
        fungespace: FungeSpace,
        stdin: String,
    },
    Playing {
        snapshot: (FungeSpace, String),
        time_since_step: Instant,
        time_since_avg: Instant,
        bf_state: Box<BefungeVersion>,
        instruction_since: usize,
        running: bool,
        follow: bool,
        speed: u8,
        error_state: Option<&'static str>,
        watch_list: HashSet<Position>,
        run_and_exit: bool,
    },
}

#[derive(serde::Deserialize, serde::Serialize, PartialEq, Clone)]
pub enum InvalidOperationBehaviour {
    Reflect,
    Halt,
    Ignore,
}

#[derive(serde::Deserialize, serde::Serialize, Clone)]
pub struct Settings {
    pub pos_history: (bool, [u8; 3]),
    pub get_history: (bool, [u8; 3]),
    pub put_history: (bool, [u8; 3]),
    pub skip_spaces: bool,
    pub render_unicode: bool,
    pub display_debug_info: bool,
    pub run_until_breakpoint: bool,
    pub invalid_operation_behaviour: InvalidOperationBehaviour,
    pub befunge_version: BefungeVersionDiscriminants,
    pub non_blocking_input: bool,
    pub display_befunge_tooltips: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            pos_history: (true, [128, 0, 128]),
            get_history: (true, [255, 0, 0]),
            put_history: (true, [0, 255, 0]),
            skip_spaces: false,
            display_debug_info: false,
            run_until_breakpoint: false,
            render_unicode: true,
            invalid_operation_behaviour: InvalidOperationBehaviour::Halt,
            befunge_version: BefungeVersionDiscriminants::Befunge93,
            non_blocking_input: false,
            display_befunge_tooltips: true,
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

        ctx.fonts_mut(|fonts| {
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
    Text(Box<str>),
    SetPosition(i64, i64),
    Info,
    SaveFailure(SerializationError),
}

#[derive(Clone)]
enum FileType {
    Handle(FileHandle),
    Filename(String),
}

impl FileType {
    fn file_name(&self) -> String {
        match self {
            Self::Handle(handle) => handle.file_name(),
            Self::Filename(filename) => filename.clone(),
        }
    }
}

pub struct App {
    texture: TextureHandle,
    text_channel: (
        Sender<(FileHandle, Option<Vec<u8>>)>,
        Receiver<(FileHandle, Option<Vec<u8>>)>,
    ),
    settings: Settings,
    config: StartConfig,
    mode: Mode,
    scene_rect: Rect,
    open_modal: Option<ModalState>,
    scene_offset: (i64, i64),
    cursor_pos: (i64, i64),
    popup_pos: Option<(i64, i64)>,
    char_renderer: CharRenderer,
    file: Option<FileType>,
    befunge_tooltip_open: bool,
}

const fn poss(pos: (f32, f32)) -> Pos2 {
    Pos2::new((pos.0) * 13.0, (pos.1) * 17.0)
}

fn poss_reverse(pos: Pos2, offset: (i64, i64)) -> (i64, i64) {
    let x = pos.x / 13.0;
    let x = if x.is_sign_negative() { x - 1.0 } else { x };

    let y = pos.y / 17.0;
    let y = if y.is_sign_negative() { y - 1.0 } else { y };
    (
        (x as i64).saturating_add(offset.0),
        (y as i64).saturating_add(offset.1),
    )
}

fn intersects(a: ((i64, i64), (i64, i64)), b: (i64, i64)) -> bool {
    a.0.0 <= b.0 && b.0 <= a.1.0 && a.0.1 <= b.1 && b.1 <= a.1.1
}

fn step(mut pos: (i64, i64), direction: Direction, settings: &Settings) -> (i64, i64) {
    let (x, y) = pos;
    match direction {
        Direction::North => pos = (x, y.saturating_sub(1)),
        Direction::South => pos = (x, y.saturating_add(1)),
        Direction::East => pos = (x.saturating_add(1), y),
        Direction::West => pos = (x.saturating_sub(1), y),
    }

    let border_pos = settings.befunge_version.border_positions();
    if pos.0 < border_pos.0.0 {
        pos.0 = border_pos.0.0
    }
    if pos.1 < border_pos.0.1 {
        pos.1 = border_pos.0.1
    }

    if pos.0 > border_pos.1.0 {
        pos.0 = border_pos.1.0
    }
    if pos.1 > border_pos.1.1 {
        pos.1 = border_pos.1.1
    }
    pos
}

impl CursorCaret {
    fn step(&mut self, settings: &Settings) {
        self.location = step(self.location, self.direction, settings);
    }

    fn step_cursor_back(&mut self, settings: &Settings) {
        let (x, y) = self.location;
        match self.direction {
            Direction::North => self.location = (x, y.saturating_add(1)),
            Direction::South => self.location = (x, y.saturating_sub(1)),
            Direction::East => self.location = (x.saturating_sub(1), y),
            Direction::West => self.location = (x.saturating_add(1), y),
        }

        let border_pos = settings.befunge_version.border_positions();
        if self.location.0 < border_pos.0.0 {
            self.location.0 = border_pos.0.0
        }
        if self.location.1 < border_pos.0.1 {
            self.location.1 = border_pos.0.1
        }

        if self.location.0 > border_pos.1.0 {
            self.location.0 = border_pos.1.0
        }
        if self.location.1 > border_pos.1.1 {
            self.location.1 = border_pos.1.1
        }
    }
}

impl Mode {
    fn set_cursor(
        cursor_mode: &mut CursorMode,
        fungespace: &mut FungeSpace,
        undos: &mut UndoList,
        redos: &mut RedoList,
        new_mode: CursorMode,
    ) {
        let old = mem::replace(cursor_mode, new_mode);
        if let CursorMode::Span(CursorSpan::Full {
            start,
            end,
            data,
            original_start,
        }) = old
        {
            let mut ops = vec![];
            for y in 0..=end.1 - start.1 {
                for x in 0..=end.0 - start.0 {
                    let pos = (x + start.0, y + start.1);
                    let val = data[(x + y * (end.0 - start.0 + 1)) as usize];
                    if original_start != start {
                        ops.push((pos, fungespace.get(pos)));
                        ops.push(((x + original_start.0, y + original_start.1), val));
                    }
                    fungespace.set(pos, val);
                }
            }
            if original_start != start {
                undos.push(Undo::Normal(ops.into()));
                redos.clear();
            }
        }
    }

    fn reset_cursor(
        cursor_mode: &mut CursorMode,
        fungespace: &mut FungeSpace,
        undos: &mut UndoList,
        redos: &mut RedoList,
    ) {
        let pos = *match cursor_mode {
            CursorMode::Span(CursorSpan::Partial(pos)) => pos,
            CursorMode::Span(CursorSpan::Full { start, .. }) => start,
            CursorMode::Caret(CursorCaret { location, .. }) => location,
        };
        Mode::set_cursor(
            cursor_mode,
            fungespace,
            undos,
            redos,
            CursorMode::caret(pos),
        );
    }

    fn swap_mode(&mut self, settings: &Settings) {
        *self = match self.clone() {
            Mode::Editing {
                fungespace, stdin, ..
            } => {
                let mut bf_state = match settings.befunge_version {
                    BefungeVersionDiscriminants::Befunge93 => Box::new(BefungeVersion::Befunge93(
                        befunge93::State::new_from_fungespace(fungespace.clone()),
                    )),
                    BefungeVersionDiscriminants::Befunge93Mini => {
                        Box::new(BefungeVersion::Befunge93Mini(
                            befunge93mini::State::new_from_fungespace(fungespace.clone()),
                        ))
                    }
                    BefungeVersionDiscriminants::Befunge98 => Box::new(BefungeVersion::Befunge98(
                        befunge98::State::new_from_fungespace(fungespace.clone()),
                    )),
                };

                *bf_state.stdin() = stdin.clone();

                Mode::Playing {
                    snapshot: (fungespace.clone(), stdin.clone()),
                    instruction_since: 0,
                    time_since_step: Instant::now(),
                    time_since_avg: Instant::now(),
                    bf_state,
                    running: false,
                    follow: false,
                    speed: 5,
                    error_state: None,
                    watch_list: HashSet::default(),
                    run_and_exit: false,
                }
            }
            Mode::Playing {
                snapshot, bf_state, ..
            } => Mode::Editing {
                undos: Vec::new(),
                redos: Vec::new(),
                cursor_mode: CursorMode::caret(bf_state.cursor_positions()[0]),
                fungespace: snapshot.0,
                stdin: snapshot.1,
            },
        };
    }

    fn step_befunge_inner(
        bf_state: &mut BefungeVersion,
        running: &mut bool,
        error_state: &mut Option<&'static str>,
        run_and_exit: bool,
        settings: &Settings,
    ) -> bool {
        let step_state = bf_state.step(settings);
        match step_state {
            StepStatus::Normal | StepStatus::NormalNoStep => false,
            StepStatus::Breakpoint => {
                *running = false;
                true
            }
            StepStatus::EndProgram(exit_code) => {
                if run_and_exit {
                    print!("{}", bf_state.stdout());
                    std::process::exit(exit_code as i32);
                }
                *running = false;
                true
            }
            StepStatus::Error(error) => {
                use InvalidOperationBehaviour as IOpBehav;

                match bf_state {
                    BefungeVersion::Befunge93(bf_state) => {
                        match settings.invalid_operation_behaviour {
                            IOpBehav::Reflect => {
                                bf_state.direction = bf_state.direction.reverse();
                                bf_state.step_position(settings);
                                false
                            }
                            IOpBehav::Halt => {
                                *error_state = Some(error);
                                *running = false;
                                true
                            }
                            IOpBehav::Ignore => {
                                bf_state.step_position(settings);
                                false
                            }
                        }
                    }
                    BefungeVersion::Befunge93Mini(bf_state) => {
                        match settings.invalid_operation_behaviour {
                            IOpBehav::Reflect => {
                                bf_state.direction = bf_state.direction.reverse();
                                bf_state.step_position(settings);
                                false
                            }
                            IOpBehav::Halt => {
                                *error_state = Some(error);
                                *running = false;
                                true
                            }
                            IOpBehav::Ignore => {
                                bf_state.step_position(settings);
                                false
                            }
                        }
                    }
                    BefungeVersion::Befunge98(_) => match settings.invalid_operation_behaviour {
                        IOpBehav::Reflect | IOpBehav::Ignore => unreachable!(),
                        IOpBehav::Halt => {
                            *error_state = Some(error);
                            *running = false;
                            true
                        }
                    },
                }
            }
            StepStatus::SyncFrame => true,
        }
    }

    fn step_befunge(&mut self, settings: &Settings) {
        puffin::profile_function!();
        match self {
            Mode::Editing { .. } => (),
            Mode::Playing {
                time_since_step,
                speed,
                bf_state,
                running,
                error_state,
                run_and_exit,
                ..
            } => {
                if settings.run_until_breakpoint && *speed == 20 {
                    loop {
                        if Self::step_befunge_inner(
                            bf_state,
                            running,
                            error_state,
                            *run_and_exit,
                            settings,
                        ) {
                            return;
                        }
                    }
                }

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
                            Self::step_befunge_inner(
                                bf_state,
                                running,
                                error_state,
                                *run_and_exit,
                                settings,
                            );
                        }
                        6..=9 => {
                            for _ in 0..=*speed - 6 {
                                if Self::step_befunge_inner(
                                    bf_state,
                                    running,
                                    error_state,
                                    *run_and_exit,
                                    settings,
                                ) {
                                    break;
                                };
                            }
                        }
                        10..=15 => {
                            for _ in 0..=2_usize.pow(*speed as u32 - 8) {
                                if Self::step_befunge_inner(
                                    bf_state,
                                    running,
                                    error_state,
                                    *run_and_exit,
                                    settings,
                                ) {
                                    break;
                                };
                            }
                        }
                        16..=20 => 'loopy: loop {
                            for _ in 0..10000 {
                                if Self::step_befunge_inner(
                                    bf_state,
                                    running,
                                    error_state,
                                    *run_and_exit,
                                    settings,
                                ) {
                                    break 'loopy;
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
            }
        }
    }

    fn undo(fungespace: &mut FungeSpace, undos: &mut UndoList, redos: &mut RedoList) {
        let Some(undo) = undos.pop() else { return };
        match undo {
            Undo::Normal(undos) => {
                let mut ops = vec![];
                for (pos, val) in undos {
                    ops.push((pos, fungespace.get(pos)));
                    fungespace.set(pos, val);
                }
                redos.push(ops.into());
            }
            Undo::Dedupable(pos, val) => {
                redos.push(vec![(pos, fungespace.get(pos))].into());
                fungespace.set(pos, val);
            }
        }
    }

    fn redo(fungespace: &mut FungeSpace, undos: &mut UndoList, redos: &mut RedoList) {
        if let Some(redos) = redos.pop() {
            let mut ops = vec![];
            for (pos, val) in redos {
                ops.push((pos, fungespace.get(pos)));
                fungespace.set(pos, val);
            }
            undos.push(Undo::Normal(ops.into()));
        };
    }
}

impl App {
    /// Called once before the first frame.
    pub fn new(cc: &eframe::CreationContext<'_>, config: StartConfig) -> Self {
        // This is also where you can customize the look and feel of egui using
        // `cc.egui_ctx.set_visuals` and `cc.egui_ctx.set_fonts`.

        cc.egui_ctx.style_mut(|style| {
            style.url_in_tooltip = true;
        });

        egui_material_icons::initialize(&cc.egui_ctx);

        let settings = if let Some(settings) = &config.settings {
            settings.clone()
        } else if let Some(storage) = cc.storage {
            // Load previous app state
            eframe::get_value(storage, eframe::APP_KEY).unwrap_or_default()
        } else {
            Settings::default()
        };

        {
            let ctx = cc.egui_ctx.clone();
            subsecond::register_handler(Arc::new(move || {
                ctx.request_repaint();
            }));
        }

        let fungespace = match &config.contents {
            None => FungeSpace::default(),
            Some(contents) => FungeSpace::new_from_string(contents),
        };

        let mut mode = Mode::Editing {
            undos: Vec::new(),
            redos: Vec::new(),
            cursor_mode: CursorMode::default(),
            fungespace,
            stdin: String::new(),
        };

        if config.start_in_run_mode {
            Mode::swap_mode(&mut mode, &settings);

            if let Mode::Playing {
                run_and_exit,
                running,
                speed,
                bf_state,
                ..
            } = &mut mode
            {
                if let Some(pos) = config.starting_position {
                    bf_state.debug_set_position(pos);
                }

                if config.run_and_exit {
                    *speed = 20;
                    *running = true;
                    *run_and_exit = true
                };
            } else {
                unreachable!()
            }
        };

        Self {
            scene_rect: Rect::ZERO,
            text_channel: channel(),
            settings,
            config,
            scene_offset: (0, 0),
            cursor_pos: (0, 0),
            popup_pos: None,
            open_modal: None,
            mode,
            texture: cc.egui_ctx.load_texture(
                "noise",
                egui::ColorImage::example(),
                egui::TextureOptions::NEAREST,
            ),
            char_renderer: CharRenderer::empty(),
            file: None,
            befunge_tooltip_open: false,
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

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        subsecond::call(|| {
            Self::app(self, ui, _frame);
        })
    }
}

impl App {
    fn app(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        puffin::profile_function!();

        macro_rules! shortcut {
            ($icon:expr) => {
                $icon.format(&SYMBOLS, ui.os().is_mac())
            };
        }

        if !cfg!(target_arch = "wasm32") {
            puffin::GlobalProfiler::lock().new_frame();
            puffin_egui::show_viewport_if_enabled(ui);
        }

        self.char_renderer.update(ui);

        if let Mode::Playing { running, speed, .. } = self.mode
            && running
        {
            self.mode.step_befunge(&self.settings);
            if speed == 20 {
                ui.request_repaint_after(std::time::Duration::from_millis(0));
            } else {
                ui.request_repaint_after(std::time::Duration::from_millis((1000.0 / 33.0) as u64));
            }
        }

        if let Ok((file, text)) = self.text_channel.1.try_recv() {
            self.file = Some(FileType::Handle(file));
            if let Mode::Editing {
                fungespace: FungeSpace { dirty, .. },
                ..
            } = &mut self.mode
            {
                *dirty = false;
            }
            if let Some(text) = text {
                self.mode = Mode::Editing {
                    undos: Vec::new(),
                    redos: Vec::new(),
                    cursor_mode: CursorMode::default(),
                    // TODO: support latin-1 as well as utf-8, for the mycology suite
                    // latin-1 mode should turn form feed (12) into nothing
                    fungespace: FungeSpace::new_from_string(&String::from_utf8_lossy(&text)),
                    stdin: String::new(),
                }
            }
        }

        if !self.config.hide_toolbar {
            egui::TopBottomPanel::top("top_panel").show(ui, |ui| {
                self.menu_bar(ui);
            });
        }

        if !self.config.hide_extra {
            egui::TopBottomPanel::bottom("bottom_panel").show(ui, |ui| {
            puffin::profile_scope!("bottom panel");
            egui::MenuBar::new().ui(ui, |ui| {
                // prob should figure out a better way of doing this instead of hardcoding 750
                if ui.available_width() > 750.0 {
                    powered_by_egui_and_eframe(ui);
                    ui.add(egui::github_link_file!(
                        "https://github.com/PartyWumpus/WCBE/blob/main/",
                        "Source code."
                    ));
                }

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

                    let val = match &self.mode {
                        Mode::Playing { bf_state, .. } => bf_state.get(self.cursor_pos),
                        Mode::Editing { fungespace, .. } => fungespace.get(self.cursor_pos),
                    };
                    ui.add(egui::Label::new(
                        RichText::new(format!("{:04}", val))
                            .text_style(TextStyle::Monospace),
                    ));
                    ui.label("Value: ");

                    if let Mode::Playing {
                        bf_state,
                        time_since_avg,
                        instruction_since,
                        speed,
                        ..
                    } = &mut self.mode
                        && *speed == 20
                    {
                        ui.separator();
                        let now = Instant::now();
                        let time_since = now.duration_since(*time_since_avg).as_micros();
                        let hz =
                            ((bf_state.instruction_count()-*instruction_since) as f64 * 1000000.0) / time_since as f64;
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
                        .on_hover_text("Estimate is only vaguely accurate, calculated per frame.\nWhen skip spaces is on, this estimate is totally wrong.");
                        ui.label("Speed: ");
                        *instruction_since = bf_state.instruction_count();
                        *time_since_avg = now;
                    };
                });
            });
        });
        }

        egui::SidePanel::left("left_panel")
            .resizable(false)
            .exact_width(150.0)
            .show(ui, |ui| {
                self.info_panel(ui);
            });

        egui::CentralPanel::default().show(ui, |ui| {
            puffin::profile_scope!("central panel");

            {
                puffin::profile_scope!("control bar");
                match &mut self.mode {
                    Mode::Playing {
                        bf_state,
                        running,
                        follow,
                        speed,
                        error_state,
                        snapshot,
                        watch_list,
                        run_and_exit,
                        ..
                    } => {
                        ui.horizontal(|ui| {
                            ui.scope(|ui| {
                                if error_state.is_some() {
                                    ui.disable();
                                }
                                if ui
                                    .add(
                                        egui::Button::new(icon!(icons::ICON_STEP, "Step"))
                                            .shortcut_text(icons::ICON_ARROW_RIGHT_ALT),
                                    )
                                    .clicked()
                                {
                                    *running = false;
                                    Mode::step_befunge_inner(
                                        bf_state,
                                        running,
                                        error_state,
                                        *run_and_exit,
                                        &self.settings,
                                    );
                                }
                                if ui
                                    .add(
                                        egui::Button::new(if *running {
                                            icon!(icons::ICON_PAUSE, "Pause")
                                        } else {
                                            icon!(icons::ICON_PLAY_ARROW, "Play")
                                        })
                                        .shortcut_text(icons::ICON_SPACE_BAR),
                                    )
                                    .clicked()
                                {
                                    *running = !(*running);
                                };
                            });
                            if ui
                                .add(
                                    egui::Button::new(icon!(icons::ICON_REPLAY, "Reset"))
                                        .shortcut_text("R"),
                                )
                                .clicked()
                            {
                                *running = false;
                                *error_state = None;
                                // teeny bit wasteful
                                let breakpoints = bf_state.breakpoints().clone();
                                **bf_state = match self.settings.befunge_version {
                                    BefungeVersionDiscriminants::Befunge93 => {
                                        BefungeVersion::Befunge93(
                                            befunge93::State::new_from_fungespace(
                                                snapshot.0.clone(),
                                            ),
                                        )
                                    }
                                    BefungeVersionDiscriminants::Befunge93Mini => {
                                        BefungeVersion::Befunge93Mini(
                                            befunge93mini::State::new_from_fungespace(
                                                snapshot.0.clone(),
                                            ),
                                        )
                                    }
                                    BefungeVersionDiscriminants::Befunge98 => {
                                        BefungeVersion::Befunge98(
                                            befunge98::State::new_from_fungespace(
                                                snapshot.0.clone(),
                                            ),
                                        )
                                    }
                                };
                                *bf_state.breakpoints() = breakpoints;
                                *bf_state.stdin() = snapshot.1.clone();

                                if let Some(pos) = self.config.starting_position {
                                    bf_state.debug_set_position(pos);
                                }
                            };

                            if !self.config.hide_extra {
                                checkbox_with_underline(ui, follow, "Follow").on_hover_text(
                                    "Makes the camera follow the instruction pointer automatically",
                                );

                                ui.add(egui::Slider::new(speed, 1..=20).text("speed"));
                            }
                        });

                        if !watch_list.is_empty() {
                            ui.horizontal(|ui| {
                                for x in watch_list.iter() {
                                    ui.group(|ui| {
                                        ui.add(egui::Label::new(
                                            RichText::new(format!("({:03}, {:03})", x.0, x.1))
                                                .text_style(TextStyle::Monospace),
                                        ));
                                        ui.label(format!("{}", bf_state.get(*x)));
                                    });
                                }
                            });
                        }

                        if self.settings.display_debug_info {
                            ui.separator();
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.label("execution state");
                                    ui.label(format!("{:?}", error_state));
                                });
                                ui.vertical(|ui| {
                                    ui.label("step");
                                    ui.label(bf_state.instruction_count().to_string());
                                });
                                ui.vertical(|ui| {
                                    ui.label("location");
                                    ui.label(format!("{:?}", bf_state.cursor_positions()));
                                });
                                ui.vertical(|ui| {
                                    ui.label("direction");
                                    ui.label(format!("{:?}", bf_state.cursor_direction()));
                                });
                                ui.vertical(|ui| {
                                    ui.label("string mode");
                                    ui.label(format!("{:?}", bf_state.string_mode()));
                                });
                            });
                        }
                    }
                    Mode::Editing {
                        cursor_mode,
                        undos,
                        redos,
                        fungespace,
                        ..
                    } => {
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(
                                    !undos.is_empty(),
                                    egui::Button::new(icon!(icons::ICON_UNDO, "Undo"))
                                        .shortcut_text(shortcut!(SHORTCUT_UNDO)),
                                )
                                .clicked()
                            {
                                Mode::undo(fungespace, undos, redos);
                            };

                            if ui
                                .add_enabled(
                                    !redos.is_empty(),
                                    egui::Button::new(icon!(icons::ICON_REDO, "Redo"))
                                        .shortcut_text(shortcut!(SHORTCUT_REDO)),
                                )
                                .clicked()
                            {
                                Mode::redo(fungespace, undos, redos);
                            };
                            ui.separator();
                            match cursor_mode {
                                CursorMode::Caret(caret) => {
                                    ui.label("Cursor direction:");
                                    ui.label(match caret.direction {
                                        Direction::North => "⬆",
                                        Direction::South => "⬇",
                                        Direction::East => "➡",
                                        Direction::West => "⬅",
                                    });

                                    ui.label("Cursor mode:");
                                    ui.label(if caret.string_mode {
                                        "String"
                                    } else {
                                        "Normal"
                                    });
                                }
                                CursorMode::Span(CursorSpan::Full { start, end, .. }) => {
                                    ui.label("Selection:");
                                    display_coordinate(ui, *start);
                                    ui.label("to");
                                    display_coordinate(ui, *end);
                                }
                                CursorMode::Span(CursorSpan::Partial(start)) => {
                                    ui.label("Selection:");
                                    display_coordinate(ui, *start);
                                    ui.label("to");
                                    display_coordinate(ui, self.cursor_pos);
                                }
                            }
                        });
                    }
                }
            }

            ui.add_space(3.0);

            egui::Frame::group(ui.style())
                .inner_margin(0.0)
                .show(ui, |ui| {
                    ui.set_min_height(100.0);

                    self.befunge_scene(ui);
                    if self.open_modal.is_none() && ui.memory(|mem| mem.focused().is_none()) {
                        self.befunge_input(ui);
                    }
                });
        });
    }
}

impl App {
    fn befunge_input(&mut self, ui: &mut egui::Ui) {
        puffin::profile_function!();

        let mut clipboard_string = None;

        ui.input_mut(|e| {
            if e.consume_shortcut(&SHORTCUT_SWAP_MODE) {
                self.mode.swap_mode(&self.settings);
            }

            match &mut self.mode {
                Mode::Playing {
                    bf_state,
                    running,
                    snapshot,
                    error_state,
                    follow,
                    run_and_exit,
                    ..
                } => {
                    if e.consume_key(Modifiers::NONE, egui::Key::R) {
                        *running = false;
                        *error_state = None;
                        // teeny bit wasteful
                        let breakpoints = bf_state.breakpoints().clone();
                        **bf_state = match self.settings.befunge_version {
                            BefungeVersionDiscriminants::Befunge93 => BefungeVersion::Befunge93(
                                befunge93::State::new_from_fungespace(snapshot.0.clone()),
                            ),
                            BefungeVersionDiscriminants::Befunge93Mini => {
                                BefungeVersion::Befunge93Mini(
                                    befunge93mini::State::new_from_fungespace(snapshot.0.clone()),
                                )
                            }
                            BefungeVersionDiscriminants::Befunge98 => BefungeVersion::Befunge98(
                                befunge98::State::new_from_fungespace(snapshot.0.clone()),
                            ),
                        };
                        *bf_state.breakpoints() = breakpoints;
                        *bf_state.stdin() = snapshot.1.clone();

                        if let Some(pos) = self.config.starting_position {
                            bf_state.debug_set_position(pos);
                        }
                    }

                    if e.consume_key(Modifiers::NONE, egui::Key::F) {
                        *follow = !(*follow);
                    }

                    if error_state.is_none() {
                        if e.consume_key(Modifiers::NONE, egui::Key::Space) {
                            *running = !(*running);
                        }

                        if e.consume_key(Modifiers::NONE, egui::Key::ArrowRight) {
                            *running = false;
                            Mode::step_befunge_inner(
                                bf_state,
                                running,
                                error_state,
                                *run_and_exit,
                                &self.settings,
                            );
                        }
                    }
                }
                Mode::Editing {
                    cursor_mode,
                    fungespace,
                    undos,
                    redos,
                    ..
                } => {
                    let direction = if e.consume_key(Modifiers::NONE, egui::Key::ArrowDown) {
                        Some(Direction::South)
                    } else if e.consume_key(Modifiers::NONE, egui::Key::ArrowUp) {
                        Some(Direction::North)
                    } else if e.consume_key(Modifiers::NONE, egui::Key::ArrowLeft) {
                        Some(Direction::West)
                    } else if e.consume_key(Modifiers::NONE, egui::Key::ArrowRight) {
                        Some(Direction::East)
                    } else {
                        None
                    };
                    match cursor_mode {
                        CursorMode::Caret(caret) => {
                            if let Some(direction) = direction {
                                caret.direction = direction;
                                caret.step(&self.settings);
                            };

                            if e.consume_key(Modifiers::NONE, egui::Key::Backspace) {
                                caret.step_cursor_back(&self.settings);
                            }
                        }
                        CursorMode::Span(CursorSpan::Full { start, end, .. }) => {
                            if let Some(direction) = direction {
                                let start2 = step(*start, direction, &self.settings);
                                let end2 = step(*end, direction, &self.settings);
                                if *start != start2 && *end != end2 {
                                    *start = start2;
                                    *end = end2;
                                }
                            };
                        }
                        _ => (),
                    }

                    if e.key_pressed(egui::Key::Escape) {
                        Mode::reset_cursor(cursor_mode, fungespace, undos, redos);
                    }

                    let save_as = e.consume_shortcut(&SHORTCUT_SAVE_AS);
                    let save = e.consume_shortcut(&SHORTCUT_SAVE);
                    let reload = e.consume_shortcut(&SHORTCUT_RELOAD_FILE);
                    // on wasm FileHandles are EITHER read or write + reusing them doesn't work great
                    // anyways so don't reuse on wasm
                    let can_refresh_save = matches!(self.file, Some(FileType::Handle(_)))
                        && cfg!(not(target_arch = "wasm32"));

                    // duplicate of code in the File menu :/
                    if save_as || (save && !can_refresh_save) {
                        let sender = self.text_channel.0.clone();
                        let mut task = rfd::AsyncFileDialog::new();
                        if let Some(file) = &self.file {
                            task = task.set_file_name(file.file_name());
                        }

                        let task = task.save_file();
                        let contents = fungespace.serialize();

                        match contents {
                            Ok(contents) => {
                                let ctx = ui.ctx().clone();
                                execute(async move {
                                    let file = task.await;
                                    if let Some(file) = file {
                                        if file.write(contents.as_bytes()).await.is_ok() {
                                            let _ = sender.send((file, None));
                                        }
                                        ctx.request_repaint();
                                    }
                                });
                            }
                            Err(err) => self.open_modal = Some(ModalState::SaveFailure(err)),
                        }
                    }

                    if can_refresh_save
                        && save
                        && let Some(FileType::Handle(file)) = self.file.clone()
                    {
                        let sender = self.text_channel.0.clone();
                        let contents = fungespace.serialize();

                        match contents {
                            Ok(contents) => {
                                let ctx = ui.ctx().clone();
                                execute(async move {
                                    if file.write(contents.as_bytes()).await.is_ok() {
                                        let _ = sender.send((file, None));
                                    }
                                    ctx.request_repaint();
                                });
                            }
                            Err(err) => self.open_modal = Some(ModalState::SaveFailure(err)),
                        }
                    }

                    if can_refresh_save
                        && reload
                        && let Some(FileType::Handle(file)) = &self.file
                    {
                        let sender = self.text_channel.0.clone();
                        let ctx = ui.ctx().clone();
                        let file = file.clone();
                        execute(async move {
                            let text = file.read().await;
                            let _ = sender.send((file, Some(text)));
                            ctx.request_repaint();
                        });
                    }

                    if e.consume_shortcut(&SHORTCUT_REDO) || e.consume_shortcut(&SHORTCUT_REDO_ALT)
                    {
                        Mode::redo(fungespace, undos, redos);
                    }

                    if e.consume_shortcut(&SHORTCUT_UNDO) {
                        Mode::undo(fungespace, undos, redos);
                    }

                    enum CursorChange {
                        Drop,
                        Clear,
                        None,
                    }

                    let mut reset_cursor = CursorChange::None;
                    match cursor_mode {
                        CursorMode::Span(CursorSpan::Full {
                            start, end, data, ..
                        }) => {
                            for event in e.filtered_events(&egui::EventFilter {
                                tab: true,
                                escape: false,
                                horizontal_arrows: true,
                                vertical_arrows: true,
                            }) {
                                match event {
                                    egui::Event::Copy => {
                                        let mut str = String::new();
                                        for y in 0..=end.1 - start.1 {
                                            for x in 0..=end.0 - start.0 {
                                                let val =
                                                    data[(x + y * (end.0 - start.0 + 1)) as usize];
                                                str.push(u32::try_into(val as u32).unwrap());
                                            }
                                            str.push('\n');
                                        }
                                        str.pop();
                                        clipboard_string = Some(str);
                                        reset_cursor = CursorChange::Drop;
                                    }
                                    egui::Event::Cut => {
                                        let mut str = String::new();
                                        for y in 0..=end.1 - start.1 {
                                            for x in 0..=end.0 - start.0 {
                                                let val =
                                                    data[(x + y * (end.0 - start.0 + 1)) as usize];
                                                str.push(u32::try_into(val as u32).unwrap());
                                            }
                                            str.push('\n');
                                        }
                                        str.pop();
                                        clipboard_string = Some(str);

                                        reset_cursor = CursorChange::Clear;
                                    }
                                    _ => (),
                                }
                            }
                        }
                        CursorMode::Caret(caret) => {
                            for event in e.filtered_events(&egui::EventFilter {
                                tab: true,
                                escape: false,
                                horizontal_arrows: true,
                                vertical_arrows: true,
                            }) {
                                match event {
                                    egui::Event::Text(text) => {
                                        let mut ops = vec![];
                                        for char in text.chars() {
                                            ops.push((
                                                caret.location,
                                                fungespace.get(caret.location),
                                            ));
                                            fungespace.set(caret.location, char as i64);

                                            if char == '"' {
                                                caret.string_mode = !caret.string_mode;
                                            };

                                            if !caret.string_mode {
                                                match char {
                                                    '>' => caret.direction = Direction::East,
                                                    'v' => caret.direction = Direction::South,
                                                    '<' => caret.direction = Direction::West,
                                                    '^' => caret.direction = Direction::North,
                                                    _ => (),
                                                }
                                            }

                                            caret.step(&self.settings);
                                        }

                                        undos.push(Undo::Normal(ops.into()));
                                        redos.clear();
                                    }
                                    egui::Event::Paste(text) => {
                                        let (mut x, mut y) = caret.location;
                                        let mut ops = vec![];
                                        for char in text.chars() {
                                            if char == '\n' {
                                                y += 1;
                                                x = caret.location.0;
                                                continue;
                                            };
                                            ops.push(((x, y), fungespace.get((x, y))));
                                            fungespace.set((x, y), char as i64);
                                            x += 1
                                        }
                                        undos.push(Undo::Normal(ops.into()));
                                        redos.clear();
                                    }
                                    _ => (),
                                }
                            }
                        }
                        _ => (),
                    }

                    match reset_cursor {
                        CursorChange::Drop => {
                            Mode::reset_cursor(cursor_mode, fungespace, undos, redos)
                        }
                        CursorChange::Clear => {
                            let pos = *match cursor_mode {
                                CursorMode::Span(CursorSpan::Partial(pos)) => pos,
                                CursorMode::Caret(CursorCaret { location, .. }) => location,
                                CursorMode::Span(CursorSpan::Full {
                                    start,
                                    end,
                                    data,
                                    original_start,
                                }) => {
                                    let mut ops = vec![];
                                    for y in 0..=end.1 - start.1 {
                                        for x in 0..=end.0 - start.0 {
                                            let val =
                                                data[(x + y * (end.0 - start.0 + 1)) as usize];
                                            ops.push((
                                                (x + original_start.0, y + original_start.1),
                                                val,
                                            ));
                                        }
                                    }
                                    undos.push(Undo::Normal(ops.into()));
                                    redos.clear();

                                    start
                                }
                            };
                            *cursor_mode = CursorMode::caret(pos);
                        }
                        _ => (),
                    }
                }
            }
        });

        if let Some(clipboard_string) = clipboard_string {
            ui.copy_text(clipboard_string);
        }
    }

    fn befunge_scene(&mut self, ui: &mut egui::Ui) {
        puffin::profile_function!();
        let mut scene = Scene::new()
            .max_inner_size([f32::INFINITY, f32::INFINITY])
            .zoom_range(0.01..=5.0);

        let available_rect = ui.available_rect_before_wrap();

        if let Mode::Playing {
            follow: true,
            bf_state,
            ..
        } = &self.mode
        {
            const CENTER: Pos2 = poss((0.5, 0.5));
            // TODO: zoom out (and back in?) if cursors cannot all fit on screen
            // Probably need to make focus more than just a boolean, so user can pick a specific
            // cursor to follow, or possibly a set of them as well as an option for all of em
            for cursor_pos in bf_state.cursor_positions() {
                let left_top = poss_reverse(self.scene_rect.left_top(), self.scene_offset);
                let left_top = (left_top.0 + 2, left_top.1 + 2);
                let right_bottom = poss_reverse(self.scene_rect.right_bottom(), self.scene_offset);
                let right_bottom = (right_bottom.0 - 2, right_bottom.1 - 2);

                let width = right_bottom.0 - left_top.0;
                let height = right_bottom.1 - left_top.1;

                if cursor_pos.1 > right_bottom.1 {
                    self.scene_rect.set_center(CENTER);
                    self.scene_offset = (self.scene_offset.0, cursor_pos.1 - height / 2);
                }
                if cursor_pos.0 > right_bottom.0 {
                    self.scene_rect.set_center(CENTER);
                    self.scene_offset = (cursor_pos.0 - width / 2, self.scene_offset.1);
                }

                if cursor_pos.1 < left_top.1 {
                    self.scene_rect.set_center(CENTER);
                    self.scene_offset = (self.scene_offset.0, cursor_pos.1 + height / 2);
                }
                if cursor_pos.0 < left_top.0 {
                    self.scene_rect.set_center(CENTER);
                    self.scene_offset = (cursor_pos.0 + width / 2, self.scene_offset.1);
                }
            }
        } else if self.config.fix_camera_to_program {
            // TODO: disable zooming
            self.scene_offset = (0, 0);
            let program_size = match &self.mode {
                Mode::Playing { bf_state, .. } => bf_state.program_size(),
                Mode::Editing { fungespace, .. } => fungespace.program_size(),
            };

            self.scene_rect = Rect::from_min_max(
                Pos2::new(-13.0, -17.0),
                poss(((program_size.0 + 2) as f32, (program_size.1 + 2) as f32)),
            );
            ui.input_mut(|input| {
                input.smooth_scroll_delta = Vec2::ZERO;
            });
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

        let scene_response = scene
            .show(ui, &mut self.scene_rect, |ui| {
                let painter = ui.painter();
                let clip_rect = painter.clip_rect();

                // Grid dots
                {
                    puffin::profile_scope!("grid dots");

                    let border_pos = self.settings.befunge_version.border_positions();

                    // TODO: remove overlap of bottom/right dots with border line
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
                                    (border_pos.1.1.saturating_sub(self.scene_offset.0)) as f32 * 13.0,
                                ) {
                                    break;
                                };
                                x += 13.0;
                            }
                            if y > f32::min(
                                clip_rect.bottom(),
                                (border_pos.1.1.saturating_sub(self.scene_offset.1)) as f32 * 17.0,
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

                    let border_pos = self.settings.befunge_version.border_positions();

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
                                    ((border_pos.1.1.saturating_sub(self.scene_offset.0)) as f32 + 1.0)
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
                                0.5 - ((self.scene_offset.1 - border_pos.1.1 - 1) as f32) * 17.0,
                            ),
                            Pos2::new(
                                f32::min(
                                    clip_rect.right(),
                                    ((border_pos.1.1.saturating_sub(self.scene_offset.0)) as f32 + 1.0)
                                        * 13.0,
                                ),
                                0.5 - ((self.scene_offset.1 - border_pos.1.1 - 1) as f32) * 17.0,
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
                                    ((border_pos.1.1.saturating_sub(self.scene_offset.1)) as f32 + 1.0)
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
                                0.5 - ((self.scene_offset.0 - border_pos.1.1 - 1) as f32) * 13.0,
                                f32::max(
                                    clip_rect.top(),
                                    -1.0 - (self.scene_offset.1 as f32) * 17.0,
                                ),
                            ),
                            Pos2::new(
                                0.5 - ((self.scene_offset.0 - border_pos.1.1 - 1) as f32) * 13.0,
                                f32::min(
                                    clip_rect.bottom(),
                                    ((border_pos.1.1.saturating_sub(self.scene_offset.1)) as f32 + 1.0)
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
                            Stroke::new(1.0, Color32::RED),
                        );

                        painter.line_segment(
                            [
                                Pos2::new(0.0, clip_rect.top()),
                                Pos2::new(0.0, clip_rect.bottom()),
                            ],
                            Stroke::new(1.0, Color32::RED),
                        );
                    }
                }

                {
                    puffin::profile_scope!("history heatmap");
                    match &mut self.mode {
                        Mode::Playing { bf_state, .. } => {
                            // TODO: move this somewhere more sensible
                            let now = Instant::now();
                            bf_state
                                .pos_history()
                                .retain(|_, v| v.time_since(now) < Duration::from_millis(5000));

                            bf_state
                                .put_history()
                                .retain(|_, v| v.elapsed() < Duration::from_millis(5000));

                            bf_state
                                .get_history()
                                .retain(|_, v| v.elapsed() < Duration::from_millis(5000));

                            for pos in bf_state.cursor_positions() {
                                painter.rect(
                                    recter(pos, self.scene_offset).shrink(1.0),
                                    0.0,
                                    Color32::PURPLE,
                                    Stroke::NONE,
                                    StrokeKind::Outside,
                                );
                            }

                            for (pos, visited) in bf_state.pos_history() {
                                let time = (visited.time_since(now).as_millis() as f32) / 1000.0;

                                if let Some(mult) = calculate_decay(time) {
                                    let rect = recter(*pos, self.scene_offset);
                                    let pos = poss((
                                        (pos.0 - self.scene_offset.0) as f32,
                                        (pos.1 - self.scene_offset.1) as f32,
                                    ));
                                    let [r, g, b] = self.settings.pos_history.1;

                                    painter.rect(
                                        rect.shrink(1.0),
                                        0.0,
                                        Color32::from_rgb(r, g, b).gamma_multiply(mult),
                                        Stroke::NONE,
                                        StrokeKind::Outside,
                                    );

                                    if visited.wawa.north()
                                        && let Some(mult) = calculate_decay(
                                            now.duration_since(visited.north).as_millis() as f32
                                                / 1000.0,
                                        )
                                    {
                                        painter.rect(
                                            Rect::from_min_max(
                                                pos + Vec2::new(1.0, -1.0),
                                                pos + Vec2::new(12.0, 1.0),
                                            ),
                                            0.0,
                                            Color32::from_rgb(r, g, b).gamma_multiply(mult),
                                            Stroke::NONE,
                                            StrokeKind::Outside,
                                        );
                                    }

                                    if visited.wawa.south()
                                        && let Some(mult) = calculate_decay(
                                            now.duration_since(visited.south).as_millis() as f32
                                                / 1000.0,
                                        )
                                    {
                                        painter.rect(
                                            Rect::from_min_max(
                                                pos + Vec2::new(1.0, 16.0),
                                                pos + Vec2::new(12.0, 18.0),
                                            ),
                                            0.0,
                                            Color32::from_rgb(r, g, b).gamma_multiply(mult),
                                            Stroke::NONE,
                                            StrokeKind::Outside,
                                        );
                                    }

                                    if visited.wawa.east()
                                        && let Some(mult) = calculate_decay(
                                            now.duration_since(visited.east).as_millis() as f32
                                                / 1000.0,
                                        )
                                    {
                                        painter.rect(
                                            Rect::from_min_max(
                                                pos + Vec2::new(12.0, 1.0),
                                                pos + Vec2::new(14.0, 16.0),
                                            ),
                                            0.0,
                                            Color32::from_rgb(r, g, b).gamma_multiply(mult),
                                            Stroke::NONE,
                                            StrokeKind::Outside,
                                        );
                                    }

                                    if visited.wawa.west()
                                        && let Some(mult) = calculate_decay(
                                            now.duration_since(visited.west).as_millis() as f32
                                                / 1000.0,
                                        )
                                    {
                                        painter.rect(
                                            Rect::from_min_max(
                                                pos + Vec2::new(-1.0, 1.0),
                                                pos + Vec2::new(1.0, 16.0),
                                            ),
                                            0.0,
                                            Color32::from_rgb(r, g, b).gamma_multiply(mult),
                                            Stroke::NONE,
                                            StrokeKind::Outside,
                                        );
                                    }
                                }
                            }

                            for (pos, instant) in bf_state.put_history() {
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

                            for (pos, instant) in bf_state.get_history() {
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

                            for pos in bf_state.breakpoints().iter() {
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
                        Mode::Editing { cursor_mode, .. } => {
                            match cursor_mode {
                                CursorMode::Caret(caret) => {
                                    painter.rect(
                                        recter(caret.location, self.scene_offset),
                                        0.0,
                                        if caret.string_mode {
                                            Color32::LIGHT_GREEN
                                        } else {
                                            CURSOR_COLOR
                                        },
                                        Stroke::new(0.25, Color32::from_gray(90)),
                                        StrokeKind::Inside,
                                    );

                                    let mut cursor_copy = *caret;
                                    cursor_copy.step(&self.settings);

                                    painter.rect(
                                        recter(cursor_copy.location, self.scene_offset),
                                        0.0,
                                        if caret.string_mode {
                                            Color32::LIGHT_GREEN
                                        } else {
                                            CURSOR_COLOR
                                        }
                                        .gamma_multiply_u8(80),
                                        Stroke::NONE,
                                        StrokeKind::Inside,
                                    );
                                }
                                CursorMode::Span(CursorSpan::Partial(pos)) => {
                                    let rect_a = recter(*pos, self.scene_offset);
                                    let rect_b = recter(self.cursor_pos, self.scene_offset);
                                    let rect = rect_a.union(rect_b);

                                    ui.painter().rect(rect, 0.0, CURSOR_COLOR.gamma_multiply(0.3), Stroke::NONE, StrokeKind::Inside);

                                    ui.painter().add(Shape::dashed_line(
                                        &[
                                            rect.left_top(),
                                            rect.right_top(),
                                            rect.right_bottom(),
                                            rect.left_bottom(),
                                            rect.left_top(),
                                        ],
                                        Stroke::new(0.75, CURSOR_COLOR),
                                        1.0,
                                        1.0,
                                    ));
                                }
                                // Full selection dealt with lower down
                                _ => ()
                            }
                        }
                    };
                }

                {
                    puffin::profile_scope!("chars");

                    let mut mesh = egui::Mesh::with_texture(egui::TextureId::default());

                    let border_pos = self.settings.befunge_version.border_positions();
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

                    if integer_clip_rect.0.0 < i64::MIN + 100000 {
                        integer_clip_rect.0.0 = i64::MAX;
                    }

                    if integer_clip_rect.0.1 < i64::MIN + 100000 {
                        integer_clip_rect.0.1 = i64::MAX;
                    }

                    integer_clip_rect = (
                        (
                            integer_clip_rect.0.0.max(border_pos.0.0),
                            integer_clip_rect.0.1.max(border_pos.0.1)
                        ),
                        (
                            integer_clip_rect.1.0.min(border_pos.1.0),
                            integer_clip_rect.1.1.min(border_pos.1.1)
                        ),
                    );

                    for x in integer_clip_rect.0.0..=integer_clip_rect.1.0 {
                        for y in integer_clip_rect.0.1..=integer_clip_rect.1.1 {
                            let pos = recter((x, y), self.scene_offset);
                            let val = match &mut self.mode {
                                Mode::Playing { bf_state, .. } => bf_state.get((x, y)),
                                Mode::Editing { fungespace, .. } => fungespace.get((x, y)),
                            };
                            if val != b' ' as i64
                            {
                                App::draw_char(
                                    ui,
                                    &self.char_renderer,
                                    &mut mesh,
                                    &self.settings,
                                    pos,
                                    val,
                                );
                            }
                        }
                    }

                    ui.painter().add(egui::Shape::Mesh(mesh.into()));
                }

                if let Mode::Editing{cursor_mode, ..} = &mut self.mode && let CursorMode::Span(CursorSpan::Full{start,end,data,original_start}) = cursor_mode {
                    let rect_a = recter(*start, self.scene_offset);
                    let rect_b = recter(*end, self.scene_offset);
                    let rect = rect_a.union(rect_b);

                    ui.painter().rect(rect, 0.0, ui.visuals().window_fill, Stroke::NONE, StrokeKind::Inside);
                    ui.painter().rect(rect, 0.0, CURSOR_COLOR.gamma_multiply(0.3), Stroke::NONE, StrokeKind::Inside);

                    ui.painter().add(Shape::dashed_line(
                        &[
                            rect.left_top(),
                            rect.right_top(),
                            rect.right_bottom(),
                            rect.left_bottom(),
                            rect.left_top(),
                        ],
                        Stroke::new(0.75, CURSOR_COLOR),
                        1.0,
                        1.0,
                    ));

                    let mut mesh = egui::Mesh::with_texture(egui::TextureId::default());

                    for y in 0..=end.1-start.1 {
                        for x in 0..=end.0-start.0 {
                            let pos = recter((x+start.0, y+start.1), self.scene_offset);
                            let val = data[(x + y*(end.0-start.0+1)) as usize];
                            if val != b' ' as i64
                            {
                                App::draw_char(
                                    ui,
                                    &self.char_renderer,
                                    &mut mesh,
                                    &self.settings,
                                    pos,
                                    val,
                                );
                            }
                        }
                    }

                    ui.painter().add(egui::Shape::Mesh(mesh.into()));
                }

                if let Some(popup_pos) = self.popup_pos {
                    puffin::profile_scope!("popup");
                    let transform = ui
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
                            Mode::Playing { bf_state, watch_list, .. } => {
                                Self::dual_char_and_numeric_input(
                                    ui,
                                    bf_state.get(popup_pos),
                                    |val| bf_state.set(popup_pos, val),
                                );

                                let mut breakpoint = bf_state.breakpoints().contains(&popup_pos);
                                let mut watched = watch_list.contains(&popup_pos);

                                if ui.memory(|mem| mem.focused().is_none()) {
                                    ui.input_mut(|e| {
                                        if e.consume_key(Modifiers::NONE, egui::Key::B) {
                                            if breakpoint {
                                                bf_state.breakpoints().remove(&popup_pos);
                                            } else {
                                                bf_state.breakpoints().insert(popup_pos);
                                            }
                                            breakpoint = !breakpoint;
                                        }

                                        if e.consume_key(Modifiers::NONE, egui::Key::W) {
                                            if watched {
                                                watch_list.remove(&popup_pos);
                                            } else {
                                                watch_list.insert(popup_pos);
                                            }
                                            watched = !watched;
                                        }
                                    });
                                }

                                if checkbox_with_underline(ui, &mut breakpoint, "Breakpoint")
                                    .clicked()
                                {
                                    if breakpoint {
                                        bf_state.breakpoints().insert(popup_pos);
                                    } else {
                                        bf_state.breakpoints().remove(&popup_pos);
                                    }
                                };

                                if checkbox_with_underline(ui, &mut watched, "Watched")
                                    .clicked()
                                {
                                    if watched {
                                        watch_list.insert(popup_pos);
                                    } else {
                                        watch_list.remove(&popup_pos);
                                    }
                                };
                            }
                            Mode::Editing {
                                fungespace,
                                undos,
                                redos,
                                ..
                            } => {
                                let chr = fungespace.get(popup_pos);
                                Self::dual_char_and_numeric_input(
                                    ui,
                                    chr,
                                    |val| {
                                        // if previous undo was just setting this exact value,
                                        // don't update the undolist
                                        if !matches!(undos.last(), Some(Undo::Dedupable(prev_pos, _)) if *prev_pos == popup_pos) {
                                            undos.push(
                                                Undo::Dedupable(popup_pos, chr),
                                            );
                                        }
                                        redos.clear();
                                        fungespace.set(popup_pos, val)
                                    },
                                );
                            }
                        };
                    });

                    if let Some(popup) = popup
                        && popup.response.should_close()
                    {
                        self.popup_pos = None;
                    }
                }

                let cursor_op = match &self.mode {
                    Mode::Editing { fungespace, .. } => fungespace.get(self.cursor_pos),
                    Mode::Playing { bf_state, .. } => bf_state.get(self.cursor_pos)
                }.try_into();
                if self.befunge_tooltip_open && self.popup_pos.is_none() && let Ok(cursor_op) = cursor_op && cursor_op != b' ' {
                    puffin::profile_scope!("tooltip");
                    let transform = ui
                        .layer_transform_to_global(ui.layer_id())
                        .unwrap_or(TSTransform::IDENTITY);
                    let description = match self.settings.befunge_version {
                        BefungeVersionDiscriminants::Befunge93 | BefungeVersionDiscriminants::Befunge93Mini => bf93_op_info(cursor_op),
                        BefungeVersionDiscriminants::Befunge98 => bf98_op_info(cursor_op)
                    };

                    if let Some(description) = description {
                        egui::Popup::new(
                            Id::new("befunge tooltip"),
                            ui.ctx().clone(),
                            transform.mul_pos(poss((
                                (self.cursor_pos.0 - self.scene_offset.0) as f32 + 0.75,
                                (self.cursor_pos.1 - self.scene_offset.1) as f32 + 0.75,
                            ))),
                            LayerId::background(),
                        ).show(|ui| {
                            ui.set_max_width(250.0);
                            ui.label(description);
                        });
                    }

                }

            })
            .response;

        if scene_response.contains_pointer()
            && let Some(pos) = scene_response.hover_pos()
        {
            let pos = poss_reverse(pos, self.scene_offset);
            let border_pos = self.settings.befunge_version.border_positions();
            if intersects(border_pos, pos) {
                if self.cursor_pos == pos && self.settings.display_befunge_tooltips {
                    if should_show_tooltip(&scene_response, self.befunge_tooltip_open) {
                        self.befunge_tooltip_open = true;
                        ui.request_repaint()
                    }
                } else {
                    self.befunge_tooltip_open = false;
                }
                self.cursor_pos = pos;
            } else {
                self.befunge_tooltip_open = false;
            }
        };

        if let Mode::Editing {
            cursor_mode,
            fungespace,
            undos,
            redos,
            ..
        } = &mut self.mode
        {
            if ui.input(|i| i.modifiers.alt)
                || matches!(cursor_mode, CursorMode::Span(CursorSpan::Partial(_)))
            {
                let overlay_response = {
                    let mut response = None;
                    ui.with_layer_id(
                        egui::LayerId::new(
                            egui::Order::Foreground,
                            ui.id().with("span_drag_overlay"),
                        ),
                        |ui| {
                            //ui.painter().rect_filled(available_rect, 0.0, Color32::ORANGE.gamma_multiply(0.1));
                            response = Some(ui.interact(
                                available_rect,
                                ui.id().with("span_drag_overlay"),
                                egui::Sense::drag(),
                            ));
                        },
                    );
                    response.unwrap()
                };

                // sure hope this is safe to unwrap...
                let transform = ui
                    .layer_transform_from_global(scene_response.layer_id)
                    .unwrap();

                if overlay_response.contains_pointer()
                    && let Some(pos) = overlay_response.hover_pos()
                {
                    let pos = transform.mul_pos(pos);
                    let pos = poss_reverse(pos, self.scene_offset);
                    let border_pos = self.settings.befunge_version.border_positions();
                    if intersects(border_pos, pos) {
                        if self.cursor_pos == pos && self.settings.display_befunge_tooltips {
                            if should_show_tooltip(&overlay_response, self.befunge_tooltip_open) {
                                self.befunge_tooltip_open = true;
                                ui.request_repaint()
                            }
                        } else {
                            self.befunge_tooltip_open = false;
                        }
                        self.cursor_pos = pos;
                    } else {
                        self.befunge_tooltip_open = false;
                    }
                };

                if overlay_response.drag_started()
                    && let Some(pos) = overlay_response.interact_pointer_pos()
                {
                    let pos = transform.mul_pos(pos);
                    let pos = poss_reverse(pos, self.scene_offset);
                    let border_pos = self.settings.befunge_version.border_positions();
                    if intersects(border_pos, pos) {
                        Mode::set_cursor(
                            cursor_mode,
                            fungespace,
                            undos,
                            redos,
                            CursorMode::Span(CursorSpan::Partial(pos)),
                        );
                    }
                }

                if overlay_response.drag_stopped()
                    && let CursorMode::Span(CursorSpan::Partial(pos)) = cursor_mode
                {
                    let pos2 = self.cursor_pos;
                    let (start, end) = (
                        (pos.0.min(pos2.0), pos.1.min(pos2.1)),
                        (pos.0.max(pos2.0), pos.1.max(pos2.1)),
                    );

                    let mut data = vec![];
                    for y in start.1..=end.1 {
                        for x in start.0..=end.0 {
                            data.push(fungespace.get((x, y)));
                            fungespace.set((x, y), b' ' as i64);
                        }
                    }
                    Mode::set_cursor(
                        cursor_mode,
                        fungespace,
                        undos,
                        redos,
                        CursorMode::Span(CursorSpan::Full {
                            start,
                            end,
                            data,
                            original_start: start,
                        }),
                    );
                };
            }

            if scene_response.clicked()
                && let Some(pos) = scene_response.interact_pointer_pos()
            {
                let pos = poss_reverse(pos, self.scene_offset);

                let border_pos = self.settings.befunge_version.border_positions();
                if intersects(border_pos, pos) {
                    match cursor_mode {
                        CursorMode::Caret(caret) => caret.location = pos,
                        CursorMode::Span(_) => {
                            Mode::set_cursor(
                                cursor_mode,
                                fungespace,
                                undos,
                                redos,
                                CursorMode::caret(pos),
                            );
                        }
                    }
                }
            };
        }

        if scene_response.secondary_clicked()
            && let Some(pos) = scene_response.interact_pointer_pos()
        {
            let pos = poss_reverse(pos, self.scene_offset);
            if pos.0 >= 0 && pos.1 >= 0 {
                self.popup_pos = Some(pos);
            }
        };
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        puffin::profile_function!();

        let ctx = ui.ctx().clone();
        macro_rules! shortcut {
            ($icon:expr) => {
                $icon.format(&SYMBOLS, ctx.os().is_mac())
            };
        }

        egui::MenuBar::new().ui(ui, |ui| {
            let is_web = cfg!(target_arch = "wasm32");
            ui.menu_button("File", |ui| {
                if ui.button("📄 New").clicked() {
                    self.file = None;
                    self.mode = Mode::Editing {
                        undos: Vec::new(),
                        redos: Vec::new(),
                        cursor_mode: CursorMode::default(),
                        fungespace: FungeSpace::default(),
                        stdin: String::new(),
                    }
                }
                if ui.button("📂 Open").clicked() {
                    let sender = self.text_channel.0.clone();
                    let task = rfd::AsyncFileDialog::new().pick_file();

                    let ctx = ui.ctx().clone();
                    execute(async move {
                        let file = task.await;
                        if let Some(file) = file {
                            let text = file.read().await;
                            let _ = sender.send((file, Some(text)));
                            ctx.request_repaint();
                        }
                    });
                }

                let save = ui
                    .add(egui::Button::new("💾 Save").shortcut_text(shortcut!(SHORTCUT_SAVE)))
                    .clicked();
                let save_as = ui
                    .add(egui::Button::new("💾 Save As").shortcut_text(shortcut!(SHORTCUT_SAVE_AS)))
                    .clicked();
                let reload = ui
                    .add_enabled(
                        matches!(self.file, Some(FileType::Handle(_))),
                        egui::Button::new(icon!(icons::ICON_REPLAY, "Reload"))
                            .shortcut_text(shortcut!(SHORTCUT_RELOAD_FILE)),
                    )
                    .clicked();
                // on wasm FileHandles are EITHER read or write + reusing them doesn't work great
                // anyways so don't reuse on wasm
                let can_refresh_save = matches!(self.file, Some(FileType::Handle(_)))
                    && cfg!(not(target_arch = "wasm32"));

                if save_as || (save && !can_refresh_save) {
                    let sender = self.text_channel.0.clone();
                    let mut task = rfd::AsyncFileDialog::new();
                    if let Some(file) = &self.file {
                        task = task.set_file_name(file.file_name());
                    }

                    let task = task.save_file();
                    let contents = match &mut self.mode {
                        Mode::Playing { bf_state, .. } => bf_state.serialize(),
                        Mode::Editing { fungespace, .. } => fungespace.serialize(),
                    };
                    match contents {
                        Ok(contents) => {
                            let ctx = ui.ctx().clone();
                            execute(async move {
                                let file = task.await;
                                if let Some(file) = file {
                                    _ = file.write(contents.as_bytes()).await;
                                    let _ = sender.send((file, None));
                                    ctx.request_repaint();
                                }
                            });
                        }
                        Err(err) => self.open_modal = Some(ModalState::SaveFailure(err)),
                    }
                }

                if can_refresh_save
                    && save
                    && let Some(FileType::Handle(file)) = self.file.clone()
                {
                    let sender = self.text_channel.0.clone();
                    let contents = match &mut self.mode {
                        Mode::Playing { bf_state, .. } => bf_state.serialize(),
                        Mode::Editing { fungespace, .. } => fungespace.serialize(),
                    };
                    match contents {
                        Ok(contents) => {
                            let ctx = ui.ctx().clone();
                            execute(async move {
                                _ = file.write(contents.as_bytes()).await;
                                let _ = sender.send((file.clone(), None));
                                ctx.request_repaint();
                            });
                        }
                        Err(err) => self.open_modal = Some(ModalState::SaveFailure(err)),
                    }
                }

                if can_refresh_save
                    && reload
                    && let Some(FileType::Handle(file)) = &self.file
                {
                    let sender = self.text_channel.0.clone();
                    let ctx = ui.ctx().clone();
                    let file = file.clone();
                    execute(async move {
                        let text = file.read().await;
                        let _ = sender.send((file, Some(text)));
                        ctx.request_repaint();
                    });
                }

                if !is_web {
                    ui.separator();
                    if ui.add(egui::Button::new("Quit").right_text("❌")).clicked() {
                        ui.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
            });

            if let Some(open_modal) = &mut self.open_modal {
                let modal = Modal::new(Id::new("Settings modal")).show(ui.ctx(), |ui| {
                    ui.set_width(300.0);

                    match open_modal {
                        ModalState::Settings => Self::settings_modal(ui, &mut self.settings),
                        ModalState::SetPosition(x, y) => Self::set_position_modal(ui, x, y),
                        ModalState::Info => Self::info_modal(ui),
                        ModalState::Text(str) => {
                            ui.set_width(600.0);
                            ui.label(RichText::new(str.clone()).font(FontId::monospace(12.0)));
                        }
                        ModalState::SaveFailure(err) => Self::save_failure(ui, err),
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
                        ModalState::Settings | ModalState::Info | ModalState::Text(..) => (),
                        ModalState::SetPosition(x, y) => {
                            self.scene_offset = (x, y);
                            self.scene_rect.set_center(poss((0.5, 0.5)));
                        }
                        ModalState::SaveFailure(err) => match err {
                            SerializationError::InvalidChar(pos)
                            | SerializationError::Newline(pos) => {
                                self.scene_offset = pos;
                                self.scene_rect.set_center(poss((0.5, 0.5)));
                            }
                        },
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

                let settings_button = egui::Button::new("Advanced settings");

                if ui.add(settings_button).clicked() {
                    self.open_modal = Some(ModalState::Settings);
                };
            });

            ui.menu_button("View", |ui| {
                if ui.button("Show whole program").clicked() {
                    self.scene_offset = (0, 0);
                    let program_size = match &self.mode {
                        Mode::Playing { bf_state, .. } => bf_state.program_size(),
                        Mode::Editing { fungespace, .. } => fungespace.program_size(),
                    };

                    self.scene_rect = Rect::from_min_max(
                        Pos2::new(-13.0, -17.0),
                        poss(((program_size.0 + 2) as f32, (program_size.1 + 2) as f32)),
                    );
                };

                ui.checkbox(
                    &mut self.settings.display_befunge_tooltips,
                    "Show befunge tooltips",
                );
            });

            ui.menu_button("Tools", |ui| {
                if !is_web {
                    let mut profile = puffin::are_scopes_on();
                    ui.checkbox(&mut profile, "Enable UI profiling");
                    puffin::set_scopes_on(profile);
                }

                ui.checkbox(&mut self.settings.display_debug_info, "Display debug info");

                if ui.button("Set viewport position").clicked() {
                    self.open_modal = Some(ModalState::SetPosition(0, 0));
                };
            });

            if ui.button("Help").clicked() {
                self.open_modal = Some(ModalState::Info);
            }

            egui::widgets::global_theme_preference_switch(ui);

            ui.separator();

            let mode = match self.mode {
                Mode::Editing { .. } => false,
                Mode::Playing { .. } => true,
            };

            if ui.add(egui::Button::selectable(!mode, "Edit")).clicked() && mode {
                self.mode.swap_mode(&self.settings);
            };

            if ui.add(egui::Button::selectable(mode, "Run")).clicked() && !mode {
                self.mode.swap_mode(&self.settings);
            };

            if let Mode::Playing {
                error_state: Some(error),
                ..
            } = &self.mode
            {
                ui.label(RichText::new(error.to_string()).color(Color32::RED));
            }

            if let Some(file) = &self.file {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    if let Mode::Editing {
                        fungespace: FungeSpace { dirty: true, .. },
                        ..
                    } = &mut self.mode
                    {
                        ui.label("*");
                    }
                    ui.label(file.file_name());
                });
            }
        });
    }

    fn info_panel(&mut self, ui: &mut egui::Ui) {
        puffin::profile_function!();
        match &mut self.mode {
            Mode::Playing {
                bf_state, running, ..
            } => {
                if let Some(graphics) = &mut bf_state.graphics() {
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
                                egui::ColorImage::new(
                                    [graphics.size.0, graphics.size.1],
                                    graphics.texture.clone(),
                                ),
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
                                    .push_back(GraphicalEvent::MouseClick(pixel_pos));
                            }
                        });
                };

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

                ui.label("Stack:");

                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    ui.add_space(2.0);

                    let resp = ui.text_edit_multiline(bf_state.stdin());
                    if resp.changed()
                        && let val = bf_state.get(bf_state.cursor_positions()[0])
                        && (val == b'~' as i64 || val == b'&' as i64)
                    {
                        *running = true
                    }
                    ui.label("Input:");

                    ui.add_space(2.0);

                    ui.allocate_ui_with_layout(
                        Vec2::new(ui.available_width(), 75.0),
                        egui::Layout::top_down(egui::Align::LEFT),
                        |ui| {
                            // change to ::both ?
                            ScrollArea::vertical()
                                .auto_shrink([false; 2])
                                .scroll_bar_visibility(ScrollBarVisibility::AlwaysVisible)
                                .id_salt("wawa")
                                .stick_to_bottom(true)
                                .show(ui, |ui| {
                                    let painter = ui.painter();
                                    painter.rect_filled(
                                        ui.clip_rect(),
                                        5.0,
                                        ui.visuals().faint_bg_color,
                                    );
                                    ui.label(bf_state.stdout())
                                });
                        },
                    );

                    ui.label("Output:");
                    ui.add_space(2.0);

                    ui.vertical(|ui| {
                        let text_style = TextStyle::Body;

                        ScrollArea::vertical()
                            .auto_shrink([false; 2])
                            .scroll_bar_visibility(ScrollBarVisibility::AlwaysVisible)
                            .show_rows(
                                ui,
                                ui.text_style_height(&text_style),
                                bf_state.stack(0).len(),
                                |ui, row_range| {
                                    let painter = ui.painter();
                                    painter.rect_filled(
                                        ui.clip_rect(),
                                        5.0,
                                        ui.visuals().faint_bg_color,
                                    );
                                    for value in row_range {
                                        ui.label(bf_state.stack(0)[value].to_string());
                                    }
                                },
                            );
                    });
                });
            }
            Mode::Editing {
                stdin, fungespace, ..
            } => {
                ui.label("Version:");
                let version = self.settings.befunge_version;
                if ui
                    .add(egui::Button::selectable(
                        matches!(version, BefungeVersionDiscriminants::Befunge93),
                        "64 bit Befunge93",
                    ))
                    .clicked()
                {
                    self.settings.befunge_version = BefungeVersionDiscriminants::Befunge93
                };
                if ui
                    .add(egui::Button::selectable(
                        matches!(version, BefungeVersionDiscriminants::Befunge93Mini),
                        "8 bit Befunge93",
                    ))
                    .clicked()
                {
                    self.settings.befunge_version = BefungeVersionDiscriminants::Befunge93Mini
                };
                if ui
                    .add(egui::Button::selectable(
                        matches!(version, BefungeVersionDiscriminants::Befunge98),
                        "Befunge98 (WIP)",
                    ))
                    .on_hover_text("Works pretty well (almost completely conformant), but can cause crashes and the ui still needs some work.")
                    .clicked()
                {
                    self.settings.befunge_version = BefungeVersionDiscriminants::Befunge98
                };

                ui.label("Presets:");
                for dir in BF_PRESETS.dirs() {
                    Self::preset(
                        ui,
                        ".bf",
                        dir,
                        fungespace,
                        &mut self.open_modal,
                        &mut self.file,
                    );
                }

                match self.settings.befunge_version {
                    BefungeVersionDiscriminants::Befunge93
                    | BefungeVersionDiscriminants::Befunge93Mini => {
                        for dir in BF93_PRESETS.dirs() {
                            Self::preset(
                                ui,
                                ".b93",
                                dir,
                                fungespace,
                                &mut self.open_modal,
                                &mut self.file,
                            );
                        }
                    }

                    BefungeVersionDiscriminants::Befunge98 => {
                        for dir in BF98_PRESETS.dirs() {
                            Self::preset(
                                ui,
                                ".b98",
                                dir,
                                fungespace,
                                &mut self.open_modal,
                                &mut self.file,
                            );
                        }
                    }
                };

                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    ui.add_space(2.0);

                    ui.text_edit_multiline(stdin);
                    ui.label("Input:");
                });
            }
        }
    }

    fn preset(
        ui: &mut egui::Ui,
        ext: &str,
        dir: &Dir,
        fungespace: &mut FungeSpace,
        open_modal: &mut Option<ModalState>,
        open_file: &mut Option<FileType>,
    ) {
        ui.horizontal(|ui| {
            let filename = dir.path().file_stem().unwrap().to_string_lossy();
            let attribution = dir.get_file(filename.to_string() + "/Attribution").unwrap();
            let attribution = "By ".to_string() + attribution.contents_utf8().unwrap().trim();

            let file = dir
                .get_file(filename.to_string() + "/" + &filename + ext)
                .unwrap();

            if ui
                .button(filename.clone().replace("_", " "))
                .on_hover_text(&attribution)
                .clicked()
            {
                *open_file = Some(FileType::Filename(
                    file.path()
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .to_string(),
                ));

                *fungespace = FungeSpace::new_from_string(&String::from_utf8_lossy(file.contents()))
            }

            #[allow(clippy::collapsible_if)]
            if let Some(info) = dir.get_file(filename.to_string() + "/Info")
                && ui.button("ℹ").on_hover_text(&attribution).clicked()
            {
                *open_modal = Some(ModalState::Text(
                    (info.contents_utf8().unwrap().to_string() + "\n" + &attribution).into(),
                ));
            };

            if let Some(license) = dir.get_file(filename.to_string() + "/License")
                && ui.button("⚖").on_hover_text(&attribution).clicked()
            {
                *open_modal = Some(ModalState::Text(
                    (license.contents_utf8().unwrap().to_string() + "\n" + &attribution).into(),
                ));
            };
        });
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
        ui.horizontal(|ui| ui.checkbox(&mut settings.non_blocking_input, "Non-blocking input"));
        ui.horizontal(|ui| ui.checkbox(&mut settings.run_until_breakpoint, "Run until breakpoint (DANGER)").on_hover_text("Will freeze the UI while working.\nIf there are no breakpoints then this effectively crashes the app."));

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

    fn info_modal(ui: &mut egui::Ui) {
        ui.heading("What is befunge?");
        ui.label("Befunge is a stack-based, self-overwriting 2D programming language.");
        ui.label("More description TODO.");

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.label("Just go read the ");
            ui.hyperlink_to("esolang wiki page", "https://esolangs.org/wiki/Befunge");
            ui.label(".");
        });
    }

    fn save_failure(ui: &mut egui::Ui, err: &SerializationError) {
        ui.heading("Save failed");
        match err {
            SerializationError::InvalidChar((x, y)) => {
                ui.label(format!("There is an invalid char at {x}, {y}"));
            }
            SerializationError::Newline((x, y)) => {
                ui.label(format!("There is a newline at {x}, {y}"));
            }
        }
    }

    fn draw_char(
        ui: &mut egui::Ui,
        char_renderer: &CharRenderer,
        mesh: &mut Mesh,
        settings: &Settings,
        pos: Rect,
        val: i64,
    ) {
        if let Ok(val) = TryInto::<u8>::try_into(val) {
            if val < b' ' {
                puffin::profile_scope_if!(PROFILE_EACH_CHAR, "char boxed");
                char_renderer.draw(
                    mesh,
                    pos,
                    match val {
                        0..=9 => val + b'0',
                        10.. => val - 10 + b'A',
                    },
                    Color32::GRAY,
                );
                char_renderer.draw(mesh, pos, b' ', Color32::GRAY);
            // this is ugly AF. TODO: improve
            } else if let Some(color) = if matches!(
                settings.befunge_version,
                BefungeVersionDiscriminants::Befunge98
            ) {
                befunge98::get_color_of_bf_op(val)
            } else {
                befunge::get_color_of_bf_op(val)
            } {
                puffin::profile_scope_if!(PROFILE_EACH_CHAR, "char colored");
                char_renderer.draw(mesh, pos, val, color);
            } else {
                puffin::profile_scope_if!(PROFILE_EACH_CHAR, "char simple");
                char_renderer.draw(mesh, pos, val, Color32::GRAY);
            }
        } else if settings.render_unicode
            && let Ok(val) = val.try_into()
            && let Some(val) = char::from_u32(val)
            && ui.fonts_mut(|fonts| fonts.has_glyph(&egui::FontId::monospace(1.0), val))
        {
            puffin::profile_scope_if!(PROFILE_EACH_CHAR, "char unicode");
            ui.place(pos, egui::Label::new(String::from(val)).selectable(false));
        } else {
            puffin::profile_scope_if!(PROFILE_EACH_CHAR, "char unknown");
            char_renderer.draw(mesh, pos, b' ', Color32::GRAY);
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
                    egui::Label::new(RichText::new(str).font(egui::FontId::monospace(font_size)))
                        .selectable(false),
                )
            });
        };
    }

    fn dual_char_and_numeric_input(ui: &mut egui::Ui, chr: i64, setter: impl FnOnce(i64)) {
        let mut value = None;
        // TODO: clean this up & make the text input box a lil wider
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
                    b' ' => icons::ICON_SPACE_BAR.to_string(),
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
                ui.input_mut(|e| {
                    if e.consume_key(Modifiers::NONE, egui::Key::Backspace) {
                        value = Some(b' ' as i64);
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
                                    value = Some(chr as i64);
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
                value = Some(chr);
            };
        });
        if let Some(val) = value {
            setter(val);
        };
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

// Mostly a copy of egui's Tooltip::should_show_tooltip
fn should_show_tooltip(response: &Response, is_tooltip_open: bool) -> bool {
    let style = response.ctx.global_style();

    let tooltip_delay = style.interaction.tooltip_delay;
    let (
        time_since_last_scroll,
        time_since_last_click,
        time_since_last_pointer_movement,
        pointer_pos,
    ) = response.ctx.input(|i| {
        (
            i.time_since_last_scroll(),
            i.pointer.time_since_last_click(),
            i.pointer.time_since_last_movement(),
            i.pointer.hover_pos(),
        )
    });

    if time_since_last_scroll < tooltip_delay {
        // See https://github.com/emilk/egui/issues/4781
        // Note that this means we cannot have `ScrollArea`s in a tooltip.
        response
            .ctx
            .request_repaint_after_secs(tooltip_delay - time_since_last_scroll);
        return false;
    }

    let clicked_more_recently_than_moved =
        time_since_last_click < time_since_last_pointer_movement + 0.1;
    if clicked_more_recently_than_moved {
        // It is common to click a widget and then rest the mouse there.
        // It would be annoying to then see a tooltip for it immediately.
        // Similarly, clicking should hide the existing tooltip.
        // Only hovering should lead to a tooltip, not clicking.
        // The offset is only to allow small movement just right after the click.
        return false;
    }

    if is_tooltip_open {
        // Check if we should automatically stay open:

        if pointer_pos.is_some_and(|pointer_pos| response.rect.contains(pointer_pos)) {
            // Handle the case of a big tooltip that covers the widget:
            return true;
        }
    }

    // Fast early-outs:
    if response.enabled() {
        if !response.hovered() || !response.ctx.input(|i| i.pointer.has_pointer()) {
            return false;
        }
    } else if !response
        .ctx
        .rect_contains_pointer(response.layer_id, response.rect)
    {
        return false;
    }

    if !response
        .ctx
        .input(|i| i.pointer.is_still() && !i.is_scrolling())
    {
        // wait for mouse to stop
        response.ctx.request_repaint();
        return false;
    }

    let time_since_last_interaction = time_since_last_scroll
        .min(time_since_last_pointer_movement)
        .min(time_since_last_click);
    let time_til_tooltip = tooltip_delay - time_since_last_interaction;

    if 0.0 < time_til_tooltip {
        // Wait until the mouse has been still for a while
        response.ctx.request_repaint_after_secs(time_til_tooltip);
        return false;
    }

    true
}

fn checkbox_with_underline(ui: &mut egui::Ui, checked: &mut bool, text: &str) -> Response {
    ui.scope(|ui| {
        ui.spacing_mut().icon_spacing = 0.0;
        ui.checkbox(
            checked,
            (" ", RichText::new(&text[..1]).underline(), &text[1..]),
        )
    })
    .inner
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

fn display_coordinate(ui: &mut Ui, pos: (i64, i64)) {
    ui.add(egui::Label::new(
        RichText::new(format!("({:03}, {:03})", pos.0, pos.1)).text_style(TextStyle::Monospace),
    ));
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
