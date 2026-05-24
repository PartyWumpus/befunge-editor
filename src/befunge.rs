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

use crate::{app::Settings, befunge93, befunge93mini, befunge98};

pub type Position = (i64, i64);
pub type Value = i64;

#[derive(RandGen, Default, Clone, Copy, PartialEq, PartialOrd, Ord, Eq, Hash, Debug)]
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
    EndProgram(i64),
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

pub enum SerializationError {
    InvalidChar((i64, i64)),
    Newline((i64, i64)),
}

pub trait FungeSpaceTrait {
    fn set(&mut self, pos: Position, val: Value);
    fn get(&self, pos: Position) -> Value;
    fn entries(&self) -> impl Iterator<Item = (Position, Value)>;
    fn program_size(&self) -> (i64, i64);

    fn serialize(&self) -> Result<String, SerializationError> {
        let height = self.program_size().1 + 1;
        let mut lines: Vec<Vec<char>> = vec![vec![]; height as usize];
        for ((x, y), val) in self.entries() {
            let line = &mut lines[y as usize];
            let Some(val) = char::from_u32(val as u32) else {
                return Err(SerializationError::InvalidChar((x, y)));
            };

            if val == '\n' || val == '\r' {
                return Err(SerializationError::Newline((x, y)));
            }
            if line.len() <= x as usize {
                line.extend(std::iter::repeat_n(' ', x as usize - line.len()));
                line.push(val);
            } else {
                line[x as usize] = val;
            };
        }
        let mut out = String::new();
        for line in lines {
            out += &line.iter().collect::<String>();
            out += "\n";
        }
        Ok(out)
    }
}

pub fn bf93_op_info(op: u8) -> Option<&'static str> {
    Some(match op {
        b'0'..=b'9' => "Loads a number onto the stack",
        b'+' => "Pops a then b, then pushes b + a",
        b'-' => "Pops a then b, then pushes b - a",
        b'*' => "Pops a then b, then pushes b * a",
        b'/' => "Pops a then b, then pushes b / a (Integer division)",
        b'%' => "Pops a then b, then pushes b % a (Remainder)",
        b'`' => "Pops a then b, then pushes b > a (1 if true, 0 if false)",

        b'"' => {
            "Enters 'string mode', all following characters just push their unicode codepoint value until the next \""
        }
        b'\\' => "Swaps the top 2 values on the stack",
        b'!' => "Pops a value. If it is 0, 1 is pushed. Otherwise 0 is pushed. (Logical not)",
        b':' => "Duplicates the top value on the stack",
        b'$' => "Pops a, then discards it",

        b'>' | b'<' | b'^' | b'v' => "Changes instruction pointer direction",
        b'#' => "Skips the next operation",
        b'?' => "Points the instruction pointer in a random direction",
        b'_' => "Pops a value. If it is 0, point the instruction pointer right. Otherwise go left.",
        b'|' => "Pops a value. If it is 0, point the instruction pointer down. Otherwise go up.",

        b'p' => "Pops x, y and val. Places val in the position (x,y) in the program's space",
        b'g' => {
            "Pops x and y. Pushes the value of the position (x,y) in the program's space. 0 if out of bounds"
        }

        b'&' => "Pushes an integer from stdin",
        b'~' => "Pushes a character from stdin",
        b'.' => "Pops a value and prints it as an integer",
        b',' => "Pops a value and prints it as a unicode character",
        b'@' => "Ends the program",

        b's' => "(Graphics) Pops y then x. Creates a screen with those dimensions",
        b'f' => "(Graphics) Pops r, g and b. Sets the drawing colour to rgb(r, g, b)",
        b'x' => {
            "(Graphics) Pops y then x. Sets the pixel on the screen position (x,y) to the current drawing colour"
        }
        b'c' => "(Graphics) Fills the screen with the current drawing colour",
        b'u' => "(Graphics) Pause the interpreter for one frame to sync drawing",
        b'l' => "(Graphics) Pops y2, x2, y1, x1. Draws a line from (x1, y1) to (x2, y2)",
        b'z' => "(Graphics) Pushes a screen event. Info TODO",

        _ => return None,
    })
}

