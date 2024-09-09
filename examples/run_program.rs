use std::collections::HashMap;
use std::error::Error;
use std::path::Path;
use std::rc::Rc;

use cairo_vm::cairo_run::{cairo_run_program_with_initial_scope, CairoRunConfig};
use cairo_vm::hint_processor::builtin_hint_processor::builtin_hint_processor_definition::HintFunc;
use cairo_vm::hint_processor::builtin_hint_processor::hint_utils::{
    get_ptr_from_var_name, insert_value_from_var_name,
};
use cairo_vm::types::exec_scope::ExecutionScopes;
use cairo_vm::types::layout_name::LayoutName;
use cairo_vm::types::program::Program;
use cairo_vm::vm::errors::cairo_run_errors::CairoRunError;
use cairo_vm::vm::runners::cairo_runner::CairoRunner;
use cairo_vm::Felt252;

use cairo_bootloader::bootloaders::load_bootloader;
use cairo_bootloader::tasks::make_bootloader_tasks;
use cairo_bootloader::{
    insert_bootloader_input, BootloaderConfig, BootloaderHintProcessor, BootloaderInput,
    PackedOutput, SimpleBootloaderInput, TaskSpec,
};

fn cairo_run_bootloader_in_proof_mode(
    bootloader_program: &Program,
    tasks: Vec<TaskSpec>,
) -> Result<CairoRunner, CairoRunError> {
    let mut hint_processor = BootloaderHintProcessor::new();
    hint_processor.add_hint(
        "ids.fibonacci_claim_index = program_input['fibonacci_claim_index']".to_string(),
        Rc::new(HintFunc(Box::new(
            |vm, exec_scopes, ids_data, ap_tracking, _constants| {
                let program_input: HashMap<String, serde_json::Value> = exec_scopes
                    .get::<HashMap<String, serde_json::Value>>("program_input")
                    .unwrap()
                    .clone();
                let fibonacci_claim_index: Felt252 = program_input
                    .get("fibonacci_claim_index")
                    .unwrap()
                    .as_u64()
                    .unwrap()
                    .into();
                insert_value_from_var_name(
                    "fibonacci_claim_index",
                    fibonacci_claim_index,
                    vm,
                    ids_data,
                    ap_tracking,
                )?;
                Ok(())
            },
        ))),
    );

    let cairo_run_config = CairoRunConfig {
        entrypoint: "main",
        trace_enabled: false,
        relocate_mem: false,
        layout: LayoutName::starknet_with_keccak,
        proof_mode: true,
        secure_run: None,
        disable_trace_padding: false,
        allow_missing_builtins: None,
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
    };

    // Note: the method used to set the bootloader input depends on
    // https://github.com/lambdaclass/cairo-vm/pull/1772 and may change depending on review.
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

fn main() -> Result<(), Box<dyn Error>> {
    let bootloader_program = load_bootloader()?;
    let program_paths = vec![Path::new("./examples/fibonacci_with_hint.json")];
    // let pie_paths = vec![Path::new(
    //     "./dependencies/test-programs/bootloader/pies/fibonacci/cairo_pie.zip",
    // )];
    let program_inputs_path = Path::new("./examples/fibonacci_input.json");
    let program_inputs_str = std::fs::read_to_string(program_inputs_path)?;
    let program_inputs =
        serde_json::from_str::<HashMap<String, serde_json::Value>>(&program_inputs_str)?;
    let program_inputs = vec![program_inputs];
    let tasks = make_bootloader_tasks(Some(&program_paths), Some(&program_inputs), None)?;
    // let tasks = make_bootloader_tasks(None, None, Some(&pie_paths))?;

    let mut runner = cairo_run_bootloader_in_proof_mode(&bootloader_program, tasks)?;

    let mut output_buffer = "Program Output:\n".to_string();
    runner.vm.write_output(&mut output_buffer)?;
    print!("{output_buffer}");

    Ok(())
}
