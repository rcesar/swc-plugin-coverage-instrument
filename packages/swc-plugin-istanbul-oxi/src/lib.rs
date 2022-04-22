use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

use constants::idents::*;
use istanbul_oxi_instrument::{Range, SourceCoverage};
use once_cell::sync::Lazy;
use serde_json::Value;
use swc_ecma_quote::swc_common::source_map::Pos;
use swc_plugin::{
    ast::*,
    comments::{Comment, CommentKind, Comments, PluginCommentsProxy},
    plugin_transform,
    source_map::PluginSourceMapProxy,
    syntax_pos::DUMMY_SP,
    utils::take::Take,
    TransformPluginProgramMetadata,
};

mod constants;
mod template;
mod utils;

use regex::Regex as Regexp;
use template::{
    create_coverage_fn_decl::create_coverage_fn_decl,
    create_global_stmt_template::create_global_stmt_template,
};

/// This is not fully identical to original file comments
/// https://github.com/istanbuljs/istanbuljs/blob/6f45283feo31faaa066375528f6b68e3a9927b2d5/packages/istanbul-lib-instrument/src/visitor.js#L10=
/// as regex package doesn't support lookaround
static COMMENT_FILE_REGEX: Lazy<Regexp> =
    Lazy::new(|| Regexp::new(r"^\s*istanbul\s+ignore\s+(file)(\W|$)").unwrap());

struct UnknownReserved;
impl Default for UnknownReserved {
    fn default() -> UnknownReserved {
        UnknownReserved
    }
}

struct CoverageVisitor<'a> {
    comments: Option<&'a PluginCommentsProxy>,
    source_map: &'a PluginSourceMapProxy,
    var_name: String,
    var_name_ident: Ident,
    file_path: String,
    attrs: UnknownReserved,
    next_ignore: Option<UnknownReserved>,
    cov: SourceCoverage,
    ignore_class_method: UnknownReserved,
    types: UnknownReserved,
    source_mapping_url: Option<UnknownReserved>,
    instrument_options: InstrumentOptions,
}

impl<'a> CoverageVisitor<'a> {
    pub fn new(
        comments: Option<&'a PluginCommentsProxy>,
        source_map: &'a PluginSourceMapProxy,
        var_name: &str,
        attrs: UnknownReserved,
        next_ignore: Option<UnknownReserved>,
        cov: SourceCoverage,
        ignore_class_method: UnknownReserved,
        types: UnknownReserved,
        source_mapping_url: Option<UnknownReserved>,
        instrument_options: InstrumentOptions,
    ) -> CoverageVisitor<'a> {
        let var_name_hash = CoverageVisitor::get_var_name_hash(var_name);

        CoverageVisitor {
            comments,
            source_map,
            var_name: var_name_hash.clone(),
            var_name_ident: Ident::new(var_name_hash.into(), DUMMY_SP),
            file_path: var_name.to_string(),
            attrs,
            next_ignore,
            cov,
            ignore_class_method,
            types,
            source_mapping_url,
            instrument_options,
        }
    }

    fn get_var_name_hash(name: &str) -> String {
        let mut s = DefaultHasher::new();
        name.hash(&mut s);
        return format!("cov_{}", s.finish());
    }

    /// Not implemented.
    /// TODO: is this required?
    fn is_instrumented_already(&self) -> bool {
        return false;
    }

    fn on_enter(&mut self) {}

    fn on_exit(&mut self) {}
}

/// Visit statements, create a call to increase statement counter.
struct StmtVisitor<'a> {
    pub source_map: &'a PluginSourceMapProxy,
    pub cov: &'a mut SourceCoverage,
    pub var_name: &'a Ident,
    pub before_stmts: Vec<Stmt>,
    pub after_stmts: Vec<Stmt>,
    pub replace: bool,
}

