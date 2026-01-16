use crate::{new_grammar, parsec::parser::Parser};

#[test]
fn test_parser() {
    let grammar = new_grammar!(
        expr where
        expr   -> field("rhs", r!(expr)) + t("+") + field("lhs", r!(num)) | r!(num)
        num    -> t("x")
    );

    println!("Grammar:\n{}", grammar);
    let mut parser = Parser::new("x+x+x*x", &grammar);
    let green = parser.parse_text().green;
    println!("{}", parser.alloc.display(green));
}
