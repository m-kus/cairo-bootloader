use std::any::Any;
use std::collections::HashMap;

use cairo_vm::hint_processor::builtin_hint_processor::hint_utils::{
    get_ptr_from_var_name, get_relocatable_from_var_name, insert_value_from_var_name,
};
use cairo_vm::hint_processor::hint_processor_definition::{
    ExtensionData, HintExtension, HintProcessor, HintReference,
};
use cairo_vm::serde::deserialize_program::{ApTracking, Identifier};
use cairo_vm::types::builtin_name::BuiltinName;
use cairo_vm::types::exec_scope::ExecutionScopes;
use cairo_vm::types::program::Program;
use cairo_vm::types::relocatable::Relocatable;
use cairo_vm::vm::errors::hint_errors::HintError;
use cairo_vm::vm::errors::memory_errors::MemoryError;
use cairo_vm::vm::runners::builtin_runner::{OutputBuiltinRunner, OutputBuiltinState};
use cairo_vm::vm::runners::cairo_pie::{CairoPie, StrippedProgram};
use cairo_vm::vm::vm_core::VirtualMachine;
use cairo_vm::{any_box, Felt252};
use starknet_crypto::FieldElement;

use crate::hints::fact_topologies::{get_task_fact_topology, FactTopology};
use crate::hints::load_cairo_pie::load_cairo_pie;
use crate::hints::program_hash::compute_program_hash_chain;
use crate::hints::program_loader::ProgramLoader;
use crate::hints::types::{BootloaderVersion, ProgramIdentifiers, Task};
use crate::hints::vars;
use crate::TaskSpec;

use super::types::{CairoPieTask, RunProgramTask};

fn get_stripped_program_from_task(task: &Box<dyn Task>) -> Result<StrippedProgram, HintError> {
    task.get_program()
        .map_err(|e| HintError::CustomHint(e.to_string().into_boxed_str()))
        .and_then(|p| {
            p.get_stripped_program()
                .map_err(|e| HintError::CustomHint(e.to_string().into_boxed_str()))
        })
}

fn get_program_from_task(task: &Box<dyn Task>) -> Result<Program, HintError> {
    task.get_program()
        .map_err(|e| HintError::CustomHint(e.to_string().into_boxed_str()))
}

fn get_task_from_exec_scopes(exec_scopes: &ExecutionScopes) -> Result<Box<dyn Task>, HintError> {
    let local_variables = exec_scopes.get_local_variables()?;
    let task_spec: &TaskSpec = local_variables
        .get(vars::TASK)
        .unwrap()
        .downcast_ref::<TaskSpec>()
        .unwrap();
    let task = task_spec
        .load_task()
        .map_err(|e| HintError::CustomHint(e.to_string().into_boxed_str()))?;
    Ok(task)
}

/// Implements %{ ids.program_data_ptr = program_data_base = segments.add() %}.
///
/// Creates a new segment to store the program data.
pub fn allocate_program_data_segment(
    vm: &mut VirtualMachine,
    exec_scopes: &mut ExecutionScopes,
    ids_data: &HashMap<String, HintReference>,
    ap_tracking: &ApTracking,
) -> Result<HintExtension, HintError> {
    let program_data_segment = vm.add_memory_segment();
    exec_scopes.insert_value(vars::PROGRAM_DATA_BASE, program_data_segment);
    insert_value_from_var_name(
        "program_data_ptr",
        program_data_segment,
        vm,
        ids_data,
        ap_tracking,
    )?;

    Ok(HashMap::new())
}

fn field_element_to_felt(field_element: FieldElement) -> Felt252 {
    let bytes = field_element.to_bytes_be();
    Felt252::from_bytes_be(&bytes)
}

/// Implements
///
/// from starkware.cairo.bootloaders.simple_bootloader.utils import load_program
///
/// # Call load_program to load the program header and code to memory.
/// program_address, program_data_size = load_program(
///     task=task, memory=memory, program_header=ids.program_header,
///     builtins_offset=ids.ProgramHeader.builtin_list)
/// segments.finalize(program_data_base.segment_index, program_data_size)
pub fn load_program_hint(
    vm: &mut VirtualMachine,
    exec_scopes: &mut ExecutionScopes,
    ids_data: &HashMap<String, HintReference>,
    ap_tracking: &ApTracking,
) -> Result<HintExtension, HintError> {
    let program_data_base: Relocatable = exec_scopes.get(vars::PROGRAM_DATA_BASE)?;
    let task = get_task_from_exec_scopes(exec_scopes)?;
    let program = get_stripped_program_from_task(&task)?;

    let program_header_ptr = get_ptr_from_var_name("program_header", vm, ids_data, ap_tracking)?;

    // Offset of the builtin_list field in `ProgramHeader`, cf. execute_task.cairo
    let builtins_offset = 4;
    let mut program_loader = ProgramLoader::new(vm, builtins_offset);
    let bootloader_version: BootloaderVersion = 0;
    let loaded_program = program_loader
        .load_program(program_header_ptr, &program, Some(bootloader_version))
        .map_err(Into::<HintError>::into)?;

    vm.segments.finalize(
        Some(loaded_program.size),
        program_data_base.segment_index as usize,
        None,
    );

    exec_scopes.insert_value(vars::PROGRAM_ADDRESS, loaded_program.code_address);

    Ok(HashMap::new())
}

