use clipline::AnyOctant;
//use rand::{distr::StandardUniform, prelude::*};
use bitfield_struct::bitfield;
use coarsetime::{Duration, Instant};
use egui::{
    Color32,
    ahash::{HashSet, HashSetExt},
};
use std::{collections::VecDeque, iter};
use thiserror::Error;

use egui::ahash::HashMap;

use crate::app::Settings;
/*
#[cfg(target_arch = "wasm32")]
use egui::ahash::HashMap;
#[cfg(not(target_arch = "wasm32"))]
use gxhash::HashMap;
*/

const MAX_IMAGE_SIZE: i64 = 10000;

#[derive(Default, Clone, Copy, PartialEq, PartialOrd, Ord, Eq, Hash)]
pub enum Direction {
    North,
    South,
    #[default]
    East,
    West,
}

#[bitfield(u8)]
pub struct WhereVisited {
    pub north: bool,
    pub south: bool,
    pub east: bool,
    pub west: bool,
    #[bits(4)]
    __: u8,
}

#[derive(Debug, Clone)]
pub struct Visited {
    // used instead of 4 Option<Instant>s, to save space
    pub wawa: WhereVisited,
    pub north: Instant,
    pub south: Instant,
    pub east: Instant,
    pub west: Instant,
}

impl Default for Visited {
    fn default() -> Self {
        Self {
            wawa: WhereVisited::new(),
            north: Instant::recent(),
            south: Instant::recent(),
            east: Instant::recent(),
            west: Instant::recent(),
        }
    }
}

impl Visited {
    pub fn time_since(&self, t: Instant) -> Duration {
        let mut dur = Duration::from_u64(u64::MAX);
        if self.wawa.north() {
            dur = dur.min(t.duration_since(self.north));
        }
        if self.wawa.south() {
            dur = dur.min(t.duration_since(self.south));
        }
        if self.wawa.east() {
            dur = dur.min(t.duration_since(self.east));
        }
        if self.wawa.west() {
            dur = dur.min(t.duration_since(self.west));
        }

        dur
    }
}

impl Direction {
    pub fn reverse(&self) -> Self {
        match self {
            Self::North => Self::South,
            Self::South => Self::North,
            Self::East => Self::West,
            Self::West => Self::East,
        }
    }
}

/*impl Distribution<Direction> for StandardUniform {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> Direction {
        match rng.random_range(0..=3) {
            0 => Direction::North,
            1 => Direction::South,
            2 => Direction::East,
            _ => Direction::West
        }
    }
}*/

#[derive(Clone)]
pub struct FungeSpace {
    map: HashMap<(i64, i64), i64>,
    zero_page: Box<[i64; 100]>,
    pub max_size: (i64, i64),
    undos: Option<VecDeque<((i64, i64), i64)>>,
}

#[derive(Debug, Clone, Error)]
pub enum Error {
    #[error("Invalid operation")]
    InvalidOperation,
    #[error("Division by zero")]
    DivisionByZero,
    #[error("Out of bounds graphics operation")]
    OutOfBoundsGraphics,
    #[error("& is not yet supported")]
    TodoAmpersand,
}

#[derive(Debug)]
pub enum StepStatus {
    Normal,
    NormalNoStep,
    Breakpoint,
    Error(Error),
    SyncFrame,
}

#[derive(Clone)]
pub enum Event {
    Close,
    //KeyDown(i64),
    //KeyUp(i64),
    MouseClick(i64, i64),
}

#[derive(Clone)]
pub struct Graphics {
    pub size: (usize, usize),
    pub texture: Vec<Color32>,
    pub current_color: Color32,
    pub event_queue: VecDeque<Event>,
}

#[derive(Clone)]
pub struct State {
    pub instruction_count: usize,
    pub map: FungeSpace,
    pub string_mode: bool,
    pub position: (i64, i64),
    pub direction: Direction,
    pub pos_history: HashMap<(i64, i64), Visited>,
    pub get_history: HashMap<(i64, i64), Instant>,
    pub put_history: HashMap<(i64, i64), Instant>,
    pub stack: Vec<i64>,
    pub output: String,
    pub graphics: Option<Graphics>,
    pub breakpoints: HashSet<(i64, i64)>,
    //pub input_buffer: VecDeque<i64>,
    pub input_buffer: String,
    //pub input_number: i64,
}

