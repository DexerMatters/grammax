use rustc_hash::FxHashMap;

use crate::{
    parsec::tree::{TreeAllocRef, TreeAllocRefExt},
    semantic::{Command, command::NodePath},
};

pub(crate) fn generate_commands_incremental(
    alloc: &TreeAllocRef,
    path: &NodePath,
    old_green: usize,
    new_green: usize,
    new_green_offset: usize,
    source_text: &str,
    current_is_root: bool,
) -> Vec<Command> {
    let mut commands = Vec::new();
    let mut next_node_id = 1u64;
    let mut eq_cache = FxHashMap::default();
    let mut align_cache = FxHashMap::default();

    emit_commands_for_delta(
        alloc,
        old_green,
        new_green,
        new_green_offset,
        source_text,
        path,
        current_is_root,
        &mut commands,
        &mut next_node_id,
        &mut eq_cache,
        &mut align_cache,
    );

    commands
}

fn emit_commands_for_delta(
    alloc: &TreeAllocRef,
    old_green: usize,
    new_green: usize,
    new_green_offset: usize,
    source_text: &str,
    path: &NodePath,
    current_is_root: bool,
    out: &mut Vec<Command>,
    next_node_id: &mut u64,
    eq_cache: &mut FxHashMap<(usize, usize), bool>,
    align_cache: &mut FxHashMap<(usize, usize), bool>,
) {
    if old_green == new_green || greens_equivalent(alloc, old_green, new_green, eq_cache) {
        return;
    }

    if path.0.is_empty() {
        emit_replace_at_path(
            alloc,
            path,
            new_green,
            new_green_offset,
            source_text,
            current_is_root,
            out,
            next_node_id,
        );
        return;
    }

    let old_node = alloc.get_node(old_green);
    let new_node = alloc.get_node(new_green);

    if old_node.tag != new_node.tag {
        emit_replace_at_path(
            alloc,
            path,
            new_green,
            new_green_offset,
            source_text,
            current_is_root,
            out,
            next_node_id,
        );
        return;
    }

    let old_children = &old_node.children;
    let new_children = &new_node.children;

    if old_children.is_empty() && new_children.is_empty() {
        emit_replace_at_path(
            alloc,
            path,
            new_green,
            new_green_offset,
            source_text,
            current_is_root,
            out,
            next_node_id,
        );
        return;
    }

    let prefix = common_prefix_len(alloc, old_children, new_children, eq_cache);
    let suffix = common_suffix_len(alloc, old_children, new_children, prefix, eq_cache);

    let old_mid_start = prefix;
    let old_mid_end = old_children.len().saturating_sub(suffix);
    let new_mid_start = prefix;
    let new_mid_end = new_children.len().saturating_sub(suffix);

    if old_mid_start == old_mid_end && new_mid_start == new_mid_end {
        return;
    }

    let old_mid_len = old_mid_end.saturating_sub(old_mid_start);
    let new_mid_len = new_mid_end.saturating_sub(new_mid_start);

    if old_mid_len == 0 {
        for (rel_ix, &insert_child) in new_children[new_mid_start..new_mid_end].iter().enumerate() {
            let mut insert_path = path.clone();
            insert_path.0.push(new_mid_start + rel_ix);
            let insert_offset = child_offset_at(
                alloc,
                new_children,
                new_green_offset,
                new_mid_start + rel_ix,
            );
            let node_id = emit_create_commands_from_green(
                alloc,
                insert_child,
                insert_offset,
                source_text,
                out,
                next_node_id,
            );
            out.push(Command::InsertNodeAtPath {
                path: insert_path.clone(),
                node_id,
                cascade_to_root: false,
            });
        }
        return;
    }

    if new_mid_len == 0 {
        for ix in (old_mid_start..old_mid_end).rev() {
            let mut delete_path = path.clone();
            delete_path.0.push(ix);
            out.push(Command::DeleteNodeAtPath { path: delete_path });
        }
        return;
    }

    if try_emit_insertions_as_subsequence_aligned(
        alloc,
        path,
        old_children,
        new_children,
        old_mid_start,
        old_mid_end,
        new_mid_start,
        new_mid_end,
        new_green_offset,
        source_text,
        current_is_root,
        out,
        next_node_id,
        eq_cache,
        align_cache,
    ) {
        return;
    }

    if try_emit_deletions_as_subsequence_aligned(
        alloc,
        path,
        old_children,
        new_children,
        old_mid_start,
        old_mid_end,
        new_mid_start,
        new_mid_end,
        new_green_offset,
        source_text,
        current_is_root,
        out,
        next_node_id,
        eq_cache,
        align_cache,
    ) {
        return;
    }

    emit_replace_at_path(
        alloc,
        path,
        new_green,
        new_green_offset,
        source_text,
        current_is_root,
        out,
        next_node_id,
    );
}

