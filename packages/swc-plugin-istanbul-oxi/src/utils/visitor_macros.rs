#[macro_export]
macro_rules! insert_logical_expr_helper {
    () => {
        /// Attempt to wrap expression with branch increase counter. Given Expr may be left, or right of the logical expression.
        fn wrap_bin_expr_with_branch_counter(&mut self, branch: u32, expr: &mut Expr) {
            // Logical expression can have inner logical expression as non-direct child
            // (i.e `args[0] > 0 && (args[0] < 5 || args[0] > 10)`, logical || expr is child of ParenExpr.
            // Try to look up if current expr is the `leaf` of whole logical expr tree.
            let mut has_inner_logical_expr = crate::visitors::finders::LogicalExprLeafFinder(false);
            expr.visit_with(&mut has_inner_logical_expr);

            // If current expr have inner logical expr, traverse until reaches to the leaf
            if has_inner_logical_expr.0 {
                let span = get_expr_span(expr);
                let should_ignore =
                    crate::utils::hint_comments::should_ignore(&self.comments, span);

                let mut visitor = crate::visitors::logical_expr_visitor::LogicalExprVisitor::new(
                    self.source_map,
                    self.comments,
                    &mut self.cov,
                    &self.instrument_options,
                    &self.nodes,
                    should_ignore,
                    branch,
                );

                expr.visit_mut_children_with(&mut visitor);
            } else {
                // Now we believe this expr is the leaf of the logical expr tree.
                // Wrap it with branch counter.
                if self.instrument_options.report_logic {
                    /*
                    // TODO
                    const increment = this.getBranchLogicIncrement(
                        leaf,
                        b,
                        leaf.node.loc
                    );
                    if (!increment[0]) {
                        continue;
                    }
                    leaf.parent[leaf.property] = T.sequenceExpression([
                        increment[0],
                        increment[1]
                    ]);
                    */
                } else {
                    self.replace_expr_with_branch_counter(expr, branch);
                }
            }
        }
    };
}

