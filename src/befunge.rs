use clipline::AnyOctant;
//use rand::{distr::StandardUniform, prelude::*};
use coarsetime::{Duration, Instant};
use egui::Color32;
use std::collections::VecDeque;

use egui::ahash::HashMap;
/*
#[cfg(target_arch = "wasm32")]
use egui::ahash::HashMap;
#[cfg(not(target_arch = "wasm32"))]
use gxhash::HashMap;
*/

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

pub struct FungeSpace {
    map: HashMap<(i64, i64), i64>,
    zero_page: [i64; 100],
}

pub enum Event {
    Close,
    //KeyDown(i64),
    //KeyUp(i64),
    MouseClick(i64, i64),
}

pub struct Graphics {
    pub size: (usize, usize),
    pub texture: Vec<Color32>,
    pub current_color: Color32,
    pub event_queue: VecDeque<Event>,
}

pub struct State {
    pub map: FungeSpace,
    pub position: (i64, i64),
    pub direction: Direction,
    pub pos_history: HashMap<(i64, i64), Instant>,
    pub stack: Vec<i64>,
    pub string_mode: bool,
    pub output: String,
    halted: bool,
    pub graphics: Option<Graphics>,
}

impl FungeSpace {
    pub fn new() -> Self {
        Self {
            map: HashMap::default(),
            zero_page: [b' '.into(); 100],
        }
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
            Some(self.zero_page[(pos.0 + pos.1 * 10) as usize])
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

    pub fn serialize(&mut self) -> String {
        todo!()
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
            stack: Vec::new(),
            output: String::new(),
            halted: false,
            graphics: None,
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

    pub fn new_from_string(input: String) -> Self {
        let mut map = FungeSpace::new();
        for (y, line) in input.lines().enumerate() {
            for (x, char) in line.chars().enumerate() {
                map.set((x.try_into().unwrap(), y.try_into().unwrap()), char as i64);
            }
        }
        Self {
            map,
            ..Default::default()
        }
    }

    fn step_position(&mut self) {
        let (x, y) = self.position;
        if let Some(prev_time) = self.pos_history.get(&(x, y)) {
            if prev_time.elapsed_since_recent() > Duration::from_millis(500) {
                self.pos_history.insert((x, y), Instant::recent());
            }
        } else {
            self.pos_history.insert((x, y), Instant::recent());
        }
        match self.direction {
            Direction::North => self.position = (x, y - 1),
            Direction::South => self.position = (x, y + 1),
            Direction::East => self.position = (x + 1, y),
            Direction::West => self.position = (x - 1, y),
        }
    }

    pub fn step(&mut self) {
        if self.halted {
            return;
        }

        self.step_inner();
        let mut safety_counter = 0;
        loop {
            safety_counter += 1;
            if safety_counter < 100 && self.map.get_wrapped(self.position) == b' ' as i64 {
                self.step_position();
            } else {
                break;
            }
        }
    }

    fn step_inner(&mut self) {
        if self.halted {
            return;
        }
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
        {
            self.do_op(op);
        }
        self.step_position();
    }

    fn do_op(&mut self, op: u8) {
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
            b'#' => self.step_position(), // skip forwards one

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

                self.map.set((x, y), value);
            }

            // get
            b'g' => {
                let y = self.pop();
                let x = self.pop();
                self.stack.push(self.map.get_wrapped((x, y)));
            }

            // input
            b'&' | b'~' => {}

            // halt
            b'@' => {
                self.halted = true;
            }

            // -- IO output
            b'.' => {}
            b',' => {}

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
        }
    }
}
