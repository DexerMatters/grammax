import React, { useState, useReducer } from 'react';
import type { CreateTokenCommand, CreateNodeCommand, CreateErrorCommand, DeleteNodeAtPathCommand, InsertNodeAtPathCommand, Command, RuleInfo } from '../Fetch';

// ============ Runtime Tree Node Model ============

type TreeNode = TokenNode | InternalNode | ErrorNode;

interface TokenNode {
  type: 'token';
  text: string;
  field: string;
  ruleIx: number;
  span: [number, number];
}

interface InternalNode {
  type: 'node';
  field: string;
  ruleIx: number;
  span: [number, number];
  children: TreeNode[];
}

interface ErrorNode {
  type: 'error';
  field: string;
  text: string;
  errorKind: 'unexpected' | 'missing' | 'incomplete';
  expectedRuleIx: number[];
  span: [number, number];
}

// ============ Command Application Logic ============

/**
 * Apply a batch of commands to the current tree, returning a new tree.
 * Handles node_id reset per batch using a local map.
 */
function applyCommandBatch(tree: TreeNode | null, commands: Command[]): TreeNode | null {
  let currentTree = tree;

  // Build local node map from create commands
  const nodeMap = new Map<number, TreeNode>();

  // First pass: create all nodes/tokens/errors
  for (const cmd of commands) {
    if (cmd.type === 'createToken') {
      const c = cmd as CreateTokenCommand;
      nodeMap.set(c.node_id, {
        type: 'token',
        text: c.text,
        field: c.field,
        ruleIx: c.rule_ix,
        span: [0, 0], // Note: API doesn't provide span for tokens yet
      });
    } else if (cmd.type === 'createNode') {
      const c = cmd as CreateNodeCommand;
      const children = c.children.map(id => nodeMap.get(id));
      if (children.some(child => !child)) {
        continue;
      }
      nodeMap.set(c.node_id, {
        type: 'node',
        field: c.field,
        ruleIx: c.rule_ix,
        children: children as TreeNode[],
        span: [0, 0], // Note: would be computed from children
      });
    } else if (cmd.type === 'createError') {
      const c = cmd as CreateErrorCommand;
      const errorKind: 'unexpected' | 'missing' | 'incomplete' =
        c.kind.type === 'unexpectedToken' ? 'unexpected' :
          c.kind.type === 'missingToken' ? 'missing' :
            'incomplete';
      nodeMap.set(c.node_id, {
        type: 'error',
        field: c.field,
        text: c.text,
        errorKind,
        expectedRuleIx: c.kind.expected || [],
        span: [0, 0],
      });
    }
  }

  // Second pass: apply structural changes
  for (const cmd of commands) {
    if (cmd.type === 'deleteNodeAtPath') {
      const c = cmd as DeleteNodeAtPathCommand;
      currentTree = deleteAtPath(currentTree, c.path);
    } else if (cmd.type === 'insertNodeAtPath') {
      const c = cmd as InsertNodeAtPathCommand;
      const nodeToInsert = nodeMap.get(c.node_id);
      if (nodeToInsert) {
        currentTree = insertAtPath(currentTree, c.path, nodeToInsert);
      }
    }
  }

  return currentTree;
}

function deleteAtPath(node: TreeNode | null, path: number[]): TreeNode | null {
  if (!node) return null;
  if (path.length === 0) return null;
  if (node.type !== 'node') return node;

  const [head, ...rest] = path;
  if (head < 0 || head >= node.children.length) return node;

  const newChildren = [...node.children];
  if (rest.length === 0) {
    newChildren.splice(head, 1);
  } else {
    const next = deleteAtPath(newChildren[head], rest);
    if (next === null) {
      newChildren.splice(head, 1);
    } else {
      newChildren[head] = next;
    }
  }

  return {
    ...node,
    children: newChildren,
  };
}

function insertAtPath(node: TreeNode | null, path: number[], newNode: TreeNode): TreeNode {
  // If no tree, start a new one
  if (!node) return newNode;
  if (path.length === 0) return newNode;

  const [head, ...rest] = path;
  if (node.type !== 'node') return node;

  const newChildren = [...node.children];
  if (rest.length === 0) {
    // Insert at this level
    const insertIndex = Math.max(0, Math.min(head, newChildren.length));
    newChildren.splice(insertIndex, 0, newNode);
  } else {
    // Recurse only when path exists; ignore invalid deep paths defensively
    if (head < 0 || head >= newChildren.length) return node;
    newChildren[head] = insertAtPath(newChildren[head], rest, newNode);
  }

  return {
    ...node,
    children: newChildren,
  };
}

// ============ Tree Rendering Components ============

interface TreeDisplayProps {
  node: TreeNode;
  rules: Map<number, RuleInfo>;
}

