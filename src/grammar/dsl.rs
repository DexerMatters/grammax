use crate::{
    grammar::Grammar,
    new_grammar,
    parsec::words::{ALPHANUMS, EndOfInput, IDENT, Matcher, NUMS, STRING},
};

thread_local! {
    pub static GRAMMAX_DSL_GRAMMAR: &'static Grammar = new_grammar! {
        table where
        table -> sep(r!(rule), t('\n')) + tt(EndOfInput)
        rule -> field("name", tt(IDENT)) + tt("->") + field("definition", r!(expr))
        expr -> r!(alternative) | r!(sequence) | r!(drop) | r!(many) | r!(some) | r!(terminal) | r!(reference)
        alternative -> r!(expr).drop(1) + tt("|") + r!(expr)
        sequence -> r!(expr).drop(2) + t(" ") + r!(expr).drop(1)
        drop -> r!(expr).drop(3) + t("/") + tt(NUMS)
        many -> r!(expr).drop(3) + t("*")
        some -> r!(expr).drop(4) + t("+")
        reference -> tt(IDENT)
        terminal -> (tt("(") + r!(expr) + tt(")")) | r!(literal) | r!(token)
        token -> tt("IDENT") | tt("STRING") | tt("NUMBER") | tt("ALPHANUMS") | tt("ALPHABETS") | tt("EOF")
        literal -> tt('"') + tt(STRING) + tt('"')
};
}

#[cfg(test)]
mod tests {
    use crate::{
        interface::webui::WebPreviewInterface,
        runtime::{CompilerBuilder, ComposedCompiler, ParserPass, RuntimeService},
        scheme::layers::RedGreenTreeIR,
    };

    use super::*;

    #[test]
    fn test_dsl_grammar() {
        let grammar = GRAMMAX_DSL_GRAMMAR.with(|g| *g);
        let (pass, _) = CompilerBuilder::new()
            .then_pass(ParserPass::new(grammar))
            .then_layer(RedGreenTreeIR::default())
            .tap();

        let runtime = RuntimeService::<WebPreviewInterface>::new(grammar, move |evt_tx| {
            ComposedCompiler::from_pass_with_events(pass, evt_tx)
        });
        runtime.run().expect("runtime failed");
    }
}
