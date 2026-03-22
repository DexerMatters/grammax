use tower_lsp::LanguageServer;

use crate::interface::Interface;
use crate::runtime::TypedTree;

pub trait LspInterface<Tree: TypedTree>: LanguageServer + Interface<Tree> {}
