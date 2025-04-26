#![warn(clippy::all, rust_2018_idioms)]

mod app;
mod befunge;
pub use app::App;
pub use befunge::State as BefungeState;