impl<'a> StmtVisitor<'a> {
    fn insert_statement_counter(&mut self, stmt: &mut Stmt) {
        let stmt_span = match stmt {
            Stmt::Block(BlockStmt { span, .. })
            | Stmt::Empty(EmptyStmt { span, .. })
            | Stmt::Debugger(DebuggerStmt { span, .. })
            | Stmt::With(WithStmt { span, .. })
            | Stmt::Return(ReturnStmt { span, .. })
            | Stmt::Labeled(LabeledStmt { span, .. })
            | Stmt::Break(BreakStmt { span, .. })
            | Stmt::Continue(ContinueStmt { span, .. })
            | Stmt::If(IfStmt { span, .. })
            | Stmt::Switch(SwitchStmt { span, .. })
            | Stmt::Throw(ThrowStmt { span, .. })
            | Stmt::Try(TryStmt { span, .. })
            | Stmt::While(WhileStmt { span, .. })
            | Stmt::DoWhile(DoWhileStmt { span, .. })
            | Stmt::For(ForStmt { span, .. })
            | Stmt::ForIn(ForInStmt { span, .. })
            | Stmt::ForOf(ForOfStmt { span, .. })
            | Stmt::Decl(Decl::Class(ClassDecl { class: Class { span, .. }, ..}))
            | Stmt::Decl(Decl::Fn(FnDecl { function: Function { span, .. }, ..}))
            | Stmt::Decl(Decl::Var(VarDecl { span, ..}))
            // TODO: need this?
            | Stmt::Decl(Decl::TsInterface(TsInterfaceDecl { span, ..}))
            | Stmt::Decl(Decl::TsTypeAlias(TsTypeAliasDecl { span, ..}))
            | Stmt::Decl(Decl::TsEnum(TsEnumDecl { span, ..}))
            | Stmt::Decl(Decl::TsModule(TsModuleDecl { span, ..}))
            | Stmt::Expr(ExprStmt { span, .. })
            => span,
        };

        let stmt_hi_loc = self.source_map.lookup_char_pos(stmt_span.hi);
        let stmt_lo_loc = self.source_map.lookup_char_pos(stmt_span.lo);

        let stmt_range = Range::new(
            stmt_hi_loc.line as u32,
            stmt_hi_loc.col.to_u32(),
            stmt_lo_loc.line as u32,
            stmt_lo_loc.col.to_u32(),
        );

        let idx = self.cov.new_statement(&stmt_range);
        let increment_expr = self.build_increase_expression(&IDENT_S, idx, None);
        self.insert_counter(stmt, increment_expr);
    }

    /// Creates a expr like `cov_17709493053001988098().s[0]++;`
    fn build_increase_expression(
        &mut self,
        type_ident: &Ident,
        id: u32,
        i: Option<UnknownReserved>,
    ) -> Stmt {
        if let Some(_i) = i {
            todo!("Not implemented yet!")
        } else {
            let call = CallExpr {
                span: DUMMY_SP,
                callee: Callee::Expr(Box::new(Expr::Ident(self.var_name.clone()))),
                args: vec![],
                type_args: None,
            };

            let c = MemberExpr {
                span: DUMMY_SP,
                obj: Box::new(Expr::Call(call)),
                prop: MemberProp::Ident(type_ident.clone()),
            };

            let expr = MemberExpr {
                span: DUMMY_SP,
                obj: Box::new(Expr::Member(c)),
                prop: MemberProp::Computed(ComputedPropName {
                    span: DUMMY_SP,
                    expr: Box::new(Expr::Lit(Lit::Num(Number {
                        span: DUMMY_SP,
                        value: id as f64,
                        raw: None,
                    }))),
                }),
            };

            let expr_update = UpdateExpr {
                span: DUMMY_SP,
                op: UpdateOp::PlusPlus,
                prefix: false,
                arg: Box::new(Expr::Member(expr)),
            };

            Stmt::Expr(ExprStmt {
                span: DUMMY_SP,
                expr: Box::new(Expr::Update(expr_update)),
            })
        }
    }

    fn insert_counter(&mut self, current: &Stmt, increment_expr: Stmt) {
        match current {
            _ => {
                self.before_stmts.push(increment_expr);
            }
        }
    }
}

impl VisitMut for StmtVisitor<'_> {
    fn visit_mut_stmt(&mut self, stmt: &mut Stmt) {
        stmt.visit_mut_children_with(self);

        self.insert_statement_counter(stmt);
    }
}

