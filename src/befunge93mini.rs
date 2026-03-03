use coarsetime::{Duration, Instant};
use egui::{
    Color32,
    ahash::{HashSet, HashSetExt},
};
use rand::Rng;
use std::iter;

use egui::ahash::HashMap;

use crate::{
    app::{self, Settings},
    befunge::{
        Befunge, Direction, GraphicalEvent, Graphics, Position, StepStatus, Value, Visited,
        WhereVisited,
    },
};

#[derive(Clone)]
pub struct FungeSpace {
    zero_page: Box<[i8; 128 * 128]>,
}

#[derive(Clone)]
pub struct State {
    pub instruction_count: usize,
    pub map: FungeSpace,
    pub string_mode: bool,
    pub position: (i8, i8),
    pub direction: Direction,
    pub pos_history: HashMap<Position, Visited>,
    pub get_history: HashMap<Position, Instant>,
    pub put_history: HashMap<Position, Instant>,
    pub stack: Vec<i8>,
    pub output: String,
    pub graphics: Option<Graphics>,
    pub breakpoints: HashSet<Position>,
    //pub input_buffer: VecDeque<i64>,
    pub input_buffer: String,
}

// TODO: implement FungeSpaceTrait

impl FungeSpace {
    pub fn new() -> Self {
        Self {
            zero_page: Box::new([b' ' as i8; 128 * 128]),
        }
    }

    pub fn new_from_fungespace(input: app::FungeSpace) -> Self {
        let mut map = FungeSpace::new();
        for ((x, y), char) in input.map {
            if let Ok(x) = x.try_into()
                && let Ok(y) = y.try_into()
            {
                map.set((x, y), char as i8);
            }
        }
        map
    }

    pub fn set(&mut self, pos: (i8, i8), val: i8) {
        if pos.0 < 0 || pos.1 < 0 {
            return;
        };

        self.zero_page[(pos.0 as usize) + (pos.1 as usize) * 128] = val
    }

    pub fn get(&self, pos: (i8, i8)) -> i8 {
        if pos.0 < 0 || pos.1 < 0 {
            return 0;
        };

        self.zero_page[(pos.0 as usize) + (pos.1 as usize) * 128]
    }

    pub fn entries(&self) -> impl Iterator<Item = (Position, i8)> {
        self.zero_page.iter().enumerate().map(|(i, val)| {
            let i = i as i64;
            ((i % 127, i / 127), *val)
        })
    }