/// Implements
/// from starkware.cairo.bootloaders.simple_bootloader.utils import get_task_fact_topology
///
/// # Add the fact topology of the current task to 'fact_topologies'.
/// output_start = ids.pre_execution_builtin_ptrs.output
/// output_end = ids.return_builtin_ptrs.output
/// fact_topologies.append(get_task_fact_topology(
///     output_size=output_end - output_start,
///     task=task,
///     output_builtin=output_builtin,
///     output_runner_data=output_runner_data,
/// ))
pub fn append_fact_topologies(
    vm: &mut VirtualMachine,
    exec_scopes: &mut ExecutionScopes,
    ids_data: &HashMap<String, HintReference>,
    ap_tracking: &ApTracking,
) -> Result<HintExtension, HintError> {
    let task = get_task_from_exec_scopes(exec_scopes)?;
    let output_runner_data = exec_scopes.get(vars::OUTPUT_RUNNER_DATA)?;

    let pre_execution_builtin_ptrs_addr =
        get_relocatable_from_var_name("pre_execution_builtin_ptrs", vm, ids_data, ap_tracking)?;
    let return_builtin_ptrs_addr =
        get_relocatable_from_var_name("return_builtin_ptrs", vm, ids_data, ap_tracking)?;

    // The output field is the first one in the BuiltinData struct
    let output_start = vm.get_relocatable(pre_execution_builtin_ptrs_addr)?;
    let output_end = vm.get_relocatable(return_builtin_ptrs_addr)?;
    let output_size = (output_end - output_start)?;

    let output_builtin = vm.get_output_builtin_mut()?;
    let fact_topology =
        get_task_fact_topology(output_size, &task, output_builtin, output_runner_data)
            .map_err(Into::<HintError>::into)?;
    exec_scopes
        .get_mut_ref::<Vec<FactTopology>>(vars::FACT_TOPOLOGIES)?
        .push(fact_topology);

    Ok(HashMap::new())
}

/// Implements
/// # Validate hash.
/// from starkware.cairo.bootloaders.hash_program import compute_program_hash_chain
///
/// assert memory[ids.output_ptr + 1] == compute_program_hash_chain(
///     program=task.get_program(),
///     use_poseidon=bool(ids.use_poseidon)), 'Computed hash does not match input.'
pub fn validate_hash(
    vm: &mut VirtualMachine,
    exec_scopes: &mut ExecutionScopes,
    ids_data: &HashMap<String, HintReference>,
    ap_tracking: &ApTracking,
) -> Result<HintExtension, HintError> {
    let task = get_task_from_exec_scopes(exec_scopes)?;
    let program = get_stripped_program_from_task(&task)?;

    let output_ptr = get_ptr_from_var_name("output_ptr", vm, ids_data, ap_tracking)?;
    let program_hash_ptr = (output_ptr + 1)?;

    let program_hash = vm.get_integer(program_hash_ptr)?.into_owned();

    // Compute the hash of the program
    let computed_program_hash = compute_program_hash_chain(&program, 0).map_err(|e| {
        HintError::CustomHint(format!("Could not compute program hash: {e}").into_boxed_str())
    })?;
    let computed_program_hash = field_element_to_felt(computed_program_hash);

    if program_hash != computed_program_hash {
        return Err(HintError::AssertionFailed(
            "Computed hash does not match input"
                .to_string()
                .into_boxed_str(),
        ));
    }

    Ok(HashMap::new())
}

/// List of all builtins in the order used by the bootloader.
pub const ALL_BUILTINS: [BuiltinName; 11] = [
    BuiltinName::output,
    BuiltinName::pedersen,
    BuiltinName::range_check,
    BuiltinName::ecdsa,
    BuiltinName::bitwise,
    BuiltinName::ec_op,
    BuiltinName::keccak,
    BuiltinName::poseidon,
    BuiltinName::range_check96,
    BuiltinName::add_mod,
    BuiltinName::mul_mod,
];

fn check_cairo_pie_builtin_usage(
    vm: &mut VirtualMachine,
    builtin_name: &BuiltinName,
    builtin_index: usize,
    cairo_pie: &CairoPie,
    return_builtins_addr: Relocatable,
    pre_execution_builtins_addr: Relocatable,
) -> Result<HintExtension, HintError> {
    let return_builtin_value = vm.get_relocatable((return_builtins_addr + builtin_index)?)?;
    let pre_execution_builtin_value =
        vm.get_relocatable((pre_execution_builtins_addr + builtin_index)?)?;
    let expected_builtin_size = (return_builtin_value - pre_execution_builtin_value)?;

    let builtin_size = cairo_pie.metadata.builtin_segments[builtin_name].size;

    if builtin_size != expected_builtin_size {
        return Err(HintError::AssertionFailed(
            "Builtin usage is inconsistent with the CairoPie."
                .to_string()
                .into_boxed_str(),
        ));
    }

    Ok(HashMap::new())
}

