use rustc_hash::FxHashMap;

use crate::{
    parsec::tree::{Tag, TreeAllocRef, TreeAllocRefExt},
    semantic::{
        Command,
        command::{NodePath, PathTargetKind},
    },
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

/// Generate a full-tree snapshot as commands: creates every node and inserts the root.
/// Used to bootstrap a fresh client that has no prior tree state.
pub(crate) fn generate_commands_for_full_tree(
    alloc: &TreeAllocRef,
    root_green: usize,
    source_text: &str,
) -> Vec<Command> {
    let mut commands = Vec::new();
    let mut next_node_id = 1u64;

    let node_id = emit_create_commands_from_green(
        alloc,
        root_green,
        0,
        source_text,
        &mut commands,
        &mut next_node_id,
    );

    commands.push(Command::InsertNodeAtPath {
        path: NodePath(vec![]),
        node_id,
        cascade_to_root: false,
    });

    commands
}

fn emit_commands_for_delta(
    alloc: &TreeAllocRef,
    old_green: usize,
    new_green: usize,
    new_green_offset: usize,
    source_text: &str,
    path: &NodePath,
    _current_is_root: bool,
    out: &mut Vec<Command>,
    next_node_id: &mut u64,
    eq_cache: &mut FxHashMap<(usize, usize), bool>,
    align_cache: &mut FxHashMap<(usize, usize), bool>,
) {
    if old_green == new_green || greens_equivalent(alloc, old_green, new_green, eq_cache) {
        return;
    }

    let old_node = alloc.get_node(old_green);
    let new_node = alloc.get_node(new_green);

    if old_node.tag != new_node.tag {
        emit_replace_at_path(
            alloc,
            old_green,
            path,
            new_green,
            new_green_offset,
            source_text,
            out,
            next_node_id,
        );
        return;
    }

    // Field nodes with a single child are transparent in the frontend tree:
    // emit_create_commands_from_green_with_field skips them and hoists their name onto the child.
    // Therefore the path emitted by the delta must NOT include the Field wrapper level.
    // When both old and new are single-child Field nodes, recurse into the children at the
    // *same* path (no extra index pushed), so generated paths stay aligned with the frontend.
    if let Tag::Field { .. } = &old_node.tag {
        if old_node.children.len() == 1 && new_node.children.len() == 1 {
            let old_child = old_node.children[0];
            let new_child = new_node.children[0];
            drop(old_node);
            drop(new_node);
            emit_commands_for_delta(
                alloc,
                old_child,
                new_child,
                new_green_offset,
                source_text,
                path,
                false,
                out,
                next_node_id,
                eq_cache,
                align_cache,
            );
            return;
        }
    }

    let old_children = &old_node.children;
    let new_children = &new_node.children;

    if old_children.is_empty() && new_children.is_empty() {
        emit_replace_at_path(
            alloc,
            old_green,
            path,
            new_green,
            new_green_offset,
            source_text,
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

    // Unified greedy tag-match: match new_mid children to old_mid children left-to-right by
    // immediate tag.  Unmatched old children are deleted; unmatched new children are inserted.
    // This subsumes both the equal-count same-tag pairing and the subsequence alignment
    // strategies, and crucially never contaminates `out` on failure — all commands are buffered
    // and only flushed on success.
    if try_emit_by_greedy_tag_match(
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
        out,
        next_node_id,
        eq_cache,
        align_cache,
    ) {
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
        out,
        next_node_id,
        eq_cache,
        align_cache,
    ) {
        return;
    }

    emit_lcs_diff(
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
        out,
        next_node_id,
        eq_cache,
        align_cache,
    );
}

fn emit_replace_at_path(
    alloc: &TreeAllocRef,
    old_green: usize,
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

    if path.0.is_empty() {
        out.push(Command::DeleteNodeAtPath { path: path.clone() });
        out.push(Command::InsertNodeAtPath {
            path: path.clone(),
            node_id,
            cascade_to_root: false,
        });
        return;
    }

    out.push(Command::ReplaceNodeAtPath {
        path: path.clone(),
        node_id,
        target_kind: target_kind_for_green(alloc, old_green),
    });
}

fn target_kind_for_green(alloc: &TreeAllocRef, green: usize) -> PathTargetKind {
    let tag = alloc.get_node(green).tag.clone();
    if matches!(tag, Tag::Token { .. } | Tag::Error(_)) {
        PathTargetKind::Leaf
    } else {
        PathTargetKind::Node
    }
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

fn try_emit_by_greedy_tag_match(
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
    out: &mut Vec<Command>,
    next_node_id: &mut u64,
    eq_cache: &mut FxHashMap<(usize, usize), bool>,
    align_cache: &mut FxHashMap<(usize, usize), bool>,
) -> bool {
    let old_mid = &old_children[old_mid_start..old_mid_end];
    let new_mid = &new_children[new_mid_start..new_mid_end];

    // Greedy left-to-right matching by immediate tag only.
    // For each new child, find the next old child (from old_ix forward) with the same tag.
    // This determines which old children to match (recurse) vs. skip (delete).
    // New children that skip over old children themselves will be inserted.
    //
    // Strategy: scan new_mid left-to-right.  For each new child, advance old_ix while tags
    // differ — those unmatched old children become deletions.  When a match is found, pair them
    // for recursion and advance both indices.  Any new children that have no remaining old match
    // become insertions.  We only proceed if the greedy scan consumes all remaining old children
    // either as matches or deletions (i.e. old cannot have leftover items that can't be handled).

    // First, compute the matching permutation (dry run into a buffer — no writes yet).
    // matched_old[i] = new_rel index that old_mid[i] maps to, or None (→ delete)
    // matched_new[j] = old_rel index that new_mid[j] maps to, or None (→ insert)
    let mut matched_old: Vec<Option<usize>> = vec![None; old_mid.len()];
    let mut matched_new: Vec<Option<usize>> = vec![None; new_mid.len()];
    {
        let mut old_ix = 0usize;
        for (new_rel, &new_child) in new_mid.iter().enumerate() {
            let new_tag = &alloc.get_node(new_child).tag;
            // Look for next old child with the same tag.
            if let Some(oi) =
                (old_ix..old_mid.len()).find(|&oi| &alloc.get_node(old_mid[oi]).tag == new_tag)
            {
                // Old children between old_ix and oi are unmatched (deletions).
                matched_old[oi] = Some(new_rel);
                matched_new[new_rel] = Some(oi);
                old_ix = oi + 1;
            }
            // No match found → this new child will be an insertion.
        }
    }

    // Buffer all commands so we only flush to `out` when we're done (never pollute on failure).
    let mut buf: Vec<Command> = Vec::new();
    let mut child_node_id = *next_node_id;

    // Replay: walk old_mid left-to-right.  Interleave insertions of new children whose matched
    // old partner comes *after* the current old position (or whose match is None and they come
    // before the next matched old child).
    //
    // Simpler: walk new_mid left-to-right, and for each new child either:
    //   • It's matched → first delete any skipped old children that precede its old partner,
    //     then recurse into (old_partner, new_child) at the CURRENT live-tree index.
    //   • It's unmatched → insert it at the current live-tree index.
    //
    // `current_index` tracks the live-tree position: starts at old_mid_start, advances by +1
    // for each old child we keep (match+recurse) or new child we insert, stays the same after
    // every deletion (the deleted slot disappears, shifting the rest back).

    let mut old_cursor = 0usize; // next old_mid index to process
    let mut current_index = old_mid_start; // live-tree index

    for (new_rel, (&new_child, &old_rel_opt)) in new_mid.iter().zip(matched_new.iter()).enumerate()
    {
        if let Some(old_rel) = old_rel_opt {
            // Delete every unmatched old child that comes before this match.
            while old_cursor < old_rel {
                let mut del_path = path.clone();
                del_path.0.push(current_index);
                buf.push(Command::DeleteNodeAtPath { path: del_path });
                // Deletion: live-tree index does NOT advance (next child shifts into this slot).
                old_cursor += 1;
            }

            // Recurse into (old_mid[old_rel], new_mid[new_rel]).
            let old_child = old_mid[old_rel];
            let mut child_path = path.clone();
            child_path.0.push(current_index);
            let child_offset = child_offset_at(
                alloc,
                new_children,
                new_green_offset,
                new_mid_start + new_rel,
            );
            emit_commands_for_delta(
                alloc,
                old_child,
                new_child,
                child_offset,
                source_text,
                &child_path,
                false,
                &mut buf,
                &mut child_node_id,
                eq_cache,
                align_cache,
            );
            current_index += 1;
            old_cursor += 1;
        } else {
            // No old match → insert this new child at the current live-tree index.
            let insert_offset = child_offset_at(
                alloc,
                new_children,
                new_green_offset,
                new_mid_start + new_rel,
            );
            let node_id = emit_create_commands_from_green(
                alloc,
                new_child,
                insert_offset,
                source_text,
                &mut buf,
                &mut child_node_id,
            );
            let mut insert_path = path.clone();
            insert_path.0.push(current_index);
            buf.push(Command::InsertNodeAtPath {
                path: insert_path,
                node_id,
                cascade_to_root: false,
            });
            current_index += 1;
        }
    }

    // Delete any remaining old children that had no matching new child.
    while old_cursor < old_mid.len() {
        let mut del_path = path.clone();
        del_path.0.push(current_index);
        buf.push(Command::DeleteNodeAtPath { path: del_path });
        old_cursor += 1;
    }

    out.append(&mut buf);
    *next_node_id = child_node_id;
    true
}

#[allow(clippy::too_many_arguments)]
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
            false,
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
    out: &mut Vec<Command>,
    next_node_id: &mut u64,
    eq_cache: &mut FxHashMap<(usize, usize), bool>,
    align_cache: &mut FxHashMap<(usize, usize), bool>,
) -> bool {
    let old_mid = &old_children[old_mid_start..old_mid_end];
    let new_mid = &new_children[new_mid_start..new_mid_end];

    let mut new_ix = 0usize;
    let mut current_index = old_mid_start;

    // Buffer all commands so we only write them to `out` when we know the attempt succeeds.
    // Previously, DeleteNodeAtPath commands were pushed into `out` during the loop and were
    // left behind as stale entries when the function ultimately returned `false`.
    let mut buf: Vec<Command> = Vec::new();
    let mut child_node_id = *next_node_id;

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
                false,
                &mut buf,
                &mut child_node_id,
                eq_cache,
                align_cache,
            );
            new_ix += 1;
            current_index += 1;
        } else {
            let mut delete_path = path.clone();
            delete_path.0.push(current_index);
            buf.push(Command::DeleteNodeAtPath { path: delete_path });
        }
    }

    if new_ix == new_mid.len() {
        out.append(&mut buf);
        *next_node_id = child_node_id;
        true
    } else {
        false
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_lcs_diff(
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
    out: &mut Vec<Command>,
    next_node_id: &mut u64,
    eq_cache: &mut FxHashMap<(usize, usize), bool>,
    align_cache: &mut FxHashMap<(usize, usize), bool>,
) {
    let old_mid = &old_children[old_mid_start..old_mid_end];
    let new_mid = &new_children[new_mid_start..new_mid_end];
    let m = old_mid.len();
    let n = new_mid.len();

    // Compute LCS lengths table using greens_align_equivalent
    let mut dp = vec![vec![0usize; n + 1]; m + 1];
    for i in (0..m).rev() {
        for j in (0..n).rev() {
            dp[i][j] = if greens_align_equivalent(alloc, old_mid[i], new_mid[j], align_cache) {
                1 + dp[i + 1][j + 1]
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }

    // Trace the LCS to classify each position as delete, insert, or match
    let mut unmatched_old: Vec<usize> = Vec::new();
    let mut unmatched_new: Vec<usize> = Vec::new();
    let mut matched_pairs: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    let mut j = 0;
    while i < m || j < n {
        if i < m && j < n && greens_align_equivalent(alloc, old_mid[i], new_mid[j], align_cache) {
            matched_pairs.push((i, j));
            i += 1;
            j += 1;
        } else if j >= n || (i < m && dp[i + 1][j] >= dp[i][j + 1]) {
            unmatched_old.push(i);
            i += 1;
        } else {
            unmatched_new.push(j);
            j += 1;
        }
    }

    // 1. Emit deletions in reverse original order so earlier indices stay valid
    for &old_rel in unmatched_old.iter().rev() {
        let mut delete_path = path.clone();
        delete_path.0.push(old_mid_start + old_rel);
        out.push(Command::DeleteNodeAtPath { path: delete_path });
    }

    // 2. Emit insertions using new-tree positions (valid after all deletes above)
    for &new_rel in &unmatched_new {
        let mut insert_path = path.clone();
        insert_path.0.push(new_mid_start + new_rel);
        let insert_offset = child_offset_at(
            alloc,
            new_children,
            new_green_offset,
            new_mid_start + new_rel,
        );
        emit_insert_at_path(
            alloc,
            &insert_path,
            new_mid[new_rel],
            insert_offset,
            source_text,
            out,
            next_node_id,
        );
    }

    // 3. Recurse on matched pairs using new-tree positions
    for (old_rel, new_rel) in matched_pairs {
        let mut child_path = path.clone();
        child_path.0.push(new_mid_start + new_rel);
        let child_offset = child_offset_at(
            alloc,
            new_children,
            new_green_offset,
            new_mid_start + new_rel,
        );
        emit_commands_for_delta(
            alloc,
            old_mid[old_rel],
            new_mid[new_rel],
            child_offset,
            source_text,
            &child_path,
            false,
            out,
            next_node_id,
            eq_cache,
            align_cache,
        );
    }
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
    emit_create_commands_from_green_with_field(
        alloc,
        green,
        node_offset,
        source_text,
        out,
        next_node_id,
        None,
    )
}

fn emit_create_commands_from_green_with_field(
    alloc: &TreeAllocRef,
    green: usize,
    node_offset: usize,
    source_text: &str,
    out: &mut Vec<Command>,
    next_node_id: &mut u64,
    inherited_field: Option<&str>,
) -> u64 {
    let node = alloc.get_node(green);

    if let Tag::Field { name, .. } = &node.tag {
        if node.children.len() == 1 {
            return emit_create_commands_from_green_with_field(
                alloc,
                node.children[0],
                node_offset,
                source_text,
                out,
                next_node_id,
                Some(name),
            );
        }
    }

    let field = inherited_field.unwrap_or("").to_string();
    let mut child_ids = Vec::with_capacity(node.children.len());
    let mut child_offset = node_offset;
    for &child in &node.children {
        child_ids.push(emit_create_commands_from_green_with_field(
            alloc,
            child,
            child_offset,
            source_text,
            out,
            next_node_id,
            None,
        ));
        child_offset += alloc.get_node(child).width;
    }

    let node_id = *next_node_id;
    *next_node_id = next_node_id.saturating_add(1);

    // Emit different command types based on tag
    match &node.tag {
        Tag::Token { rule_ix } => {
            let text = token_text_for_node(&node.tag, node_offset, node.width, source_text)
                .unwrap_or_default();
            out.push(Command::CreateToken {
                node_id,
                rule_ix: *rule_ix,
                text,
                field,
            });
        }
        Tag::Error(err) => {
            let text = token_text_for_node(&node.tag, node_offset, node.width, source_text)
                .unwrap_or_default();
            out.push(Command::CreateError {
                node_id,
                kind: err.clone(),
                text,
                field,
            });
        }
        Tag::Rule { rule_ix, .. } => {
            out.push(Command::CreateNode {
                node_id,
                rule_ix: *rule_ix,
                children: child_ids,
                field,
            });
        }
        Tag::Field { rule_ix, .. } => {
            out.push(Command::CreateNode {
                node_id,
                rule_ix: *rule_ix,
                children: child_ids,
                field,
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
    tag: &Tag,
    offset: usize,
    width: usize,
    source_text: &str,
) -> Option<String> {
    if !matches!(tag, Tag::Token { .. } | Tag::Error(_)) {
        return None;
    }

    let start = offset.min(source_text.len());
    let end = start.saturating_add(width).min(source_text.len());
    source_text.get(start..end).map(ToString::to_string)
}