impl FungeSpace {
    pub fn new(undo_enabled: bool) -> Self {
        Self {
            map: HashMap::default(),
            zero_page: Box::new([b' '.into(); 100]),
            max_size: (11, 11),
            undos: if undo_enabled {
                Some(VecDeque::new())
            } else {
                None
            },
        }
    }
    pub fn new_from_string(input: &str, undo_enabled: bool) -> Self {
        let mut map = FungeSpace::new(undo_enabled);
        for (y, line) in input.lines().enumerate() {
            for (x, char) in line.chars().enumerate() {
                map.set((x.try_into().unwrap(), y.try_into().unwrap()), char as i64);
            }
        }
        map
    }

    pub fn set(&mut self, pos: (i64, i64), val: i64) {
        if pos.0 < 0 || pos.1 < 0 {
            return;
        };

        if self.undos.is_some() {
            let old = self.get_wrapped(pos);
            let undos = self.undos.as_mut().unwrap();
            undos.push_back((pos, old));
            undos.truncate(254);
        }

        self.set_inner(pos, val);
    }

    fn set_inner(&mut self, pos: (i64, i64), val: i64) {
        if pos.0 < 0 || pos.1 < 0 {
            return;
        };

        if pos.0 < 10 && pos.1 < 10 {
            self.zero_page[(pos.0 + pos.1 * 10) as usize] = val
        } else {
            if val == b' ' as i64 {
                self.map.remove(&pos);
            } else {
                self.map.insert(pos, val);
            }

            if pos.0 > self.max_size.0 {
                self.max_size.0 = pos.0
            }
            if pos.1 > self.max_size.1 {
                self.max_size.1 = pos.1
            }
        };
    }

    pub fn get(&self, pos: (i64, i64)) -> Option<i64> {
        if pos.0 < 10 && pos.1 < 10 {
            Some(self.zero_page[usize::try_from(pos.0 + pos.1 * 10).unwrap()])
        } else {
            self.map.get(&pos).copied()
        }
    }

    pub fn get_wrapped(&self, pos: (i64, i64)) -> i64 {
        if pos.0 < 0 || pos.1 < 0 {
            return 0;
        }
        if pos.0 < 10 && pos.1 < 10 {
            self.zero_page[(pos.0 + pos.1 * 10) as usize]
        } else {
            *self.map.get(&pos).unwrap_or(&(b' ' as i64))
        }
    }

    pub fn undo(&mut self) -> Option<()> {
        let Some(undos) = &mut self.undos else {
            return None;
        };
        let (pos, val) = undos.pop_back()?;
        self.set_inner(pos, val);
        Some(())
    }

    pub fn entries(&mut self) -> impl Iterator<Item = ((i64, i64), i64)> {
        self.map
            .iter()
            .map(|(k, v)| (*k, *v))
            .chain(self.zero_page.iter().enumerate().map(|(i, val)| {
                let i = i as i64;
                ((i % 10, i / 10), *val)
            }))
    }

    fn height(&mut self) -> usize {
        let mut height = 10;
        for (_x, y) in self.map.keys() {
            if *y > height {
                height = *y
            }
        }
        height as usize + 1
    }

    pub fn serialize(&mut self) -> String {
        let height = self.height();
        let mut lines: Vec<Vec<char>> = vec![vec![]; height];
        for ((x, y), val) in self.entries() {
            let line = &mut lines[y as usize];
            if line.len() <= x as usize {
                line.extend(iter::repeat_n(' ', x as usize - line.len()));
                assert_ne!(val, b'\n' as i64);
                assert_ne!(val, b'\r' as i64);
                line.push(char::from_u32(val as u32).expect("wawa"));
            } else {
                line[x as usize] = char::from_u32(val as u32).expect("wawa");
            };
        }
        let mut out = String::new();
        for line in lines {
            out += &line.iter().collect::<String>();
            out += "\n";
        }
        out
    }
}

impl Graphics {
    fn new(x: usize, y: usize) -> Self {
        Self {
            size: (x, y),
            texture: vec![Color32::BLACK; y * x],
            current_color: Color32::BLACK,
            event_queue: VecDeque::default(),
        }
    }

    fn pixel(&mut self, x: usize, y: usize) -> StepStatus {
        if x >= self.size.0 || y >= self.size.1 {
            return StepStatus::Error(Error::OutOfBoundsGraphics);
        }

        let index = x + y * self.size.0;
        self.texture[index] = self.current_color;
        StepStatus::Normal
    }
}

