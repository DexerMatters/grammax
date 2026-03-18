use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::grammar::Grammar;
use crate::grammar::GrammarError;
use crate::grammar::analysis::GrammarStateAnalysis;
use crate::grammar::bridge;
use crate::grammar::bundle;
use crate::grammar::ir::{NormalizedNode, Production, RuleInfo, Symbol};
use crate::grammar::norm::RuleTable;
use crate::parsec::words::MatcherRef;

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

const CACHE_FORMAT_VERSION: u32 = 9;

static CACHE_DIR_OVERRIDE: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();

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
    terminals: Vec<MatcherRef>,
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
    /// Bridge specs derived from productions (Nilsson-Nyman 2009 section 3).
    bridge_specs: Vec<bridge::BridgeSpec>,
    /// Delimiter terminals used as stop points by scope recovery.
    recovery_delimiters: Vec<usize>,
    /// Terminals with matching delimiters on both ends (e.g. strings, comments).
    bracketed_terminals: Vec<usize>,
    /// Delimiter terminals that open/close bracketed content scopes.
    bracketed_delimiters: Vec<usize>,
}

pub(crate) fn ensure_cacheable_terminals(terminals: &[MatcherRef]) -> Result<(), GrammarError> {
    for matcher in terminals {
        if let Err(err) = bincode::serialize(matcher) {
            return Err(GrammarError::UncacheableMatcher(format!(
                "{}: {}",
                matcher.display(),
                err
            )));
        }
    }
    Ok(())
}

pub(crate) fn load(cache_key: u64) -> Option<Grammar> {
    let path = cache_file_path(cache_key);
    let bytes = fs::read(path).ok()?;
    let decoded = bundle::decode(&bytes).ok()?;
    let cache = deserialize_cache_payload(&decoded.cache_payload).ok()?;

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
        bracketed_delimiters: cache.bracketed_delimiters,
    })
}

pub(crate) fn store(cache_key: u64, grammar: &Grammar) -> io::Result<()> {
    let dir = cache_dir();
    fs::create_dir_all(&dir)?;

    let payload = serialize_cache_payload(grammar, cache_key)?;
    let bytes = bundle::encode(&payload, &[])?;

    fs::write(cache_file_path(cache_key), bytes)
}

pub(crate) fn serialize_grammar_file(grammar: &Grammar) -> Result<Vec<u8>, io::Error> {
    serialize_grammar_file_for_targets(grammar, &[])
}

pub(crate) fn serialize_grammar_file_for_targets(
    grammar: &Grammar,
    targets: &[String],
) -> Result<Vec<u8>, io::Error> {
    let payload = serialize_cache_payload(grammar, 0)?;
    bundle::encode(&payload, targets)
}

pub(crate) fn deserialize_grammar_file(bytes: &[u8]) -> Result<Grammar, io::Error> {
    let decoded = bundle::decode(bytes)?;
    let _targets = decoded.targets;
    let cache = deserialize_cache_payload(&decoded.cache_payload)?;

    Ok(Grammar {
        table: decode_rule_table(cache.table)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?,
        analysis: Arc::new(cache.analysis),
        rule_analyses: cache
            .rule_analyses
            .into_iter()
            .map(|(rule_ix, state)| (rule_ix, Arc::new(state)))
            .collect(),
        bridge_specs: cache.bridge_specs,
        recovery_delimiters: cache.recovery_delimiters,
        bracketed_terminals: cache.bracketed_terminals,
        bracketed_delimiters: cache.bracketed_delimiters,
    })
}

fn serialize_cache_payload(grammar: &Grammar, cache_key: u64) -> io::Result<Vec<u8>> {
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
        bracketed_delimiters: grammar.bracketed_delimiters.clone(),
    };

    bincode::serialize(&cache)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))
}

fn deserialize_cache_payload(bytes: &[u8]) -> io::Result<GrammarCacheFile> {
    bincode::deserialize(bytes)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))
}

fn encode_rule_table(table: &RuleTable) -> Result<CachedRuleTable, String> {
    let mut matcher_idx_by_ptr: FxHashMap<usize, usize> = FxHashMap::default();
    let mut matcher_idx_by_fingerprint: FxHashMap<Vec<u8>, usize> = FxHashMap::default();

    for (idx, matcher) in table.terminals.iter().enumerate() {
        matcher_idx_by_ptr.insert(Arc::as_ptr(matcher) as *const () as usize, idx);

        let fingerprint = matcher_fingerprint(matcher)?;
        matcher_idx_by_fingerprint.entry(fingerprint).or_insert(idx);
    }

    let rules = table
        .rules
        .iter()
        .map(|rule| {
            Ok(CachedRuleInfo {
                name: rule.name.to_string(),
                description: rule.description.to_string(),
                node: encode_node(&rule.node, &matcher_idx_by_ptr, &matcher_idx_by_fingerprint)?,
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
        terminals: table.terminals.clone(),
        terminal_map: table.terminal_map.clone(),
        start_rule: table.start_rule,
    })
}

fn decode_rule_table(table: CachedRuleTable) -> Result<RuleTable, String> {
    let terminals = table.terminals;

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
    matcher_idx_by_ptr: &FxHashMap<usize, usize>,
    matcher_idx_by_fingerprint: &FxHashMap<Vec<u8>, usize>,
) -> Result<CachedNode, String> {
    match node {
        NormalizedNode::Terminal(matcher) => {
            let key = Arc::as_ptr(matcher) as *const () as usize;
            if let Some(idx) = matcher_idx_by_ptr.get(&key) {
                return Ok(CachedNode::Terminal(*idx));
            }

            let fingerprint = matcher_fingerprint(matcher)?;
            if let Some(idx) = matcher_idx_by_fingerprint.get(&fingerprint) {
                return Ok(CachedNode::Terminal(*idx));
            }

            Err(format!(
                "terminal matcher not found in table: {}",
                matcher.display()
            ))
        }
        NormalizedNode::Alternative(nodes) => Ok(CachedNode::Alternative(
            nodes
                .iter()
                .map(|n| encode_node(n, matcher_idx_by_ptr, matcher_idx_by_fingerprint))
                .collect::<Result<Vec<_>, _>>()?,
        )),
        NormalizedNode::Sequence(nodes) => Ok(CachedNode::Sequence(
            nodes
                .iter()
                .map(|n| encode_node(n, matcher_idx_by_ptr, matcher_idx_by_fingerprint))
                .collect::<Result<Vec<_>, _>>()?,
        )),
        NormalizedNode::Reference(ix) => Ok(CachedNode::Reference(*ix)),
        NormalizedNode::Field(name, inner) => Ok(CachedNode::Field(
            (*name).to_string(),
            Box::new(encode_node(
                inner,
                matcher_idx_by_ptr,
                matcher_idx_by_fingerprint,
            )?),
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

fn matcher_fingerprint(matcher: &MatcherRef) -> Result<Vec<u8>, String> {
    bincode::serialize(matcher).map_err(|err| {
        format!(
            "failed to serialize matcher '{}': {}",
            matcher.display(),
            err
        )
    })
}

pub(crate) fn leak_str(value: String) -> &'static str {
    value.leak()
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
