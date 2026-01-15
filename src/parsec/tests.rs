use crate::{new_grammar, parsec::parser::Parser};

#[test]
fn test_parser() {
    let grammar = new_grammar!(
        expr where
        expr   -> r!(expr) + t("+") + r!(mul) | r!(mul)
        mul    -> t("num")
    );

    println!("Grammar:\n{}", grammar);
    let mut parser = Parser::new("num+num+num+num", &grammar);
    let green = parser.parse_text().green;
    println!("{}", parser.alloc.display(green));
}
