use std::fmt;

use super::{
    osdi_str, JacobianFlags, OsdiDescriptor, OsdiNodePair, OsdiNoiseSource, ParameterFlags,
};

impl fmt::Debug for OsdiDescriptor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        macro_rules! w {
            ($($tt: tt)*) => {
                write!(f, $($tt)*)?;
            };
        }
        macro_rules! wn {
            ($($tt: tt)*) => {
                writeln!(f, $($tt)*)?;
            };
        }

        unsafe {
            for param in self.params() {
                assert_eq!(param.len, 0);
                w!("param ");
                for i in 0..=param.num_alias {
                    if i != 0 {
                        w!(", ");
                    }
                    w!("{:?}", osdi_str(*param.name.add(i as usize)));
                }
                w!("  ");
                let desc = osdi_str(param.description);
                let units = osdi_str(param.units);
                let ty = ParameterFlags::from_bits(param.flags).unwrap();
                wn!("units = {units:?}, desc = {desc:?}, flags = {ty:?}");
            }

            wn!();
            wn!("{} terminals", self.num_terminals);
            for node in self.nodes() {
                let flow = if node.is_flow { "(flow)" } else { "" };
                wn!(
                    "node{flow} {:?} units = {:?}, runits = {:?}",
                    osdi_str(node.name),
                    osdi_str(node.units),
                    osdi_str(node.residual_units)
                );
                w!("residual ",);
                if node.resist_residual_off == u32::MAX {
                    w!("N/A ");
                } else {
                    w!("{} ", node.resist_residual_off);
                }
                if node.react_residual_off == u32::MAX {
                    w!("N/A ");
                } else {
                    w!("{} ", node.react_residual_off);
                }
                if node.resist_limit_rhs_off == u32::MAX {
                    w!("N/A ");
                } else {
                    w!("{} ", node.resist_limit_rhs_off);
                }
                if node.react_limit_rhs_off == u32::MAX {
                    wn!("N/A");
                } else {
                    wn!("{}", node.react_limit_rhs_off);
                }
            }

            wn!();
            wn!("{} collapsible node pairs", self.num_collapsible);
            for OsdiNodePair { node_1, node_2 } in self.collapsibles() {
                let hi = self.nodes()[*node_1 as usize].name;
                let lo = if *node_2 == u32::MAX {
                    "gnd"
                } else {
                    osdi_str(self.nodes()[*node_2 as usize].name)
                };
                wn!("collapsible ({}, {})", osdi_str(hi), lo);
            }

            wn!();
            wn!("{} jacobian entries", self.num_jacobian_entries);
            for matrix_entry in self.matrix_entries() {
                let hi = self.nodes()[matrix_entry.nodes.node_1 as usize].name;
                let lo = self.nodes()[matrix_entry.nodes.node_2 as usize].name;
                w!(
                    "jacobian ({}, {}) {:?} ",
                    osdi_str(hi),
                    osdi_str(lo),
                    JacobianFlags::from_bits(matrix_entry.flags).unwrap(),
                );
                if matrix_entry.react_ptr_off == u32::MAX {
                    wn!("react_ptr = N/A");
                } else {
                    wn!("react_ptr = {}", matrix_entry.react_ptr_off);
                }
            }

            wn!();
            wn!("{} noise sources", self.num_noise_src);
            for OsdiNoiseSource { name, nodes: OsdiNodePair { node_1, node_2 } } in self.noises() {
                let hi = self.nodes()[*node_1 as usize].name;
                let lo = if *node_2 == u32::MAX {
                    "gnd"
                } else {
                    osdi_str(self.nodes()[*node_2 as usize].name)
                };
                wn!("noise {:?} ({}, {})", osdi_str(*name), osdi_str(hi), lo);
            }

            wn!();
            wn!("{} states", self.num_states);
            wn!("has bound_step {}", self.bound_step_offset != u32::MAX);

            wn!();
            wn!("instance size {}", self.instance_size);
            wn!("model size {}", self.model_size);

            Ok(())
        }
    }
}
