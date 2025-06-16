use std::error::Error;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use bincode::enc::write::Writer;
use clap::Parser;

use cairo_vm::air_public_input::PublicInput;
use cairo_vm::cairo_run::{cairo_run_program_with_initial_scope, write_encoded_memory, write_encoded_trace, CairoRunConfig};
use cairo_vm::types::exec_scope::ExecutionScopes;
use cairo_vm::types::layout_name::LayoutName;
use cairo_vm::types::program::Program;
use cairo_vm::vm::errors::cairo_run_errors::CairoRunError;
use cairo_vm::vm::runners::cairo_runner::CairoRunner;
use cairo_vm::vm::trace::trace_entry::RelocatedTraceEntry;
use cairo_vm::Felt252;

use cairo_bootloader::bootloaders::load_bootloader;
use cairo_bootloader::tasks::make_bootloader_tasks;
use cairo_bootloader::{
    insert_bootloader_input, BootloaderConfig, BootloaderHintProcessor, BootloaderInput,
    PackedOutput, SimpleBootloaderInput, TaskSpec,
};
use serde::{Deserialize, Serialize};

fn cairo_run_bootloader_in_proof_mode(
    bootloader_program: &Program,
    tasks: Vec<TaskSpec>,
) -> Result<CairoRunner, CairoRunError> {
    let mut hint_processor = BootloaderHintProcessor::new();

    let cairo_run_config = CairoRunConfig {
        entrypoint: "main",
        trace_enabled: true,
        relocate_mem: true,
        layout: LayoutName::all_cairo_stwo,
        proof_mode: true,
        secure_run: None,
        disable_trace_padding: true,
        allow_missing_builtins: None,
        dynamic_layout_params: None,
        ..Default::default()
    };

    // Build the bootloader input
    let n_tasks = tasks.len();
    let bootloader_input = BootloaderInput {
        simple_bootloader_input: SimpleBootloaderInput {
            fact_topologies_path: None,
            single_page: false,
            tasks,
        },
        bootloader_config: BootloaderConfig {
            simple_bootloader_program_hash: Felt252::from(0),
            supported_cairo_verifier_program_hashes: vec![],
        },
        packed_outputs: vec![PackedOutput::Plain(vec![]); n_tasks],
        ignore_fact_topologies: true,
    };

    let mut exec_scopes = ExecutionScopes::new();
    insert_bootloader_input(&mut exec_scopes, bootloader_input);

    // Run the bootloader
    cairo_run_program_with_initial_scope(
        &bootloader_program,
        &cairo_run_config,
        &mut hint_processor,
        exec_scopes,
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrivateInput {
    pub trace_path: PathBuf,
    pub memory_path: PathBuf,
}

pub struct FileWriter {
    buf_writer: std::io::BufWriter<std::fs::File>,
    bytes_written: usize,
}

impl Writer for FileWriter {
    fn write(&mut self, bytes: &[u8]) -> Result<(), bincode::error::EncodeError> {
        self.buf_writer
            .write_all(bytes)
            .map_err(|e| bincode::error::EncodeError::Io {
                inner: e,
                index: self.bytes_written,
            })?;

        self.bytes_written += bytes.len();

        Ok(())
    }
}

impl FileWriter {
    fn new(buf_writer: std::io::BufWriter<std::fs::File>) -> Self {
        Self {
            buf_writer,
            bytes_written: 0,
        }
    }
}

pub fn prover_input_from_runner<'r>(runner: &'r CairoRunner, output_dir: &Path) -> (PrivateInput, PublicInput<'r>) {
    let public_input = runner.get_air_public_input().unwrap();
    let trace = runner
        .relocated_trace
        .as_ref()
        .unwrap()
        .iter()
        .map(|x| RelocatedTraceEntry {
            ap: x.ap,
            fp: x.fp,
            pc: x.pc,
        })
        .collect::<Vec<_>>();
   
    let trace_path = output_dir.join("trace");
    let trace_file = File::create(&trace_path).unwrap();
    let mut trace_writer =
            FileWriter::new(std::io::BufWriter::with_capacity(3 * 1024 * 1024, trace_file));
    write_encoded_trace(&trace, &mut trace_writer).unwrap();

    let memory_path = output_dir.join("memory");
    let memory_file = File::create(&memory_path).unwrap();
    let mut memory_writer =
            FileWriter::new(std::io::BufWriter::with_capacity(5 * 1024 * 1024, memory_file));
    write_encoded_memory(&runner.relocated_memory, &mut memory_writer).unwrap();

    let private_input = PrivateInput {
        trace_path: std::fs::canonicalize(&trace_path).unwrap(),
        memory_path: std::fs::canonicalize(&memory_path).unwrap(),
    };
    (private_input, public_input)
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Paths to the PIE files (*.zip)
    #[arg(short, long, num_args = 1..)]
    pie: Vec<PathBuf>,

    /// Output directory for the generated files
    #[arg(short, long)]
    output_path: PathBuf,
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    let bootloader_program = load_bootloader()?;
    
    let pie_paths: Vec<&Path> = args.pie.iter().map(|p| p.as_ref()).collect();
    let tasks = make_bootloader_tasks(None, None, Some(&pie_paths))?;

    let mut runner = cairo_run_bootloader_in_proof_mode(&bootloader_program, tasks)?;

    let mut output_buffer = "Program Output:\n".to_string();
    runner.vm.write_output(&mut output_buffer)?;
    print!("{output_buffer}");

    std::fs::create_dir_all(&args.output_path).unwrap();
    let (private_input, public_input) = prover_input_from_runner(&runner, &args.output_path);

    let priv_json = serde_json::to_string(&private_input).unwrap();
    let pub_json = serde_json::to_string(&public_input).unwrap();
    std::fs::write(args.output_path.join("priv.json"), priv_json).unwrap();
    std::fs::write(args.output_path.join("pub.json"), pub_json).unwrap();

    Ok(())
}