/// Writes the updated builtin pointers after the program execution to the given return builtins
/// address.
///
/// `used_builtins` is the list of builtins used by the program and thus updated by it.
fn write_return_builtins(
    vm: &mut VirtualMachine,
    return_builtins_addr: Relocatable,
    used_builtins: &[BuiltinName],
    used_builtins_addr: Relocatable,
    pre_execution_builtins_addr: Relocatable,
    task: &Box<dyn Task>,
) -> Result<HintExtension, HintError> {
    let mut used_builtin_offset: usize = 0;
    for (index, builtin) in ALL_BUILTINS.iter().enumerate() {
        if used_builtins.contains(builtin) {
            let builtin_value = vm.get_relocatable((used_builtins_addr + used_builtin_offset)?)?;
            vm.insert_value((return_builtins_addr + index)?, builtin_value)?;
            used_builtin_offset += 1;

            if let Some(cairo_pie_task) = task.as_any().downcast_ref::<CairoPieTask>() {
                check_cairo_pie_builtin_usage(
                    vm,
                    builtin,
                    index,
                    &cairo_pie_task.cairo_pie,
                    return_builtins_addr,
                    pre_execution_builtins_addr,
                )?;
            }
        }
        // The builtin is unused, hence its value is the same as before calling the program.
        else {
            let pre_execution_builtin_addr = (pre_execution_builtins_addr + index)?;
            let pre_execution_value =
                vm.get_maybe(&pre_execution_builtin_addr).ok_or_else(|| {
                    MemoryError::UnknownMemoryCell(Box::new(pre_execution_builtin_addr))
                })?;
            vm.insert_value((return_builtins_addr + index)?, pre_execution_value)?;
        }
    }
    Ok(HashMap::new())
}

/// Implements
/// from starkware.cairo.bootloaders.simple_bootloader.utils import write_return_builtins
///
/// # Fill the values of all builtin pointers after executing the task.
/// builtins = task.get_program().builtins
/// write_return_builtins(
///     memory=memory, return_builtins_addr=ids.return_builtin_ptrs.address_,
///     used_builtins=builtins, used_builtins_addr=ids.used_builtins_addr,
///     pre_execution_builtins_addr=ids.pre_execution_builtin_ptrs.address_, task=task)
///
/// vm_enter_scope({'n_selected_builtins': n_builtins})
///
/// This hint looks at the builtins written by the program and merges them with the stored
/// pre-execution values (stored in a struct named ids.pre_execution_builtin_ptrs) to
/// create a final BuiltinData struct for the program.
pub fn write_return_builtins_hint(
    vm: &mut VirtualMachine,
    exec_scopes: &mut ExecutionScopes,
    ids_data: &HashMap<String, HintReference>,
    ap_tracking: &ApTracking,
) -> Result<HintExtension, HintError> {
    let task = get_task_from_exec_scopes(exec_scopes)?;
    let n_builtins: usize = exec_scopes.get(vars::N_BUILTINS)?;

    // builtins = task.get_program().builtins
    let program = get_stripped_program_from_task(&task)?;
    let builtins = &program.builtins;

    // write_return_builtins(
    //     memory=memory, return_builtins_addr=ids.return_builtin_ptrs.address_,
    //     used_builtins=builtins, used_builtins_addr=ids.used_builtins_addr,
    //     pre_execution_builtins_addr=ids.pre_execution_builtin_ptrs.address_, task=task)
    let return_builtins_addr =
        get_relocatable_from_var_name("return_builtin_ptrs", vm, ids_data, ap_tracking)?;
    let used_builtins_addr =
        get_ptr_from_var_name("used_builtins_addr", vm, ids_data, ap_tracking)?;
    let pre_execution_builtins_addr =
        get_relocatable_from_var_name("pre_execution_builtin_ptrs", vm, ids_data, ap_tracking)?;

    write_return_builtins(
        vm,
        return_builtins_addr,
        builtins,
        used_builtins_addr,
        pre_execution_builtins_addr,
        &task,
    )?;

    // vm_enter_scope({'n_selected_builtins': n_builtins})
    let n_builtins: Box<dyn Any> = Box::new(n_builtins);
    exec_scopes.enter_scope(HashMap::from([(
        vars::N_SELECTED_BUILTINS.to_string(),
        n_builtins,
    )]));

    Ok(HashMap::new())
}

fn get_bootloader_identifiers(
    exec_scopes: &ExecutionScopes,
) -> Result<&ProgramIdentifiers, HintError> {
    if let Some(bootloader_identifiers) =
        exec_scopes.data[0].get(vars::BOOTLOADER_PROGRAM_IDENTIFIERS)
    {
        if let Some(program) = bootloader_identifiers.downcast_ref::<ProgramIdentifiers>() {
            return Ok(program);
        }
    }

    Err(HintError::VariableNotInScopeError(
        vars::BOOTLOADER_PROGRAM_IDENTIFIERS
            .to_string()
            .into_boxed_str(),
    ))
}

fn get_identifier(
    identifiers: &HashMap<String, Identifier>,
    name: &str,
) -> Result<usize, HintError> {
    if let Some(identifier) = identifiers.get(name) {
        if let Some(pc) = identifier.pc {
            return Ok(pc);
        }
    }

    Err(HintError::VariableNotInScopeError(
        name.to_string().into_boxed_str(),
    ))
}

