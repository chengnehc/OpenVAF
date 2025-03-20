//! Simulator backend

use stdx::impl_debug_display;

use hir::{BranchWrite, CompilationDB, Node};
use hir_lower::{CurrentKind, HirInterner, ImplicitEquation, ParamKind};
use lasso::Rodeo;
use mir::Function;
use mir_opt::{simplify_cfg, sparse_conditional_constant_propagation};

pub mod dae;
pub mod init;
pub mod node_collapse;

mod context;
mod module_info;
mod noise;
mod topology;
mod util;

pub use module_info::{collect_modules, ModuleInfo};

use context::{Context, OptimizationStage};
use dae::DaeSystem;
use init::Initialization;
use node_collapse::NodeCollapse;
use topology::Topology;

#[derive(PartialEq, Eq, Clone, Copy, Hash)]
pub enum SimUnknownKind {
    KirchhoffLaw(Node),
    Current(CurrentKind),
    Implicit(ImplicitEquation),
}

impl_debug_display! {
    match SimUnknownKind{
        SimUnknownKind::KirchhoffLaw(node) => "{node:?}";
        SimUnknownKind::Current(curr) => "br[{curr:?}]";
        SimUnknownKind::Implicit(node) => "{node}";
    }
}

pub struct CompiledModule<'a> {
    pub info: &'a ModuleInfo,
    pub dae: DaeSystem,
    pub init: Initialization,
    pub node_collapse: NodeCollapse,
    pub model_param_setup: Function,
    pub model_param_intern: HirInterner,
    pub eval: Function,
    pub intern: HirInterner,
}

impl<'a> CompiledModule<'a> {
    pub fn new(
        db: &CompilationDB,
        info: &'a ModuleInfo,
        literals: &mut Rodeo,
    ) -> CompiledModule<'a> {
        let mut ctxt = Context::new(db, literals, info);
        ctxt.compute_outputs(true);
        ctxt.compute_cfg();
        ctxt.optimize(OptimizationStage::Initial);
        debug_assert!(ctxt.func.validate());

        let topology = Topology::new(&mut ctxt);
        debug_assert!(ctxt.func.validate());
        let mut dae = DaeSystem::new(&mut ctxt, topology);
        debug_assert!(ctxt.func.validate());
        ctxt.compute_cfg();
        let gvn = ctxt.optimize(OptimizationStage::PostDerivative);
        dae.sparsify(&mut ctxt);

        // For debugging purposes - print parameters
        let debugging = false; //  && cfg!(debug_assertions);
        if debugging {
            println!("Parameters:");
            ctxt.intern.params.iter().for_each(|(p, val)| {
                print!("  {:?}", p);
                match p {
                    ParamKind::Param(param) | ParamKind::ParamGiven { param } => {
                        println!(" .. {:?} -> {:?}", param.name(db), val);
                    }
                    ParamKind::Voltage { hi, lo } => {
                        if lo.is_some() {
                            print!(" .. V({:?},{:?})", hi.name(db), lo.unwrap().name(db));
                        } else {
                            print!(" .. V({:?})", hi.name(db));
                        }
                        println!(" -> {:?}", val);
                    }
                    ParamKind::Current(ck) => match ck {
                        CurrentKind::Branch(br) => {
                            println!(" .. {:?} -> {:?}", br.name(db), val);
                        }
                        CurrentKind::Unnamed { hi, lo } => {
                            if lo.is_some() {
                                print!(" .. I({:?},{:?})", hi.name(db), lo.unwrap().name(db));
                            } else {
                                print!(" .. I({:?})", hi.name(db));
                            }
                            println!(" -> {:?}", val);
                        }
                        CurrentKind::Port(n) => {
                            println!(" .. {:?} -> {:?}", n.name(db), val);
                        }
                    },
                    ParamKind::HiddenState(var) => {
                        println!(" .. {:?} -> {:?}", var.name(db), val);
                    }
                    // ParamKind::ImplicitUnknown
                    ParamKind::PortConnected { port } => {
                        println!(" .. {:?} -> {:?}", port.name(db), val);
                    }
                    _ => {
                        println!(" -> {:?}", val);
                    }
                }
            });
            println!();

            println!("Outputs:");
            ctxt.intern.outputs.iter().for_each(|(p, val)| {
                if val.is_some() {
                    println!("  {:?} -> {:?}", p, val.unwrap());
                } else {
                    println!("  {:?} -> None", p);
                }
            });
            println!();

            println!("Tagged reads:");
            ctxt.intern.tagged_reads.iter().for_each(|(val, var)| {
                println!("  {:?} -> {:?}", val, var);
            });
            println!();

            println!("Implicit equations:");
            for (i, &iek) in ctxt.intern.implicit_equations.iter().enumerate() {
                println!("  {:?} : {:?}", i, iek);
            }
            println!();

            let cu = db.compilation_unit();
            println!("Compilation unit: {}", cu.name(db));

            let m = info.module;
            println!("Module: {:?}", m.name(db));
            println!("Ports: {:?}", m.ports(db));
            println!("Internal nodes: {:?}", m.internal_nodes(db));

            println!("DAE system");
            let str = format!("{dae:#?}");
            println!("{}", str);
            println!();

            println!("CX function");
            println!("{:?}", ctxt.func);
            println!();
        }

        debug_assert!(ctxt.func.validate());

        ctxt.refresh_op_dependent_insts();
        let mut init = Initialization::new(&mut ctxt, gvn);
        let node_collapse = NodeCollapse::new(&init, &dae, &ctxt);
        debug_assert!(ctxt.func.validate());

        // For debugging purposes - print MIR
        if debugging {
            println!("Init function");
            println!("{:?}", init.func);
            println!();
        }

        debug_assert!(init.func.validate());

        // TODO: refactor param intilization to use tables
        let inst_params: Vec<_> = info
            .params
            .iter()
            .filter_map(|(param, info)| info.is_instance.then_some(*param))
            .collect();
        init.intern.insert_param_init(db, &mut init.func, literals, false, true, &inst_params);

        let mut model_param_setup = Function::default();
        let model_params: Vec<_> = info.params.keys().copied().collect();
        let mut model_param_intern = HirInterner::default();
        model_param_intern.insert_param_init(
            db,
            &mut model_param_setup,
            literals,
            false,
            true,
            &model_params,
        );
        ctxt.cfg.compute(&model_param_setup);
        simplify_cfg(&mut model_param_setup, &mut ctxt.cfg);
        sparse_conditional_constant_propagation(&mut model_param_setup, &ctxt.cfg);
        simplify_cfg(&mut model_param_setup, &mut ctxt.cfg);

        CompiledModule {
            info,
            dae,
            init,
            node_collapse,
            model_param_intern,
            model_param_setup,
            eval: ctxt.func,
            intern: ctxt.intern,
        }
    }
}