impl Default for State {
    fn default() -> Self {
        Self {
            instruction_count: 0,
            map: FungeSpace::new(false),
            string_mode: false,
            position: (0, 0),
            direction: Direction::East,
            pos_history: HashMap::default(),
            put_history: HashMap::default(),
            get_history: HashMap::default(),
            stack: Vec::new(),
            output: String::new(),
            graphics: None,
            breakpoints: HashSet::new(),
            //input_buffer: VecDeque::new(),
            input_buffer: String::new(),
            //input_number: i64,
        }
    }
}

impl State {
    fn pop(&mut self) -> i64 {
        self.stack.pop().unwrap_or(0)
    }

    pub fn new() -> Self {
        Self::default()
    }

    pub fn new_from_fungespace(fungespace: FungeSpace) -> Self {
        Self {
            map: fungespace,
            ..Default::default()
        }
    }

    pub fn step_position(&mut self, settings: &Settings) {
        let (x, y) = self.position;
        self.step_position_inner();
        if settings.pos_history.0 {
            if let Some(visited) = self.pos_history.get_mut(&(x, y)) {
                match self.direction {
                    Direction::North => {
                        visited.wawa.set_north(true);
                        visited.north = Instant::recent();
                    }
                    Direction::South => {
                        visited.wawa.set_south(true);
                        visited.south = Instant::recent();
                    }
                    Direction::East => {
                        visited.wawa.set_east(true);
                        visited.east = Instant::recent();
                    }
                    Direction::West => {
                        visited.wawa.set_west(true);
                        visited.west = Instant::recent();
                    }
                }
            } else {
                self.pos_history.insert(
                    (x, y),
                    match self.direction {
                        Direction::North => Visited {
                            wawa: WhereVisited::new().with_north(true),
                            north: Instant::recent(),
                            ..Default::default()
                        },
                        Direction::South => Visited {
                            wawa: WhereVisited::new().with_south(true),
                            south: Instant::recent(),
                            ..Default::default()
                        },
                        Direction::East => Visited {
                            wawa: WhereVisited::new().with_east(true),
                            east: Instant::recent(),
                            ..Default::default()
                        },
                        Direction::West => Visited {
                            wawa: WhereVisited::new().with_west(true),
                            west: Instant::recent(),
                            ..Default::default()
                        },
                    },
                );
            }
        }
    }

    fn step_position_inner(&mut self) {
        let (x, y) = self.position;
        match self.direction {
            Direction::North => self.position = (x, y - 1),
            Direction::South => self.position = (x, y + 1),
            Direction::East => self.position = (x + 1, y),
            Direction::West => self.position = (x - 1, y),
        }

        if self.position.0 == -1 {
            self.position.0 = self.map.max_size.0.saturating_add(1);
        } else if self.position.0.wrapping_sub(1) >= self.map.max_size.0 {
            self.position.0 = 0
        };

        if self.position.1 == -1 {
            self.position.1 = self.map.max_size.1.saturating_add(1);
        } else if self.position.1.wrapping_sub(1) >= self.map.max_size.1 {
            self.position.1 = 0
        };
    }

    pub fn step(&mut self, settings: &Settings) -> StepStatus {
        self.instruction_count += 1;
        let status = self.step_inner(settings);
        if self.breakpoints.contains(&self.position) {
            return StepStatus::Breakpoint;
        }
        // skip up to 100 spaces if not in string mode
        if settings.skip_spaces && !self.string_mode {
            let mut safety_counter = 0;
            loop {
                safety_counter += 1;
                if safety_counter < 1000 && self.map.get_wrapped(self.position) == b' ' as i64 {
                    self.step_position(settings);
                } else {
                    break;
                }
            }
        };
        status
    }

    fn step_inner(&mut self, settings: &Settings) -> StepStatus {
        let op = self.map.get(self.position);

        if self.string_mode {
            let op = op.unwrap_or(b' ' as i64);
            if op == b'"' as i64 {
                self.string_mode = false;
            } else {
                self.stack.push(op);
            }
            self.step_position(settings);
            StepStatus::Normal
        } else if let Some(op) = op {
            if let Ok(op) = op.try_into() {
                let status = self.do_op(op, settings);
                match status {
                    StepStatus::Normal | StepStatus::SyncFrame => {
                        self.step_position(settings);
                    }
                    _ => (),
                };
                status
            } else {
                StepStatus::Error(Error::InvalidOperation)
            }
        } else {
            self.step_position(settings);
            StepStatus::Normal
        }
    }