fn emit_replace_at_path(
    alloc: &TreeAllocRef,
    path: &NodePath,
    new_green: usize,
    new_green_offset: usize,
    source_text: &str,
    current_is_root: bool,
    out: &mut Vec<Command>,
    next_node_id: &mut u64,
) {
    let node_id = emit_create_commands_from_green(
        alloc,
        new_green,
        new_green_offset,
        source_text,
        out,
        next_node_id,
    );

    if !path.0.is_empty() || current_is_root {
        out.push(Command::DeleteNodeAtPath { path: path.clone() });
    }

    out.push(Command::InsertNodeAtPath {
        path: path.clone(),
        node_id,
        cascade_to_root: false,
    });
}

fn emit_insert_at_path(
    alloc: &TreeAllocRef,
    path: &NodePath,
    new_green: usize,
    new_green_offset: usize,
    source_text: &str,
    out: &mut Vec<Command>,
    next_node_id: &mut u64,
) {
    let node_id = emit_create_commands_from_green(
        alloc,
        new_green,
        new_green_offset,
        source_text,
        out,
        next_node_id,
    );
    out.push(Command::InsertNodeAtPath {
        path: path.clone(),
        node_id,
        cascade_to_root: false,
    });
}

fn try_emit_insertions_as_subsequence_aligned(
    alloc: &TreeAllocRef,
    path: &NodePath,
    old_children: &[usize],
    new_children: &[usize],
    old_mid_start: usize,
    old_mid_end: usize,
    new_mid_start: usize,
    new_mid_end: usize,
    new_green_offset: usize,
    source_text: &str,
    current_is_root: bool,
    out: &mut Vec<Command>,
    next_node_id: &mut u64,
    eq_cache: &mut FxHashMap<(usize, usize), bool>,
    align_cache: &mut FxHashMap<(usize, usize), bool>,
) -> bool {
    let old_mid = &old_children[old_mid_start..old_mid_end];
    let new_mid = &new_children[new_mid_start..new_mid_end];

    let mut old_ix = 0usize;
    let mut inserts = Vec::new();
    let mut matches = Vec::new();

    for (new_rel, &new_child) in new_mid.iter().enumerate() {
        if old_ix < old_mid.len()
            && greens_align_equivalent(alloc, old_mid[old_ix], new_child, align_cache)
        {
            matches.push((old_ix, new_rel));
            old_ix += 1;
        } else {
            inserts.push((new_rel, new_child));
        }
    }

    if old_ix != old_mid.len() {
        return false;
    }

    for (new_rel, insert_child) in inserts {
        let mut insert_path = path.clone();
        insert_path.0.push(old_mid_start + new_rel);
        let insert_offset = child_offset_at(
            alloc,
            new_children,
            new_green_offset,
            old_mid_start + new_rel,
        );
        emit_insert_at_path(
            alloc,
            &insert_path,
            insert_child,
            insert_offset,
            source_text,
            out,
            next_node_id,
        );
    }

    for (old_rel, new_rel) in matches {
        let mut child_path = path.clone();
        child_path.0.push(old_mid_start + new_rel);
        let child_offset = child_offset_at(
            alloc,
            new_children,
            new_green_offset,
            old_mid_start + new_rel,
        );
        emit_commands_for_delta(
            alloc,
            old_mid[old_rel],
            new_mid[new_rel],
            child_offset,
            source_text,
            &child_path,
            current_is_root,
            out,
            next_node_id,
            eq_cache,
            align_cache,
        );
    }

    true
}

#[allow(clippy::too_many_arguments)]
fn try_emit_deletions_as_subsequence_aligned(
    alloc: &TreeAllocRef,
    path: &NodePath,
    old_children: &[usize],
    new_children: &[usize],
    old_mid_start: usize,
    old_mid_end: usize,
    new_mid_start: usize,
    new_mid_end: usize,
    new_green_offset: usize,
    source_text: &str,
    current_is_root: bool,
    out: &mut Vec<Command>,
    next_node_id: &mut u64,
    eq_cache: &mut FxHashMap<(usize, usize), bool>,
    align_cache: &mut FxHashMap<(usize, usize), bool>,
) -> bool {
    let old_mid = &old_children[old_mid_start..old_mid_end];
    let new_mid = &new_children[new_mid_start..new_mid_end];

    let mut new_ix = 0usize;
    let mut current_index = old_mid_start;

    for &old_child in old_mid {
        if new_ix < new_mid.len()
            && greens_align_equivalent(alloc, old_child, new_mid[new_ix], align_cache)
        {
            let mut child_path = path.clone();
            child_path.0.push(current_index);
            let child_offset =
                child_offset_at(alloc, old_children, new_green_offset, current_index);
            emit_commands_for_delta(
                alloc,
                old_child,
                new_mid[new_ix],
                child_offset,
                source_text,
                &child_path,
                current_is_root,
                out,
                next_node_id,
                eq_cache,
                align_cache,
            );
            new_ix += 1;
            current_index += 1;
        } else {
            let mut delete_path = path.clone();
            delete_path.0.push(current_index);
            out.push(Command::DeleteNodeAtPath { path: delete_path });
        }
    }

    new_ix == new_mid.len()
}