/*
Implements hint:
%{
    "from starkware.cairo.bootloaders.simple_bootloader.objects import (
        CairoPieTask,
        RunProgramTask,
        Task,
    )
    from starkware.cairo.bootloaders.simple_bootloader.utils import (
        load_cairo_pie,
        prepare_output_runner,
    )

    assert isinstance(task, Task)
    n_builtins = len(task.get_program().builtins)
    new_task_locals = {}
    if isinstance(task, RunProgramTask):
        new_task_locals['program_input'] = task.program_input
        new_task_locals['WITH_BOOTLOADER'] = True

        vm_load_program(task.program, program_address)
    elif isinstance(task, CairoPieTask):
        ret_pc = ids.ret_pc_label.instruction_offset_ - ids.call_task.instruction_offset_ + pc
        load_cairo_pie(
            task=task.cairo_pie, memory=memory, segments=segments,
            program_address=program_address, execution_segment_address= ap - n_builtins,
            builtin_runners=builtin_runners, ret_fp=fp, ret_pc=ret_pc)
    else:
        raise NotImplementedError(f'Unexpected task type: {type(task).__name__}.')

    output_runner_data = prepare_output_runner(
        task=task,
        output_builtin=output_builtin,
        output_ptr=ids.pre_execution_builtin_ptrs.output)
    vm_enter_scope(new_task_locals)"
%}
*/
pub fn call_task(
    hint_processor: &mut dyn HintProcessor,
    vm: &mut VirtualMachine,
    exec_scopes: &mut ExecutionScopes,
    ids_data: &HashMap<String, HintReference>,
    ap_tracking: &ApTracking,
) -> Result<HintExtension, HintError> {
    let mut hint_extension = HashMap::new();
    // assert isinstance(task, Task)
    let task = get_task_from_exec_scopes(exec_scopes)?;
    // n_builtins = len(task.get_program().builtins)
    let n_builtins = get_stripped_program_from_task(&task)?.builtins.len();

    let mut new_task_locals = HashMap::new();

    if let Some(run_program_task) = task.as_any().downcast_ref::<RunProgramTask>() {
        let program_input = run_program_task.program_input.clone();
        // new_task_locals['program_input'] = task.program_input
        new_task_locals.insert("program_input".to_string(), any_box![program_input]);
        // new_task_locals['WITH_BOOTLOADER'] = True
        new_task_locals.insert("WITH_BOOTLOADER".to_string(), any_box![true]);

        // TODO: the content of this function is mostly useless for the Rust VM.
        //       check with SW if there is nothing of interest here.
        // vm_load_program(task.program, program_address)
        let task_hint_extension = vm_load_program(hint_processor, exec_scopes)?;
        hint_extension.extend(task_hint_extension);
    } else if let Some(cairo_pie_task) = task.as_any().downcast_ref::<CairoPieTask>() {
        let program_address: Relocatable = exec_scopes.get("program_address")?;

        // ret_pc = ids.ret_pc_label.instruction_offset_ - ids.call_task.instruction_offset_ + pc
        // TODO: replace with proper way of getting `ret_pc_label` and `call_task` labels from `cairo-vm`
        // Temporary solution:
        //   `starkware.cairo.bootloaders.simple_bootloader.execute_task.execute_task.ret_pc_label` is a label at pc=279 and
        //   `starkware.cairo.bootloaders.simple_bootloader.execute_task.execute_task.call_task` is a label at pc=278
        //   And since this hint is called at pc=278, `ret_pc` can be calculated as:
        //     ret_pc = 279 - 278 + pc = 1 + pc
        let ret_pc = (vm.get_pc() + 1)?;

        // load_cairo_pie(
        //     task=task.cairo_pie, memory=memory, segments=segments,
        //     program_address=program_address, execution_segment_address= ap - n_builtins,
        //     builtin_runners=builtin_runners, ret_fp=fp, ret_pc=ret_pc)
        load_cairo_pie(
            &cairo_pie_task.cairo_pie,
            vm,
            program_address,
            (vm.get_ap() - n_builtins)?,
            vm.get_fp(),
            ret_pc,
        )
        .map_err(Into::<HintError>::into)?;
    } else {
        return Err(HintError::CustomHint(
            "Unexpected task type".to_string().into_boxed_str(),
        ));
    }

    // output_runner_data = prepare_output_runner(
    //     task=task,
    //     output_builtin=output_builtin,
    //     output_ptr=ids.pre_execution_builtin_ptrs.output)
    let pre_execution_builtin_ptrs_addr =
        get_relocatable_from_var_name(vars::PRE_EXECUTION_BUILTIN_PTRS, vm, ids_data, ap_tracking)?;
    // The output field is the first one in the BuiltinData struct
    let output_ptr = vm.get_relocatable((pre_execution_builtin_ptrs_addr + 0)?)?;
    let output_runner_data =
        util::prepare_output_runner(&task, vm.get_output_builtin_mut()?, output_ptr)?;

    exec_scopes.insert_value(vars::N_BUILTINS, n_builtins);
    exec_scopes.insert_value(vars::OUTPUT_RUNNER_DATA, output_runner_data);

    exec_scopes.enter_scope(new_task_locals);

    Ok(hint_extension)
}