    fn do_op(&mut self, op: u8, settings: &Settings) -> StepStatus {
        match op {
            b'"' => self.string_mode = true,

            b'0'..=b'9' => self.stack.push((op - b'0').into()),

            // 2 op operations
            b'+' => {
                let a = self.pop();
                let b = self.pop();
                self.stack.push(b + a);
            }
            b'-' => {
                let a = self.pop();
                let b = self.pop();
                self.stack.push(b - a);
            }
            b'*' => {
                let a = self.pop();
                let b = self.pop();
                self.stack.push(b * a);
            }
            b'/' => {
                let a = self.pop();
                let b = self.pop();
                if a == 0 {
                    return StepStatus::Error(Error::DivisionByZero);
                }
                self.stack.push(b / a);
            }
            b'%' => {
                let a = self.pop();
                let b = self.pop();
                if a == 0 {
                    return StepStatus::Error(Error::DivisionByZero);
                }
                self.stack.push(b % a);
            }
            b'`' => {
                let a = self.pop();
                let b = self.pop();
                self.stack.push(if b > a { 1 } else { 0 });
            }
            b'\\' => {
                let a = self.pop();
                let b = self.pop();
                self.stack.push(a);
                self.stack.push(b);
            }

            // one op operations
            b'!' => {
                let a = self.pop();
                self.stack.push(if a == 0 { 1 } else { 0 });
            }
            b':' => {
                let a = self.pop();
                self.stack.push(a);
                self.stack.push(a);
            }
            b'$' => {
                self.pop();
            }

            // static direction changes
            b'>' => self.direction = Direction::East,
            b'<' => self.direction = Direction::West,
            b'^' => self.direction = Direction::North,
            b'v' => self.direction = Direction::South,
            b'#' => {
                self.step_position(settings);
                self.step_position_inner();
                return StepStatus::NormalNoStep;
            }

            // dynamic direction changes
            //b'?' => self.direction = (rand::rng()).random(),
            b'_' => {
                let status = self.pop();
                if status == 0 {
                    self.direction = Direction::East;
                } else {
                    self.direction = Direction::West;
                }
            }

            b'|' => {
                let status = self.pop();
                if status == 0 {
                    self.direction = Direction::South;
                } else {
                    self.direction = Direction::North;
                }
            }

            // put (this is the big one!)
            b'p' => {
                let y = self.pop();
                let x = self.pop();
                let value = self.pop();

                if settings.put_history.0 {
                    if let Some(prev_time) = self.put_history.get(&(x, y)) {
                        if prev_time.elapsed_since_recent() > Duration::from_millis(500) {
                            self.put_history.insert((x, y), Instant::recent());
                        }
                    } else {
                        self.put_history.insert((x, y), Instant::recent());
                    }
                }

                self.map.set((x, y), value);
            }

            // get
            b'g' => {
                let y = self.pop();
                let x = self.pop();
                self.stack.push(self.map.get_wrapped((x, y)));

                if settings.get_history.0 {
                    if let Some(prev_time) = self.get_history.get(&(x, y)) {
                        if prev_time.elapsed_since_recent() > Duration::from_millis(500) {
                            self.get_history.insert((x, y), Instant::recent());
                        }
                    } else {
                        self.get_history.insert((x, y), Instant::recent());
                    }
                }
            }

            // input
            b'&' => return StepStatus::Error(Error::TodoAmpersand),

            b'~' => {
                // FIXME TODO oh my god use a vecdqueue i beg
                let mut itr = self.input_buffer.chars();
                match itr.next() {
                    None => return StepStatus::Breakpoint,
                    Some(chr) => {
                        self.stack.push(chr as i64);
                        self.input_buffer = itr.as_str().into();
                    }
                }
            }

            // halt is dealt with higher up
            b'@' => return StepStatus::Breakpoint,

            // -- IO output
            b'.' => {
                let a = self.pop().to_string();
                self.output.push_str(&a);
                self.output.push(' ');
            }
            b',' => {
                let a = (self.pop() as u32).try_into().unwrap();
                self.output.push(a);
            }

            // befunge with graphics
            b's' => {
                // setup
                let y = self.pop();
                let x = self.pop();

                if y <= 0 || x <= 0 || x > MAX_IMAGE_SIZE || y > MAX_IMAGE_SIZE {
                    return StepStatus::Error(Error::OutOfBoundsGraphics);
                }

                self.graphics = Some(Graphics::new(x as usize, y as usize));
            }

            b'f' => {
                // configure color
                if let Some(graphics) = &mut self.graphics {
                    let r = self.stack.pop().unwrap_or(0).try_into();
                    let g = self.stack.pop().unwrap_or(0).try_into();
                    let b = self.stack.pop().unwrap_or(0).try_into();
                    if let Ok(r) = r
                        && let Ok(g) = g
                        && let Ok(b) = b
                    {
                        graphics.current_color = Color32::from_rgb(r, g, b);
                    } else {
                        return StepStatus::Error(Error::OutOfBoundsGraphics);
                    }
                }
            }

            b'x' => {
                // set pixel
                if let Some(graphics) = &mut self.graphics {
                    let Ok(y) = self.stack.pop().unwrap_or(0).try_into() else {
                        return StepStatus::Error(Error::OutOfBoundsGraphics);
                    };
                    let Ok(x) = self.stack.pop().unwrap_or(0).try_into() else {
                        return StepStatus::Error(Error::OutOfBoundsGraphics);
                    };

                    return graphics.pixel(x, y);
                }
            }

            b'c' => {
                // fill
                if let Some(graphics) = &mut self.graphics {
                    graphics.texture =
                        vec![graphics.current_color; graphics.size.0 * graphics.size.1];
                }
            }

            b'u' => return StepStatus::SyncFrame, // update (noop for now)

            b'l' => {
                // line
                if let Some(graphics) = &mut self.graphics {
                    let y1: i32 = self.stack.pop().unwrap_or(0).try_into().unwrap();
                    let x1: i32 = self.stack.pop().unwrap_or(0).try_into().unwrap();

                    let y2: i32 = self.stack.pop().unwrap_or(0).try_into().unwrap();
                    let x2: i32 = self.stack.pop().unwrap_or(0).try_into().unwrap();

                    if x1 >= graphics.size.0 as i32
                        || y1 >= graphics.size.1 as i32
                        || x2 >= graphics.size.0 as i32
                        || y2 >= graphics.size.1 as i32
                    {
                        return StepStatus::Error(Error::OutOfBoundsGraphics);
                    }

                    // TODO: use clippin n stuff
                    for (x, y) in AnyOctant::<i32>::new((x1, y1), (x2, y2)) {
                        graphics.pixel(x.try_into().unwrap(), y.try_into().unwrap());
                    }
                }
            }

            b'z' => {
                // event
                if let Some(graphics) = &mut self.graphics {
                    if let Some(event) = graphics.event_queue.pop_front() {
                        match event {
                            //None is event 0
                            Event::Close => self.stack.extend([1]),
                            //Event::KeyDown(key) => self.stack.extend([key,2]),
                            //Event::KeyUp(key) => self.stack.extend([key,3]),
                            Event::MouseClick(x, y) => self.stack.extend([x, y, 4]),
                        }
                    } else {
                        self.stack.push(0);
                    }
                }
            }

            // noop
            b' ' => (),

            _ => return StepStatus::Error(Error::InvalidOperation),
        };
        StepStatus::Normal
    }
}