fn common_prefix_len(
    alloc: &TreeAllocRef,
    old_children: &[usize],
    new_children: &[usize],
    eq_cache: &mut FxHashMap<(usize, usize), bool>,
) -> usize {
    let mut prefix = 0usize;
    while prefix < old_children.len()
        && prefix < new_children.len()
        && greens_equivalent(alloc, old_children[prefix], new_children[prefix], eq_cache)
    {
        prefix += 1;
    }
    prefix
}

fn common_suffix_len(
    alloc: &TreeAllocRef,
    old_children: &[usize],
    new_children: &[usize],
    prefix: usize,
    eq_cache: &mut FxHashMap<(usize, usize), bool>,
) -> usize {
    let mut suffix = 0usize;
    while suffix < old_children.len().saturating_sub(prefix)
        && suffix < new_children.len().saturating_sub(prefix)
        && greens_equivalent(
            alloc,
            old_children[old_children.len() - 1 - suffix],
            new_children[new_children.len() - 1 - suffix],
            eq_cache,
        )
    {
        suffix += 1;
    }
    suffix
}

fn greens_equivalent(
    alloc: &TreeAllocRef,
    old_green: usize,
    new_green: usize,
    cache: &mut FxHashMap<(usize, usize), bool>,
) -> bool {
    if old_green == new_green {
        return true;
    }

    if let Some(&cached) = cache.get(&(old_green, new_green)) {
        return cached;
    }

    let old_node = alloc.get_node(old_green);
    let new_node = alloc.get_node(new_green);

    let equivalent =
        old_node.tag == new_node.tag
            && old_node.width == new_node.width
            && old_node.children.len() == new_node.children.len()
            && old_node.children.iter().zip(new_node.children.iter()).all(
                |(&old_child, &new_child)| greens_equivalent(alloc, old_child, new_child, cache),
            );

    cache.insert((old_green, new_green), equivalent);
    equivalent
}

fn greens_align_equivalent(
    alloc: &TreeAllocRef,
    old_green: usize,
    new_green: usize,
    cache: &mut FxHashMap<(usize, usize), bool>,
) -> bool {
    if old_green == new_green {
        return true;
    }

    if let Some(&cached) = cache.get(&(old_green, new_green)) {
        return cached;
    }

    let old_node = alloc.get_node(old_green);
    let new_node = alloc.get_node(new_green);

    let equivalent = old_node.tag == new_node.tag
        && old_node.children.len() == new_node.children.len()
        && old_node.children.iter().zip(new_node.children.iter()).all(
            |(&old_child, &new_child)| greens_align_equivalent(alloc, old_child, new_child, cache),
        );

    cache.insert((old_green, new_green), equivalent);
    equivalent
}

fn emit_create_commands_from_green(
    alloc: &TreeAllocRef,
    green: usize,
    node_offset: usize,
    source_text: &str,
    out: &mut Vec<Command>,
    next_node_id: &mut u64,
) -> u64 {
    let node = alloc.get_node(green);
    let mut child_ids = Vec::with_capacity(node.children.len());
    let mut child_offset = node_offset;
    for &child in &node.children {
        child_ids.push(emit_create_commands_from_green(
            alloc,
            child,
            child_offset,
            source_text,
            out,
            next_node_id,
        ));
        child_offset += alloc.get_node(child).width;
    }

    let node_id = *next_node_id;
    *next_node_id = next_node_id.saturating_add(1);

    // Emit different command types based on tag
    match &node.tag {
        crate::parsec::tree::Tag::Token { .. } => {
            let text = token_text_for_node(&node.tag, node_offset, node.width, source_text)
                .unwrap_or_default();
            out.push(Command::CreateToken {
                node_id,
                tag: node.tag.clone(),
                text,
                field: String::new(),
            });
        }
        crate::parsec::tree::Tag::Error(err) => {
            let text = token_text_for_node(&node.tag, node_offset, node.width, source_text)
                .unwrap_or_default();
            out.push(Command::CreateError {
                node_id,
                kind: err.clone(),
                text,
                field: String::new(),
            });
        }
        crate::parsec::tree::Tag::Field { name, .. } => {
            out.push(Command::CreateNode {
                node_id,
                tag: node.tag.clone(),
                children: child_ids,
                field: name.to_string(),
            });
        }
        crate::parsec::tree::Tag::Rule { .. } => {
            out.push(Command::CreateNode {
                node_id,
                tag: node.tag.clone(),
                children: child_ids,
                field: String::new(),
            });
        }
    }

    node_id
}

fn child_offset_at(
    alloc: &TreeAllocRef,
    children: &[usize],
    base_offset: usize,
    child_index: usize,
) -> usize {
    children
        .iter()
        .take(child_index)
        .fold(base_offset, |acc, &child| acc + alloc.get_node(child).width)
}

fn token_text_for_node(
    tag: &crate::parsec::tree::Tag,
    offset: usize,
    width: usize,
    source_text: &str,
) -> Option<String> {
    if !matches!(tag, crate::parsec::tree::Tag::Token { .. }) {
        return None;
    }

    let start = offset.min(source_text.len());
    let end = start.saturating_add(width).min(source_text.len());
    source_text.get(start..end).map(ToString::to_string)
}