fn vm_load_program(
    hint_processor: &mut dyn HintProcessor,
    exec_scopes: &mut ExecutionScopes,
) -> Result<HashMap<Relocatable, ExtensionData>, HintError> {
    let task_program_address: Relocatable = exec_scopes.get(vars::PROGRAM_ADDRESS).unwrap();
    let task = get_task_from_exec_scopes(exec_scopes)?;
    let task_program = get_program_from_task(&task)?;

    let mut task_program_compiled_hints = HashMap::new();
    let task_program_hints = task_program.get_hints();
    let task_program_hint_ranges = task_program.get_hints_ranges();
    let task_program_references = task_program.get_references();
    let task_program_constants = task_program.get_constants();

    for hint_range in task_program_hint_ranges {
        let hint_pc = hint_range.0;
        let hint_range_indices = hint_range.1;
        let (s, l) = hint_range_indices;
        for idx in *s..(*s + l.get()) {
            let hint = task_program_hints.get(idx).unwrap();

            let new_hint_pc_segment = hint_pc.segment_index + task_program_address.segment_index;
            let new_hint_pc_offset = hint_pc.offset + task_program_address.offset;
            let new_hint_pc = Relocatable::from((new_hint_pc_segment, new_hint_pc_offset));

            let compiled_hint = hint_processor.compile_hint(
                hint.code.as_str(),
                &hint.flow_tracking_data.ap_tracking,
                &hint.flow_tracking_data.reference_ids,
                task_program_references,
            )?;
            task_program_compiled_hints
                .entry(new_hint_pc)
                .or_insert_with(ExtensionData::default)
                .hints
                .push(compiled_hint);
        }
    }

    task_program_compiled_hints
        .iter_mut()
        .for_each(|(_, extension_data)| {
            extension_data
                .constants
                .extend(task_program_constants.iter().map(|(k, v)| (k.clone(), *v)));
        });

    Ok(task_program_compiled_hints)
}

pub fn exit_scope_with_comments(
    exec_scopes: &mut ExecutionScopes,
) -> Result<HintExtension, HintError> {
    exec_scopes
        .exit_scope()
        .map_err(HintError::FromScopeError)?;
    Ok(HashMap::new())
}

mod util {
    // TODO: clean up / organize
    use super::*;