const TreeNodeDisplay: React.FC<TreeDisplayProps> = ({ node, rules }) => {
  // All hooks must be called unconditionally at the top
  const [isExpanded, setIsExpanded] = useState(true);
  const [showDetails, setShowDetails] = useState(false);

  // Token rendering
  if (node.type === 'token') {
    return (
      <div className="flex items-center gap-1 px-1.5 py-0.25">
        <span className="px-2 py-0.5 rounded-lg border border-[#d8a878]/40 text-[#d8a878] font-mono text-xs break-all">
          {node.text}
        </span>
      </div>
    );
  }

  // Error rendering
  if (node.type === 'error') {
    const label =
      node.errorKind === 'unexpected' ? 'unexpected' :
        node.errorKind === 'missing' ? 'missing' :
          'incomplete';
    const color =
      node.errorKind === 'unexpected' ? 'text-[#ff8899]' :
        node.errorKind === 'missing' ? 'text-[#ffd700]' :
          'text-[#66ddff]';

    // For missing errors, display expected rule name; otherwise display text
    let displayContent: React.ReactNode;
    if (node.errorKind === 'missing' && node.expectedRuleIx.length > 0) {
      const expectedRule = rules.get(node.expectedRuleIx[0]);
      displayContent = <span className="text-[#66ddff]">{'{' + (expectedRule?.name || `rule_${node.expectedRuleIx[0]}`) + '}'}</span>;
    } else {
      displayContent = <span className="text-[#8bdb8b]">"{node.text}"</span>;
    }

    return (
      <div>
        <div
          className={`flex items-center gap-3 px-1.5 py-0.25 font-mono text-xs cursor-pointer hover:opacity-80 transition-opacity`}
          onClick={() => setShowDetails(!showDetails)}
        >
          <span className={`${color} font-bold whitespace-nowrap min-w-max`}>{label}</span>
          {displayContent}
        </div>

        {showDetails && (
          <div className="ml-3 mt-0.5 px-1.5 py-1 bg-[#1a1a1a] border-l border-[#8bdb8b]/30 font-mono text-xs">
            <div className="flex flex-wrap gap-1">
              {node.expectedRuleIx.map((ix) => {
                const rule = rules.get(ix);
                return (
                  <span
                    key={ix}
                    className="px-1 py-0.25 bg-[#2a2a2a] border border-[#66ddff]/40 rounded text-[#66ddff] font-mono text-xs"
                    title={`Rule #${ix}`}
                  >
                    {rule ? rule.name : `#${ix}`}
                  </span>
                );
              })}
            </div>
          </div>
        )}
      </div>
    );
  }

  // Internal node rendering
  const ruleName = rules.get(node.ruleIx)?.name || `rule_${node.ruleIx}`;
  const hasChildren = node.children.length > 0;

  return (
    <div className="select-none">
      <div
        className="flex items-center gap-1.5 px-1.5 py-0.25 hover:bg-[#1a1a1a]/50 cursor-pointer transition-colors"
        onClick={() => setIsExpanded(!isExpanded)}
      >
        <span className="font-semibold text-[#66ddff] text-xs tracking-wide">{node.field}</span>
        <span className="text-[#666] text-xs font-mono">
          <span className="text-[#8bdb8b]">{ruleName}</span>
        </span>
      </div>

      {isExpanded && hasChildren && (
        <div className="ml-3 border-l border-[#8bdb8b]/20">
          {node.children.map((child, idx) => (
            <div key={idx} className="pl-2">
              <TreeNodeDisplay node={child} rules={rules} />
            </div>
          ))}
        </div>
      )}
    </div>
  );
};

interface TreeViewerProps {
  tree: TreeNode | null;
  rules: Map<number, RuleInfo>;
}

const TreeViewer: React.FC<TreeViewerProps> = ({ tree, rules }) => {
  if (!tree) {
    return (
      <div className="px-2 py-1 text-[#666] italic">
        No parse tree yet...
      </div>
    );
  }

  return <TreeNodeDisplay node={tree} rules={rules} />;
};

function useTreeReducer(initialTree: TreeNode | null = null) {
  const [tree, dispatch] = useReducer(
    (state: TreeNode | null, action: { type: 'applyBatch'; batch: Command[] }) => {
      if (action.type === 'applyBatch') {
        return applyCommandBatch(state, action.batch);
      }
      return state;
    },
    initialTree
  );

  return [tree, (batch: Command[]) => dispatch({ type: 'applyBatch', batch })] as const;
}

export { TreeViewer, TreeNodeDisplay, applyCommandBatch, useTreeReducer };
export type { TreeNode, TokenNode, InternalNode, ErrorNode };
