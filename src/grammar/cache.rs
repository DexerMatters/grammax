use std::env;
use std::fmt;
use std::fs;
use std::hash::Hash;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::grammar::Grammar;
use crate::grammar::analysis::GrammarStateAnalysis;
use crate::grammar::bridge;
use crate::grammar::ir::{NormalizedNode, Production, RuleInfo, Symbol};
use crate::grammar::norm::RuleTable;
use crate::parsec::words::{
    self, CharMatcherWithEscapes, EndOfInput, MatcherRef, OwnedLiteral, RegexMatcher,
    TokenizedMatcher, token,
};

pub(crate) mod serde_fxhashmap {
    use std::hash::Hash;

    use rustc_hash::FxHashMap;
    use serde::de::DeserializeOwned;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<K, V, S>(map: &FxHashMap<K, V>, serializer: S) -> Result<S::Ok, S::Error>
    where
        K: Serialize + Eq + Hash,
        V: Serialize,
        S: Serializer,
    {
        let entries: Vec<(&K, &V)> = map.iter().collect();
        entries.serialize(serializer)
    }

    pub fn deserialize<'de, K, V, D>(deserializer: D) -> Result<FxHashMap<K, V>, D::Error>
    where
        K: DeserializeOwned + Eq + Hash,
        V: DeserializeOwned,
        D: Deserializer<'de>,
    {
        let entries: Vec<(K, V)> = Vec::deserialize(deserializer)?;
        Ok(entries.into_iter().collect())
    }
}

const CACHE_FORMAT_VERSION: u32 = 8;