/// Interfaces to mark counters. Parent node visitor should pick up and insert marked counter accordingly.
/// Unlike istanbul we can't have single insert logic to be called in any arbitary child node.
#[macro_export]
macro_rules! insert_counter_helper {
    () => {
        fn print_node(&self) -> String {
            if self.nodes.len() > 0 {
                format!(
                    "{}",
                    self.nodes
                        .iter()
                        .map(|n| n.to_string())
                        .collect::<Vec<String>>()
                        .join(":")
                )
            } else {
                "unexpected".to_string()
            }
        }

        #[tracing::instrument(skip(self, span, idx), fields(stmt_id))]
        fn create_stmt_increase_counter_expr(&mut self, span: &Span, idx: Option<u32>) -> Expr {
            let stmt_range = get_range_from_span(self.source_map, span);

            let stmt_id = self.cov.new_statement(&stmt_range);

            tracing::Span::current().record("stmt_id", &stmt_id);

            crate::instrument::create_increase_expression_expr(
                &IDENT_S,
                stmt_id,
                &self.cov_fn_ident,
                idx,
            )
        }

        // Mark to prepend statement increase counter to current stmt.
        // if (path.isStatement()) {
        //    path.insertBefore(T.expressionStatement(increment));
        // }
        #[tracing::instrument(skip_all)]
        fn mark_prepend_stmt_counter(&mut self, span: &Span) {
            let increment_expr = self.create_stmt_increase_counter_expr(span, None);
            self.before.push(Stmt::Expr(ExprStmt {
                span: DUMMY_SP,
                expr: Box::new(increment_expr),
            }));
        }

        // if (path.isExpression()) {
        //    path.replaceWith(T.sequenceExpression([increment, path.node]));
        //}
        #[tracing::instrument(skip_all)]
        fn replace_expr_with_stmt_counter(&mut self, expr: &mut Expr) {
            self.replace_expr_with_counter(expr, |cov, cov_fn_ident, range| {
                let idx = cov.new_statement(&range);
                create_increase_expression_expr(&IDENT_S, idx, cov_fn_ident, None)
            });
        }

        #[tracing::instrument(skip_all)]
        fn replace_expr_with_branch_counter(&mut self, expr: &mut Expr, branch: u32) {
            self.replace_expr_with_counter(expr, |cov, cov_fn_ident, range| {
                let idx = cov.add_branch_path(branch, &range);

                create_increase_expression_expr(&IDENT_B, branch, cov_fn_ident, Some(idx))
            });
        }

        // Base wrapper fn to replace given expr to wrapped paren expr with counter
        #[tracing::instrument(skip_all)]
        fn replace_expr_with_counter<F>(&mut self, expr: &mut Expr, get_counter: F)
        where
            F: core::ops::Fn(&mut SourceCoverage, &Ident, &istanbul_oxi_instrument::Range) -> Expr,
        {
            let span = get_expr_span(expr);
            if let Some(span) = span {
                let init_range = get_range_from_span(self.source_map, span);
                let prepend_expr = get_counter(&mut self.cov, &self.cov_fn_ident, &init_range);

                let paren_expr = Expr::Paren(ParenExpr {
                    span: DUMMY_SP,
                    expr: Box::new(Expr::Seq(SeqExpr {
                        span: DUMMY_SP,
                        exprs: vec![Box::new(prepend_expr), Box::new(expr.take())],
                    })),
                });

                // replace init with increase expr + init seq
                *expr = paren_expr;
            }
        }

        // if (path.isBlockStatement()) {
        //    path.node.body.unshift(T.expressionStatement(increment));
        // }
        fn mark_prepend_stmt_counter_for_body(&mut self) {
            todo!("not implemented");
        }

        fn mark_prepend_stmt_counter_for_hoisted(&mut self) {}

        #[tracing::instrument(skip_all)]
        fn visit_mut_fn(&mut self, ident: &Option<&Ident>, function: &mut Function) {
            let (span, name) = if let Some(ident) = &ident {
                (&ident.span, Some(ident.sym.to_string()))
            } else {
                (&function.span, None)
            };

            let range = get_range_from_span(self.source_map, span);
            let body_span = if let Some(body) = &function.body {
                body.span
            } else {
                // TODO: probably this should never occur
                function.span
            };

            let body_range = get_range_from_span(self.source_map, &body_span);
            let index = self.cov.new_function(&name, &range, &body_range);

            match &mut function.body {
                Some(blockstmt) => {
                    let b =
                        create_increase_expression_expr(&IDENT_F, index, &self.cov_fn_ident, None);
                    let mut prepended_vec = vec![Stmt::Expr(ExprStmt {
                        span: DUMMY_SP,
                        expr: Box::new(b),
                    })];
                    prepended_vec.extend(blockstmt.stmts.take());
                    blockstmt.stmts = prepended_vec;
                }
                _ => {
                    unimplemented!("Unable to process function body node type")
                }
            }
        }

        fn is_injected_counter_expr(&self, expr: &swc_plugin::ast::Expr) -> bool {
            use swc_plugin::ast::*;

            if let Expr::Update(UpdateExpr { arg, .. }) = expr {
                if let Expr::Member(MemberExpr { obj, .. }) = &**arg {
                    if let Expr::Member(MemberExpr { obj, .. }) = &**obj {
                        if let Expr::Call(CallExpr { callee, .. }) = &**obj {
                            if let Callee::Expr(expr) = callee {
                                if let Expr::Ident(ident) = &**expr {
                                    if ident == &self.cov_fn_ident {
                                        return true;
                                    }
                                }
                            }
                        }
                    }
                }
            };
            false
        }

        /// Determine if given stmt is an injected counter by transform.
        fn is_injected_counter_stmt(&self, stmt: &swc_plugin::ast::Stmt) -> bool {
            use swc_plugin::ast::*;

            if let Stmt::Expr(ExprStmt { expr, .. }) = stmt {
                self.is_injected_counter_expr(&**expr)
            } else {
                false
            }
        }

        fn cover_statement(&mut self, expr: &mut Expr) {
            let span = get_expr_span(expr);
            // This is ugly, poor man's substitute to istanbul's `insertCounter` to determine
            // when to replace givn expr to wrapped Paren or prepend stmt counter.
            // We can't do insert parent node's sibling in downstream's child node.
            // TODO: there should be a better way.
            if let Some(span) = span {
                let mut block = crate::visitors::finders::BlockStmtFinder::new();
                expr.visit_with(&mut block);
                // TODO: this may not required as visit_mut_block_stmt recursively visits inner instead.
                if block.0 {
                    //path.node.body.unshift(T.expressionStatement(increment));
                    self.mark_prepend_stmt_counter(span);
                    return;
                }

                let mut stmt = crate::visitors::finders::StmtFinder::new();
                expr.visit_with(&mut stmt);
                if stmt.0 {
                    //path.insertBefore(T.expressionStatement(increment));
                    self.mark_prepend_stmt_counter(span);
                }

                let mut hoist = crate::visitors::finders::HoistingFinder::new();
                expr.visit_with(&mut hoist);
                let parent = self.nodes.last().unwrap().clone();
                if hoist.0 && parent == Node::VarDeclarator {
                    let parent = self.nodes.get(self.nodes.len() - 3);
                    if let Some(parent) = parent {
                        /*if (parent && T.isExportNamedDeclaration(parent.parentPath)) {
                            parent.parentPath.insertBefore(
                                T.expressionStatement(increment)
                            );
                        }  */
                        let parent = self.nodes.get(self.nodes.len() - 4);
                        if let Some(parent) = parent {
                            match parent {
                                Node::BlockStmt | Node::Program => {
                                    self.mark_prepend_stmt_counter(span);
                                }
                                _ => {}
                            }
                        }
                    } else {
                        self.replace_expr_with_stmt_counter(expr);
                    }

                    return;
                }

                let mut expr_finder = crate::visitors::finders::ExprFinder::new();
                expr.visit_with(&mut expr_finder);
                if expr_finder.0 {
                    self.replace_expr_with_stmt_counter(expr);
                }
            }
        }
    };
}

