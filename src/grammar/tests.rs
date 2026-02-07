use crate::grammar::dsl::{GrammarNode, field, many, opt, sep, sep1, t};
use crate::grammar::ir::{Associativity, OperatorKind, Symbol};
use crate::grammar::norm::RuleTable;
use crate::r;

#[test]
fn detects_infix_precedence_and_associativity() {
    fn primary() -> GrammarNode {
        t("num")
    }

    fn expr() -> GrammarNode {
        r!(expr) + t("+") + r!(expr) | r!(expr) + t("*") + r!(expr) | r!(primary)
    }
    let table = RuleTable::normalize(expr(), "expr");
    println!("Table: {:#?}", table);
    let expr_ix = table
        .rules
        .iter()
        .position(|r| r.name == "expr")
        .expect("expr rule missing");
    assert!(table.rules[expr_ix].is_expression);

    let ops = table
        .operator_tables
        .get(&expr_ix)
        .expect("operator table missing for expr");

    let plus = ops
        .iter()
        .find(|o| o.token.display() == "\"+\"")
        .expect("missing + operator");
    assert_eq!(plus.precedence, 0);
    assert_eq!(plus.associativity, Associativity::Left);
    assert_eq!(plus.kind, OperatorKind::Infix);

    let star = ops
        .iter()
        .find(|o| o.token.display() == "\"*\"")
        .expect("missing * operator");
    assert_eq!(star.precedence, 1);
    assert_eq!(star.associativity, Associativity::Left);
    assert_eq!(star.kind, OperatorKind::Infix);
}

#[test]
fn detects_right_associative_infix() {
    fn atom() -> GrammarNode {
        t("num")
    }

    fn power() -> GrammarNode {
        r!(atom)
    }

    fn expr() -> GrammarNode {
        r!(expr) + t("^") + r!(power) | r!(power)
    }

    let table = RuleTable::normalize(expr(), "expr");
    let expr_ix = table
        .rules
        .iter()
        .position(|r| r.name == "expr")
        .expect("expr rule missing");

    let ops = table
        .operator_tables
        .get(&expr_ix)
        .expect("operator table missing for expr");

    let caret = ops
        .iter()
        .find(|o| o.token.display() == "\"^\"")
        .expect("missing ^ operator");
    assert_eq!(caret.associativity, Associativity::Right);
    assert_eq!(caret.kind, OperatorKind::Infix);
}

#[test]
fn detects_prefix_and_postfix() {
    fn primary() -> GrammarNode {
        t("num")
    }

    fn expr() -> GrammarNode {
        t("-") + r!(expr) | r!(expr) + t("++") | r!(primary)
    }

    let table = RuleTable::normalize(expr(), "expr");
    let expr_ix = table
        .rules
        .iter()
        .position(|r| r.name == "expr")
        .expect("expr rule missing");

    let ops = table
        .operator_tables
        .get(&expr_ix)
        .expect("operator table missing for expr");

    println!("Operators: {:#?}", ops);

    let prefix = ops
        .iter()
        .find(|o| o.token.display() == "\"-\"")
        .expect("missing - operator");
    assert_eq!(prefix.associativity, Associativity::Right);
    assert_eq!(prefix.kind, OperatorKind::Prefix);

    let postfix = ops
        .iter()
        .find(|o| o.token.display() == "\"++\"")
        .expect("missing ++ operator");
    assert_eq!(postfix.associativity, Associativity::Left);
    assert_eq!(postfix.kind, OperatorKind::Postfix);
}

#[test]
fn desugars_repetition_and_separator_rules() {
    fn list() -> GrammarNode {
        sep1(t("a"), t(","))
    }

    let table = RuleTable::normalize(list(), "list");

    assert!(table.rules.iter().any(|r| r.name.starts_with("@sep_tail_")));
}

#[test]
fn deduplicates_terminals_by_display() {
    fn rule() -> GrammarNode {
        t("a") + t("a") + t("a")
    }

    let table = RuleTable::normalize(rule(), "rule");

    assert_eq!(table.terminal_map.len(), 1);
    assert_eq!(table.terminals.len(), 1);
}

