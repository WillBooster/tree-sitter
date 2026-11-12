#![cfg_attr(not(any(test, doctest)), doc = include_str!("../README.md"))]

pub mod fuzz;
pub mod generation_profile;
pub mod init;
pub mod input;
pub mod logger;
pub mod paint;
pub mod parse;
pub mod playground;
pub mod test;
pub mod util;
pub mod version;
pub mod wasm;

#[cfg(test)]
mod tests;

#[cfg(doctest)]
mod tests;
