use acir::{
    acir_field::GenericFieldElement,
    circuit::{Circuit, ExpressionWidth, Opcode},
    native_types::{WitnessMap, WitnessStack},
};
use ark_ff::PrimeField;
use intmap::IntMap;
use mpc_core::{
    protocols::{
        plain::PlainDriver,
        rep3::{network::Rep3Network, Rep3Protocol},
    },
    traits::NoirWitnessExtensionProtocol,
};
use noirc_abi::{input_parser::Format, Abi, MAIN_RETURN_NAME};
use noirc_artifacts::program::ProgramArtifact;
use std::{collections::BTreeMap, io, path::PathBuf};
/// The default expression width defined used by the ACVM.
pub(crate) const CO_EXPRESSION_WIDTH: ExpressionWidth = ExpressionWidth::Bounded { width: 4 };

mod assert_zero_solver;
mod memory_solver;
pub type PlainCoSolver<F> = CoSolver<PlainDriver<F>, F>;
pub type Rep3CoSolver<F, N> = CoSolver<Rep3Protocol<F, N>, F>;

type CoAcvmResult<T> = std::result::Result<T, CoAcvmError>;

pub(crate) mod solver_utils {
    use acir::native_types::Expression;

    pub(crate) fn expr_to_string<F: std::fmt::Display>(expr: &Expression<F>) -> String {
        let mul_terms = expr
            .mul_terms
            .iter()
            .map(|(q_m, w_l, w_r)| format!("({q_m} * _{w_l:?} * _{w_r:?})"))
            .collect::<Vec<String>>()
            .join(" + ");
        let linear_terms = expr
            .linear_combinations
            .iter()
            .map(|(coef, w)| format!("({coef} * _{w:?})"))
            .collect::<Vec<String>>()
            .join(" + ");
        format!("EXPR [({mul_terms}) + ({linear_terms}) + {}]", expr.q_c)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CoAcvmError {
    #[error(transparent)]
    IOError(#[from] io::Error),
    #[error(transparent)]
    UnrecoverableError(#[from] eyre::Report),
}

pub struct CoSolver<T, F>
where
    T: NoirWitnessExtensionProtocol<F>,
    F: PrimeField,
{
    driver: T,
    abi: Abi,
    functions: Vec<Circuit<GenericFieldElement<F>>>,
    // maybe this can be an array. lets see..
    witness_map: Vec<WitnessMap<T::AcvmType>>,
    // there will a more fields added as we add functionality
    function_index: usize,
    // the memory blocks
    memory_access: IntMap<T::SecretSharedMap>,
}

impl<T> CoSolver<T, ark_bn254::Fr>
where
    T: NoirWitnessExtensionProtocol<ark_bn254::Fr>,
{
    pub fn read_abi_bn254<P>(path: P, abi: &Abi) -> eyre::Result<WitnessMap<T::AcvmType>>
    where
        PathBuf: From<P>,
    {
        if abi.is_empty() {
            Ok(WitnessMap::default())
        } else {
            let input_string = std::fs::read_to_string(PathBuf::from(path))?;
            let mut input_map = Format::Toml.parse(&input_string, abi)?;
            let return_value = input_map.remove(MAIN_RETURN_NAME);
            // TODO the return value can be none for the witness extension
            // do we want to keep it like that? Seems not necessary but maybe
            // we need it for proving/verifying
            let initial_witness = abi.encode(&input_map, return_value.clone())?;
            let mut witnesses = WitnessMap::<T::AcvmType>::default();
            for (witness, v) in initial_witness.into_iter() {
                witnesses.insert(witness, T::AcvmType::from(v.into_repr())); //TODO this can be
                                                                             //private for some
            }
            Ok(witnesses)
        }
    }

    pub fn new_bn254<P>(
        driver: T,
        compiled_program: ProgramArtifact,
        prover_path: P,
    ) -> eyre::Result<Self>
    where
        PathBuf: From<P>,
    {
        let mut witness_map =
            vec![WitnessMap::default(); compiled_program.bytecode.functions.len()];
        witness_map[0] = Self::read_abi_bn254(prover_path, &compiled_program.abi)?;
        Ok(Self {
            driver,
            abi: compiled_program.abi,
            functions: compiled_program
                .bytecode
                .functions
                .into_iter()
                // ignore the transformation mapping for now
                .map(|function| acvm::compiler::transform(function, CO_EXPRESSION_WIDTH).0)
                .collect::<Vec<_>>(),
            witness_map,
            function_index: 0,
            memory_access: IntMap::new(),
        })
    }
}

impl<N: Rep3Network> Rep3CoSolver<ark_bn254::Fr, N> {
    pub fn from_network<P>(
        network: N,
        compiled_program: ProgramArtifact,
        prover_path: P,
    ) -> eyre::Result<Self>
    where
        PathBuf: From<P>,
    {
        Self::new_bn254(Rep3Protocol::new(network)?, compiled_program, prover_path)
    }
}

impl<F: PrimeField> PlainCoSolver<F> {
    pub fn convert_to_plain_acvm_witness(
        mut shared_witness: WitnessStack<F>,
    ) -> WitnessStack<GenericFieldElement<F>> {
        let length = shared_witness.length();
        let mut vec = Vec::with_capacity(length);
        for _ in 0..length {
            let stack_item = shared_witness.pop().unwrap();
            vec.push((
                stack_item.index,
                stack_item
                    .witness
                    .into_iter()
                    .map(|(k, v)| (k, GenericFieldElement::from_repr(v)))
                    .collect::<BTreeMap<_, _>>(),
            ))
        }
        let mut witness = WitnessStack::default();
        //push again in reverse order
        for (index, witness_map) in vec.into_iter().rev() {
            witness.push(index, WitnessMap::from(witness_map));
        }
        witness
    }
}

impl PlainCoSolver<ark_bn254::Fr> {
    pub fn init_plain_driver<P>(
        compiled_program: ProgramArtifact,
        prover_path: P,
    ) -> eyre::Result<Self>
    where
        PathBuf: From<P>,
    {
        Self::new_bn254(PlainDriver::default(), compiled_program, prover_path)
    }

    pub fn solve_and_print_output(self) {
        let abi = self.abi.clone();
        let result = self.solve().unwrap();
        let mut result = Self::convert_to_plain_acvm_witness(result);
        let main_witness = result.pop().unwrap();
        let (_, ret_val) = abi.decode(&main_witness.witness).unwrap();
        if let Some(ret_val) = ret_val {
            println!("circuit produced: {ret_val:?}");
        } else {
            println!("no output for circuit")
        }
    }
}

impl<T, F> CoSolver<T, F>
where
    T: NoirWitnessExtensionProtocol<F>,
    F: PrimeField,
{
    #[inline(always)]
    fn witness(&mut self) -> &mut WitnessMap<T::AcvmType> {
        &mut self.witness_map[self.function_index]
    }
}

impl<T, F> CoSolver<T, F>
where
    T: NoirWitnessExtensionProtocol<F>,
    F: PrimeField,
{
    pub fn solve(mut self) -> CoAcvmResult<WitnessStack<T::AcvmType>> {
        let functions = std::mem::take(&mut self.functions);

        for opcode in functions[self.function_index].opcodes.iter() {
            match opcode {
                Opcode::AssertZero(expr) => self.solve_assert_zero(expr)?,
                Opcode::MemoryInit {
                    block_id,
                    init,
                    block_type: _, // apparently not used
                } => self.solve_memory_init_block(*block_id, init)?,
                Opcode::MemoryOp {
                    block_id,
                    op,
                    predicate,
                } => self.solve_memory_op(*block_id, op, predicate.to_owned())?,
                _ => todo!("non assert zero opcode detected, not supported yet"),
                //Opcode::Call {
                //    id,
                //    inputs,
                //    outputs,
                //    predicate,
                //} => todo!(),
            }
        }
        tracing::trace!("we are done! Wrap things up.");
        let mut witness_stack = WitnessStack::default();
        for (idx, witness) in self.witness_map.into_iter().rev().enumerate() {
            witness_stack.push(u32::try_from(idx).expect("usize fits into u32"), witness);
        }
        Ok(witness_stack)
    }
}

/*
  let binary_packages = workspace.into_iter().filter(|package| package.is_binary());
    for package in binary_packages {
*/
