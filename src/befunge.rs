use clipline::AnyOctant;
//use rand::{distr::StandardUniform, prelude::*};
use coarsetime::{Duration, Instant};
use egui::{
    Color32,
    ahash::{HashSet, HashSetExt},
};
use std::{collections::VecDeque, iter};

use egui::ahash::HashMap;

use crate::app::Settings;
/*
#[cfg(target_arch = "wasm32")]
use egui::ahash::HashMap;
#[cfg(not(target_arch = "wasm32"))]
use gxhash::HashMap;
*/

#[derive(Clone)]
pub enum Direction {
    North,
    South,
    East,
    West,
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
    pub map: FungeSpace,
    pub position: (i64, i64),
    pub direction: Direction,
    pub pos_history: HashMap<(i64, i64), Instant>,
    pub get_history: HashMap<(i64, i64), Instant>,
    pub put_history: HashMap<(i64, i64), Instant>,
    pub stack: Vec<i64>,
    pub string_mode: bool,
    pub output: String,
    pub graphics: Option<Graphics>,
    pub breakpoints: HashSet<(i64, i64)>,
    //pub input_buffer: VecDeque<i64>,
    pub input_buffer: String,
}

impl Default for FungeSpace {
    fn default() -> Self {
        Self::new()
    }
}

impl FungeSpace {
    pub fn new() -> Self {
        Self {
            map: HashMap::default(),
            zero_page: Box::new([b' '.into(); 100]),
        }
    }

    pub fn new_from_string(input: String) -> Self {
        let mut map = FungeSpace::new();
        for (y, line) in input.lines().enumerate() {
            for (x, char) in line.chars().enumerate() {
                map.set((x.try_into().unwrap(), y.try_into().unwrap()), char as i64);
            }
        }
        map
    }

    pub fn set(&mut self, pos: (i64, i64), val: i64) {
        if pos.0 < 10 && pos.1 < 10 {
            self.zero_page[(pos.0 + pos.1 * 10) as usize] = val
        } else {
            if val == b' ' as i64 {
                self.map.remove(&pos);
            }
            self.map.insert(pos, val);
        }
    }

    pub fn get(&mut self, pos: (i64, i64)) -> Option<i64> {
        if pos.0 < 10 && pos.1 < 10 {
            Some(self.zero_page[usize::try_from(pos.0 + pos.1 * 10).unwrap()])
        } else {
            self.map.get(&pos).copied()
        }
    }

    pub fn get_wrapped(&mut self, pos: (i64, i64)) -> i64 {
        if pos.0 < 0 || pos.1 < 0 {
            return 0;
        }
        if pos.0 < 10 && pos.1 < 10 {
            self.zero_page[(pos.0 + pos.1 * 10) as usize]
        } else {
            *self.map.get(&pos).unwrap_or(&(b' ' as i64))
        }
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

    pub fn pixel(&mut self, x: usize, y: usize) {
        // FIXME: error here on out of bounds
        let index = x + y * self.size.1;
        self.texture[index] = self.current_color;
    }
}

impl Default for State {
    fn default() -> Self {
        Self {
            map: FungeSpace::new(),
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

    fn step_position(&mut self, settings: &Settings) {
        self.step_position_inner();
        let (x, y) = self.position;
        if settings.pos_history.0 {
            if let Some(prev_time) = self.pos_history.get(&(x, y)) {
                if prev_time.elapsed_since_recent() > Duration::from_millis(500) {
                    self.pos_history.insert((x, y), Instant::recent());
                }
            } else {
                self.pos_history.insert((x, y), Instant::recent());
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

        if self.position.0 < 0 {
            self.position.0 += i64::MAX;
            self.position.0 += 1;
        };
        if self.position.1 < 0 {
            self.position.1 += i64::MAX;
            self.position.1 += 1;
        };
    }

    pub fn step(&mut self, settings: &Settings) -> bool {
        let mut res = self.step_inner(settings);
        if self.breakpoints.contains(&self.position) {
            res = true;
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
        res
    }

    fn step_inner(&mut self, settings: &Settings) -> bool {
        let op = self.map.get(self.position);

        if self.string_mode {
            let op = op.unwrap_or(b' ' as i64);
            if op == b'"' as i64 {
                self.string_mode = false;
            } else {
                self.stack.push(op);
            }
        } else if let Some(op) = op
            && let Ok(op) = op.try_into()
            && self.do_op(op, settings)
        {
            return true;
        }
        self.step_position(settings);
        false
    }

    fn do_op(&mut self, op: u8, settings: &Settings) -> bool {
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
                self.stack.push(b / a);
            }
            b'%' => {
                let a = self.pop();
                let b = self.pop();
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
            b'#' => self.step_position_inner(), // skip forwards one

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
            b'&' => {
                todo!()
            }

            b'~' => {
                // FIXME TODO oh my god use a vecdqueue i beg
                let mut itr = self.input_buffer.chars();
                match itr.next() {
                    None => return true,
                    Some(chr) => {
                        self.stack.push(chr as i64);
                        self.input_buffer = itr.as_str().into();
                    }
                }
            }

            // halt is dealt with higher up
            b'@' => return true,

            // -- IO output
            b'.' => {
                let a = self.pop().to_string();
                self.output.push_str(&a);
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

                self.graphics = Some(Graphics::new(x as usize, y as usize));
            }

            b'f' => {
                // configure color
                if let Some(graphics) = &mut self.graphics {
                    let b = self.stack.pop().unwrap_or(0);
                    let g = self.stack.pop().unwrap_or(0);
                    let r = self.stack.pop().unwrap_or(0);
                    graphics.current_color = Color32::from_rgb(
                        b.try_into().unwrap(),
                        g.try_into().unwrap(),
                        r.try_into().unwrap(),
                    );
                }
            }

            b'x' => {
                // set pixel
                if let Some(graphics) = &mut self.graphics {
                    let y: usize = self.stack.pop().unwrap_or(0).try_into().unwrap();
                    let x: usize = self.stack.pop().unwrap_or(0).try_into().unwrap();

                    graphics.pixel(x, y);
                }
            }

            b'c' => {
                // fill
                if let Some(graphics) = &mut self.graphics {
                    graphics.texture =
                        vec![graphics.current_color; graphics.size.0 * graphics.size.1];
                }
            }

            b'u' => (), // update (noop for now)

            b'l' => {
                // line
                if let Some(graphics) = &mut self.graphics {
                    let y1: i32 = self.stack.pop().unwrap_or(0).try_into().unwrap();
                    let x1: i32 = self.stack.pop().unwrap_or(0).try_into().unwrap();

                    let y2: i32 = self.stack.pop().unwrap_or(0).try_into().unwrap();
                    let x2: i32 = self.stack.pop().unwrap_or(0).try_into().unwrap();

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
                            Event::MouseClick(x, y) => self.stack.extend([y, x, 4]),
                        }
                    } else {
                        self.stack.push(0);
                    }
                }
            }

            // noop
            b' ' => (),

            _ => panic!("invalid operation"),
        };
        false
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

        b'&' | b'~' | b'.' | b',' => OpTypes::IO,

        b's' | b'f' | b'x' | b'c' | b'u' | b'l' | b'z' => OpTypes::Graphics,
        b'@' => OpTypes::None,

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
