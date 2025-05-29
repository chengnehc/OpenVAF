use hir::{CompilationDB, ModuleInfo, ParamSysFun};
use hir_lower::{HirInterner, ParamKind};
use llvm::{LLVMSetDLLStorageClass, LLVMSetLinkage, LLVMSetUnnamedAddress};
use mir::Function;
use mir_llvm::CodegenCx;
use sim_back::{dae::DaeSystem, init::Initialization, node_collapse::NodeCollapse, CompiledModule};

use lasso::Rodeo;
use typed_indexmap::TiSet;

use crate::inst_data::OsdiInstanceData;
use crate::metadata::{osdi_0_3::OsdiTys, sim_unknown_info, OsdiLimFunction};
use crate::model_data::OsdiModelData;

pub struct OsdiCompilationUnit<'a, 'b, 'll> {
    pub db: &'a CompilationDB,
    pub cx: &'a CodegenCx<'b, 'll>,
    pub tys: &'a OsdiTys<'ll>,
    pub module: &'a OsdiModule<'b>,
    pub inst_data: OsdiInstanceData<'ll>,
    pub model_data: OsdiModelData<'ll>,
    pub lim_dispatch_table: Option<&'ll llvm::Value>,
}

impl<'a, 'b, 'll> OsdiCompilationUnit<'a, 'b, 'll> {
    pub fn new(
        db: &'a CompilationDB,
        module: &'a OsdiModule<'b>,
        cx: &'a CodegenCx<'b, 'll>,
        tys: &'a OsdiTys<'ll>,
        is_eval: bool,
    ) -> OsdiCompilationUnit<'a, 'b, 'll> {
        let inst_data = OsdiInstanceData::new(db, module, cx);
        let model_data = OsdiModelData::new(db, module, cx, &inst_data);
        let lim_dispatch_table =
            if is_eval && !module.lim_table.is_empty() && !module.intern.lim_state.is_empty() {
                let ty = cx.ty_array(tys.osdi_lim_function, module.lim_table.len() as u32);
                let ptr = cx
                    .define_global("OSDI_LIM_TABLE", ty)
                    .unwrap_or_else(|| unreachable!("symbol OSDI_LIM_TABLE already defined"));
                unsafe {
                    LLVMSetLinkage(ptr, llvm::Linkage::ExternalLinkage);
                    LLVMSetUnnamedAddress(ptr, llvm::UnnamedAddr::No);
                    LLVMSetDLLStorageClass(ptr, llvm::DLLStorageClass::Export);
                }
                Some(ptr)
            } else {
                None
            };
        OsdiCompilationUnit { db, tys, cx, module, inst_data, model_data, lim_dispatch_table }
    }
}

pub struct OsdiModule<'a> {
    pub info: &'a ModuleInfo,
    pub dae: &'a DaeSystem,
    pub init: &'a Initialization, // setup_instance
    pub node_collapse: &'a NodeCollapse,
    pub eval: &'a Function,
    pub intern: &'a HirInterner,
    pub model_param_setup: &'a Function, // setup_model
    pub model_param_intern: &'a HirInterner,
    pub lim_table: &'a TiSet<OsdiLimId, OsdiLimFunction>,
    pub sym: String,
}

impl<'a> OsdiModule<'a> {
    pub fn new(
        db: &'a CompilationDB,
        module: &'a CompiledModule,
        lim_table: &'a TiSet<OsdiLimId, OsdiLimFunction>,
    ) -> Self {
        let sym = base_n::encode(module.info.module.uuid(db) as u128, base_n::CASE_INSENSITIVE);
        let CompiledModule {
            info,
            dae,
            eval,
            intern,
            init,
            model_param_setup,
            model_param_intern,
            node_collapse,
        } = module;
        OsdiModule {
            sym,
            lim_table,
            info,
            dae,
            eval,
            intern,
            init,
            model_param_setup,
            model_param_intern,
            node_collapse,
        }
    }

    pub fn intern_names(&self, literals: &mut Rodeo, db: &CompilationDB) {
        literals.get_or_intern(self.info.module.name(db));

        /* Simulation unknown names and units */
        for &unknown in self.dae.unknowns.iter() {
            let (name, units, _) = sim_unknown_info(unknown, db);
            literals.get_or_intern(&name);
            literals.get_or_intern(&units);
        }

        /* Parameters and op variables */
        literals.get_or_intern_static("Multiplier (Verilog-A $mfactor)");
        literals.get_or_intern_static("deg");
        literals.get_or_intern_static("m");
        literals.get_or_intern_static("");

        for param in self.info.params.values() {
            for alias in &param.aliases {
                literals.get_or_intern(&**alias);
            }
            literals.get_or_intern(&param.name);
            literals.get_or_intern(&param.units);
            literals.get_or_intern(&param.desc);
            literals.get_or_intern(&param.group);
        }

        for aliases in self.info.param_sysfuns.values() {
            for alias in aliases {
                literals.get_or_intern(&**alias);
            }
        }

        for param in ParamSysFun::iter() {
            let is_live = |intern: &HirInterner, func| {
                intern.is_param_live(func, &ParamKind::ParamSysFun(param))
            };
            if is_live(self.intern, self.eval)
                || is_live(&self.init.intern, &self.init.func)
                || is_live(self.model_param_intern, self.model_param_setup)
            {
                literals.get_or_intern(format!("${param:?}"));
            }
        }

        for (var, opvar_info) in self.info.op_vars.iter() {
            literals.get_or_intern(var.name(db));
            literals.get_or_intern(&opvar_info.units);
            literals.get_or_intern(&opvar_info.desc);
        }
    }
}

use stdx::{impl_debug_display, impl_idx_from};

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Hash)]
pub struct OsdiLimId(u32);
impl_idx_from!(OsdiLimId(u32));
impl_debug_display!(match OsdiLimId{OsdiLimId(id) => "lim{id}";});