static CACHE_DIR_OVERRIDE: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize)]
enum CachedTerminal {
    Literal(String),
    TokenLiteral(String),
    Token(Box<CachedTerminal>),
    Char(char),
    Named(String),
    Regex(String),
    CharEscapes,
    Eof,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum CachedNode {
    Terminal(usize),
    Alternative(Vec<CachedNode>),
    Sequence(Vec<CachedNode>),
    Reference(usize),
    Field(String, Box<CachedNode>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedRuleInfo {
    name: String,
    description: String,
    node: CachedNode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedProduction {
    lhs: usize,
    rhs: Vec<Symbol>,
    field_positions: Vec<(usize, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedRuleTable {
    rules: Vec<CachedRuleInfo>,
    productions: Vec<CachedProduction>,
    terminals: Vec<CachedTerminal>,
    #[serde(with = "crate::grammar::cache::serde_fxhashmap")]
    terminal_map: FxHashMap<String, usize>,
    start_rule: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GrammarCacheFile {
    format_version: u32,
    crate_version: String,
    os: String,
    arch: String,
    cache_key: u64,
    table: CachedRuleTable,
    analysis: GrammarStateAnalysis,
    #[serde(with = "crate::grammar::cache::serde_fxhashmap")]
    rule_analyses: FxHashMap<usize, GrammarStateAnalysis>,
    /// Bridge specs derived from productions (Nilsson-Nyman 2009 §3).
    bridge_specs: Vec<bridge::BridgeSpec>,
    /// Delimiter terminals used as stop points by scope recovery.
    recovery_delimiters: Vec<usize>,
    /// Terminals with matching delimiters on both ends (e.g., strings, comments).
    /// Stored as terminal indices that have the same delimiter opening and closing.
    bracketed_terminals: Vec<usize>,
}

pub(crate) fn load(cache_key: u64) -> Option<Grammar> {
    let path = cache_file_path(cache_key);
    let bytes = fs::read(path).ok()?;
    let cache: GrammarCacheFile = bincode::deserialize(&bytes).ok()?;

    if cache.format_version != CACHE_FORMAT_VERSION {
        return None;
    }
    if cache.crate_version != env!("CARGO_PKG_VERSION") {
        return None;
    }
    if cache.os != env::consts::OS {
        return None;
    }
    if cache.arch != env::consts::ARCH {
        return None;
    }
    if cache.cache_key != cache_key {
        return None;
    }

    let table = decode_rule_table(cache.table).ok()?;

    Some(Grammar {
        table,
        analysis: Arc::new(cache.analysis),
        rule_analyses: cache
            .rule_analyses
            .into_iter()
            .map(|(rule_ix, state)| (rule_ix, Arc::new(state)))
            .collect(),
        bridge_specs: cache.bridge_specs,
        recovery_delimiters: cache.recovery_delimiters,
        bracketed_terminals: cache.bracketed_terminals,
    })
}

pub(crate) fn store(cache_key: u64, grammar: &Grammar) -> io::Result<()> {
    let dir = cache_dir();
    fs::create_dir_all(&dir)?;

    let table = encode_rule_table(&grammar.table)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;

    let cache = GrammarCacheFile {
        format_version: CACHE_FORMAT_VERSION,
        crate_version: env!("CARGO_PKG_VERSION").to_string(),
        os: env::consts::OS.to_string(),
        arch: env::consts::ARCH.to_string(),
        cache_key,
        table,
        analysis: (*grammar.analysis).clone(),
        rule_analyses: grammar
            .rule_analyses
            .iter()
            .map(|(rule_ix, state)| (*rule_ix, (**state).clone()))
            .collect(),
        bridge_specs: grammar.bridge_specs.clone(),
        recovery_delimiters: grammar.recovery_delimiters.clone(),
        bracketed_terminals: grammar.bracketed_terminals.clone(),
    };

    let bytes = bincode::serialize(&cache)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))?;

    fs::write(cache_file_path(cache_key), bytes)
}

fn encode_rule_table(table: &RuleTable) -> Result<CachedRuleTable, String> {
    let mut matcher_idx: FxHashMap<usize, usize> = FxHashMap::default();
    let mut terminals = Vec::with_capacity(table.terminals.len());

    for (idx, matcher) in table.terminals.iter().enumerate() {
        let spec = terminal_to_cached(matcher)
            .ok_or_else(|| format!("unsupported matcher for cache: {}", matcher.display()))?;
        terminals.push(spec);
        matcher_idx.insert(Arc::as_ptr(matcher) as *const () as usize, idx);
    }

    let rules = table
        .rules
        .iter()
        .map(|rule| {
            Ok(CachedRuleInfo {
                name: rule.name.to_string(),
                description: rule.description.to_string(),
                node: encode_node(&rule.node, &matcher_idx, &terminals)?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    let productions = table
        .productions
        .iter()
        .map(|p| CachedProduction {
            lhs: p.lhs,
            rhs: p.rhs.clone(),
            field_positions: p
                .field_positions
                .iter()
                .map(|(i, name)| (*i, (*name).to_string()))
                .collect(),
        })
        .collect();

    Ok(CachedRuleTable {
        rules,
        productions,
        terminals,
        terminal_map: table.terminal_map.clone(),
        start_rule: table.start_rule,
    })
}

fn decode_rule_table(table: CachedRuleTable) -> Result<RuleTable, String> {
    let terminals: Vec<MatcherRef> = table
        .terminals
        .iter()
        .map(cached_to_terminal)
        .collect::<Result<Vec<_>, _>>()?;

    let rules = table
        .rules
        .into_iter()
        .map(|rule| {
            Ok(RuleInfo {
                name: leak_str(rule.name),
                description: leak_str(rule.description),
                node: decode_node(rule.node, &terminals)?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    let productions = table
        .productions
        .into_iter()
        .map(|p| Production {
            lhs: p.lhs,
            rhs: p.rhs,
            field_positions: p
                .field_positions
                .into_iter()
                .map(|(i, name)| (i, leak_str(name)))
                .collect(),
        })
        .collect();

    Ok(RuleTable {
        rules,
        productions,
        terminals,
        terminal_map: table.terminal_map,
        start_rule: table.start_rule,
    })
}

fn encode_node(
    node: &NormalizedNode,
    matcher_idx: &FxHashMap<usize, usize>,
    terminals: &[CachedTerminal],
) -> Result<CachedNode, String> {
    match node {
        NormalizedNode::Terminal(matcher) => {
            let key = Arc::as_ptr(matcher) as *const () as usize;
            if let Some(idx) = matcher_idx.get(&key) {
                return Ok(CachedNode::Terminal(*idx));
            }

            let spec = terminal_to_cached(matcher).ok_or_else(|| {
                format!("unsupported node matcher for cache: {}", matcher.display())
            })?;
            let idx = terminals
                .iter()
                .position(|s| s == &spec)
                .ok_or_else(|| "node terminal not found in terminal table".to_string())?;
            Ok(CachedNode::Terminal(idx))
        }
        NormalizedNode::Alternative(nodes) => Ok(CachedNode::Alternative(
            nodes
                .iter()
                .map(|n| encode_node(n, matcher_idx, terminals))
                .collect::<Result<Vec<_>, _>>()?,
        )),
        NormalizedNode::Sequence(nodes) => Ok(CachedNode::Sequence(
            nodes
                .iter()
                .map(|n| encode_node(n, matcher_idx, terminals))
                .collect::<Result<Vec<_>, _>>()?,
        )),
        NormalizedNode::Reference(ix) => Ok(CachedNode::Reference(*ix)),
        NormalizedNode::Field(name, inner) => Ok(CachedNode::Field(
            (*name).to_string(),
            Box::new(encode_node(inner, matcher_idx, terminals)?),
        )),
    }
}

fn decode_node(node: CachedNode, terminals: &[MatcherRef]) -> Result<NormalizedNode, String> {
    match node {
        CachedNode::Terminal(idx) => terminals
            .get(idx)
            .cloned()
            .map(NormalizedNode::Terminal)
            .ok_or_else(|| format!("invalid terminal index in cache: {}", idx)),
        CachedNode::Alternative(nodes) => Ok(NormalizedNode::Alternative(
            nodes
                .into_iter()
                .map(|n| decode_node(n, terminals))
                .collect::<Result<Vec<_>, _>>()?,
        )),
        CachedNode::Sequence(nodes) => Ok(NormalizedNode::Sequence(
            nodes
                .into_iter()
                .map(|n| decode_node(n, terminals))
                .collect::<Result<Vec<_>, _>>()?,
        )),
        CachedNode::Reference(ix) => Ok(NormalizedNode::Reference(ix)),
        CachedNode::Field(name, inner) => Ok(NormalizedNode::Field(
            leak_str(name),
            Box::new(decode_node(*inner, terminals)?),
        )),
    }
}

fn terminal_to_cached(matcher: &MatcherRef) -> Option<CachedTerminal> {
    let display = matcher.display();

    // Handle regex patterns
    if let Some(pattern) = display
        .strip_prefix("regex(")
        .and_then(|s| s.strip_suffix(")"))
    {
        return Some(CachedTerminal::Regex(pattern.to_string()));
    }

    // Handle CharMatcherWithEscapes
    if display == "characters including escapes" {
        return Some(CachedTerminal::CharEscapes);
    }

    // Handle "whitespace or newline* ..." patterns (from token() function)
    if let Some(rest) = display.strip_prefix("whitespace or newline* ") {
        let inner = cached_terminal_from_display(rest.trim())?;
        return Some(CachedTerminal::Token(Box::new(inner)));
    }

    if let Some(inner_display) = display.strip_prefix("char_predicate* ") {
        let inner = cached_terminal_from_display(inner_display.trim())?;
        return Some(CachedTerminal::Token(Box::new(inner)));
    }

    cached_terminal_from_display(&display).or_else(|| {
        if let Some(preview) = matcher.preview() {
            let quoted = format!("\"{}\"", preview);
            if display == quoted {
                return Some(CachedTerminal::Literal(preview.to_string()));
            }
            if display.contains(&quoted) {
                return Some(CachedTerminal::TokenLiteral(preview.to_string()));
            }
        }
        None
    })
}

fn cached_terminal_from_display(display: &str) -> Option<CachedTerminal> {
    if let Some(pattern) = display
        .strip_prefix("regex(")
        .and_then(|s| s.strip_suffix(")"))
    {
        return Some(CachedTerminal::Regex(pattern.to_string()));
    }

    if display == "EOF" {
        return Some(CachedTerminal::Eof);
    }

    if display == "characters including escapes" {
        return Some(CachedTerminal::CharEscapes);
    }

    if display == "whitespace or newline" {
        // Special handling for the token() function's whitespace pattern
        return Some(CachedTerminal::Named("whitespace_or_newline".to_string()));
    }

    if let Some(ch) = parse_char_display(display) {
        return Some(CachedTerminal::Char(ch));
    }

    if display.starts_with('"') && display.ends_with('"') && display.len() >= 2 {
        return Some(CachedTerminal::Literal(
            display[1..display.len() - 1].to_string(),
        ));
    }

    match display {
        "number" | "identifier" | "alphanum" | "string" | "ident" | "json_string"
        | "whitespaces" | "regexp" => Some(CachedTerminal::Named(display.to_string())),
        _ => None,
    }
}

fn cached_to_terminal(spec: &CachedTerminal) -> Result<MatcherRef, String> {
    match spec {
        CachedTerminal::Literal(s) => Ok(Arc::new(OwnedLiteral(s.clone()))),
        CachedTerminal::TokenLiteral(s) => Ok(Arc::new(token(OwnedLiteral(s.clone())))),
        CachedTerminal::Token(inner) => {
            let inner_matcher = cached_to_terminal(inner)?;
            Ok(Arc::new(TokenizedMatcher {
                inner: inner_matcher,
            }))
        }
        CachedTerminal::Char(c) => Ok(Arc::new(*c)),
        CachedTerminal::Regex(pattern) => {
            let compiled = regex::Regex::new(pattern)
                .map_err(|e| format!("failed to compile regex pattern: {}", e))?;
            Ok(Arc::new(RegexMatcher {
                pattern: compiled,
                is_nullable: OnceLock::new(),
                is_consuming: OnceLock::new(),
            }))
        }
        CachedTerminal::CharEscapes => Ok(Arc::new(CharMatcherWithEscapes)),
        CachedTerminal::Named(name) => match name.as_str() {
            "number" => Ok(Arc::new(words::NUMS)),
            "identifier" => Ok(Arc::new(words::ALPHAS)),
            "alphanum" => Ok(Arc::new(words::ALPHANUMS)),
            "string" => Ok(Arc::new(words::STRING)),
            "ident" => Ok(Arc::new(words::IDENT)),
            "regexp" => Ok(Arc::new(words::NamedMatcher::new(
                "regexp",
                RegexMatcher::new(r#"([^/\\\r\n]|\\.)+"#),
            ))),
            "json_string" => Ok(Arc::new(words::STRING)), // Backward compatibility
            "whitespaces" => Ok(Arc::new(words::WHITESPACES)),
            "whitespace_or_newline" => {
                // This is a special pattern used by token() - return WHITESPACES as best approximation
                // The actual matching is handled by TokenizedMatcher when this is wrapped as Token
                Ok(Arc::new(words::WHITESPACES))
            }
            _ => Err(format!("unsupported named matcher in cache: {}", name)),
        },
        CachedTerminal::Eof => Ok(Arc::new(EndOfInput)),
    }
}

fn parse_char_display(display: &str) -> Option<char> {
    if display.starts_with('\'') && display.ends_with('\'') {
        let mut chars = display[1..display.len() - 1].chars();
        let c = chars.next()?;
        if chars.next().is_none() {
            return Some(c);
        }
    }
    None
}

pub(crate) fn leak_str(value: String) -> &'static str {
    value.leak()
}

pub(crate) fn serialize_grammar_file(grammar: &Grammar) -> Result<Vec<u8>, io::Error> {
    let table = encode_rule_table(&grammar.table)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;

    let file = GrammarCacheFile {
        format_version: CACHE_FORMAT_VERSION,
        crate_version: env!("CARGO_PKG_VERSION").to_string(),
        os: env::consts::OS.to_string(),
        arch: env::consts::ARCH.to_string(),
        cache_key: 0,
        table,
        analysis: (*grammar.analysis).clone(),
        rule_analyses: grammar
            .rule_analyses
            .iter()
            .map(|(rule_ix, state)| (*rule_ix, (**state).clone()))
            .collect(),
        bridge_specs: grammar.bridge_specs.clone(),
        recovery_delimiters: grammar.recovery_delimiters.clone(),
        bracketed_terminals: grammar.bracketed_terminals.clone(),
    };

    bincode::serialize(&file)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))
}

pub(crate) fn deserialize_grammar_file(bytes: &[u8]) -> Result<Grammar, io::Error> {
    let cache: GrammarCacheFile = bincode::deserialize(bytes)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))?;

    let table = decode_rule_table(cache.table)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;

    Ok(Grammar {
        table,
        analysis: Arc::new(cache.analysis),
        rule_analyses: cache
            .rule_analyses
            .into_iter()
            .map(|(rule_ix, state)| (rule_ix, Arc::new(state)))
            .collect(),
        bridge_specs: cache.bridge_specs,
        recovery_delimiters: cache.recovery_delimiters,
        bracketed_terminals: cache.bracketed_terminals,
    })
}

fn cache_file_path(cache_key: u64) -> PathBuf {
    cache_dir().join(format!(
        "grammar_v{}_{}_{}_{}.bin",
        CACHE_FORMAT_VERSION,
        env::consts::OS,
        env::consts::ARCH,
        cache_key
    ))
}

fn cache_dir() -> PathBuf {
    if let Some(path) = explicit_cache_dir() {
        return path;
    }

    if let Ok(path) = env::var("GRAMMAX_CACHE_DIR") {
        if !path.trim().is_empty() {
            return Path::new(&path).to_path_buf();
        }
    }

    env::temp_dir().join("grammax").join("grammar-cache")
}

fn explicit_cache_dir() -> Option<PathBuf> {
    let lock = CACHE_DIR_OVERRIDE.get_or_init(|| Mutex::new(None));
    let guard = lock.lock().ok()?;
    guard.clone()
}

impl PartialEq for CachedTerminal {
    fn eq(&self, other: &Self) -> bool {
        use CachedTerminal::*;
        match (self, other) {
            (Literal(a), Literal(b)) => a == b,
            (TokenLiteral(a), TokenLiteral(b)) => a == b,
            (Token(a), Token(b)) => a == b,
            (Char(a), Char(b)) => a == b,
            (Named(a), Named(b)) => a == b,
            (Eof, Eof) => true,
            _ => false,
        }
    }
}

impl Eq for CachedTerminal {}

impl Hash for CachedTerminal {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        use CachedTerminal::*;
        match self {
            Literal(s) => {
                0u8.hash(state);
                s.hash(state);
            }
            TokenLiteral(s) => {
                1u8.hash(state);
                s.hash(state);
            }
            Token(inner) => {
                2u8.hash(state);
                inner.hash(state);
            }
            Char(c) => {
                3u8.hash(state);
                c.hash(state);
            }
            CharEscapes => {
                7u8.hash(state);
            }
            Named(s) => {
                4u8.hash(state);
                s.hash(state);
            }
            Regex(s) => {
                6u8.hash(state);
                s.hash(state);
            }
            Eof => 5u8.hash(state),
        }
    }
}

impl fmt::Display for CachedTerminal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use CachedTerminal::*;
        match self {
            Literal(s) => write!(f, "\"{}\"", s),
            TokenLiteral(s) => write!(f, "token(\"{}\")", s),
            Token(inner) => write!(f, "token({})", inner),
            CharEscapes => write!(f, "characters including escapes"),
            Char(c) => write!(f, "'{}'", c),
            Named(n) => write!(f, "{}", n),
            Eof => write!(f, "EOF"),
            Regex(s) => write!(f, "regex({})", s),
        }
    }
}
