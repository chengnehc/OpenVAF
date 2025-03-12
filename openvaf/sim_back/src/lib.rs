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
    pub eval: Function,
    pub intern: HirInterner,
    pub model_param_setup: Function,
    pub model_param_intern: HirInterner,
}

impl<'a> CompiledModule<'a> {
    pub fn new(
        db: &CompilationDB,
        info: &'a ModuleInfo,
        literals: &mut Rodeo,
    ) -> CompiledModule<'a> {
        let mut cxt = Context::new(db, literals, info);
        cxt.compute_outputs(true);
        cxt.compute_cfg();
        cxt.optimize(OptimizationStage::Initial);
        debug_assert!(cxt.func.validate());

        let topology = Topology::new(&mut cxt);
        debug_assert!(cxt.func.validate());
        let mut dae = DaeSystem::new(&mut cxt, topology);
        debug_assert!(cxt.func.validate());
        cxt.compute_cfg();
        let gvn = cxt.optimize(OptimizationStage::PostDerivative);
        dae.sparsify(&mut cxt);

        // For debugging purposes - print parameters
        let debugging = false; //  && cfg!(debug_assertions);
        if debugging {
            println!("Parameters:");
            cxt.intern.params.iter().for_each(|(p, val)| {
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
            cxt.intern.outputs.iter().for_each(|(p, val)| {
                if val.is_some() {
                    println!("  {:?} -> {:?}", p, val.unwrap());
                } else {
                    println!("  {:?} -> None", p);
                }
            });
            println!();

            println!("Tagged reads:");
            cxt.intern.tagged_reads.iter().for_each(|(val, var)| {
                println!("  {:?} -> {:?}", val, var);
            });
            println!();

            println!("Implicit equations:");
            for (i, &iek) in cxt.intern.implicit_equations.iter().enumerate() {
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
            println!("{:?}", cxt.func);
            println!();
        }

        debug_assert!(cxt.func.validate());

        cxt.refresh_op_dependent_insts();
        let mut init = Initialization::new(&mut cxt, gvn);
        let node_collapse = NodeCollapse::new(&init, &dae, &cxt);
        debug_assert!(cxt.func.validate());

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
        cxt.cfg.compute(&model_param_setup);
        simplify_cfg(&mut model_param_setup, &mut cxt.cfg);
        sparse_conditional_constant_propagation(&mut model_param_setup, &cxt.cfg);
        simplify_cfg(&mut model_param_setup, &mut cxt.cfg);

        CompiledModule {
            info,
            dae,
            init,
            node_collapse,
            eval: cxt.func,
            intern: cxt.intern,
            model_param_intern,
            model_param_setup,
        }
    }
}
