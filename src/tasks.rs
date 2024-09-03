use crate::{CairoPieTask, RunProgramTask, TaskSpec};
use cairo_vm::types::errors::program_errors::ProgramError;
use cairo_vm::types::program::Program;
use cairo_vm::vm::runners::cairo_pie::CairoPie;
use std::collections::HashMap;

#[derive(thiserror::Error, Debug)]
pub enum BootloaderTaskError {
    #[error("Failed to read program: {0}")]
    Program(#[from] ProgramError),

    #[error("Failed to read PIE: {0}")]
    Pie(#[from] std::io::Error),
}

pub fn make_bootloader_tasks(
    programs: &[&[u8]],
    program_inputs: Vec<HashMap<String, serde_json::Value>>,
    pies: &[&[u8]],
) -> Result<Vec<TaskSpec>, BootloaderTaskError> {
    let program_tasks =
        programs
            .iter()
            .zip(program_inputs.iter())
            .map(|(program_bytes, program_input)| {
                let program = Program::from_bytes(program_bytes, Some("main"))
                    .map_err(BootloaderTaskError::Program)?;
                Ok(TaskSpec::RunProgram(RunProgramTask {
                    program,
                    program_input: program_input.clone(),
                    use_poseidon: false,
                }))
            });

    let cairo_pie_tasks = pies.iter().map(|pie| {
        let cairo_pie = CairoPie::from_bytes(pie).map_err(BootloaderTaskError::Pie)?;
        Ok(TaskSpec::CairoPieTask(CairoPieTask {
            cairo_pie,
            use_poseidon: false,
        }))
    });

    program_tasks.chain(cairo_pie_tasks).collect()
}
