use std::any::Any;
use std::collections::HashMap;
use std::path::PathBuf;

use cairo_vm::serde::deserialize_program::Identifier;
use cairo_vm::types::errors::program_errors::ProgramError;
use cairo_vm::types::program::Program;
use cairo_vm::vm::runners::cairo_pie::CairoPie;
use cairo_vm::Felt252;
use serde::Deserialize;

pub type BootloaderVersion = u64;

pub(crate) type ProgramIdentifiers = HashMap<String, Identifier>;

#[derive(Deserialize, Debug, Clone, PartialEq)]
pub struct BootloaderConfig {
    pub simple_bootloader_program_hash: Felt252,
    pub supported_cairo_verifier_program_hashes: Vec<Felt252>,
}

#[derive(Deserialize, Debug, Default, Clone, PartialEq)]
pub struct CompositePackedOutput {
    pub outputs: Vec<Felt252>,
    pub subtasks: Vec<PackedOutput>,
}

impl CompositePackedOutput {
    pub fn elements_for_hash(&self) -> &Vec<Felt252> {
        &self.outputs
    }
}

#[derive(Deserialize, Debug, Clone, PartialEq)]
pub enum PackedOutput {
    Plain(Vec<Felt252>),
    Composite(CompositePackedOutput),
}

pub trait Task {
    fn get_program(&self) -> Result<Program, ProgramError>;
    fn as_any(&self) -> &dyn Any;
}

#[derive(Debug, Clone, PartialEq)]
pub enum TaskSpec {
    RunProgram(RunProgramTask),
    CairoPiePath(CairoPiePath),
    CairoPieTask(CairoPieTask),
}

impl TaskSpec {
    pub fn load_task(&self) -> Result<Box<dyn Task>, std::io::Error> {
        match self {
            TaskSpec::RunProgram(task) => Ok(Box::new(task.clone())),
            TaskSpec::CairoPiePath(path) => {
                let cairo_pie = CairoPie::read_zip_file(&path.path)?;
                Ok(Box::new(CairoPieTask {
                    cairo_pie,
                    use_poseidon: path.use_poseidon,
                }))
            }
            TaskSpec::CairoPieTask(task) => Ok(Box::new(task.clone())),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunProgramTask {
    pub program: Program,
    pub program_input: HashMap<String, serde_json::Value>,
    pub use_poseidon: bool,
}

impl Task for RunProgramTask {
    fn get_program(&self) -> Result<Program, ProgramError> {
        Ok(self.program.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl RunProgramTask {
    pub fn new(
        program: Program,
        program_input: HashMap<String, serde_json::Value>,
        use_poseidon: bool,
    ) -> Self {
        Self {
            program,
            program_input,
            use_poseidon,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CairoPiePath {
    pub path: PathBuf,
    pub use_poseidon: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CairoPieTask {
    pub cairo_pie: CairoPie,
    pub use_poseidon: bool,
}

impl Task for CairoPieTask {
    fn get_program(&self) -> Result<Program, ProgramError> {
        Ok(Program::from_stripped_program(
            &self.cairo_pie.metadata.program,
        ))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl CairoPieTask {
    pub fn new(cairo_pie: CairoPie, use_poseidon: bool) -> Self {
        Self {
            cairo_pie,
            use_poseidon,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SimpleBootloaderInput {
    pub fact_topologies_path: Option<PathBuf>,
    pub single_page: bool,
    pub tasks: Vec<TaskSpec>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BootloaderInput {
    pub simple_bootloader_input: SimpleBootloaderInput,
    pub bootloader_config: BootloaderConfig,
    pub packed_outputs: Vec<PackedOutput>,
    // Option not present in the original Cairo 0 hint implementation.
    // In the original implementation, all the outputs of the tasks are written to memory page 1 and onwards,
    // reserving page 0 for the bootloader program and arguments.
    // Setting this to true will ignore the fact_topologies and add all outputs of tasks to page 0.
    pub ignore_fact_topologies: bool,
}