    /// Prepares the output builtin if the type of task is Task, so that pages of the inner program
    /// will be recorded separately.
    /// If the type of task is CairoPie, nothing should be done, as the program does not contain
    /// hints that may affect the output builtin.
    /// The return value of this function should be later passed to get_task_fact_topology().
    pub(crate) fn prepare_output_runner(
        task: &Box<dyn Task>,
        output_builtin: &mut OutputBuiltinRunner,
        output_ptr: Relocatable,
    ) -> Result<Option<OutputBuiltinState>, HintError> {
        let output_state = if task.as_any().downcast_ref::<RunProgramTask>().is_some() {
            let output_state = output_builtin.get_state();
            output_builtin.new_state(output_ptr.segment_index as usize, 0, true);
            Ok(Some(output_state))
        } else if task.as_any().downcast_ref::<CairoPieTask>().is_some() {
            Ok(None)
        } else {
            Err(HintError::CustomHint(
                "Unexpected task type".to_string().into_boxed_str(),
            ))
        };
        output_state
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use cairo_vm::hint_processor::builtin_hint_processor::builtin_hint_processor_definition::HintProcessorData;

    use cairo_vm::any_box;
    use cairo_vm::hint_processor::hint_processor_definition::HintProcessorLogic;
    use cairo_vm::types::errors::math_errors::MathError;
    use cairo_vm::types::program::Program;
    use cairo_vm::types::relocatable::MaybeRelocatable;
    use cairo_vm::vm::runners::builtin_runner::BuiltinRunner;
    use cairo_vm::vm::runners::cairo_pie::{BuiltinAdditionalData, PublicMemoryPage};

    use rstest::{fixture, rstest};

    use crate::hints::codes::EXECUTE_TASK_CALL_TASK;

    use crate::{
        add_segments, define_segments, ids_data, non_continuous_ids_data, run_hint, vm,
        BootloaderHintProcessor,
    };

    use super::*;

    #[rstest]
    fn test_allocate_program_data_segment() {
        let mut vm = vm!();
        // Allocate space for program_data_ptr
        vm.set_fp(1);
        add_segments!(vm, 2);
        let ids_data = ids_data!["program_data_ptr"];
        let expected_program_data_segment_index = vm.segments.num_segments();

        let mut exec_scopes = ExecutionScopes::new();
        let ap_tracking = ApTracking::new();

        allocate_program_data_segment(&mut vm, &mut exec_scopes, &ids_data, &ap_tracking)
            .expect("Hint failed unexpectedly");

        let program_data_ptr =
            get_ptr_from_var_name("program_data_ptr", &mut vm, &ids_data, &ap_tracking)
                .expect("program_data_ptr is not set");

        let program_data_base: Relocatable = exec_scopes
            .get(vars::PROGRAM_DATA_BASE)
            .unwrap_or_else(|_| panic!("{} is not set", vars::PROGRAM_DATA_BASE));

        assert_eq!(program_data_ptr, program_data_base);
        // Check that we allocated a new segment and that the pointers point to it
        assert_eq!(
            vm.segments.num_segments(),
            expected_program_data_segment_index + 1
        );
        assert_eq!(
            program_data_ptr,
            Relocatable {
                segment_index: expected_program_data_segment_index as isize,
                offset: 0
            }
        );
    }

    #[fixture]
    fn fibonacci() -> Program {
        let program_content =
            include_bytes!("../../dependencies/test-programs/cairo0/fibonacci/fibonacci.json")
                .to_vec();

        Program::from_bytes(&program_content, Some("main"))
            .expect("Loading example program failed unexpectedly")
    }

    #[fixture]
    fn fibonacci_pie() -> CairoPie {
        let pie_content = include_bytes!(
            "../../dependencies/test-programs/bootloader/pies/fibonacci/cairo_pie.zip"
        );
        CairoPie::from_bytes(pie_content).expect("Failed to load the program PIE")
    }

    #[fixture]
    fn field_arithmetic_program() -> Program {
        let program_content = include_bytes!(
            "../../dependencies/test-programs/cairo0/field-arithmetic/field_arithmetic.json"
        )
        .to_vec();

        Program::from_bytes(&program_content, Some("main"))
            .expect("Loading example program failed unexpectedly")
    }

    #[fixture]
    fn fibonacci_with_hint() -> Program {
        let program_content = include_bytes!("../../examples/fibonacci_with_hint.json").to_vec();
        Program::from_bytes(&program_content, Some("main"))
            .expect("Loading example program failed unexpectedly")
    }

    #[rstest]
    fn test_load_program(fibonacci: Program) {
        let task = TaskSpec::RunProgram(RunProgramTask::new(
            fibonacci.clone(),
            HashMap::new(),
            false,
        ));

        let mut vm = vm!();
        vm.set_fp(1);
        // Set program_header_ptr to (2, 0)
        define_segments!(vm, 2, [((1, 0), (2, 0))]);
        let program_header_ptr = Relocatable::from((2, 0));
        add_segments!(vm, 1);

        let mut exec_scopes = ExecutionScopes::new();
        exec_scopes.insert_value(vars::PROGRAM_DATA_BASE, program_header_ptr);
        exec_scopes.insert_value(vars::TASK, task);

        let ids_data = ids_data!["program_header"];
        let ap_tracking = ApTracking::new();

        load_program_hint(&mut vm, &mut exec_scopes, &ids_data, &ap_tracking)
            .expect("Hint failed unexpectedly");

        // Note that we do not check the loaded content in memory here, this is tested
        // in `program_loader.rs`

        // The Fibonacci program has no builtins -> the header size is 4
        let header_size = 4;
        let expected_code_address: Result<Relocatable, MathError> =
            program_header_ptr + header_size;

        let program_address: Relocatable = exec_scopes.get(vars::PROGRAM_ADDRESS).unwrap();
        assert_eq!(program_address, expected_code_address.unwrap());

        // Check that the segment was finalized
        let expected_program_size = header_size + fibonacci.data_len();
        assert_eq!(
            vm.segments.segment_sizes[&(program_address.segment_index as usize)],
            expected_program_size
        );
    }

    #[rstest]
    fn test_call_task(fibonacci: Program) {
        let mut vm = vm!();

        // Allocate space for pre-execution (8 felts), which mimics the `BuiltinData` struct in the
        // Bootloader's Cairo code. Our code only uses the first felt (`output` field in the struct)
        define_segments!(vm, 2, [((1, 0), (2, 0))]);
        vm.set_fp(8);
        add_segments!(vm, 1);

        let ids_data = non_continuous_ids_data![(vars::PRE_EXECUTION_BUILTIN_PTRS, -8)];

        let mut exec_scopes = ExecutionScopes::new();

        let mut output_builtin = OutputBuiltinRunner::new(true);
        output_builtin.initialize_segments(&mut vm.segments);
        vm.builtin_runners
            .push(BuiltinRunner::Output(output_builtin));

        // Set program_address for `vm_load_program`, which will try to update the hints of the
        // Cairo program if needed.
        let program_address = Relocatable::from((2, 0));
        exec_scopes.insert_value(vars::PROGRAM_ADDRESS, program_address);

        let task = TaskSpec::RunProgram(RunProgramTask::new(fibonacci, HashMap::new(), false));
        exec_scopes.insert_box(vars::TASK, Box::new(task));

        assert_matches!(
            run_hint!(
                vm,
                ids_data.clone(),
                EXECUTE_TASK_CALL_TASK,
                &mut exec_scopes
            ),
            Ok(map) if map.is_empty()
        );
    }

    #[rstest]
    fn test_call_task_with_hint(fibonacci_with_hint: Program) {
        let mut vm = vm!();

        // Allocate space for pre-execution (8 felts), which mimics the `BuiltinData` struct in the
        // Bootloader's Cairo code. Our code only uses the first felt (`output` field in the struct)
        define_segments!(vm, 2, [((1, 0), (2, 0))]);
        vm.set_fp(8);
        add_segments!(vm, 1);

        let ids_data = non_continuous_ids_data![(vars::PRE_EXECUTION_BUILTIN_PTRS, -8)];

        let mut exec_scopes = ExecutionScopes::new();

        let mut output_builtin = OutputBuiltinRunner::new(true);
        output_builtin.initialize_segments(&mut vm.segments);
        vm.builtin_runners
            .push(BuiltinRunner::Output(output_builtin));

        // Set program_address for `vm_load_program`, which will try to update the hints of the
        // Cairo program if needed.
        let program_address = Relocatable::from((2, 0));
        exec_scopes.insert_value(vars::PROGRAM_ADDRESS, program_address);

        let task = TaskSpec::RunProgram(RunProgramTask::new(
            fibonacci_with_hint.clone(),
            HashMap::new(),
            false,
        ));
        exec_scopes.insert_box(vars::TASK, Box::new(task));

        let hint_processor = BootloaderHintProcessor::new();
        let hint_code = "ids.fibonacci_claim_index = program_input['fibonacci_claim_index']";
        let hint = fibonacci_with_hint.get_hints().first().unwrap();
        let references = fibonacci_with_hint.get_references();
        let ap_tracking = ApTracking {
            group: 2,
            offset: 1,
        };
        let compiled_hint = hint_processor
            .compile_hint(
                hint_code,
                &ap_tracking,
                &hint.flow_tracking_data.reference_ids,
                references,
            )
            .expect("Failed to compile hint")
            .downcast::<HintProcessorData>()
            .expect("Failed to downcast hint");
        let expected_hint_map_key = Relocatable::from((2, 8));
        let actual_hint_map = run_hint!(
            vm,
            ids_data.clone(),
            EXECUTE_TASK_CALL_TASK,
            &mut exec_scopes
        )
        .unwrap();

        let actual_hint_map_value = actual_hint_map.get(&expected_hint_map_key).unwrap();
        let actual_hint = actual_hint_map_value.hints[0]
            .downcast_ref::<HintProcessorData>()
            .unwrap();
        assert_eq!(actual_hint_map_value.hints.len(), 1);
        assert_eq!(actual_hint.code, compiled_hint.code);
        assert_eq!(actual_hint.ap_tracking, compiled_hint.ap_tracking);
        assert_eq!(actual_hint.ids_data, compiled_hint.ids_data);
    }

    /// Creates a fake Program struct to act as a placeholder for the `BOOTLOADER_PROGRAM` variable.
    /// These other options have been considered:
    /// * a `HasIdentifiers` trait cannot be used as exec_scopes requires to cast to `Box<dyn Any>`,
    ///   making casting back to the trait impossible.
    /// * using an enum requires defining test-only variants.
    fn mock_program_identifiers(symbols: HashMap<String, usize>) -> ProgramIdentifiers {
        

        symbols
            .into_iter()
            .map(|(name, pc)| {
                (
                    name,
                    Identifier {
                        pc: Some(pc),
                        type_: None,
                        value: None,
                        full_name: None,
                        members: None,
                        cairo_type: None,
                        size: None,
                    },
                )
            })
            .collect()
    }

    #[rstest]
    fn test_call_cairo_pie_task(fibonacci_pie: CairoPie) {
        let mut vm = vm!();

        // We set the program header pointer at (1, 0) and make it point to the start of segment #2.
        // Allocate space for pre-execution (8 values), which follows the `BuiltinData` struct in
        // the Bootloader Cairo code. Our code only uses the first felt (`output` field in the
        // struct). Finally, we put the mocked output of `select_input_builtins` in the next
        // memory address and increase the AP register accordingly.
        define_segments!(
            vm,
            4,
            [((1, 0), (2, 0)), ((1, 1), (4, 0)), ((1, 9), (4, 42))]
        );
        vm.set_ap(10);
        vm.set_fp(9);

        let program_header_ptr = Relocatable::from((2, 0));
        let ids_data = non_continuous_ids_data![
            ("program_header", -9),
            (vars::PRE_EXECUTION_BUILTIN_PTRS, -8),
        ];
        let ap_tracking = ApTracking::new();

        let mut exec_scopes = ExecutionScopes::new();

        let mut output_builtin = OutputBuiltinRunner::new(true);
        output_builtin.initialize_segments(&mut vm.segments);
        vm.builtin_runners
            .push(BuiltinRunner::Output(output_builtin));

        let task = TaskSpec::CairoPieTask(CairoPieTask::new(fibonacci_pie, false));
        exec_scopes.insert_value(vars::TASK, task);
        let bootloader_identifiers = HashMap::from(
            [
                ("starkware.cairo.bootloaders.simple_bootloader.execute_task.execute_task.ret_pc_label".to_string(), 10usize),
                ("starkware.cairo.bootloaders.simple_bootloader.execute_task.execute_task.call_task".to_string(), 8usize)
            ]
        );
        let program_identifiers = mock_program_identifiers(bootloader_identifiers);
        exec_scopes.insert_value(vars::PROGRAM_DATA_BASE, program_header_ptr);
        exec_scopes.insert_value(vars::BOOTLOADER_PROGRAM_IDENTIFIERS, program_identifiers);

        // Load the program in memory
        load_program_hint(&mut vm, &mut exec_scopes, &ids_data, &ap_tracking)
            .expect("Failed to load Cairo PIE task in the VM memory");

        let mut hint_processor = BootloaderHintProcessor::new();

        // Execute it
        call_task(
            &mut hint_processor,
            &mut vm,
            &mut exec_scopes,
            &ids_data,
            &ap_tracking,
        )
        .expect("Hint failed unexpectedly");
    }

    #[rstest]
    fn test_append_fact_topologies(fibonacci: Program) {
        let task = TaskSpec::RunProgram(RunProgramTask::new(
            fibonacci.clone(),
            HashMap::new(),
            false,
        ));

        let mut vm = vm!();

        // Allocate space for the pre-execution and return builtin structs (2 x 8 felts).
        // The pre-execution struct starts at (1, 0) and the return struct at (1, 8).
        // We only set the output values to (2, 0) and (2, 10), respectively, to get an output size
        // of 10.
        define_segments!(vm, 2, [((1, 0), (2, 0)), ((1, 8), (2, 10)),]);
        vm.set_fp(16);
        add_segments!(vm, 1);

        let tree_structure = vec![1, 2, 3, 4];
        let program_output_data = OutputBuiltinState {
            pages: HashMap::from([
                (1, PublicMemoryPage { start: 0, size: 7 }),
                (2, PublicMemoryPage { start: 7, size: 3 }),
            ]),
            attributes: HashMap::from([("gps_fact_topology".to_string(), tree_structure.clone())]),
            base: 0,
            base_offset: 0,
        };
        let mut output_builtin = OutputBuiltinRunner::new(true);
        output_builtin.set_state(program_output_data.clone());
        output_builtin.initialize_segments(&mut vm.segments);
        vm.builtin_runners
            .push(BuiltinRunner::Output(output_builtin));

        let ids_data = non_continuous_ids_data![
            ("pre_execution_builtin_ptrs", -16),
            ("return_builtin_ptrs", -8),
        ];

        let ap_tracking = ApTracking::new();

        let mut exec_scopes = ExecutionScopes::new();

        let output_runner_data = OutputBuiltinState {
            pages: HashMap::new(),
            attributes: HashMap::new(),
            base: 0,
            base_offset: 0,
        };
        exec_scopes.insert_value(vars::OUTPUT_RUNNER_DATA, Some(output_runner_data.clone()));
        exec_scopes.insert_value(vars::TASK, task);
        exec_scopes.insert_value(vars::FACT_TOPOLOGIES, Vec::<FactTopology>::new());

        append_fact_topologies(&mut vm, &mut exec_scopes, &ids_data, &ap_tracking)
            .expect("Hint failed unexpectedly");

        // Check that the fact topology matches the data from the output builtin
        let fact_topologies: Vec<FactTopology> = exec_scopes.get(vars::FACT_TOPOLOGIES).unwrap();
        assert_eq!(fact_topologies.len(), 1);

        let fact_topology = &fact_topologies[0];
        assert_eq!(fact_topology.page_sizes, vec![0, 7, 3]);
        assert_eq!(fact_topology.tree_structure, tree_structure);

        // Check that the output builtin was updated
        let output_builtin_additional_data =
            vm.get_output_builtin_mut().unwrap().get_additional_data();
        assert!(matches!(
            output_builtin_additional_data,
            BuiltinAdditionalData::Output(data) if data.pages == output_runner_data.pages && data.attributes == output_runner_data.attributes,
        ));
    }

    #[rstest]
    #[ignore] // FIXME
    fn test_write_output_builtins(field_arithmetic_program: Program) {
        let task = TaskSpec::RunProgram(RunProgramTask::new(
            field_arithmetic_program.clone(),
            HashMap::new(),
            false,
        ));

        let mut vm = vm!();
        // Allocate space for all the builtin list structs (3 x 8 felts).
        // The pre-execution struct starts at (1, 0), the used builtins list at (1, 8)
        // and the return struct at (1, 16).
        // Initialize the pre-execution struct to [1, 2, 3, 4, 5, 6, 7, 8].
        // Initialize the used builtins to {range_check: 30, bitwise: 50} as these two
        // are used by the field arithmetic program. Note that the used builtins list
        // does not contain empty elements (i.e. offsets are 8 and 9 instead of 10 and 12).
        define_segments!(
            vm,
            2,
            [
                ((1, 0), (2, 1)),
                ((1, 1), (2, 2)),
                ((1, 2), (2, 3)),
                ((1, 3), (2, 4)),
                ((1, 4), (2, 5)),
                ((1, 5), (2, 6)),
                ((1, 6), (2, 7)),
                ((1, 7), (2, 8)),
                ((1, 8), (2, 9)),
                ((1, 9), (2, 30)),
                ((1, 10), (2, 50)),
                ((1, 26), (1, 9)),
            ]
        );
        vm.set_fp(27);
        add_segments!(vm, 1);

        // Note that used_builtins_addr is a pointer to the used builtins list at (1, 8)
        let ids_data = non_continuous_ids_data![
            ("pre_execution_builtin_ptrs", -27),
            ("return_builtin_ptrs", -10),
            ("used_builtins_addr", -1),
        ];
        let ap_tracking = ApTracking::new();

        let mut exec_scopes = ExecutionScopes::new();
        let n_builtins = field_arithmetic_program.builtins_len();
        exec_scopes.insert_value(vars::N_BUILTINS, n_builtins);
        exec_scopes.insert_value(vars::TASK, task);

        write_return_builtins_hint(&mut vm, &mut exec_scopes, &ids_data, &ap_tracking)
            .expect("Hint failed unexpectedly");

        // Check that the return builtins were written correctly
        let return_builtins = vm
            .get_continuous_range(Relocatable::from((1, 17)), 9)
            .expect("Return builtin was not properly written to memory.");
        let expected_builtins = vec![
            Relocatable::from((2, 1)),
            Relocatable::from((2, 2)),
            Relocatable::from((2, 30)),
            Relocatable::from((2, 4)),
            Relocatable::from((2, 50)),
            Relocatable::from((2, 6)),
            Relocatable::from((2, 7)),
            Relocatable::from((2, 8)),
        ];
        for (expected, actual) in std::iter::zip(expected_builtins, return_builtins) {
            assert_eq!(MaybeRelocatable::RelocatableValue(expected), actual);
        }

        // Check that the exec scope changed
        assert_eq!(
            exec_scopes.data.len(),
            2,
            "A new scope should have been declared"
        );
        assert_eq!(
            exec_scopes.data[1].len(),
            1,
            "The new scope should only contain one variable"
        );
        let n_selected_builtins: usize = exec_scopes
            .get(vars::N_SELECTED_BUILTINS)
            .expect("n_selected_builtins should be set");
        assert_eq!(n_selected_builtins, n_builtins);
    }
}
