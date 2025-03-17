//! A renderer of control flow graph using GraphViz API

use std::fs::File;
use std::path::Path;

use crate::{Block, Function};

use super::ControlFlowGraph;

pub struct CfgRender<'a> {
    pub cfg: &'a ControlFlowGraph,
    pub func: &'a Function,
    pub name: &'a str,
}

impl ControlFlowGraph {
    pub fn render(&self, dst: &Path, name: &str, func: &Function) {
        CfgRender { cfg: self, func, name }.to_dot(dst)
    }
}

impl CfgRender<'_> {
    pub fn to_dot(&self, dst: &Path) {
        let mut dst = File::create(dst).unwrap();
        dot::render(self, &mut dst).unwrap()
    }
}

impl<'a> dot::Labeller<'a, Block, (Block, Block)> for CfgRender<'a> {
    fn graph_id(&'a self) -> dot::Id<'a> {
        dot::Id::new(self.name).unwrap()
    }

    fn node_id(&'a self, n: &Block) -> dot::Id<'a> {
        dot::Id::new(n.to_string()).unwrap()
    }
}

impl<'a> dot::GraphWalk<'a, Block, (Block, Block)> for CfgRender<'a> {
    fn nodes(&'a self) -> dot::Nodes<'a, Block> {
        self.func.layout.blocks().collect()
    }

    fn edges(&'a self) -> dot::Edges<'a, (Block, Block)> {
        self.func
            .layout
            .blocks()
            .flat_map(|bb| self.cfg.data[bb].successors.iter().map(move |succ| (bb, succ)))
            .collect()
    }

    fn source(&'a self, edge: &(Block, Block)) -> Block {
        edge.0
    }

    fn target(&'a self, edge: &(Block, Block)) -> Block {
        edge.1
    }
}
