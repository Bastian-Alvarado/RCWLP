//! RCWLP host: turns paired Android phones (with a controller attached) into virtual Xbox 360
//! or DualShock 4 controllers on this PC. No video, no audio, just input.
//!
//! [`server`] holds the protocol logic and [`platform`] runs it on a real UDP socket and the
//! ViGEmBus driver. The console host (`main.rs`) and the tray app share both.

pub mod clock;
pub mod crypto;
pub mod pad;
pub mod platform;
pub mod protocol;
pub mod server;
pub mod store;

#[cfg(test)]
mod vectors;
