use rustc::lint::*;
use rustc::ty::TypeVariants::{TyRawPtr, TyRef};
use rustc::ty;
use rustc::hir::*;
use utils::{match_def_path, paths, snippet_opt, span_lint, span_lint_and_then};

/// **What it does:** This lint checks for transmutes that can't ever be correct on any architecture
///
/// **Why is this bad?** It's basically guaranteed to be undefined behaviour
///
/// **Known problems:** When accessing C, users might want to store pointer sized objects in `extradata` arguments to save an allocation.
///
/// **Example:** `let ptr: *const T = core::intrinsics::transmute('x')`.
declare_lint! {
    pub WRONG_TRANSMUTE,
    Warn,
    "transmutes that are confusing at best, undefined behaviour at worst and always useless"
}

/// **What it does:** This lint checks for transmutes to the original type of the object and transmutes that could be a cast.
///
/// **Why is this bad?** Readability. The code tricks people into thinking that something complex is going on
///
/// **Known problems:** None.
///
/// **Example:** `core::intrinsics::transmute(t)` where the result type is the same as `t`'s.
declare_lint! {
    pub USELESS_TRANSMUTE,
    Warn,
    "transmutes that have the same to and from types or could be a cast/coercion"
}

/// **What it does:*** This lint checks for transmutes between a type `T` and `*T`.
///
/// **Why is this bad?** It's easy to mistakenly transmute between a type and a pointer to that type.
///
/// **Known problems:** None.
///
/// **Example:** `core::intrinsics::transmute(t)` where the result type is the same as `*t` or `&t`'s.
declare_lint! {
    pub CROSSPOINTER_TRANSMUTE,
    Warn,
    "transmutes that have to or from types that are a pointer to the other"
}

/// **What it does:*** This lint checks for transmutes from a pointer to a reference.
///
/// **Why is this bad?** This can always be rewritten with `&` and `*`.
///
/// **Known problems:** None.
///
/// **Example:**
/// ```rust
/// let _: &T = std::mem::transmute(p); // where p: *const T
/// // can be written:
/// let _: &T = &*p;
/// ```
declare_lint! {
    pub TRANSMUTE_PTR_TO_REF,
    Warn,
    "transmutes from a pointer to a reference type"
}

pub struct Transmute;

impl LintPass for Transmute {
    fn get_lints(&self) -> LintArray {
        lint_array![CROSSPOINTER_TRANSMUTE, TRANSMUTE_PTR_TO_REF, USELESS_TRANSMUTE, WRONG_TRANSMUTE]
    }
}

impl LateLintPass for Transmute {
    fn check_expr(&mut self, cx: &LateContext, e: &Expr) {
        if let ExprCall(ref path_expr, ref args) = e.node {
            if let ExprPath(None, _) = path_expr.node {
                let def_id = cx.tcx.expect_def(path_expr.id).def_id();

                if match_def_path(cx, def_id, &paths::TRANSMUTE) {
                    let from_ty = cx.tcx.expr_ty(&args[0]);
                    let to_ty = cx.tcx.expr_ty(e);

                    match (&from_ty.sty, &to_ty.sty) {
                        _ if from_ty == to_ty => span_lint(
                            cx,
                            USELESS_TRANSMUTE,
                            e.span,
                            &format!("transmute from a type (`{}`) to itself", from_ty),
                        ),
                        (&TyRef(_, rty), &TyRawPtr(ptr_ty)) => span_lint_and_then(
                            cx,
                            USELESS_TRANSMUTE,
                            e.span,
                            "transmute from a reference to a pointer",
                            |db| {
                                if let Some(arg) = snippet_opt(cx, args[0].span) {
                                    let sugg = if ptr_ty == rty {
                                        format!("{} as {}", arg, to_ty)
                                    } else {
                                        format!("{} as {} as {}", arg, cx.tcx.mk_ptr(rty), to_ty)
                                    };

                                    db.span_suggestion(e.span, "try", sugg);
                                }
                            },
                        ),
                        (&ty::TyInt(_), &TyRawPtr(_)) |
                        (&ty::TyUint(_), &TyRawPtr(_)) => span_lint_and_then(
                            cx,
                            USELESS_TRANSMUTE,
                            e.span,
                            "transmute from an integer to a pointer",
                            |db| {
                                if let Some(arg) = snippet_opt(cx, args[0].span) {
                                    db.span_suggestion(e.span, "try", format!("{} as {}", arg, to_ty));
                                }
                            },
                        ),
                        (&ty::TyFloat(_), &TyRef(..)) |
                        (&ty::TyFloat(_), &TyRawPtr(_)) |
                        (&ty::TyChar, &TyRef(..)) |
                        (&ty::TyChar, &TyRawPtr(_)) => span_lint(
                            cx,
                            WRONG_TRANSMUTE,
                            e.span,
                            &format!("transmute from a `{}` to a pointer", from_ty),
                        ),
                        (&TyRawPtr(from_ptr), _) if from_ptr.ty == to_ty => span_lint(
                            cx,
                            CROSSPOINTER_TRANSMUTE,
                            e.span,
                            &format!("transmute from a type (`{}`) to the type that it points to (`{}`)",
                                     from_ty,
                                     to_ty),
                        ),
                        (_, &TyRawPtr(to_ptr)) if to_ptr.ty == from_ty => span_lint(
                            cx,
                            CROSSPOINTER_TRANSMUTE,
                            e.span,
                            &format!("transmute from a type (`{}`) to a pointer to that type (`{}`)",
                                     from_ty,
                                     to_ty),
                        ),
                        (&TyRawPtr(from_pty), &TyRef(_, to_rty)) => span_lint_and_then(
                            cx,
                            TRANSMUTE_PTR_TO_REF,
                            e.span,
                            &format!("transmute from a pointer type (`{}`) to a reference type (`{}`)",
                                    from_ty,
                                    to_ty),
                            |db| {
                                if let Some(arg) = snippet_opt(cx, args[0].span) {
                                    let (deref, cast) = if to_rty.mutbl == Mutability::MutMutable {
                                        ("&mut *", "*mut")
                                    } else {
                                        ("&*", "*const")
                                    };


                                    let sugg = if from_pty.ty == to_rty.ty {
                                        // Put things in parentheses if they are more complex
                                        match args[0].node {
                                            ExprPath(..) | ExprCall(..) | ExprMethodCall(..) | ExprBlock(..) => {
                                                format!("{}{}", deref, arg)
                                            }
                                            _ => format!("{}({})", deref, arg)
                                        }
                                    } else {
                                        format!("{}({} as {} {})", deref, arg, cast, to_rty.ty)
                                    };

                                    db.span_suggestion(e.span, "try", sugg);
                                }
                            },
                        ),
                        _ => return,
                    };
                }
            }
        }
    }
}
