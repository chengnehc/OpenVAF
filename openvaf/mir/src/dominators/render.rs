use std::fs::File;
use std::path::Path;

use crate::{Block, Function};

use super::DominatorTree;

pub struct DomTreeRender<'a> {
    pub dom_tree: &'a DominatorTree,
    pub func: &'a Function,
    pub name: &'a str,
    pub reverse: bool,
}

impl DominatorTree {
    pub fn render_idom(&self, dst: &Path, name: &str, func: &Function) {
        DomTreeRender { dom_tree: self, func, name, reverse: false }.to_dot(dst)
    }

    pub fn render_ipdom(&self, dst: &Path, name: &str, func: &Function) {
        DomTreeRender { dom_tree: self, func, name, reverse: true }.to_dot(dst)
    }

    // pub fn render_dom_frontier(&self, dst: &Path, name: &str, func: &Function) {
    //     DomTreeRender { dom_tree: self, func, name, postdom: false, frontiers: true }.to_dot(dst)
    // }
}

impl DomTreeRender<'_> {
    pub fn to_dot(&self, dst: &Path) {
        let mut dst = File::create(dst).unwrap();
        dot::render(self, &mut dst).unwrap()
    }
}

impl<'a> dot::Labeller<'a, Block, (Block, Block)> for DomTreeRender<'a> {
    fn graph_id(&'a self) -> dot::Id<'a> {
        dot::Id::new(self.name).unwrap()
    }

    fn node_id(&'a self, n: &Block) -> dot::Id<'a> {
        dot::Id::new(n.to_string()).unwrap()
    }
}

impl<'a> dot::GraphWalk<'a, Block, (Block, Block)> for DomTreeRender<'a> {
    fn nodes(&'a self) -> dot::Nodes<'a, Block> {
        self.func.layout.blocks().collect()
    }

    fn edges(&'a self) -> dot::Edges<'a, (Block, Block)> {
        let nodes = if self.reverse { &self.dom_tree.reverse_nodes } else { &self.dom_tree.nodes };
        self.func.layout.blocks().filter_map(|bb| Some((nodes[bb].idom.expand()?, bb))).collect()
    }

    fn source(&'a self, edge: &(Block, Block)) -> Block {
        edge.0
    }

    fn target(&'a self, edge: &(Block, Block)) -> Block {
        edge.1
    }
}