/// Generate common visitors to visit stmt.
#[macro_export]
macro_rules! visit_mut_coverage {
    () => {
        noop_visit_mut_type!();

        // BlockStatement: entries(), // ignore processing only
        #[instrument(skip_all, fields(node = %self.print_node()))]
        fn visit_mut_block_stmt(&mut self, block_stmt: &mut BlockStmt) {
            self.nodes.push(Node::BlockStmt);

            // Recursively visit inner for the blockstmt
            block_stmt.visit_mut_children_with(self);
            self.nodes.pop();
        }

        // FunctionDeclaration: entries(coverFunction),
        #[instrument(skip_all, fields(node = %self.print_node()))]
        fn visit_mut_fn_decl(&mut self, fn_decl: &mut FnDecl) {
            self.nodes.push(Node::FnDecl);
            self.visit_mut_fn(&Some(&fn_decl.ident), &mut fn_decl.function);
            fn_decl.visit_mut_children_with(self);
            self.nodes.pop();
        }

        // ArrowFunctionExpression: entries(convertArrowExpression, coverFunction),
        #[instrument(skip_all, fields(node = %self.print_node()))]
        fn visit_mut_arrow_expr(&mut self, arrow_expr: &mut ArrowExpr) {
            let (old, ignore_current) = self.on_enter(arrow_expr);
            match ignore_current {
                Some(crate::utils::hint_comments::IgnoreScope::Next) => {}
                _ => match &mut arrow_expr.body {
                    BlockStmtOrExpr::BlockStmt(block_stmt) => {
                        let range = get_range_from_span(self.source_map, &arrow_expr.span);
                        let body_range = get_range_from_span(self.source_map, &block_stmt.span);
                        let index = self.cov.new_function(&None, &range, &body_range);
                        let b = create_increase_expression_expr(
                            &IDENT_F,
                            index,
                            &self.cov_fn_ident,
                            None,
                        );

                        // insert fn counter expression
                        let mut new_stmts = vec![Stmt::Expr(ExprStmt {
                            span: DUMMY_SP,
                            expr: Box::new(b),
                        })];
                        // if arrow fn body is already blockstmt, insert stmt counter for each
                        self.insert_stmts_counter(&mut block_stmt.stmts);
                        new_stmts.extend(block_stmt.stmts.drain(..));
                        block_stmt.stmts = new_stmts;
                    }
                    BlockStmtOrExpr::Expr(expr) => {
                        // TODO: refactor common logics creates a blockstmt from single expr
                        let range = get_range_from_span(self.source_map, &arrow_expr.span);
                        let span = get_expr_span(expr);
                        if let Some(span) = span {
                            let body_range = get_range_from_span(self.source_map, &span);
                            let index = self.cov.new_function(&None, &range, &body_range);
                            let b = create_increase_expression_expr(
                                &IDENT_F,
                                index,
                                &self.cov_fn_ident,
                                None,
                            );

                            // insert fn counter expression
                            let mut stmts = vec![Stmt::Expr(ExprStmt {
                                span: DUMMY_SP,
                                expr: Box::new(b),
                            })];

                            // single line expr in arrow fn need to be converted into return stmt
                            // Note we should preserve original expr's span, otherwise statementmap will lose correct
                            // code location
                            let ret = Stmt::Return(ReturnStmt {
                                span: span.clone(),
                                arg: Some(expr.take()),
                            });
                            stmts.push(ret);

                            let mut new_stmts = vec![];
                            // insert stmt counter for the returnstmt we made above
                            self.insert_stmts_counter(&mut stmts);
                            new_stmts.extend(stmts.drain(..));

                            arrow_expr.body = BlockStmtOrExpr::BlockStmt(BlockStmt {
                                span: DUMMY_SP,
                                stmts: new_stmts,
                            });
                        }
                    }
                },
            }
            self.on_exit(old);
        }

        /*
        #[instrument(skip_all, fields(node = %self.print_node()))]
        fn visit_mut_stmt(&mut self, stmt: &mut Stmt) {
            if !self.is_injected_counter_stmt(stmt) {
                let span = crate::utils::lookup_range::get_stmt_span(&stmt);
                if let Some(span) = span {
                    let increment_expr = self.create_stmt_increase_counter_expr(span, None);

                    self.before.push(Stmt::Expr(ExprStmt {
                        span: DUMMY_SP,
                        expr: Box::new(increment_expr),
                    }));
                }
            }
        } */

        #[instrument(skip_all, fields(node = %self.print_node()))]
        fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
            self.nodes.push(Node::Stmts);
            self.insert_stmts_counter(stmts);
            self.nodes.pop();
        }

        // FunctionExpression: entries(coverFunction),
        #[instrument(skip_all, fields(node = %self.print_node()))]
        fn visit_mut_fn_expr(&mut self, fn_expr: &mut FnExpr) {
            self.nodes.push(Node::FnExpr);
            // We do insert counter _first_, then iterate child:
            // Otherwise inner stmt / fn will get the first idx to the each counter.
            // StmtVisitor filters out injected counter internally.
            self.visit_mut_fn(&fn_expr.ident.as_ref(), &mut fn_expr.function);
            fn_expr.visit_mut_children_with(self);
            self.nodes.pop();
        }

        // ExpressionStatement: entries(coverStatement),
        #[instrument(skip_all, fields(node = %self.print_node()))]
        fn visit_mut_expr_stmt(&mut self, expr_stmt: &mut ExprStmt) {
            let (old, ignore_current) = self.on_enter(expr_stmt);

            match ignore_current {
                Some(crate::utils::hint_comments::IgnoreScope::Next) => {}
                _ => {
                    if !self.is_injected_counter_expr(&*expr_stmt.expr) {
                        self.mark_prepend_stmt_counter(&expr_stmt.span);
                    }
                }
            }
            expr_stmt.visit_mut_children_with(self);

            self.on_exit(old);
        }

        // ReturnStatement: entries(coverStatement),
        #[instrument(skip_all, fields(node = %self.print_node()))]
        fn visit_mut_return_stmt(&mut self, return_stmt: &mut ReturnStmt) {
            self.nodes.push(Node::ReturnStmt);
            self.mark_prepend_stmt_counter(&return_stmt.span);
            return_stmt.visit_mut_children_with(self);
            self.nodes.pop();
        }

        // VariableDeclaration: entries(), // ignore processing only
        #[instrument(skip_all, fields(node = %self.print_node()))]
        fn visit_mut_var_decl(&mut self, var_decl: &mut VarDecl) {
            let (old, _ignore_current) = self.on_enter(var_decl);
            //noop?
            var_decl.visit_mut_children_with(self);
            self.on_exit(old);
        }

        // ClassDeclaration: entries(parenthesizedExpressionProp('superClass')),
        #[instrument(skip_all, fields(node = %self.print_node()))]
        fn visit_mut_class_decl(&mut self, class_decl: &mut ClassDecl) {
            let (old, ignore_current) = self.on_enter(class_decl);
            match ignore_current {
                Some(crate::utils::hint_comments::IgnoreScope::Next) => {}
                _ => {
                    //self.mark_prepend_stmt_counter(&class_decl.class.span);
                    class_decl.visit_mut_children_with(self);
                }
            }

            self.on_exit(old);
        }

        // ClassProperty: entries(coverClassPropDeclarator),
        #[instrument(skip_all, fields(node = %self.print_node()))]
        fn visit_mut_class_prop(&mut self, class_prop: &mut ClassProp) {
            let (old, ignore_current) = self.on_enter(class_prop);
            match ignore_current {
                Some(crate::utils::hint_comments::IgnoreScope::Next) => {}
                _ => {
                    if let Some(value) = &mut class_prop.value {
                        self.cover_statement(&mut *value);
                    }
                }
            }
            self.on_exit(old);
        }

        // ClassPrivateProperty: entries(coverClassPropDeclarator),
        #[instrument(skip_all, fields(node = %self.print_node()))]
        fn visit_mut_private_prop(&mut self, private_prop: &mut PrivateProp) {
            // TODO: this is same as visit_mut_class_prop
            let (old, ignore_current) = self.on_enter(private_prop);
            match ignore_current {
                Some(crate::utils::hint_comments::IgnoreScope::Next) => {}
                _ => {
                    if let Some(value) = &mut private_prop.value {
                        self.cover_statement(&mut *value);
                    }
                }
            }
            self.on_exit(old);
        }

        // ClassMethod: entries(coverFunction),
        #[instrument(skip_all, fields(node = %self.print_node()))]
        fn visit_mut_class_method(&mut self, class_method: &mut ClassMethod) {
            let (old, ignore_current) = self.on_enter(class_method);
            match ignore_current {
                Some(crate::utils::hint_comments::IgnoreScope::Next) => {}
                _ => {
                    // TODO: this does not cover all of PropName enum yet
                    if let PropName::Ident(ident) = &class_method.key {
                        self.visit_mut_fn(&Some(&ident), &mut class_method.function);
                        class_method.visit_mut_children_with(self);
                    }
                }
            }
            self.on_exit(old);
        }

        // VariableDeclarator: entries(coverVariableDeclarator),
        #[instrument(skip_all, fields(node = %self.print_node()))]
        fn visit_mut_var_declarator(&mut self, declarator: &mut VarDeclarator) {
            let (old, ignore_current) = self.on_enter(declarator);

            match ignore_current {
                Some(crate::utils::hint_comments::IgnoreScope::Next) => {}
                _ => {
                    if let Some(init) = &mut declarator.init {
                        let init = &mut **init;
                        self.cover_statement(init);
                    }

                    declarator.visit_mut_children_with(self);
                }
            }

            self.on_exit(old);
        }

        // ForStatement: entries(blockProp('body'), coverStatement),
        #[instrument(skip_all, fields(node = %self.print_node()))]
        fn visit_mut_for_stmt(&mut self, for_stmt: &mut ForStmt) {
            crate::visit_mut_for_like!(self, for_stmt);
        }

        // ForInStatement: entries(blockProp('body'), coverStatement),
        #[instrument(skip_all, fields(node = %self.print_node()))]
        fn visit_mut_for_in_stmt(&mut self, for_in_stmt: &mut ForInStmt) {
            crate::visit_mut_for_like!(self, for_in_stmt);
        }

        // ForOfStatement: entries(blockProp('body'), coverStatement),
        #[instrument(skip_all, fields(node = %self.print_node()))]
        fn visit_mut_for_of_stmt(&mut self, for_of_stmt: &mut ForOfStmt) {
            crate::visit_mut_for_like!(self, for_of_stmt);
        }

        //LabeledStatement: entries(coverStatement),
        #[instrument(skip_all, fields(node = %self.print_node()))]
        fn visit_mut_labeled_stmt(&mut self, labeled_stmt: &mut LabeledStmt) {
            let (old, ignore_current) = self.on_enter(labeled_stmt);

            match ignore_current {
                Some(crate::utils::hint_comments::IgnoreScope::Next) => {}
                _ => {
                    // cover_statement's is_stmt prepend logic for individual child stmt visitor
                    self.mark_prepend_stmt_counter(&labeled_stmt.span);
                }
            }

            labeled_stmt.visit_mut_children_with(self);

            self.on_exit(old);
        }

        // ContinueStatement: entries(coverStatement),
        #[instrument(skip_all, fields(node = %self.print_node()))]
        fn visit_mut_continue_stmt(&mut self, continue_stmt: &mut ContinueStmt) {
            let (old, ignore_current) = self.on_enter(continue_stmt);

            match ignore_current {
                Some(crate::utils::hint_comments::IgnoreScope::Next) => {}
                _ => {
                    // cover_statement's is_stmt prepend logic for individual child stmt visitor
                    self.mark_prepend_stmt_counter(&continue_stmt.span);
                }
            }

            continue_stmt.visit_mut_children_with(self);
            self.on_exit(old);
        }

        // IfStatement: entries(blockProp('consequent'), blockProp('alternate'), coverStatement, coverIfBranches)
        #[instrument(skip_all, fields(node = %self.print_node()))]
        fn visit_mut_if_stmt(&mut self, if_stmt: &mut IfStmt) {
            let (old, ignore_current) = self.on_enter(if_stmt);

            match ignore_current {
                Some(crate::utils::hint_comments::IgnoreScope::Next) => {
                    self.on_exit(old);
                }
                _ => {
                    // cover_statement's is_stmt prepend logic for individual child stmt visitor
                    self.mark_prepend_stmt_counter(&if_stmt.span);

                    let range = get_range_from_span(self.source_map, &if_stmt.span);
                    let branch =
                        self.cov
                            .new_branch(istanbul_oxi_instrument::BranchType::If, &range, false);

                    let mut wrap_with_counter = |stmt: &mut Box<Stmt>| {
                        let mut stmt_body = *stmt.take();

                        // create a branch path counter
                        let idx = self.cov.add_branch_path(branch, &range);
                        let expr = create_increase_expression_expr(
                            &IDENT_B,
                            branch,
                            &self.cov_fn_ident,
                            Some(idx),
                        );

                        let expr = Stmt::Expr(ExprStmt {
                            span: DUMMY_SP,
                            expr: Box::new(expr),
                        });

                        let body = if let Stmt::Block(mut block_stmt) = stmt_body {
                            // if cons / alt is already blockstmt, insert stmt counter for each
                            self.insert_stmts_counter(&mut block_stmt.stmts);

                            let mut new_stmts = vec![expr];
                            new_stmts.extend(block_stmt.stmts.drain(..));

                            block_stmt.stmts = new_stmts;
                            block_stmt
                        } else {
                            let mut stmts = vec![expr];
                            let mut visitor = StmtVisitor::new(
                                self.source_map,
                                self.comments,
                                &mut self.cov,
                                &self.instrument_options,
                                &self.nodes,
                                ignore_current,
                            );
                            stmt_body.visit_mut_with(&mut visitor);
                            stmts.extend(visitor.before.drain(..));

                            stmts.push(stmt_body);

                            BlockStmt {
                                span: DUMMY_SP,
                                stmts,
                            }
                        };

                        *stmt = Box::new(Stmt::Block(body));
                    };

                    if ignore_current == Some(crate::utils::hint_comments::IgnoreScope::If) {
                        //setAttr(if_stmt.cons, 'skip-all', true);
                    } else {
                        wrap_with_counter(&mut if_stmt.cons);
                    }

                    if ignore_current == Some(crate::utils::hint_comments::IgnoreScope::Else) {
                        //setAttr(if_stmt.alt, 'skip-all', true);
                    } else {
                        if let Some(alt) = &mut if_stmt.alt {
                            wrap_with_counter(alt);
                        } else {
                            // alt can be none (`if some {}` without else).
                            // Inject empty blockstmt then insert branch counters
                            let mut alt = Box::new(Stmt::Block(BlockStmt::dummy()));
                            wrap_with_counter(&mut alt);
                            if_stmt.alt = Some(alt);

                            // We visit individual cons / alt depends on its state, need to run visitor for the `test` as well
                            if_stmt.test.visit_mut_with(self);

                            self.on_exit(old);
                            return;
                        }
                    }

                    // We visit individual cons / alt depends on its state, need to run visitor for the `test` as well
                    if_stmt.test.visit_mut_with(self);

                    self.on_exit(old);
                }
            };
        }

        // LogicalExpression: entries(coverLogicalExpression)
        #[instrument(skip_all, fields(node = %self.print_node()))]
        fn visit_mut_bin_expr(&mut self, bin_expr: &mut BinExpr) {
            match &bin_expr.op {
                BinaryOp::LogicalOr | BinaryOp::LogicalAnd | BinaryOp::NullishCoalescing => {
                    self.nodes.push(Node::LogicalExpr);

                    // escape if there's ignore comments
                    let hint = crate::utils::hint_comments::lookup_hint_comments(
                        &self.comments,
                        Some(bin_expr.span).as_ref(),
                    );
                    if hint.as_deref() == Some("next") {
                        bin_expr.visit_mut_children_with(self);
                        self.nodes.pop();
                        return;
                    }

                    // Create a new branch. This id should be reused for any inner logical expr.
                    let range = get_range_from_span(self.source_map, &bin_expr.span);
                    let branch = self.cov.new_branch(
                        istanbul_oxi_instrument::BranchType::BinaryExpr,
                        &range,
                        self.instrument_options.report_logic,
                    );

                    // Iterate over each expr, wrap it with branch counter.
                    self.wrap_bin_expr_with_branch_counter(branch, &mut *bin_expr.left);
                    self.wrap_bin_expr_with_branch_counter(branch, &mut *bin_expr.right);
                }
                _ => {
                    // iterate as normal for non loigical expr
                    self.nodes.push(Node::BinExpr);
                    bin_expr.visit_mut_children_with(self);
                }
            }
            self.nodes.pop();
        }
    };
}

