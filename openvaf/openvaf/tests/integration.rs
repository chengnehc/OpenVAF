use std::f64::consts;
use std::path::Path;
use stdx::{ignore_dev_tests, openvaf_test_data, project_root};

use camino::Utf8Path;
use expect_test::expect_file;
use mini_harness::{harness, Result};
use target::spec::Target;

use openvaf::{CompilationDestination, CompilationTermination, OptLevel};

mod load;
mod mock_sim;
use load::{load_osdi_lib, EvalFlags, OsdiDescriptor, OsdiModel};
use mock_sim::{MockSimulation, ALPHA};

fn integration_test(dir: &Path) -> Result {
    let name = dir.file_name().unwrap().to_str().unwrap().to_lowercase();
    let main_file = dir.join(name).with_extension("va");
    test_descriptor(&main_file)?;

    Ok(())
}

fn test_descriptor(main_file: &Path) -> Result<&'static OsdiDescriptor> {
    let main_file: &Utf8Path = main_file.try_into().unwrap();
    let desc = compile_and_load(main_file);
    let actual = format!("{desc:?}");

    let name = main_file.file_stem().unwrap();
    let file_name = openvaf_test_data("osdi").join(name).with_extension("snap");
    expect_file![file_name].assert_eq(&actual);
    // let _ = std::fs::write(file_name), &actual);

    // setup model parameters
    let model = OsdiModel::new(desc);
    model.process_params()?;

    // setup instance parameters and collapse internal nodes
    let mut instance = model.new_instance();
    // assume all terminals are connected
    let connected_terminals = desc.num_terminals;
    instance.process_params(&model, connected_terminals, 300.0)?;

    Ok(desc)
}

fn compile_and_load(root_file: &Utf8Path) -> &'static OsdiDescriptor {
    let opts = openvaf::Opts {
        input: root_file.to_path_buf(),
        output: CompilationDestination::Path { lib_file: root_file.with_extension("osdi") },
        defines: Vec::new(),
        include: Vec::new(),
        lints: Vec::new(),
        dry_run: false,
        codegen_opts: Vec::new(),
        target: Target::host_target().unwrap(),
        target_cpu: "native".to_owned(),
        opt_lvl: OptLevel::Aggressive,
    };

    let lib_file = match openvaf::compile(&opts).unwrap() {
        CompilationTermination::Compiled { lib_file } => lib_file,
        CompilationTermination::FatalDiagnostic => {
            panic!("openvaf: compilation of {root_file} failed");
        }
    };
    let descriptors = unsafe { load_osdi_lib(&lib_file).unwrap() };
    assert_eq!(descriptors.len(), 1);

    &descriptors[0]
}