pub fn bf98_op_info(op: u8) -> Option<&'static str> {
    Some(match op {
        b'0'..=b'9' | b'a'..=b'f' => "Loads a number onto the stack",
        b'+' => "Pops a then b, then pushes b + a",
        b'-' => "Pops a then b, then pushes b - a",
        b'*' => "Pops a then b, then pushes b * a",
        b'/' => "Pops a then b, then pushes b / a (Integer division)",
        b'%' => "Pops a then b, then pushes b % a (Remainder)",
        b'`' => "Pops a then b, then pushes b > a (1 if true, 0 if false)",

        b'"' => {
            "Enters 'string mode', all following characters just push their unicode codepoint value until the next \""
        }
        b'\\' => "Swaps the top 2 values on the stack",
        b'!' => "Pops a value. If it is 0, 1 is pushed. Otherwise 0 is pushed. (Logical not)",

        b':' => "Duplicates the top value on the stack",
        b'$' => "Pops a, then discards it",
        b'n' => "Clears the stack",
        b'u' => {
            "Pops a value. Transfers that many values from the top stack to the second top stack on the stack stack"
        }
        b'k' => "Pops a value. Executes the next cell that many times",

        b'>' | b'<' | b'^' | b'v' => "Changes instruction pointer direction",
        b'#' => "Skips the next operation",
        b'?' => "Points the instruction pointer in a random direction",
        b'_' => "Pops a value. If it is 0, point the instruction pointer right. Otherwise go left.",
        b'|' => "Pops a value. If it is 0, point the instruction pointer down. Otherwise go up.",
        b'w' => "Pops a then b. If a>b turns right, if a<b turns left, if a=b continues forwards",
        b']' => "Turns right (90 degrees)",
        b'[' => "Turns left (90 degrees)",
        b'j' => "Pops a value. Jumps forwards that many cells",
        b'r' => "Turns around (180 degrees)",
        b'x' => {
            "Pops y then x. Sets the direction to (x,y). This allows for non-cardinal directions, like (3,2)"
        }

        b'p' => "Pops x, y and val. Places val in the position (x,y) in the program's space",
        b'g' => "Pops x and y. Pushes the value of the position (x,y) in the program's space",
        b's' => "Pops a value. Places it in the position in front of this instruction.",
        b'\'' => "Loads the value of the next character onto the stack, and skips over it",

        b'&' => "Pushes an integer from stdin",
        b'~' => "Pushes a character from stdin",
        b'.' => "Pops a value and prints it as an integer",
        b',' => "Pops a value and prints it as a unicode character",
        b'@' => "Kills this IP. Ends the program if it is the only one",
        b'q' => "Ends the program",

        b';' => "Skips all operations until the next ;",

        b'(' => "Loads a fingerprint. Go read the befunge98 spec",
        b')' => "Unloads a fingerprint. Go read the befunge98 spec",
        b'A'..=b'Z' => "Fingerprint instruction",

        b'{' => {
            "Pops a value N. Makes a new stack on the top of the stack stack. Transfers N values from the previous stack onto the new one. Offsets all g/p operations to here. Go read the befunge98 spec"
        }
        b'}' => {
            "Pops a value. Transfers that many values from the top of the stack stack to the second top of the stack stack. Deletes the top of the stack stack. Resets the g/p offset. Go read the befunge98 spec"
        }

        b'y' => "Spits a bunch of sysinfo out. Go read the befunge98 spec",
        b't' => "Splits IP in two. One continues forwards, and one reflects. Stack is copied",
        b'=' => "Executes a string a sh command. Not implemented",
        b'i' => "Loads a file from your filesytem into fungespace. Not implemented",
        b'o' => "Writes a file from fungespace to your filesystem. Not implemented",

        _ => return None,
    })
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
    fn cursor_positions(&self) -> Vec<Position>;
    fn cursor_direction(&self) -> (Value, Value);

    // TODO: make this &[Value]
    fn stack(&self, index: usize) -> Vec<Value>;
    fn stdout(&self) -> &str;
    fn stdin(&mut self) -> &mut String;
    fn graphics(&mut self) -> Option<&mut Graphics>;

    fn pos_history(&mut self) -> &mut HashMap<Position, Visited>;
    fn get_history(&mut self) -> &mut HashMap<Position, Instant>;
    fn put_history(&mut self) -> &mut HashMap<Position, Instant>;
    fn breakpoints(&mut self) -> &mut HashSet<Position>;

    fn serialize(&self) -> Result<String, SerializationError>;

    fn debug_set_position(&mut self, pos: Position);
}

#[derive(Clone, EnumDiscriminants)]
#[strum_discriminants(derive(serde::Deserialize, serde::Serialize))]
#[enum_dispatch(Befunge)]
pub enum BefungeVersion {
    Befunge93(befunge93::State),
    Befunge93Mini(befunge93mini::State),
    Befunge98(befunge98::State),
}

impl BefungeVersionDiscriminants {
    pub fn border_positions(self) -> ((i64, i64), (i64, i64)) {
        match self {
            BefungeVersionDiscriminants::Befunge93 => ((0, 0), (i64::MAX, i64::MAX)),
            BefungeVersionDiscriminants::Befunge93Mini => {
                ((0, 0), (i8::MAX as i64, i8::MAX as i64))
            }
            BefungeVersionDiscriminants::Befunge98 => ((i64::MIN, i64::MIN), (i64::MAX, i64::MAX)),
        }
    }
}
