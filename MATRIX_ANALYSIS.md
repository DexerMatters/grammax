# Adjacency Matrix and Transitive Closure Analysis

## The State Graph

From the test, we have 8 states in a simplified expression grammar:
```
Rule 0: (Expr + Term) | Term
Rule 1: Num
```

States generated:
- State 0: `Tok(1, "num")` - Terminal for "num" (from Rule 1)
- State 1: `Tok(0, "+")` - Terminal for "+" (from Rule 0)
- State 2: `Seq(0, 1, 0)` - Binary sequence node in Rule 0
- State 3: `Alt(0, 5, 0)` - Alternative node in Rule 0
- State 4: `Seq(0, 1, 0)` - Another sequence node (duplicate structure)
- State 5: `Seq(0, 3, 4)` - Nested sequence
- State 6: `Seq(0, 3, 2)` - Another nested sequence
- State 7: `Alt(0, 6, 0)` - Another alternative

## Adjacency Matrix Properties

The **sequence_adjacency_matrix()** encodes direct edges only from `Seq(...)` nodes:
- For each `State::Seq(_, left, right)` at position `i`, sets `matrix[i][left] = 1` and `matrix[i][right] = 1`

### Sparse Structure
- Only rows corresponding to `Seq` states have non-zero entries
- Rows for `Tok` and `Alt` states are all zeros
- This represents the **data-flow dependency graph** of the sequence decomposition

### Mathematical Properties
1. **Directed Graph**: Edges point from sequence nodes to their children
2. **Acyclic**: The parse tree structure guarantees no cycles (unless there are left-recursive references, which are handled via the placeholder mechanism)
3. **Sparse**: Most entries are 0
4. **Non-symmetric**: `matrix[i][j]` ≠ `matrix[j][i]` in general

### Example from test output:
```
State 1 (Tok) -> [0 0 0 0 0 0 0 0]  (Terminal has no outgoing edges)
State 2 (Seq) -> [1 1 0 0 0 0 0 0]  (Seq points to states 0 and 1)
State 3 (Alt) -> [0 0 0 0 0 0 0 0]  (Alternative has no edges in this matrix)
State 5 (Seq) -> [0 0 0 1 1 0 0 0]  (Seq points to states 3 and 4)
```

## Transitive Closure Properties

The **transitive_closure()** uses Floyd-Warshall algorithm to compute reachability:
```rust
for k in 0..n:
    for i in 0..n:
        for j in 0..n:
            if closure[i][k] and closure[k][j]:
                closure[i][j] = 1
```

### What it Computes
- `closure[i][j] = 1` iff there exists a path from state `i` to state `j` (direct or indirect)
- Preserves original edges and adds transitive ones

### Properties
1. **Reflexive only if original matrix was**: The closure doesn't add self-loops
2. **Preserves acyclicity**: If the original DAG has no cycles, the closure won't create them
3. **Idempotent**: Applying transitive_closure twice gives the same result

### Interpretation
```
Original:     State 5 -> State 3 -> State 0
Transitive:   State 5 -> State 3 ✓
              State 5 -> State 0 ✓  (added by closure)
```

## Semantic Meaning in Grammar Context

For **parsing/analysis purposes**, this matrix can be used to:

1. **Consumption Patterns**: Determine which states consume which sub-expressions
2. **First/Follow Sets**: Compute lookahead for predictive parsing
3. **Dependency Analysis**: Understand which rules depend on which tokens
4. **Epsilon Reachability**: Mark states reachable through epsilon productions
5. **Left-Factoring Detection**: Find states that share common prefixes
6. **Grammar Transformation**: Optimize grammar by understanding state dependencies

## Key Observation

The adjacency matrix captures **immediate containment** in the parse tree:
- `Seq` nodes explicitly list their left and right children
- `Alt` nodes are NOT included in this matrix (only Seq nodes)

This means:
- **For matching/recognition**: Need to track all reachable states
- **For code generation**: Could use transitive closure to determine variable dependencies
- **For optimization**: Could identify unreachable code paths

## Why Alt is Missing

The current `sequence_adjacency_matrix()` only includes edges from `Seq` nodes. To fully capture the grammar structure, we might also want:
```rust
// Alternative edges
if let State::Alt(_, l, r) = state {
    mat[(i, *l)] = 1;
    mat[(i, *r)] = 1;
}
```

This would give us the **full state graph reachability**, not just sequence-based dependencies.
