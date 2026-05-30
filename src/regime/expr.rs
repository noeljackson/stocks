//! Tiny expression evaluator: `<op><number>` against a single float.
//! Supported ops: `>`, `>=`, `<`, `<=`, `==`, `!=`.

use thiserror::Error;

#[derive(Debug, Error)]
pub(super) enum EvalError {
    #[error("empty expression")]
    Empty,
    #[error("unknown operator in {0:?}")]
    UnknownOp(String),
    #[error("bad number: {0}")]
    BadNumber(String),
}

pub(super) fn eval(v: f64, expr: &str) -> Result<bool, EvalError> {
    let expr = expr.trim();
    if expr.is_empty() {
        return Err(EvalError::Empty);
    }
    // Longest ops first so `<=` doesn't get parsed as `<`.
    let (op, rest) = if let Some(r) = expr.strip_prefix(">=") {
        (">=", r)
    } else if let Some(r) = expr.strip_prefix("<=") {
        ("<=", r)
    } else if let Some(r) = expr.strip_prefix("==") {
        ("==", r)
    } else if let Some(r) = expr.strip_prefix("!=") {
        ("!=", r)
    } else if let Some(r) = expr.strip_prefix('>') {
        (">", r)
    } else if let Some(r) = expr.strip_prefix('<') {
        ("<", r)
    } else {
        return Err(EvalError::UnknownOp(expr.to_string()));
    };
    let n: f64 = rest
        .trim()
        .parse()
        .map_err(|_| EvalError::BadNumber(rest.trim().to_string()))?;
    Ok(match op {
        ">" => v > n,
        ">=" => v >= n,
        "<" => v < n,
        "<=" => v <= n,
        "==" => (v - n).abs() < f64::EPSILON,
        "!=" => (v - n).abs() >= f64::EPSILON,
        _ => unreachable!(),
    })
}
