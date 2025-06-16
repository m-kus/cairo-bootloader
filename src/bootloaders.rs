use cairo_vm::types::errors::program_errors::ProgramError;
use cairo_vm::types::program::Program;

pub use crate::hints::*;

const BOOTLOADER: &[u8] = include_bytes!("../resources/stwo-bootloader.json");

/// Loads the bootloader and returns it as a Cairo VM `Program` object.
pub fn load_bootloader() -> Result<Program, ProgramError> {
    Program::from_bytes(BOOTLOADER, Some("main"))
}
