//! Classifies a `#[when]` expression as pure when it stays inside the closed subset that
//! tooling may evaluate: it calls no user function, so evaluating it over supplied config
//! cannot run arbitrary application code.
//!
//! The subset is boolean input field reads, `X.eligible()`, `&&`, `||`, `!` and `true` /
//! `false`, plus `==` / `!=` between input field reads, literals, paths (unit enum variants,
//! constants, `None`), `Some(<operand>)` and `resolve("<path>")`. Anything else, macros
//! included, is ordinary trusted code.
//!
//! Classification is syntactic, so it cannot see name resolution: `Some` and `resolve` must be
//! written bare, and an expression whose scope shadows either with a user function is
//! misclassified. Like a custom `PartialEq`, that shadowing is the application's to avoid.

use syn::{BinOp, Expr, ExprCall, ExprLit, ExprMethodCall, ExprPath, ExprUnary, Lit, UnOp};

use super::inputs::{input_field_chain, is_input_receiver, is_resolve};

/// The method a component input's `Eligibility` exposes.
const ELIGIBLE_METHOD: &str = "eligible";

/// The `Option` constructor allowed around a compared operand.
const SOME_CONSTRUCTOR: &str = "Some";

/// The framework's raw config read.
const RESOLVE_FUNCTION: &str = "resolve";

/// Whether `expr`, as written before input lowering, is a pure boolean expression.
pub(super) fn is_pure(expr: &Expr) -> bool {
    match expr {
        Expr::Paren(paren) => is_pure(&paren.expr),

        Expr::Group(group) => is_pure(&group.expr),

        Expr::Lit(ExprLit {
            lit: Lit::Bool(_), ..
        }) => true,

        Expr::Unary(ExprUnary {
            op: UnOp::Not(_),
            expr,
            ..
        }) => is_pure(expr),

        Expr::Binary(binary) => match binary.op {
            BinOp::And(_) | BinOp::Or(_) => is_pure(&binary.left) && is_pure(&binary.right),

            BinOp::Eq(_) | BinOp::Ne(_) => is_operand(&binary.left) && is_operand(&binary.right),

            _ => false,
        },

        _ => input_field_chain(expr).is_some() || is_eligibility(expr),
    }
}

/// Whether `expr` is a side of a pure comparison.
fn is_operand(expr: &Expr) -> bool {
    match expr {
        Expr::Paren(paren) => is_operand(&paren.expr),

        Expr::Group(group) => is_operand(&group.expr),

        Expr::Lit(_) => true,

        Expr::Unary(ExprUnary {
            op: UnOp::Neg(_),
            expr,
            ..
        }) => matches!(**expr, Expr::Lit(_)),

        Expr::Path(_) => true,

        Expr::Call(call) => is_some_operand(call) || is_resolve_call(call),

        _ => input_field_chain(expr).is_some() || is_eligibility(expr),
    }
}

/// `Some(<operand>)`, written as the bare prelude constructor.
fn is_some_operand(call: &ExprCall) -> bool {
    is_bare(&call.func, SOME_CONSTRUCTOR)
        && call.args.len() == 1
        && call.args.iter().all(is_operand)
}

/// `resolve("<path>")`, written as the bare imported framework function; input lowering
/// rejects any other argument. A qualified path may name another function, so it is
/// trusted.
fn is_resolve_call(call: &ExprCall) -> bool {
    is_resolve(&call.func)
        && is_bare(&call.func, RESOLVE_FUNCTION)
        && matches!(
            call.args.first(),
            Some(Expr::Lit(ExprLit {
                lit: Lit::Str(_),
                ..
            }))
        )
        && call.args.len() == 1
}

/// Whether `func` is the single identifier `name`.
fn is_bare(func: &Expr, name: &str) -> bool {
    matches!(func, Expr::Path(ExprPath { qself: None, path, .. }) if path.is_ident(name))
}

/// `X.eligible()` on an input receiver.
fn is_eligibility(expr: &Expr) -> bool {
    let Expr::MethodCall(ExprMethodCall {
        receiver,
        method,
        args,
        turbofish,
        ..
    }) = expr
    else {
        return false;
    };

    let on_input = matches!(&**receiver, Expr::Path(path) if is_input_receiver(path));

    on_input && method == ELIGIBLE_METHOD && args.is_empty() && turbofish.is_none()
}

#[cfg(test)]
mod tests;
