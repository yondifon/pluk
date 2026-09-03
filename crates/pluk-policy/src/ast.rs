//! Maps a parsed `sqlparser` statement to a policy category.

use sqlparser::ast::Statement;

use crate::category::StatementCategory;

/// Category for one parsed statement. `None` means "recognized syntax but no
/// safe category" — the evaluator denies it (fail-closed), exactly like the
/// TS engine's unmapped AST types (`lock`, `comment`, `execute`, `load_data`,
/// `attach`, `deallocate`, …).
pub fn statement_category(stmt: &Statement) -> Option<StatementCategory> {
    use StatementCategory as C;
    match stmt {
        Statement::Query(_) => Some(C::Select),
        Statement::Insert(insert) => {
            if insert.replace_into {
                Some(C::Merge) // MySQL REPLACE INTO
            } else {
                Some(C::Insert)
            }
        }
        Statement::Update(_) => Some(C::Update),
        Statement::Delete(_) => Some(C::Delete),
        Statement::Merge(_) => Some(C::Merge),
        // TS keyword fallback maps COPY to insert; the dangerous-construct scan
        // separately catches COPY ... PROGRAM.
        Statement::Copy { .. } => Some(C::Insert),

        Statement::CreateView(_)
        | Statement::CreateTable(_)
        | Statement::CreateVirtualTable { .. }
        | Statement::CreateIndex(_)
        | Statement::CreateRole(_)
        | Statement::CreateSecret { .. }
        | Statement::CreateServer(_)
        | Statement::CreatePolicy(_)
        | Statement::CreateConnector(_)
        | Statement::CreateOperator(_)
        | Statement::CreateOperatorFamily(_)
        | Statement::CreateOperatorClass(_)
        | Statement::CreateExtension(_)
        | Statement::CreateCollation(_)
        | Statement::CreateSchema { .. }
        | Statement::CreateDatabase { .. }
        | Statement::CreateFunction(_)
        | Statement::CreateTrigger(_)
        | Statement::CreateProcedure { .. }
        | Statement::CreateMacro { .. }
        | Statement::CreateStage { .. }
        | Statement::CreateSequence { .. }
        | Statement::CreateDomain(_)
        | Statement::CreateType { .. }
        | Statement::CreateUser(_) => Some(C::Create),

        Statement::AlterTable(_)
        | Statement::AlterSchema(_)
        | Statement::AlterIndex { .. }
        | Statement::AlterView { .. }
        | Statement::AlterFunction(_)
        | Statement::AlterType(_)
        | Statement::AlterCollation(_)
        | Statement::AlterOperator(_)
        | Statement::AlterOperatorFamily(_)
        | Statement::AlterOperatorClass(_)
        | Statement::AlterRole { .. }
        | Statement::AlterPolicy(_)
        | Statement::AlterConnector { .. }
        | Statement::AlterSession { .. }
        | Statement::AlterUser(_) => Some(C::Alter),

        Statement::Drop { .. }
        | Statement::DropFunction(_)
        | Statement::DropDomain(_)
        | Statement::DropProcedure { .. }
        | Statement::DropSecret { .. }
        | Statement::DropPolicy(_)
        | Statement::DropConnector { .. }
        | Statement::DropExtension(_)
        | Statement::DropOperator(_)
        | Statement::DropOperatorFamily(_)
        | Statement::DropOperatorClass(_)
        | Statement::DropTrigger(_) => Some(C::Drop),

        Statement::Truncate(_) => Some(C::Truncate),
        Statement::RenameTable(_) => Some(C::Rename),

        Statement::StartTransaction { .. }
        | Statement::Commit { .. }
        | Statement::Rollback { .. }
        | Statement::Savepoint { .. }
        | Statement::ReleaseSavepoint { .. } => Some(C::Transaction),

        Statement::ShowFunctions { .. }
        | Statement::ShowVariable { .. }
        | Statement::ShowStatus { .. }
        | Statement::ShowVariables { .. }
        | Statement::ShowCreate { .. }
        | Statement::ShowColumns { .. }
        | Statement::ShowCatalogs { .. }
        | Statement::ShowDatabases { .. }
        | Statement::ShowProcessList { .. }
        | Statement::ShowSchemas { .. }
        | Statement::ShowCharset(_)
        | Statement::ShowObjects(_)
        | Statement::ShowTables { .. }
        | Statement::ShowViews { .. }
        | Statement::ShowCollation { .. } => Some(C::Inspect),
        Statement::Explain { .. } | Statement::ExplainTable { .. } => Some(C::Inspect),
        Statement::Pragma { .. } => Some(C::Inspect),

        Statement::Set(_) | Statement::Reset(_) | Statement::Use(_) => Some(C::Session),

        Statement::Call(_) => Some(C::Procedure),

        Statement::Vacuum(_) => Some(C::Maintenance),
        Statement::Analyze(_) => Some(C::Maintenance),
        Statement::OptimizeTable { .. } => Some(C::Maintenance),

        Statement::Grant(_) | Statement::Revoke(_) => Some(C::Grant),

        // Recognized syntax with no safe category: cursors, prepared statements,
        // locking, server filesystem, replication, T-SQL control flow, staging,
        // and everything else unmapped. Denied.
        _ => None,
    }
}

/// True for an `UPDATE`/`DELETE` parsed without a `WHERE` clause.
pub fn update_or_delete_without_where(stmt: &Statement) -> bool {
    match stmt {
        Statement::Update(update) => update.selection.is_none(),
        Statement::Delete(delete) => delete.selection.is_none(),
        _ => false,
    }
}