/// Create a fn inserts stmt counter for each stmt
#[macro_export]
macro_rules! insert_stmt_counter {
    () => {
        /// Visit individual statements with stmt_visitor and update.
        #[instrument(skip_all, fields(node = %self.print_node()))]
        fn insert_stmts_counter(&mut self, stmts: &mut Vec<Stmt>) {
            let mut new_stmts = vec![];

            for mut stmt in stmts.drain(..) {
                if !self.is_injected_counter_stmt(&stmt) {
                    let span = crate::utils::lookup_range::get_stmt_span(&stmt);

                    let should_ignore =
                        crate::utils::hint_comments::should_ignore(&self.comments, span);

                    let mut visitor = StmtVisitor::new(
                        self.source_map,
                        self.comments,
                        &mut self.cov,
                        &self.instrument_options,
                        &self.nodes,
                        should_ignore,
                    );
                    stmt.visit_mut_children_with(&mut visitor);

                    if visitor.before.len() == 0 {
                        //println!("{:#?}", stmt);
                    }

                    new_stmts.extend(visitor.before.drain(..));

                    /*
                    if let Some(span) = span {
                        // if given stmt is not a plain stmt and omit to insert stmt counter,
                        // visit it to collect inner stmt counters


                    } else {
                        //stmt.visit_mut_children_with(self);
                        //new_stmts.extend(visitor.before.drain(..));
                    } */
                }

                new_stmts.push(stmt);
            }

            *stmts = new_stmts;
        }
    };
}
