# A befunge editor

A fast 64 bit befunge93 IDE with breakpoint support. Inspired largely by [BefunExec](https://github.com/Mikescher/BefunExec). Can run [in the web](https://partywumpus.github.io/befunge-editor/) or locally.
I estimate a speed of about 30MHz max on my CPU (with the position history turned off, it lowers to 25MHz with it on). It likely runs slower in the web, but is still fast.

https://github.com/user-attachments/assets/43f07a05-3276-41d9-9f64-e17970626852

## Running locally

If you have nix, you can run `nix develop` to get all depencencies, otherwise you should take a look inside flake.nix and figure out how to get the deps you need.

To run: `cargo run --release`

## Features

- Breakpoints
- Effectively infinite fungespace (up to 2^64)
- Supports (most of) the [befunge-with-graphics](https://github.com/Jachdich/befunge-with-graphics) operations

## Features I would like to add in future:

- A reload hotkey (possibly a "clever" one like befunexec that keeps the state?), although won't work on the web...
- Watching the values of locations (like befunexec)
- Breakpoints that pause on value change
- Some of the preprocessor things from befunexec (break & watch, but not replace)
- An easier way to move the screen large distances. Possibly a "minimap" style thing?
