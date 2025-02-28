use std::fmt::{self, Write};

use crate::db::HirDefDB;
use crate::nameres::{DefMap, LocalScopeId};

use super::ScopeItemDef;

impl DefMap {
    pub fn dump(&self, db: &dyn HirDefDB) -> Result<String, fmt::Error> {
        let mut p = Printer { db, buf: String::new(), indent_level: 0, needs_indent: true };
        p.print_def_map_root(self)?;
        p.buf.push('\n');

        Ok(p.buf)
    }
}

struct Printer<'a> {
    db: &'a dyn HirDefDB,
    buf: String,
    indent_level: usize,
    needs_indent: bool,
}

impl<'a> Printer<'a> {
    fn indented(&mut self, f: impl FnOnce(&mut Self) -> fmt::Result) -> fmt::Result {
        self.indent_level += 1;
        writeln!(self)?;
        f(self)?;
        self.indent_level -= 1;
        self.buf = self.buf.trim_end_matches('\n').to_string();

        Ok(())
    }

    fn print_def_map_root(&mut self, map: &DefMap) -> fmt::Result {
        self.print_scope(map, map.root_scope())
    }

    fn print_def_map(&mut self, map: &DefMap) -> fmt::Result {
        self.print_scope(map, map.entry_scope())
    }

    fn print_scope(&mut self, map: &DefMap, local_scope: LocalScopeId) -> fmt::Result {
        let mut declarations: Vec<_> = map.scopes[local_scope]
            .declarations
            .iter()
            .map(|(name, def)| (name.clone(), *def))
            .collect();
        declarations.sort_unstable_by_key(|(name, _)| name.clone());
        for (name, def) in declarations {
            write!(self, "{} = {};", name, def.item_kind())?;
            match def {
                ScopeItemDef::BlockId(block) => {
                    if let Some(def_map) = self.db.block_def_map(block) {
                        self.indented(|s| s.print_def_map(&def_map))?;
                    }
                }
                ScopeItemDef::FunctionId(fun) => {
                    let def_map = self.db.function_def_map(fun);
                    self.indented(|s| s.print_def_map(&def_map))?;
                }
                _ => {
                    if let Some(child) = map.scopes[local_scope].children.get(&name) {
                        self.indented(|s| s.print_scope(map, *child))?;
                    } else {
                        writeln!(self)?;
                    }
                }
            }
        }

        Ok(())
    }
}

impl<'a> Write for Printer<'a> {
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