#[test]
fn creates_list_rule_for_many() {
    fn rule() -> GrammarNode {
        many(t("x"))
    }

    let table = RuleTable::normalize(rule(), "rule");

    assert!(table.rules.iter().any(|r| r.name.starts_with("@list_")));
}

#[test]
fn list_rule_has_epsilon_and_recursive_production() {
    fn rule() -> GrammarNode {
        many(t("x"))
    }

    let table = RuleTable::normalize(rule(), "rule");
    let list_ix = table
        .rules
        .iter()
        .position(|r| r.name.starts_with("@list_"))
        .expect("list rule missing");

    let list_productions: Vec<_> = table
        .productions
        .iter()
        .filter(|p| p.lhs == list_ix)
        .collect();

    assert!(list_productions.iter().any(|p| p.rhs.is_empty()));
    assert!(list_productions.iter().any(|p| {
        p.rhs.len() == 2 && matches!(p.rhs[1], Symbol::NonTerminal(ix) if ix == list_ix)
    }));
}

#[test]
fn sep_min0_produces_empty_and_tail_rules() {
    fn rule() -> GrammarNode {
        sep(t("a"), t(","))
    }

    let table = RuleTable::normalize(rule(), "rule");
    let rule_ix = table
        .rules
        .iter()
        .position(|r| r.name == "rule")
        .expect("rule missing");
    let tail_ix = table
        .rules
        .iter()
        .position(|r| r.name.starts_with("@sep_tail_"))
        .expect("sep tail rule missing");

    let rule_prods: Vec<_> = table
        .productions
        .iter()
        .filter(|p| p.lhs == rule_ix)
        .collect();
    assert!(rule_prods.iter().any(|p| p.rhs.is_empty()));

    let tail_prods: Vec<_> = table
        .productions
        .iter()
        .filter(|p| p.lhs == tail_ix)
        .collect();
    assert!(tail_prods.iter().any(|p| p.rhs.is_empty()));
    assert!(tail_prods.iter().any(|p| {
        p.rhs.len() == 3 && matches!(p.rhs[2], Symbol::NonTerminal(ix) if ix == tail_ix)
    }));
}

#[test]
fn optional_produces_empty_production() {
    fn rule() -> GrammarNode {
        opt(t("a"))
    }

    let table = RuleTable::normalize(rule(), "rule");
    let rule_ix = table
        .rules
        .iter()
        .position(|r| r.name == "rule")
        .expect("rule missing");

    let rule_prods: Vec<_> = table
        .productions
        .iter()
        .filter(|p| p.lhs == rule_ix)
        .collect();
    assert!(rule_prods.iter().any(|p| p.rhs.is_empty()));
    assert!(rule_prods.iter().any(|p| p.rhs.len() == 1));
}

#[test]
fn field_positions_preserve_sequence_order() {
    fn rule() -> GrammarNode {
        field("lhs", t("a")) + t("+") + field("rhs", t("b"))
    }

    let table = RuleTable::normalize(rule(), "rule");
    let rule_ix = table
        .rules
        .iter()
        .position(|r| r.name == "rule")
        .expect("rule missing");

    let prod = table
        .productions
        .iter()
        .find(|p| p.lhs == rule_ix && p.rhs.len() == 3)
        .expect("production missing");

    assert!(prod.field_positions.contains(&(0, "lhs")));
    assert!(prod.field_positions.contains(&(2, "rhs")));
}

#[test]
fn non_expression_rules_are_not_marked() {
    fn atom() -> GrammarNode {
        t("b")
    }

    fn rule() -> GrammarNode {
        t("a") | r!(atom)
    }

    let table = RuleTable::normalize(rule(), "rule");
    let rule_ix = table
        .rules
        .iter()
        .position(|r| r.name == "rule")
        .expect("rule missing");

    assert!(!table.rules[rule_ix].is_expression);
    assert!(!table.operator_tables.contains_key(&rule_ix));
}
