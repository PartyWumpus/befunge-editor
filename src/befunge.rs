use std::collections::VecDeque;

use bitfield_struct::bitfield;
use clipline::AnyOctant;
use coarsetime::{Duration, Instant};
use egui::{
    Color32,
    ahash::{HashMap, HashSet},
};
use enum_dispatch::enum_dispatch;
use rand_derive2::RandGen;
use strum_macros::EnumDiscriminants;

use crate::{app::Settings, befunge93, befunge93mini};

pub type Position = (i64, i64);
pub type Value = i64;

#[derive(RandGen, Default, Clone, Copy, PartialEq, PartialOrd, Ord, Eq, Hash)]
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

#[derive(Debug)]
pub enum StepStatus {
    Normal,
    NormalNoStep,
    Breakpoint,
    Error(&'static str),
    SyncFrame,
}

#[derive(Clone)]
pub enum GraphicalEvent {
    Close,
    //KeyDown(i64),
    //KeyUp(i64),
    MouseClick(Position),
}

#[derive(Clone)]
pub struct Graphics {
    pub size: (usize, usize),
    pub texture: Vec<Color32>,
    pub current_color: Color32,
    pub event_queue: VecDeque<GraphicalEvent>,
}

impl Graphics {
    pub const MAX_IMAGE_SIZE: i64 = 10000;
    pub fn new(x: usize, y: usize) -> Self {
        Self {
            size: (x, y),
            texture: vec![Color32::BLACK; y * x],
            current_color: Color32::BLACK,
            event_queue: VecDeque::default(),
        }
    }

    pub fn pixel(&mut self, x: i64, y: i64) -> StepStatus {
        let Ok(y): Result<usize, _> = y.try_into() else {
            return StepStatus::Error("Out of bounds graphical operation");
        };
        let Ok(x): Result<usize, _> = x.try_into() else {
            return StepStatus::Error("Out of bounds graphical operation");
        };

        if x >= self.size.0 || y >= self.size.1 {
            return StepStatus::Error("Out of bounds graphical operation");
        }

        let index = x + y * self.size.0;
        self.texture[index] = self.current_color;
        StepStatus::Normal
    }

    pub fn fill(&mut self) {
        self.texture = vec![self.current_color; self.size.0 * self.size.1];
    }

    pub fn line(&mut self, x1: i32, y1: i32, x2: i32, y2: i32) {
        // TODO: use clippin n stuff
        for (x, y) in AnyOctant::<i32>::new((x1, y1), (x2, y2)) {
            self.pixel(x as i64, y as i64);
        }
    }
}

// TODO: replace with graph traversal maybe
// TODO: make generic over the version of befunge being used
pub fn get_color_of_bf_op(op: u8) -> Option<Color32> {
    enum OpTypes {
        Number,
        Operator,
        Direction,
        Modification,
        IO,
        Graphics,
        None,
    }

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

pub trait FungeSpaceTrait {
    fn set(&mut self, pos: Position, val: Value);
    fn get(&self, pos: Position) -> Value;
    fn entries(&self) -> impl Iterator<Item = (Position, Value)>;
    fn program_size(&self) -> (i64, i64);

    // TODO: make this fallible
    fn serialize(&self) -> String {
        let height = self.program_size().1;
        let mut lines: Vec<Vec<char>> = vec![vec![]; height as usize];
        for ((x, y), val) in self.entries() {
            let line = &mut lines[y as usize];
            if line.len() <= x as usize {
                line.extend(std::iter::repeat_n(' ', x as usize - line.len()));
                assert_ne!(val, b'\n' as Value);
                assert_ne!(val, b'\r' as Value);
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

#[enum_dispatch]
pub trait Befunge {
    fn get(&self, pos: Position) -> Value;
    fn set(&mut self, pos: Position, val: Value);
    fn step(&mut self, settings: &Settings) -> StepStatus;

    // TODO: make this a rect, so befunge98 can go into negative space
    fn program_size(&self) -> (i64, i64);
    fn instruction_count(&self) -> usize;
    fn string_mode(&self) -> bool;
    fn cursor_position(&self) -> Position;
    fn cursor_direction(&self) -> Direction;

    // TODO: make this &[Value]
    fn stack(&self) -> Vec<Value>;
    fn stdout(&self) -> &str;
    fn stdin(&mut self) -> &mut String;
    fn graphics(&mut self) -> Option<&mut Graphics>;

    fn pos_history(&mut self) -> &mut HashMap<Position, Visited>;
    fn get_history(&mut self) -> &mut HashMap<Position, Instant>;
    fn put_history(&mut self) -> &mut HashMap<Position, Instant>;
    fn breakpoints(&mut self) -> &mut HashSet<Position>;

    fn serialize(&self) -> String;
}

#[derive(Clone, EnumDiscriminants)]
#[strum_discriminants(derive(serde::Deserialize, serde::Serialize))]
#[enum_dispatch(Befunge)]
pub enum BefungeVersion {
    Befunge93(befunge93::State),
    Befunge93Mini(befunge93mini::State),
}

impl BefungeVersionDiscriminants {
    pub fn border_positions(self) -> ((i64, i64), (i64, i64)) {
        match self {
            BefungeVersionDiscriminants::Befunge93 => ((0, 0), (i64::MAX, i64::MAX)),
            BefungeVersionDiscriminants::Befunge93Mini => {
                ((0, 0), (i8::MAX as i64, i8::MAX as i64))
            }
        }
    }
}
