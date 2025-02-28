use std::fmt::{self, Write};

use basedb::AstId;
use syntax::ast;

use super::{
    BlockItem, Discipline, Function, FunctionItem, ItemTree, ItemTreeId, Module, ModuleItem,
    Nature, Param, Var,
};

impl ItemTree {
    pub fn dump(&self) -> Result<String, fmt::Error> {
        let mut p = Printer { tree: self, buf: String::new(), indent_level: 0, needs_indent: true };
        p.print()?;
        p.buf.push('\n');

        Ok(p.buf)
    }
}

struct Printer<'a> {
    tree: &'a ItemTree,
    buf: String,
    indent_level: usize,
    needs_indent: bool,
}

impl Printer<'_> {
    fn indented(&mut self, f: impl FnOnce(&mut Self) -> fmt::Result) -> fmt::Result {
        self.indent_level += 1;
        writeln!(self)?;
        f(self)?;
        self.indent_level -= 1;
        self.buf = self.buf.trim_end_matches('\n').to_string();

        Ok(())
    }

    fn print(&mut self) -> fmt::Result {
        for nature in &self.tree.data.natures {
            write!(self, "nature {}", nature.name)?;
            self.indented(|s| s.print_nature(nature))?;
        }

        for discipline in &self.tree.data.disciplines {
            write!(self, "discipline {}", discipline.name)?;
            self.indented(|s| s.print_discipline(discipline))?;
        }

        for module in &self.tree.data.modules {
            write!(self, "module {}", module.name)?;
            self.indented(|s| s.print_module(module))?;
        }

        Ok(())
    }

    fn print_nature(&mut self, nature: &Nature) -> fmt::Result {
        let Nature { parent, units, ddt_nature, idt_nature, access, .. } = nature;
        write!(
            self,
            "parent = {parent:?}\n\
            units = {units:?}\n\
            ddt_nature = {ddt_nature:?}\n\
            idt_nature = {idt_nature:?}\n\
            access = {access:?}\n"
        )?;
        for attr in nature.attrs.clone() {
            writeln!(self, "attr{}: {}", u32::from(attr), self.tree[attr].name)?;
        }

        Ok(())
    }

    fn print_discipline(&mut self, discipline: &Discipline) -> fmt::Result {
        let Discipline { potential, flow, domain, .. } = discipline;
        write!(
            self,
            "potential = {potential:?}\n\
            flow = {flow:?}\n\
            domain = {domain:?}\n"
        )?;
        for attr in discipline.attrs.clone() {
            writeln!(
                self,
                "attr{}: {} ({:?})",
                u32::from(attr),
                self.tree[attr].name,
                self.tree[attr].kind
            )?;
        }

        Ok(())
    }

    fn print_module(&mut self, module: &Module) -> fmt::Result {
        for item in &module.items {
            match *item {
                ModuleItem::Block(block) => self.print_block(block)?,
                ModuleItem::Parameter(param) => self.print_parameter(param)?,
                ModuleItem::Variable(var) => self.print_var(var)?,
                ModuleItem::Branch(branch) => {
                    let branch = &self.tree[branch];
                    writeln!(self, "branch {} = {:?}", branch.name, branch.kind)?
                }
                ModuleItem::Node(node) => {
                    let node = &module.nodes[node];
                    let (is_input, is_output) = node.direction(self.tree);
                    writeln!(
                        self,
                        "node {} = {{is_input: {}, is_output:{}, gnd: {} , discipline {:?}}}",
                        node.name,
                        is_input,
                        is_output,
                        node.is_gnd(self.tree),
                        node.discipline(self.tree),
                    )?;
                }
                ModuleItem::Function(function) => {
                    let function = &self.tree[function];
                    write!(self, "function {}", function.name)?;
                    self.indented(|s| s.print_function(function))?;
                }
                ModuleItem::AliasParam(param) => {
                    let param = &self.tree[param];
                    writeln!(self, "aliasparam {} = {:?}", param.name, param.src)?;
                }
            }
        }

        Ok(())
    }

    fn print_block(&mut self, block: AstId<ast::BlockStmt>) -> fmt::Result {
        let block = &self.tree[block];
        write!(self, "block {:?}", block.name)?;
        self.indented(|s| s.print_block_items(&block.block_items))
    }

    fn print_parameter(&mut self, param: ItemTreeId<Param>) -> fmt::Result {
        let param = &self.tree[param];
        writeln!(self, "param {} {}", param.ty.as_ref().unwrap_or(&crate::Type::Err), param.name)
    }

    fn print_var(&mut self, var: ItemTreeId<Var>) -> fmt::Result {
        let var = &self.tree[var];
        writeln!(self, "var {} {}", var.ty, var.name)
    }

    fn print_block_items(&mut self, items: &[BlockItem]) -> fmt::Result {
        for item in items {
            match *item {
                BlockItem::Block(block) => self.print_block(block)?,
                BlockItem::Parameter(param) => self.print_parameter(param)?,
                BlockItem::Variable(var) => self.print_var(var)?,
            }
        }

        Ok(())
    }

    fn print_function(&mut self, function: &Function) -> fmt::Result {
        for item in &function.items {
            match *item {
                FunctionItem::Block(block) => self.print_block(block)?,
                FunctionItem::Parameter(param) => self.print_parameter(param)?,
                FunctionItem::Variable(var) => self.print_var(var)?,
                FunctionItem::FunctionArg(arg) => {
                    let arg = &function.args[arg];
                    writeln!(
                        self,
                        "arg {:?} {} = {{ is_input = {}, is_output = {}}}",
                        arg.ty(self.tree),
                        arg.name,
                        arg.is_input,
                        arg.is_output
                    )?;
                }
            }
        }

        Ok(())
    }
}

impl fmt::Write for Printer<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for line in s.split_inclusive('\n') {
            if self.needs_indent {
                match self.buf.chars().last() {
                    Some('\n') | None => {}
                    _ => self.buf.push('\n'),
                }
                if line != "\n" {
                    // don't indent empty lines! required to play nice with expect_test
                    self.buf.push_str(&"    ".repeat(self.indent_level));
                }
                self.needs_indent = false;
            }

            self.buf.push_str(line);
            self.needs_indent = line.ends_with('\n');
        }

        Ok(())
    }
}