enum OpTypes {
    Number,
    Operator,
    Direction,
    Modification,
    IO,
    Graphics,
    None,
}

pub fn get_color_of_bf_op(op: u8) -> Option<Color32> {
    // TODO: replace with graph traversal maybe
    let flavor = match op {
        b'0'..=b'9' => OpTypes::Number,
        b'+' | b'-' | b'*' | b'/' | b'%' | b'`' | b'"' | b'\\' | b'!' | b':' | b'$' => {
            OpTypes::Operator
        }

        b'>' | b'<' | b'^' | b'v' | b'#' | b'?' | b'_' | b'|' => OpTypes::Direction,

        b'p' | b'g' => OpTypes::Modification,

        b'&' | b'~' | b'.' | b',' | b'@' => OpTypes::IO,

        b's' | b'f' | b'x' | b'c' | b'u' | b'l' | b'z' => OpTypes::Graphics,

        // noop
        _ => OpTypes::None,
    };

    match flavor {
        OpTypes::Number => Some(Color32::from_rgb(32, 159, 181)),
        OpTypes::Operator => Some(Color32::from_rgb(210, 15, 57)),
        OpTypes::Direction => Some(Color32::from_rgb(64, 160, 43)),
        OpTypes::Modification => Some(Color32::from_rgb(136, 57, 239)),
        OpTypes::IO => Some(Color32::from_rgb(234, 118, 203)),
        OpTypes::Graphics => Some(Color32::from_rgb(114, 135, 253)),
        OpTypes::None => None,
    }
}