impl VisitMut for CoverageVisitor<'_> {
    fn visit_mut_program(&mut self, program: &mut Program) {
        if should_ignore_file(&self.comments, program) {
            return;
        }

        if self.is_instrumented_already() {
            return;
        }

        program.visit_mut_children_with(self);

        let span = match &program {
            Program::Module(m) => m.span,
            Program::Script(s) => s.span,
        };

        let coverage_data_json_str = serde_json::to_string(self.cov.as_ref())
            .expect("Should able to serialize coverage data");

        // Append coverage data as stringified JSON comments at the bottom of transformed code.
        // Currently plugin does not have way to pass any other data to the host except transformed program.
        // This attaches arbitary data to the transformed code itself to retrieve it.
        self.comments.add_trailing(
            span.hi,
            Comment {
                kind: CommentKind::Block,
                span: DUMMY_SP,
                text: format!("__coverage_data_json_comment__::{}", coverage_data_json_str).into(),
            },
        );
    }

    fn visit_mut_for_stmt(&mut self, for_stmt: &mut ForStmt) {
        self.on_enter();
        for_stmt.visit_mut_children_with(self);
        self.on_exit();
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);

        let mut new_stmts: Vec<Stmt> = vec![];

        for mut stmt in stmts.drain(..) {
            let mut stmt_visitor = StmtVisitor {
                source_map: self.source_map,
                var_name: &self.var_name_ident,
                cov: &mut self.cov,
                before_stmts: vec![],
                after_stmts: vec![],
                replace: false,
            };

            stmt.visit_mut_with(&mut stmt_visitor);

            new_stmts.extend(stmt_visitor.before_stmts);
            if !stmt_visitor.replace {
                new_stmts.push(stmt);
            }
            new_stmts.extend(stmt_visitor.after_stmts);
        }

        *stmts = new_stmts;
    }

    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        if self.is_instrumented_already() {
            return;
        }

        for item in items.iter_mut() {
            item.visit_mut_children_with(self);
        }

        self.cov.freeze();

        //TODO: option: global coverage variable scope. (optional, default `this`)
        let coverage_global_scope = "this";
        //TODO: option: use an evaluated function to find coverageGlobalScope.
        let coverage_global_scope_func = true;

        let gv_template = if coverage_global_scope_func {
            // TODO: path.scope.getBinding('Function')
            let is_function_binding_scope = false;

            if is_function_binding_scope {
                /*
                gvTemplate = globalTemplateAlteredFunction({
                    GLOBAL_COVERAGE_SCOPE: T.stringLiteral(
                        'return ' + opts.coverageGlobalScope
                    )
                });
                 */
                unimplemented!("");
            } else {
                create_global_stmt_template(coverage_global_scope)
            }
        } else {
            unimplemented!("");
            /*
            gvTemplate = globalTemplateVariable({
                GLOBAL_COVERAGE_SCOPE: opts.coverageGlobalScope
            });
            */
        };

        let (coverage_fn_ident, coverage_template) = create_coverage_fn_decl(
            &self.instrument_options.coverage_variable,
            gv_template,
            &self.var_name,
            &self.file_path,
            self.cov.as_ref(),
        );

        // explicitly call this.varName to ensure coverage is always initialized
        let m = ModuleItem::Stmt(Stmt::Expr(ExprStmt {
            span: DUMMY_SP,
            expr: Box::new(Expr::Call(CallExpr {
                callee: Callee::Expr(Box::new(Expr::Ident(coverage_fn_ident))),
                ..CallExpr::dummy()
            })),
        }));

        // prepend template to the top of the code
        items.insert(0, coverage_template);
        items.insert(1, m);
    }
}

fn should_ignore_file(comments: &Option<&PluginCommentsProxy>, program: &Program) -> bool {
    if let Some(comments) = *comments {
        let pos = match program {
            Program::Module(module) => module.span,
            Program::Script(script) => script.span,
        };

        let validate_comments = |comments: &Option<Vec<Comment>>| {
            if let Some(comments) = comments {
                comments
                    .iter()
                    .any(|comment| COMMENT_FILE_REGEX.is_match(&comment.text))
            } else {
                false
            }
        };

        vec![
            comments.get_leading(pos.lo),
            comments.get_leading(pos.hi),
            comments.get_trailing(pos.lo),
            comments.get_trailing(pos.hi),
        ]
        .iter()
        .any(|c| validate_comments(c))
    } else {
        false
    }
}

struct InstrumentOptions {
    pub coverage_variable: String,
    pub compact: bool,
    pub report_logic: bool,
}

#[plugin_transform]
pub fn process(program: Program, metadata: TransformPluginProgramMetadata) -> Program {
    let context: Value = serde_json::from_str(&metadata.transform_context)
        .expect("Should able to deserialize context");
    let filename = if let Some(filename) = (&context["filename"]).as_str() {
        filename
    } else {
        "unknown.js"
    };

    let instrument_options_value: Value = serde_json::from_str(&metadata.plugin_config)
        .expect("Should able to deserialize plugin config");
    let instrument_options = InstrumentOptions {
        coverage_variable: instrument_options_value["coverageVariable"]
            .as_str()
            .unwrap_or("__coverage__")
            .to_string(),
        compact: instrument_options_value["compact"]
            .as_bool()
            .unwrap_or(false),
        report_logic: instrument_options_value["reportLogic"]
            .as_bool()
            .unwrap_or(false),
    };

    let visitor = CoverageVisitor::new(
        metadata.comments.as_ref(),
        &metadata.source_map,
        filename,
        UnknownReserved,
        None,
        SourceCoverage::new(filename.to_string(), instrument_options.report_logic),
        UnknownReserved,
        UnknownReserved,
        None,
        instrument_options,
    );

    program.fold_with(&mut as_folder(visitor))
}
