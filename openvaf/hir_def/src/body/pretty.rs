use std::fmt::{self, Write};

use crate::db::HirDefDB;
use crate::expr::CaseCond;
use crate::nameres::DefMapSource;
use crate::{Expr, ExprId, Lookup, Stmt, StmtId};

use super::Body;

impl Body {
    pub fn dump(&self, db: &dyn HirDefDB) -> Result<String, fmt::Error> {
        let mut p =
            Printer { body: self, db, buf: String::new(), indent_level: 0, needs_indent: true };
        for stmt in &self.entry_stmts {
            write!(&mut p, "analog ")?;
            p.pretty_print_stmt(*stmt)?;
        }

        Ok(p.buf)
    }
}

struct Printer<'a> {
    body: &'a Body,
    db: &'a dyn HirDefDB,
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

    pub fn pretty_print_stmt(&mut self, s: StmtId) -> fmt::Result {
        match self.body.stmts[s] {
            Stmt::Missing => writeln!(self, "<missing>;")?,
            Stmt::Empty => writeln!(self, ";")?,
            Stmt::Expr(e) => {
                self.pretty_print_expr(e)?;
                writeln!(self, ";")?;
            }
            Stmt::EventControl { ref event, body } => {
                writeln!(self, "@({:?})", event)?;
                self.pretty_print_stmt(body)?;
            }
            Stmt::Assignment { dst, val, op_kind } => {
                self.pretty_print_expr(dst)?;
                write!(self, " {:?} ", op_kind)?;
                self.pretty_print_expr(val)?;
                writeln!(self, ";")?;
            }
            Stmt::Block { ref body } => {
                write!(self, "begin")?;
                if let Some(first) = body.iter().next() {
                    if let DefMapSource::Block(block) = self.body.stmt_scopes[*first].src {
                        let parent = block.lookup(self.db).parent;
                        write!(self, ": {:?} ({:?})", block, parent)?;
                    } else {
                        write!(self, ": ({:?})", self.body.stmt_scopes[s].src)?;
                    }
                }
                self.indented(|p| {
                    for stmt in body {
                        p.pretty_print_stmt(*stmt)?;
                    }
                    Ok(())
                })?;
                writeln!(self, "end")?;
            }
            Stmt::If { cond, then_branch, else_branch } => {
                write!(self, "if ")?;
                self.pretty_print_expr(cond)?;
                writeln!(self)?;
                self.pretty_print_stmt(then_branch)?;
                writeln!(self, "else")?;
                self.pretty_print_stmt(else_branch)?;
            }
            Stmt::ForLoop { init, cond, incr, body } => {
                write!(self, "for(")?;
                self.indented(|sel| {
                    sel.pretty_print_stmt(init)?;
                    sel.pretty_print_expr(cond)?;
                    writeln!(sel, ";")?;
                    sel.pretty_print_stmt(incr)?;
                    Ok(())
                })?;
                writeln!(self, ")")?;
                self.pretty_print_stmt(body)?;
            }
            Stmt::WhileLoop { cond, body } => {
                write!(self, "while(")?;
                self.pretty_print_expr(cond)?;
                writeln!(self, ")")?;
                self.indented(|p| p.pretty_print_stmt(body))?;
            }
            Stmt::Case { discr, ref case_arms } => {
                write!(self, "case(")?;
                self.pretty_print_expr(discr)?;
                self.indented(|p| {
                    for case in case_arms {
                        match case.cond {
                            CaseCond::Default => write!(p, "default")?,
                            CaseCond::Vals(ref vals) => {
                                for val in vals {
                                    p.pretty_print_expr(*val)?;
                                    writeln!(p, ", ")?;
                                }
                            }
                        }
                        write!(p, ":")?;
                        p.pretty_print_stmt(case.body)?;
                    }
                    Ok(())
                })?;
                writeln!(self, "endcase")?;
            }
        }

        Ok(())
    }

    pub fn pretty_print_expr(&mut self, e: ExprId) -> fmt::Result {
        match self.body.exprs[e] {
            Expr::Missing => write!(self, "<missing>")?,
            Expr::Path { ref path, port: false } => write!(self, "{:?}", path)?,
            Expr::Path { ref path, port: true } => write!(self, "<{:?}>", path)?,
            Expr::BinaryOp { lhs, rhs, op } => {
                self.pretty_print_expr(lhs)?;
                match op {
                    Some(op) => write!(self, " {} ", op)?,
                    None => write!(self, " <invalid> ")?,
                }
                self.pretty_print_expr(rhs)?;
            }
            Expr::UnaryOp { expr, op } => {
                write!(self, "{}", op)?;
                self.pretty_print_expr(expr)?;
            }
            Expr::Select { cond, then_val, else_val } => {
                self.pretty_print_expr(cond)?;
                write!(self, "?")?;
                self.pretty_print_expr(then_val)?;
                write!(self, ":")?;
                self.pretty_print_expr(else_val)?;
            }
            Expr::Call { ref fun, ref args } => {
                match fun {
                    Some(path) => write!(self, "{:?}", path)?,
                    None => write!(self, "<missing>")?,
                }
                write!(self, "(")?;
                for arg in args {
                    self.pretty_print_expr(*arg)?;
                    write!(self, ", ")?;
                }
                write!(self, ")")?;
            }
            Expr::Array(ref vals) => {
                write!(self, "'{{")?;
                for val in vals {
                    self.pretty_print_expr(*val)?;
                }
                write!(self, "}}")?;
            }
            Expr::Literal(ref lit) => write!(self, "{:?}", lit)?,
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