    pub fn serialize(&self) -> String {
        let mut lines: Vec<Vec<char>> = vec![vec![]; 127];
        for ((x, y), val) in self.entries() {
            let line = &mut lines[y as usize];
            if line.len() <= x as usize {
                line.extend(iter::repeat_n(' ', x as usize - line.len()));
                assert_ne!(val, b'\n' as i8);
                assert_ne!(val, b'\r' as i8);
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

impl Default for State {
    fn default() -> Self {
        Self {
            instruction_count: 0,
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
    fn pop(&mut self) -> i8 {
        self.stack.pop().unwrap_or(0)
    }

    pub fn new_from_fungespace(fungespace: app::FungeSpace) -> Self {
        Self {
            map: FungeSpace::new_from_fungespace(fungespace),
            ..Default::default()
        }
    }

    pub fn step_position(&mut self, settings: &Settings) {
        let (x, y) = self.position;
        self.step_position_inner();
        if settings.pos_history.0 {
            if let Some(visited) = self.pos_history.get_mut(&(x as i64, y as i64)) {
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
                    (x as i64, y as i64),
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
            self.position.0 = 127;
        } else if self.position.0.wrapping_sub(1) == 127 {
            self.position.0 = 0
        };

        if self.position.1 == -1 {
            self.position.1 = 127;
        } else if self.position.1.wrapping_sub(1) == 127 {
            self.position.1 = 0
        };
    }

    pub fn step(&mut self, settings: &Settings) -> StepStatus {
        self.instruction_count += 1;
        let status = self.step_inner(settings);
        if self
            .breakpoints
            .contains(&(self.position.0 as i64, self.position.1 as i64))
        {
            return StepStatus::Breakpoint;
        }
        // skip up to 100 spaces if not in string mode
        if settings.skip_spaces && !self.string_mode {
            let mut safety_counter = 0;
            loop {
                safety_counter += 1;
                if safety_counter < 1000 && self.map.get(self.position) == b' ' as i8 {
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
            if op == b'"' as i8 {
                self.string_mode = false;
            } else {
                self.stack.push(op);
            }
            self.step_position(settings);
            StepStatus::Normal
        } else if let Ok(op) = op.try_into() {
            let status = self.do_op(op, settings);
            match status {
                StepStatus::Normal | StepStatus::SyncFrame => {
                    self.step_position(settings);
                }
                _ => (),
            };
            status
        } else {
            StepStatus::Error("Invalid operation")
        }
    }

    fn do_op(&mut self, op: u8, settings: &Settings) -> StepStatus {
        match op {
            b'"' => self.string_mode = true,

            b'0'..=b'9' => self.stack.push((op - b'0') as i8),

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
                    return StepStatus::Error("Division by zero");
                }
                self.stack.push(b / a);
            }
            b'%' => {
                let a = self.pop();
                let b = self.pop();
                if a == 0 {
                    return StepStatus::Error("Division by zero");
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
            b'?' => self.direction = rand::thread_rng().r#gen(),
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
                    if let Some(prev_time) = self.put_history.get(&(x as i64, y as i64)) {
                        if prev_time.elapsed_since_recent() > Duration::from_millis(500) {
                            self.put_history
                                .insert((x as i64, y as i64), Instant::recent());
                        }
                    } else {
                        self.put_history
                            .insert((x as i64, y as i64), Instant::recent());
                    }
                }

                self.map.set((x, y), value);
            }

            // get
            b'g' => {
                let y = self.pop();
                let x = self.pop();
                self.stack.push(self.map.get((x, y)));

                if settings.get_history.0 {
                    if let Some(prev_time) = self.get_history.get(&(x as i64, y as i64)) {
                        if prev_time.elapsed_since_recent() > Duration::from_millis(500) {
                            self.get_history
                                .insert((x as i64, y as i64), Instant::recent());
                        }
                    } else {
                        self.get_history
                            .insert((x as i64, y as i64), Instant::recent());
                    }
                }
            }

            // input
            b'&' => {
                let mut itr = self.input_buffer.chars();
                let mut num = 0;
                loop {
                    match itr.next() {
                        None => return StepStatus::Breakpoint,
                        Some(val @ '0'..='9') => {
                            num *= 10;
                            num += (val as u8 - b'0') as i8;
                        }
                        Some(' ') => {
                            self.stack.push(num);
                            self.input_buffer = itr.as_str().into();
                            return StepStatus::Normal;
                        }
                        Some(_) => {
                            return StepStatus::Error("Invalid input for Error::InvalidNumber");
                        }
                    }
                }
            }

            b'~' => {
                let mut itr = self.input_buffer.chars();
                match itr.next() {
                    None => return StepStatus::Breakpoint,
                    Some(chr) => {
                        self.stack.push(chr as i8);
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
                let Ok(a) = (self.pop() as u32).try_into() else {
                    return StepStatus::Error("Invalid UTF-8 char");
                };
                self.output.push(a);
            }

            // befunge with graphics
            b's' => {
                // setup
                let y = self.pop();
                let x = self.pop();

                if y <= 0 || x <= 0 {
                    return StepStatus::Error("Out of bounds graphical operation");
                }

                self.graphics = Some(Graphics::new(x as usize, y as usize));
            }

            b'f' => {
                // configure color
                if let Some(graphics) = &mut self.graphics {
                    let r = self.stack.pop().unwrap_or(0) as u8;
                    let g = self.stack.pop().unwrap_or(0) as u8;
                    let b = self.stack.pop().unwrap_or(0) as u8;

                    graphics.current_color = Color32::from_rgb(r, g, b);
                }
            }

            b'x' => {
                // set pixel
                if let Some(graphics) = &mut self.graphics {
                    let y = self.stack.pop().unwrap_or(0);
                    let x = self.stack.pop().unwrap_or(0);

                    return graphics.pixel(x as i64, y as i64);
                }
            }

            b'c' => {
                // fill
                if let Some(graphics) = &mut self.graphics {
                    graphics.fill();
                }
            }

            b'u' => return StepStatus::SyncFrame, // update

            b'l' => {
                // line
                if let Some(graphics) = &mut self.graphics {
                    let y1 = self.stack.pop().unwrap_or(0) as i32;
                    let x1 = self.stack.pop().unwrap_or(0) as i32;

                    let y2 = self.stack.pop().unwrap_or(0) as i32;
                    let x2 = self.stack.pop().unwrap_or(0) as i32;

                    if x1 >= graphics.size.0 as i32
                        || y1 >= graphics.size.1 as i32
                        || x2 >= graphics.size.0 as i32
                        || y2 >= graphics.size.1 as i32
                    {
                        return StepStatus::Error("Out of bounds graphical operation");
                    }

                    graphics.line(x1, y1, x2, y2);
                }
            }

            b'z' => {
                // event
                if let Some(graphics) = &mut self.graphics {
                    if let Some(event) = graphics.event_queue.pop_front() {
                        match event {
                            //None is event 0
                            GraphicalEvent::Close => self.stack.extend([1]),
                            //Event::KeyDown(key) => self.stack.extend([key,2]),
                            //Event::KeyUp(key) => self.stack.extend([key,3]),
                            GraphicalEvent::MouseClick((x, y)) => {
                                self.stack.extend([x as i8, y as i8, 4])
                            }
                        }
                    } else {
                        self.stack.push(0);
                    }
                }
            }

            // noop
            b' ' => (),

            _ => return StepStatus::Error("Invalid operation"),
        };
        StepStatus::Normal
    }
}

impl Befunge for State {
    fn get(&self, pos: Position) -> Value {
        self.map.get((pos.0 as i8, pos.1 as i8)) as Value
    }
    fn set(&mut self, pos: Position, val: Value) {
        self.map.set((pos.0 as i8, pos.1 as i8), val as i8);
    }
    fn step(&mut self, settings: &Settings) -> StepStatus {
        self.step(settings)
    }

    fn program_size(&self) -> Position {
        (128, 128)
    }
    fn instruction_count(&self) -> usize {
        self.instruction_count
    }
    fn string_mode(&self) -> bool {
        self.string_mode
    }
    fn cursor_position(&self) -> Position {
        (self.position.0 as i64, self.position.1 as i64)
    }
    fn cursor_direction(&self) -> Direction {
        self.direction
    }

    fn stack(&self) -> Vec<i64> {
        self.stack.iter().map(|a| *a as i64).collect::<Vec<_>>()
    }
    fn stdout(&self) -> &str {
        &self.output
    }
    fn stdin(&mut self) -> &mut String {
        &mut self.input_buffer
    }
    fn graphics(&mut self) -> Option<&mut Graphics> {
        self.graphics.as_mut()
    }

    fn pos_history(&mut self) -> &mut HashMap<Position, Visited> {
        &mut self.pos_history
    }
    fn get_history(&mut self) -> &mut HashMap<Position, Instant> {
        &mut self.get_history
    }
    fn put_history(&mut self) -> &mut HashMap<Position, Instant> {
        &mut self.put_history
    }
    fn breakpoints(&mut self) -> &mut HashSet<Position> {
        &mut self.breakpoints
    }

    fn serialize(&self) -> String {
        self.map.serialize()
    }
}