fn test_limit() -> Result<()> {
    macro_rules! assert_approx_eq {
        ($val: expr, $resist: expr, $react: expr) => {
            let (resist, react) = $val;
            let resist_ref: f64 = $resist;
            if (resist - resist_ref).abs() / resist.min(resist_ref) >= 0.01 {
                float_cmp::assert_approx_eq!(f64, resist, resist_ref, epsilon = 1e-10)
            }
            let react_ref: f64 = $react;
            if (react - react_ref).abs() / react.min(react_ref) >= 0.01 {
                float_cmp::assert_approx_eq!(f64, react, react_ref, epsilon = 1e-10)
            }
        };
    }

    // skipping in CI for now as we don't have a toolchain there
    // currently
    if stdx::IS_CI && cfg!(windows) {
        return Ok(());
    }

    const KB: f64 = 1.3806488e-23;
    const Q: f64 = 1.602176565e-19;
    const TEMP: f64 = 300.0;
    const VT: f64 = KB * TEMP / Q;
    const IS: f64 = 1e-12;
    const CJ0: f64 = 10e-9;

    let vcrit = VT * f64::ln(VT / (consts::SQRT_2 * IS));
    let check_dae_equations = |sim: &MockSimulation, vd_lim, vd| {
        let id = |vd| IS * (f64::exp(vd / VT) - 1.0);
        let id_vd = |vd| IS / VT * f64::exp(vd / VT);
        let cj = |vd| CJ0 * vd;
        assert_approx_eq!(sim.read_jacobian("A", "A"), id_vd(vd_lim), CJ0);
        assert_approx_eq!(sim.read_jacobian("C", "C"), id_vd(vd_lim), CJ0);
        assert_approx_eq!(sim.read_jacobian("A", "C"), -id_vd(vd_lim), -CJ0);
        assert_approx_eq!(sim.read_jacobian("C", "A"), -id_vd(vd_lim), -CJ0);
        assert_approx_eq!(
            sim.read_residual("A"),
            id(vd_lim) - id_vd(vd_lim) * (vd_lim - vd),
            cj(vd_lim) - CJ0 * (vd_lim - vd)
        );
        assert_approx_eq!(
            sim.read_residual("C"),
            id_vd(vd_lim) * (vd_lim - vd) - id(vd_lim),
            CJ0 * (vd_lim - vd) - cj(vd_lim)
        );
    };

    let check_spice_equations = |sim: &MockSimulation, vd_lim, vd| {
        let id = |vd| IS * (f64::exp(vd / VT) - 1.0);
        let id_vd = |vd| IS / VT * f64::exp(vd / VT);
        let cj = |vd| CJ0 * vd;
        assert_approx_eq!(
            sim.read_residual("A"),
            id_vd(vd_lim) * vd_lim - id(vd_lim) + ALPHA * (CJ0 * vd_lim - cj(vd_lim)),
            0.0
        );
        assert_approx_eq!(
            sim.read_residual("C"),
            id_vd(vd_lim) * (vd_lim - vd) - id(vd_lim),
            CJ0 * (vd_lim - vd) - cj(vd_lim)
        );
        assert_approx_eq!(sim.read_jacobian("A", "A"), id_vd(vd_lim) + ALPHA * CJ0, 0.0);
        assert_approx_eq!(sim.read_jacobian("C", "C"), id_vd(vd_lim) + ALPHA * CJ0, 0.0);
        assert_approx_eq!(sim.read_jacobian("A", "C"), -id_vd(vd_lim) - ALPHA * CJ0, 0.0);
        assert_approx_eq!(sim.read_jacobian("C", "A"), -id_vd(vd_lim) - ALPHA * CJ0, 0.0);
    };

    // compile model and setup simulation
    let desc = test_descriptor(&openvaf_test_data("osdi").join("diode_lim.va"))?;
    let model = OsdiModel::new(desc);
    model.set_real_param(1, IS);
    model.set_real_param(5, CJ0);
    model.process_params()?;

    let mut instance = model.new_instance();
    let connected_terminals = desc.num_terminals;
    let mut sim = instance.mock_simulation(&model, connected_terminals, TEMP)?;

    instance.eval(&model, &mut sim, EvalFlags::INIT_LIM | EvalFlags::ENABLE_LIM);
    instance.load_dae(&model, &mut sim);
    check_dae_equations(&sim, vcrit, 0.0);
    sim.clear();
    instance.load_spice(&model, &mut sim);
    check_spice_equations(&sim, vcrit, 0.0);

    sim.next_iteration();
    sim.set_voltage("A", 2.0 * vcrit);
    instance.eval(&model, &mut sim, EvalFlags::ENABLE_LIM);
    instance.load_dae(&model, &mut sim);
    check_dae_equations(&sim, 1.5 * vcrit, 2.0 * vcrit);
    sim.clear();
    instance.load_spice(&model, &mut sim);
    check_spice_equations(&sim, 1.5 * vcrit, 2.0 * vcrit);

    Ok(())
}

fn test_noise() -> Result<()> {
    macro_rules! assert_approx_eq {
        ($val: expr, $expect: expr) => {
            let resist = $val;
            let resist_ref: f64 = $expect;
            if (resist - resist_ref).abs() / resist.min(resist_ref) >= 0.01 {
                float_cmp::assert_approx_eq!(f64, resist, resist_ref, epsilon = 1e-10)
            }
        };
    }

    // skipping in CI for now as we don't have a toolchain there
    // currently
    if stdx::IS_CI && cfg!(windows) {
        return Ok(());
    }

    const MFACTOR: f64 = 2.0;
    const PWR: f64 = 3.0;
    const EXP: f64 = 7.0;
    const V_AC: f64 = 13.0;
    const TEMP: f64 = 300.0;

    // compile model and setup simulation
    let desc = test_descriptor(&openvaf_test_data("osdi").join("noise.va"))?;
    let model = OsdiModel::new(desc);
    model.set_real_param(0, MFACTOR);
    model.set_real_param(1, PWR);
    model.set_real_param(2, EXP);
    model.process_params()?;
    let mut instance = model.new_instance();
    let connected_terminals = desc.num_terminals;
    let mut sim = instance.mock_simulation(&model, connected_terminals, TEMP)?;

    sim.set_voltage("a", V_AC);
    instance.eval(&model, &mut sim, EvalFlags::empty());
    for freq in 1..10 {
        let freq = freq as f64;
        instance.load_noise(&model, &mut sim, freq);
        let white_noise1 = MFACTOR * PWR * V_AC;
        let white_noise2 = MFACTOR * PWR * PWR * V_AC;
        let flicker_noise1 = MFACTOR * V_AC * PWR * PWR / (freq.powf(EXP));
        let flicker_noise2 = MFACTOR * PWR * PWR / (freq.powf(EXP * V_AC));
        assert_approx_eq!(sim.read_noise(0), white_noise1);
        assert_approx_eq!(sim.read_noise(1), white_noise2);
        assert_approx_eq!(sim.read_noise(2), flicker_noise1);
        assert_approx_eq!(sim.read_noise(3), flicker_noise2);
    }

    Ok(())
}

harness! {
    // TODO: run this in CI, somehow this test is flakey tough regarding the linker invocation (and really slow)
    Test::from_dir("integration", &integration_test, &ignore_dev_tests, &project_root().join("integration_tests")),
    Test::new("$limit", &test_limit),
    Test::new("noise", &test_noise)
}
